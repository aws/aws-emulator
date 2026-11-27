// SPDX-License-Identifier: CC0-1.0

//! @op UpdateRecord
//! IMPLEMENTED: this operation applies a PARTIAL update to one record already held in
//! the feature group's OnlineStore in [`crate::state`]. Every posted `FeatureValue`
//! replaces the stored value of the feature it names (or is appended when the record
//! carries no such feature yet) and every other stored feature value is left standing,
//! which is what separates this operation from `PutRecord`'s full over-write.
//!
//! The `FeatureGroupName` is resolved against the feature groups the SageMaker control
//! plane holds ([`crate::state::resolve_group`]) and a name that resolves to none is
//! refused with the modelled `ValidationError` (400). The record must already exist and
//! not be soft-deleted (`ResourceNotFound`, 404) — the precondition is on the record
//! alone, so a group that holds no live record under the requested identifier is refused
//! that way whatever stores the group has and whatever stores the request targets; the
//! request's `EventTime` — the value
//! of the group's own `EventTimeFeatureName`, when this request carries it — orders the
//! update against the stored record's, and one older than it is refused with
//! `ConflictException` (409) leaving the record untouched, while an omitted `EventTime`
//! keeps the stored one. A `TtlDuration` replaces the record's TTL duration, from which
//! `ExpiresAt = EventTime + TtlDuration` is re-derived; an omitted one leaves the
//! record's TTL setting exactly as it stands — the duration the write that stored the
//! record settled it under, which is no duration at all for a record written while its
//! feature group configured no default, whatever default that group carries now. A request
//! that states a `TtlDuration` of its own must also carry an `EventTime` — the two together
//! state the instant `ExpiresAt` is — and one that does not is refused with
//! `ValidationError` (400). A request that states no `TtlDuration` needs no `EventTime`,
//! whatever TTL the record or its feature group already stands under, because the expiry
//! is then re-derived from the event time the record keeps. A posted feature value under
//! one of the three names Feature Store RESERVES — the columns it adds to every offline
//! record, which no feature group may declare
//! ([`crate::state::reserved_feature_posted`]) — names no feature of the group and is
//! refused with `ValidationError` (400), exactly as a whole-record write refuses it.
//!
//! This is the one operation that GROWS a stored record — a posted feature the record
//! does not carry yet is appended, where `PutRecord` replaces a record's feature values
//! wholesale — so it is also the one that has to respect the store's per-record capacity
//! bound, [`crate::state::MAX_FEATURES_PER_RECORD`]: a merge past it answers the modelled
//! `ServiceUnavailable` instead of growing the store without limit.
//!
//! Two things the operation documents are not acted on here: authorization (the modelled
//! `AccessForbidden` is unreachable, since it is not emulated) and the feature group's
//! online store TYPE — "this operation is available only for feature groups that use the
//! Standard_V2 or InMemory online store type". The recorded units that exercise this
//! operation's rejections create their feature group with no `StorageType` at all and
//! AWS still answered those rejections rather than refusing the update, so the recording
//! rules that gate out.
//!
//! `TargetStores` names the stores the update is made in, defaulting to all the stores the
//! feature group has, and the two are written independently: the OnlineStore takes the
//! merged record when the request names it, and the OfflineStore — a location the control
//! plane configured for the group — takes the merged record as one appended row
//! ([`crate::state::offline_store`]), since that store is append-only and holds the record
//! AS UPDATED. An update naming only the OfflineStore therefore leaves the online record
//! standing and still appends the row, and an update naming only the OnlineStore appends
//! nothing. Both writes are made off the record that must already exist, so a feature
//! group with no OnlineStore — where this data plane holds no record at all — has nothing
//! to update and answers `ResourceNotFound`, not a success that lands in no store.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateRecord::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `UpdateRecord`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/smithy-model/com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime.json")
//! ```
//!
//! Updates one or more feature values for an existing record in the specified feature
//! group. Features that you do not include in the request remain unchanged. You can update
//! up to 100 features per call. This operation is available only for feature groups that
//! use the Standard_V2 or InMemory online store type. The record must already exist. If the
//! record does not exist or has been soft-deleted, the operation returns a ResourceNotFound
//! error. To create a record, use PutRecord. If you provide an EventTime that is older than
//! the record's current EventTime, the service rejects the update with a ConflictException.
//! If the EventTime is equal to or newer than the current value, the service applies the
//! update. If you omit EventTime, the service keeps the record's existing EventTime and
//! applies the update. If you specify a TtlDuration, you must also provide an EventTime in
//! the request. Otherwise, the operation returns a ValidationError.
//!
//! ## Input
//!
//! * `FeatureGroupName` (FeatureGroupNameOrArn, len 1..150, pattern
//!   ^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$)
//!   **required** — The identifier for the feature group that contains the record to update.
//!   You can specify one of the following: The feature group name. The feature group Amazon
//!   Resource Name (ARN).
//! * `Features` (Record, len 1..) **required** — The feature values to write to the record.
//! * `RecordIdentifierValueAsString` (ValueAsString, len 0..358400, pattern .*) **required**
//!   — The value that uniquely identifies the record in the feature group. This must match
//!   the value defined by the feature group's record identifier feature.
//! * `TargetStores` (TargetStores, len 1..2) — The target stores for the record update. By
//!   default, Amazon SageMaker Feature Store updates the record in all stores associated with
//!   the FeatureGroup.
//! * `TtlDuration` (TtlDuration) — The time-to-live (TTL) duration for the record. Amazon
//!   SageMaker Feature Store deletes the record when EventTime + TtlDuration elapses. If you
//!   omit this parameter, the record's existing TTL setting remains unchanged. For
//!   information about HardDelete, see the DeleteRecord operation in the Amazon SageMaker API
//!   Reference.
//!
//! ## Errors
//!
//! * `AccessForbidden` (HTTP 403) — You do not have permission to perform an action.
//! * `ConflictException` (HTTP 409) — The service rejected the update because the provided
//!   EventTime is older than the record's current EventTime. To persist the update, retrieve
//!   the record's latest EventTime and resubmit the request with an EventTime that is equal
//!   to or newer than the current value.
//! * `InternalFailure` (HTTP 500) — An internal failure occurred. Try your request again. If
//!   the problem persists, contact Amazon Web Services customer support.
//! * `ResourceNotFound` (HTTP 404) — A resource that is required to perform an action was not
//!   found.
//! * `ServiceUnavailable` (HTTP 503) — The service is currently unavailable.
//! * `ValidationError` (HTTP 400) — There was an error validating your request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateRecordError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateRecordOutput;

impl UpdateRecordOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/FeatureGroup/{FeatureGroupName}/Record",
        Some(200u16),
    ));
}

pub static UPDATE_RECORD_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateRecordOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateRecordOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateRecordError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemakerfeaturestoreruntime#UpdateRecord")]
#[derive(Debug)]
pub enum UpdateRecordErrorModeled {
    AccessForbidden(AccessForbidden),
    ConflictException(ConflictException),
    InternalFailure(InternalFailure),
    ResourceNotFound(ResourceNotFound),
    ServiceUnavailable(ServiceUnavailable),
    ValidationError(ValidationError),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_record::UpdateRecordError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateRecordError = crate::scaffolded::Scaffolded<UpdateRecordErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateRecordErrorModeled> for UpdateRecordError {
    fn from(modeled: UpdateRecordErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateRecord` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateRecord<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateRecord<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateRecord<Eng>
{
    type Input = UpdateRecordInput;
    type Output = UpdateRecordOutput;
    type Error = UpdateRecordError;

    const NAME: &'static str = "UpdateRecord";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_RECORD_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_RECORD_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(internal_failure(
                "UpdateRecord: this service holds no state on the mounted engine",
            ));
        };

        // A value outside the `TargetStore` enum would be read below as "not the
        // OnlineStore" and silently turn the update into a no-op success. The boundary
        // validator refuses an undeclared enum value first; this is the handler's own
        // guard, against the model's own value list (`TARGET_STORE_VALUES`).
        if let Some(message) = crate::state::undeclared_target_store(input.target_stores.as_ref()) {
            return Err(validation(message));
        }

        // The data plane's one feature-group resolution: a `FeatureGroupName` naming no
        // feature group is a request defect (400 `ValidationError`), never a missing
        // record (404 `ResourceNotFound`). It also supplies the two feature names the
        // group's definition fixes, which the runtime API never carries.
        let definition = match crate::state::resolve_group(
            &*self.engine,
            &crate::state::request_region(ctx),
            &input.feature_group_name,
        ) {
            Ok(definition) => definition,
            Err(crate::state::Unresolved::Refused(message)) => return Err(validation(message)),
            Err(crate::state::Unresolved::NoDirectory) => {
                return Err(internal_failure(
                    "UpdateRecord: this service holds no feature-group directory on the \
                     mounted engine",
                ))
            }
        };

        // `Features` is modelled `len 1..` and refused at the boundary; read it
        // fallibly all the same, so a handler reached with an empty list answers the
        // modelled error instead of applying an update that changes nothing.
        if input.features.is_empty() {
            return Err(validation("Features must carry at least one FeatureValue"));
        }

        // The rule every whole-record write of this plane reads a posted record by, applied
        // to the features an update posts: a feature group cannot declare a feature named
        // after one of the columns Feature Store adds to every offline row, so a value
        // posted under one of those names belongs to no feature of this group and the
        // update is refused rather than merged into the record (and rather than appended to
        // the OfflineStore as a second column of that name).
        if let Some(name) = crate::state::reserved_feature_posted(&input.features) {
            let message = crate::state::reserved_feature_name("Features", &definition.name, name);
            return Err(validation(message));
        }

        // One value per feature name ([`crate::state::held_feature_values`]): `Features`
        // is a list with no `uniqueItems`, so a call may name a feature more than once,
        // and the value it then updates that feature to is the LAST it names — the same
        // value the merge below settles on, so the event-time and identifier reads cannot
        // disagree with what is stored.
        let posted_features = crate::state::held_feature_values(&input.features);

        // "You can update up to 100 features per call." The model bounds `Features` at
        // `len 1..` only, so the documented maximum is this handler's to refuse. It
        // bounds the FEATURES the call updates — one value per feature name, the reading
        // every write of this data plane makes ([`crate::state::held_feature_values`]) and
        // the count the store's own per-record bound is read on
        // (`crate::state::FeatureGroupStore::insert_record`) — not the entries of a list
        // that may name one feature more than once: such a call still updates that one
        // feature, so it counts once. Every check from here to the store lock is on the
        // REQUEST, so it answers the same whatever stores the caller named: a request this
        // operation cannot act on is a request defect, not something an OfflineStore-only
        // target excuses.
        if posted_features.len() > MAX_FEATURES_PER_UPDATE {
            return Err(validation(format!(
                "Features: at most {MAX_FEATURES_PER_UPDATE} features can be updated per \
                 call, but {} were supplied",
                posted_features.len()
            )));
        }

        // The `EventTime` of an update is the value the group's own
        // `EventTimeFeatureName` carries among the posted features — the runtime API has
        // no separate member for it — and "If you omit EventTime, the service keeps the
        // record's existing EventTime and applies the update." The feature is read for
        // the one value it states ([`crate::state::held_scalar_feature`]), the same
        // reading every whole-record write of this data plane makes, so a value in a form
        // this plane cannot read as an event time is refused here too.
        let event_time_feature_name = definition.event_time_feature_name.clone();
        let posted_event_time =
            match crate::state::held_scalar_feature(&posted_features, &event_time_feature_name) {
                crate::state::ScalarFeature::Absent => None,
                crate::state::ScalarFeature::Value(value) => Some(value.to_owned()),
                crate::state::ScalarFeature::Unreadable => {
                    return Err(validation(crate::state::event_time_not_scalar(
                        &event_time_feature_name,
                    )))
                }
            };

        // "If you specify a TtlDuration, you must also provide an EventTime in the
        // request. Otherwise, the operation returns a ValidationError." (`ExpiresAt` is
        // `EventTime + TtlDuration`, so a TTL with no event time states no instant.) The
        // condition is on what THIS REQUEST states: a request that names no `TtlDuration`
        // is the documented partial update — "If you omit EventTime, the service keeps the
        // record's existing EventTime and applies the update" — and the expiry of a record
        // already standing under a TTL, its own or its feature group's default, is
        // re-derived from the event time the record keeps, so nothing has to be restated.
        if input.ttl_duration.is_some() && posted_event_time.is_none() {
            return Err(validation(EVENT_TIME_REQUIRED_FOR_TTL));
        }
        // The TTL the update states, read through the data plane's one statement of this
        // guard ([`crate::state::declared_ttl_seconds`]), so a unit outside the modelled
        // enum is refused here exactly as a whole-record write refuses it.
        let ttl_secs = input
            .ttl_duration
            .as_ref()
            .map(crate::state::declared_ttl_seconds)
            .transpose()
            .map_err(validation)?;

        // An event time the request carries must be a timestamp: the update is ordered
        // against the stored record's, so a value in neither accepted format orders
        // against nothing.
        let posted_secs = match posted_event_time.as_deref() {
            None => None,
            Some(at) => match crate::state::parse_event_time_secs(at) {
                Some(secs) => Some(secs),
                None => {
                    return Err(validation(crate::state::event_time_not_timestamp(
                        &event_time_feature_name,
                        at,
                    )))
                }
            },
        };

        let identifier = input.record_identifier_value_as_string.clone();
        let key = definition.name.clone();

        // `RecordIdentifierValueAsString` "must match the value defined by the feature
        // group's record identifier feature", so an update that posts the group's
        // record-identifier feature must post the value of the record it is updating: a
        // record's identity is not what this operation changes.
        // The posted feature is read for the one value it states
        // ([`crate::state::held_scalar_feature`]), the same reading every whole-record
        // write of this data plane makes, so a value this plane cannot read as one value
        // is refused with the rule this crate states once
        // ([`crate::state::identifier_not_scalar`]) rather than reported as a value that
        // fails to match one.
        let identifier_feature = definition.record_identifier_feature_name.clone();
        match crate::state::held_scalar_feature(&posted_features, &identifier_feature) {
            crate::state::ScalarFeature::Absent => {}
            crate::state::ScalarFeature::Value(value) if value == identifier => {}
            crate::state::ScalarFeature::Value(_) => {
                return Err(validation(format!(
                    "Features: the record identifier feature '{identifier_feature}' must carry \
                     the RecordIdentifierValueAsString of the record being updated \
                     ('{identifier}')"
                )))
            }
            crate::state::ScalarFeature::Unreadable => {
                return Err(validation(crate::state::identifier_not_scalar(
                    &identifier_feature,
                )))
            }
        }

        // ONE critical section for the whole read-merge-write: the stored record's event
        // time decides whether the update is accepted at all, so reading it under the
        // same guard that performs the write is what keeps two concurrent updates from
        // each merging onto the record the other replaced. Nothing below can panic —
        // every failure is a modelled error return, which releases the guard. The guard
        // is released with this block, before the offline replication below is awaited:
        // no guard is held across an await.
        let offline_row = {
            let mut store = state.lock();
            // "The record must already exist. If the record does not exist or has been
            // soft-deleted, the operation returns a ResourceNotFound error. To create a
            // record, use PutRecord." `live_record` is exactly that: present and not
            // soft-deleted. The precondition is on the RECORD alone, so every way one can
            // be absent is answered here and answered the same: a feature group that has
            // taken no record yet holds no store here, a group whose definition configures
            // no `OnlineStore` holds no record this data plane can ever have written
            // (records live in the online store, and every write of this data plane skips
            // that store for such a group), and a record that was deleted or has expired
            // is gone from the one it does hold. None of that is decided by the stores the
            // request targets either: the merged record both stores take is read off the
            // record that must already exist.
            let Some(group) = store.group_mut(&input.feature_group_name) else {
                return Err(no_such_record(&identifier));
            };
            let Some(existing) = group.live_record(&identifier) else {
                return Err(no_such_record(&identifier));
            };
            // The merge runs on a CLONE of the stored record, so a refusal below leaves
            // the store untouched.
            let mut updated = existing.clone();

            // "If you provide an EventTime that is older than the record's current
            // EventTime, the service rejects the update with a ConflictException. If the
            // EventTime is equal to or newer than the current value, the service applies
            // the update." A stored record carrying no event time orders against nothing,
            // so the update applies.
            if let (Some(posted), Some(current)) = (posted_secs, updated.event_time_secs()) {
                if posted < current {
                    return Err(UpdateRecordErrorModeled::ConflictException(
                        ConflictException {
                            message: Some(crate::state::bounded_message(CONFLICTING_EVENT_TIME)),
                        },
                    )
                    .into());
                }
            }

            // "Updates one or more feature values for an existing record… Features that
            // you do not include in the request remain unchanged": each posted feature
            // replaces the stored value of the feature it names, in place, and a feature
            // the record does not carry yet is added. The store's per-record capacity
            // bound is read where the merged record is COMMITTED
            // (`crate::state::FeatureGroupStore::insert_record`, the one place every write
            // of this store passes), so a merge that would grow a record past it is
            // refused there with the one wording of a store that is full — this loop
            // states no bound of its own.
            for posted in posted_features.iter().copied() {
                let value = crate::state::StoredFeatureValue::from(posted);
                match updated
                    .features
                    .iter_mut()
                    .find(|held| held.feature_name == value.feature_name)
                {
                    Some(held) => *held = value,
                    None => {
                        updated.features.push(value);
                    }
                }
            }

            // "If you omit EventTime, the service keeps the record's existing EventTime."
            if let Some(at) = posted_event_time {
                updated.event_time = Some(at);
            }
            // "If you omit this parameter, the record's existing TTL setting remains
            // unchanged" (`TtlDuration`), so a request that names one replaces the
            // record's duration ([`crate::state::StoredRecord::ttl_secs`], the setting
            // itself) and one that does not leaves that setting exactly as it stands —
            // including a record standing under NO duration at all, which stays under
            // none however the feature group's default has moved since: a group's default
            // is read where a record's TTL is SETTLED, by the write that stores the
            // record (`crate::state::effective_ttl_seconds`), and an update stating no
            // duration settles nothing. `ExpiresAt` is re-derived from the duration the
            // record is left under and its event time either way, because an update may
            // have moved that event time.
            if let Some(secs) = ttl_secs {
                updated.ttl_secs = Some(secs);
            }
            updated.recompute_expiry();

            // "The target stores for the record update. By default, Amazon SageMaker
            // Feature Store updates the record in all stores associated with the
            // FeatureGroup." The merge above is made whichever stores the request named —
            // the request's own rules, the record's existence and the ordering against
            // its event time hold for every update, and the merged record is what BOTH
            // stores take — so only the two WRITES are conditioned on the target list.
            // The row the offline store appends is read off the merged record here, under
            // the same guard the merge ran under, so the appended row and the online
            // record can never disagree.
            let offline_row = crate::state::targets_offline_store(input.target_stores.as_ref())
                .then(|| updated.project(None));
            if crate::state::targets_online_store(input.target_stores.as_ref()) {
                // The record already exists, so this over-writes it rather than growing
                // the group; the capacity answer is kept all the same, so a future change
                // to the store cannot turn a refused write into a silent success.
                if !group.insert_record(updated) {
                    return Err(at_capacity(crate::state::at_capacity_message(&key)));
                }
            }
            offline_row
        };

        // The OfflineStore side of the update: the record AS UPDATED is appended to the
        // location the control plane configured for this feature group, under the role it
        // recorded with it — the same delivery every write of this data plane replicates
        // through. A group created without an `OfflineStoreConfig` has nowhere to append
        // to, and the replication is the service's own asynchronous work, so neither a
        // withheld permission nor a failed write changes the answer this call gives its
        // client.
        if let Some(row) = offline_row {
            if let Some(delivery) = definition.offline_store.as_ref() {
                crate::state::offline_store::replicate(
                    ctx.callback(),
                    &crate::state::request_region(ctx),
                    &key,
                    delivery,
                    &crate::state::offline_store::OfflineRecord::of(&definition, &row),
                    ::std::time::SystemTime::now(),
                )
                .await;
            }
        }

        Ok(UpdateRecordOutput)
    }
}

/// The most FEATURES one `UpdateRecord` call may update.
///
/// "You can update up to 100 features per call", from this operation's own
/// documentation — `smithy_query(query="doc", shape="UpdateRecord")` on this crate's
/// `smithy-model/`. A documented API limit, not an emulator capacity bound.
///
/// Counted in FEATURES, i.e. in the values the call updates — one per feature name
/// ([`crate::state::held_feature_values`]) — and not in the entries of a list the wire
/// admits one name more than once in.
const MAX_FEATURES_PER_UPDATE: usize = 100;

/// How this operation states an `EventTime` older than the record's current one.
///
/// A refusal message is the SERVICE's wording, not the model's: the modelled
/// `ConflictException`'s documentation paragraph states the rule for a reader of the API
/// reference, while the message this operation sends states the verdict and the fact a
/// retry path keys on — that the record was not modified. It names no identifier,
/// timestamp or feature group, because the verdict is on the ordering alone.
const CONFLICTING_EVENT_TIME: &str =
    "Update rejected: the provided EventTime is older than the record's current EventTime. The \
     record was not modified.";

/// How this operation states a `TtlDuration` with no `EventTime` to derive an expiry
/// from.
///
/// Fixed text, as the service words this error class: it names the two members the rule is
/// about, the way a TTL can also be in force without the request naming one, and the
/// feature a request carries an event time in — and nothing of this request or this
/// feature group.
const EVENT_TIME_REQUIRED_FOR_TTL: &str =
    "Validation Error: EventTime is required when TtlDuration is specified or when the feature \
     group has a default TTL configured. Include the EventTime feature in the Features list.";

/// The modelled `ValidationError` (HTTP 400) for a request this operation refuses.
///
/// The message is worded through [`crate::state::bounded_message`], so the text this
/// operation answers is inside the maximum length its own `ValidationError$Message`
/// declares even when the refusal quotes something the request supplied.
fn validation(message: impl Into<String>) -> UpdateRecordError {
    UpdateRecordErrorModeled::ValidationError(ValidationError {
        message: Some(crate::state::bounded_message(message)),
    })
    .into()
}

/// The modelled `InternalFailure` (HTTP 500) for a fault of the emulator itself, with its
/// message bounded the same way: `InternalFailure$Message` targets the same `Message`.
fn internal_failure(message: impl Into<String>) -> UpdateRecordError {
    UpdateRecordErrorModeled::InternalFailure(InternalFailure {
        message: Some(crate::state::bounded_message(message)),
    })
    .into()
}

/// The modelled `ResourceNotFound` (HTTP 404) for a record this operation cannot update
/// because the feature group holds no live one under that identifier — never written, or
/// soft-deleted.
///
/// The absence is the RECORD's, so the message names the record — by the
/// `RecordIdentifierValueAsString` the request carried, the only thing that identifies it
/// — and is prefixed as a missing record rather than as a missing resource of the feature
/// group's own class (`crate::state::no_such_feature_group`, a 400 `ValidationError`):
/// the two absences are routed differently by a caller, so they are stated differently.
fn no_such_record(identifier: &str) -> UpdateRecordError {
    UpdateRecordErrorModeled::ResourceNotFound(ResourceNotFound {
        message: Some(crate::state::bounded_message(format!(
            "Record Not Found: Record not found with record identifier [{identifier}]."
        ))),
    })
    .into()
}

/// The modelled `ServiceUnavailable` (HTTP 503) for a write the emulator's bounded store
/// cannot take.
///
/// Not a `ValidationError`: the request is well-formed, it is the emulator that is out of
/// room, so it must not be reported to the caller as its own mistake.
fn at_capacity(message: impl Into<String>) -> UpdateRecordError {
    UpdateRecordErrorModeled::ServiceUnavailable(ServiceUnavailable {
        message: Some(crate::state::bounded_message(message)),
    })
    .into()
}
