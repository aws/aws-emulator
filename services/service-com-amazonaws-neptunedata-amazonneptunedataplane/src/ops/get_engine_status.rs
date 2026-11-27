// SPDX-License-Identifier: CC0-1.0

//! @op GetEngineStatus
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined this operation because its
//! input is `smithy.api#Unit` (see `SKIPPED_STUBS.md`), so the input type, its schema
//! and the output shapes are declared here rather than under `types/`. Everything else
//! follows the generated siblings exactly: the `#[operation_error]` enum, the
//! `Scaffolded` error channel, the engine-owning struct and the one `impl Operation`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected). The store is reached through
//! [`crate::state::service_state`], and the engine record this operation reports is
//! built once, when that store is first created — so `startTime` is the instant this
//! emulator's engine started and is the same on every read.
//!
//! What the CLUSTER this plane is addressed at was PROVISIONED with — the engine version it
//! runs, and the features its create turned on — is not this engine's to choose: it is read
//! through [`crate::state::provisioned_cluster`], out of the directory the Neptune control
//! plane records a created cluster in, and layered over that record by
//! [`crate::state::EngineStatusRecord::reported`]. With no cluster provisioned the engine
//! reports its own defaults.
//!
//! ---
//!
//! # Modeled contract for `GetEngineStatus`
//!
//! Retrieves the status of the graph database on the host. For the full documentation of
//! any one member:
//!
//! ```text
//! smithy_query(query="doc",shape="GetEngineStatusOutput",member="<Member>",model_path="services/service-com-amazonaws-neptunedata-amazonneptunedataplane/smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json")
//! ```
//!
//! * **Route:** `GET /status -> 200`, `@readonly`
//! * **Input:** none (`smithy.api#Unit`)
//!
//! ## Output
//!
//! * `status` (String) — Set to `healthy` if the instance is not experiencing problems.
//! * `startTime` (String) — Set to the UTC time at which the current server process
//!   started.
//! * `dbEngineVersion` (String) — The Neptune engine version running on the DB cluster.
//! * `role` (String) — `reader` for a read-replica, `writer` for the primary instance.
//! * `dfeQueryEngine` (String) — `enabled` if the DFE engine is fully enabled, or
//!   `viaQueryHint` (the default) if it is used only with the `useDFE` query hint.
//! * `gremlin` / `sparql` / `opencypher` (QueryLanguageVersion) — the version of each
//!   query language available on the cluster.
//! * `labMode` (map of String to String) — the Lab Mode settings being used.
//! * `rollingBackTrxCount` (Integer) — the number of transactions being rolled back, if
//!   any; the field does not appear at all when there are none.
//! * `rollingBackTrxEarliestStartTime` (String) — the start time of the earliest
//!   transaction being rolled back; absent when none is.
//! * `features` (map of String to Document) — status information about the features
//!   enabled on the cluster.
//! * `settings` (map of String to String) — the current cluster settings, for example
//!   the cluster query timeout (`clusterQueryTimeoutInMs`).
//!
//! ## Errors
//!
//! * `ClientTimeoutException` (HTTP 408) — Raised when a request timed out in the client.
//! * `ConstraintViolationException` (HTTP 400) — Raised when a value in a request field
//!   did not satisfy required constraints.
//! * `IllegalArgumentException` (HTTP 400) — Raised when an argument in a request is not
//!   supported.
//! * `InternalFailureException` (HTTP 500) — Raised when the processing of the request
//!   failed unexpectedly.
//! * `InvalidArgumentException` (HTTP 400) — Raised when an argument in a request has an
//!   invalid value.
//! * `PreconditionsFailedException` (HTTP 400) — Raised when a precondition for
//!   processing a request is not satisfied.
//! * `TooManyRequestsException` (HTTP 429) — Raised when the number of requests being
//!   processed exceeds the limit.
//! * `UnsupportedOperationException` (HTTP 400) — Raised when a request attempts to
//!   initiate an operation that is not supported.

#[allow(unused_imports)]
use super::types::*;

use std::collections::HashMap;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetEngineStatusError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json");

/// This operation's input: the model gives it `smithy.api#Unit`, so there is nothing to
/// read off the request.
///
/// Declared here with the empty-shape form of the `#[smithy]` macro, which emits the
/// `Schema`, `GET_ENGINE_STATUS_INPUT_SCHEMA` and the serde impls the centralized
/// request walk needs. The `input` key is what names the shape without looking it up in
/// the service model: `smithy.api#Unit` is a Smithy prelude shape, so the model JSON
/// does not carry it. No member is HTTP-bound, so the schema needs no URI template:
/// `GET /status` has nothing to bind.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetEngineStatusInput {}

/// The version of one query language the engine serves.
#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#QueryLanguageVersion")]
#[derive(Debug, Clone, Default)]
pub struct QueryLanguageVersion {
    pub version: String,
}

/// The engine status, as the model declares it.
#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#GetEngineStatusOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetEngineStatusOutput {
    pub status: Option<String>,
    pub start_time: Option<String>,
    pub db_engine_version: Option<String>,
    pub role: Option<String>,
    pub dfe_query_engine: Option<String>,
    pub gremlin: Option<QueryLanguageVersion>,
    pub sparql: Option<QueryLanguageVersion>,
    pub opencypher: Option<QueryLanguageVersion>,
    pub lab_mode: Option<HashMap<String, String>>,
    pub rolling_back_trx_count: Option<i32>,
    pub rolling_back_trx_earliest_start_time: Option<String>,
    pub features: Option<HashMap<String, ::aws_smithy_types::Document>>,
    pub settings: Option<HashMap<String, String>>,
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetEngineStatusError` alias below.
#[smithy_macros::operation_error("com.amazonaws.neptunedata#GetEngineStatus")]
#[derive(Debug)]
pub enum GetEngineStatusErrorModeled {
    ClientTimeoutException(ClientTimeoutException),
    ConstraintViolationException(ConstraintViolationException),
    IllegalArgumentException(IllegalArgumentException),
    InternalFailureException(InternalFailureException),
    InvalidArgumentException(InvalidArgumentException),
    PreconditionsFailedException(PreconditionsFailedException),
    TooManyRequestsException(TooManyRequestsException),
    UnsupportedOperationException(UnsupportedOperationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_engine_status::GetEngineStatusError`).
pub type GetEngineStatusError = crate::scaffolded::Scaffolded<GetEngineStatusErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetEngineStatusErrorModeled> for GetEngineStatusError {
    fn from(modeled: GetEngineStatusErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetEngineStatus` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetEngineStatus<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetEngineStatus<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetEngineStatus<Eng>
{
    type Input = GetEngineStatusInput;
    type Output = GetEngineStatusOutput;
    type Error = GetEngineStatusError;

    const NAME: &'static str = "GetEngineStatus";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = GET_ENGINE_STATUS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = GET_ENGINE_STATUS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = (&input, ctx);
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetEngineStatusErrorModeled::InternalFailureException(
                crate::state::store_unavailable_internal(),
            )
            .into());
        };

        // A @readonly operation: the engine record is read, never written. Cloning it
        // under the lock keeps the rendering below off the critical section.
        let stored = state.lock().engine_status.clone();

        // What the CLUSTER this plane is addressed at was provisioned with: its engine
        // version, and the features a create turned on. The engine configures none of that
        // — the control plane does — so it is read out of the shared directory and layered
        // over the stored record, which is where every value this engine process owns stays.
        let record = stored.reported(crate::state::provisioned_cluster(&*self.engine).as_ref());

        // "If there are transactions being rolled back, this field is set to the number
        // of such transactions. If there are none, the field doesn't appear at all" — the
        // COUNT is what says whether the engine is rolling anything back, so it gates
        // BOTH members: with no rollback neither appears, and with one the earliest-start
        // instant goes out too whenever the record holds it. A rollback whose start the
        // engine has not recorded reports the count alone — the count is the member the
        // documentation ties to the rollback's existence, so it is never suppressed
        // because the companion instant is missing.
        let rolling_back = record.rolling_back_trx_count > 0;

        // Each feature's value is the document the engine reports for that feature: the
        // status string itself for a feature whose status is all it reports, and a document
        // of its own for one that reports more (see `crate::state::FeatureReport`).
        let features = record
            .features
            .iter()
            .map(|(name, report)| (name.clone(), report.document()))
            .collect();
        // Each lab-mode parameter's value is what the parameter's own kind reports: the
        // on/off word for a switch, and the quantity for a parameter whose name states one —
        // which is no value at all while no quantity is in force (see
        // `crate::state::LabModeParameter`).
        let lab_mode = record
            .lab_mode
            .iter()
            .map(|(name, parameter)| (name.clone(), parameter.reported()))
            .collect();
        let settings = record.settings.into_iter().collect();

        Ok(GetEngineStatusOutput {
            status: Some(record.status),
            start_time: Some(crate::state::engine_timestamp(record.start_time)),
            db_engine_version: Some(record.db_engine_version),
            role: Some(record.role),
            dfe_query_engine: Some(record.dfe_query_engine),
            gremlin: Some(QueryLanguageVersion {
                version: record.gremlin_version,
            }),
            sparql: Some(QueryLanguageVersion {
                version: record.sparql_version,
            }),
            opencypher: Some(QueryLanguageVersion {
                version: record.opencypher_version,
            }),
            lab_mode: Some(lab_mode),
            rolling_back_trx_count: rolling_back.then_some(record.rolling_back_trx_count),
            rolling_back_trx_earliest_start_time: rolling_back
                .then_some(record.rolling_back_trx_earliest_start_time)
                .flatten()
                .map(crate::state::engine_timestamp),
            features: Some(features),
            settings: Some(settings),
        })
    }
}
