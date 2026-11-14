// SPDX-License-Identifier: CC0-1.0

//! @op GetUsage
//! Gets the usage data of a usage plan in a specified time interval.
//!
//! HAND-IMPLEMENTED. The scaffold's header is gone because the operation is written: the
//! answer is computed from the usage the `execute-api` data plane recorded for the plan's
//! API keys (`crate::state::usage`) and from the plan's own `quota`, and nothing is
//! synthesised per request.
//!
//! A REPORT, so it takes a shared read of the store and writes nothing: reading the usage
//! data must not change what a later caller sees, and in particular must not consume or
//! reset any part of a quota.
//!
//! ---
//!
//! # Modeled contract for `GetUsage`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json")
//! ```
//!
//! Gets the usage data of a usage plan in a specified time interval.
//!
//! ## Input
//!
//! * `endDate` (String) **required** — The ending date (e.g., 2016-12-31) of the usage data.
//! * `keyId` (String) — The Id of the API key associated with the resultant usage data.
//! * `limit` (NullableInteger) — The maximum number of returned results per page. The default
//!   value is 25 and the maximum value is 500.
//! * `position` (String) — The current pagination position in the paged result set.
//! * `startDate` (String) **required** — The starting date (e.g., 2016-01-01) of the usage
//!   data.
//! * `usagePlanId` (String) **required** — The Id of the usage plan associated with the usage
//!   data.
//!
//! ## Output
//!
//! * `endDate` (String) — The ending date of the usage data.
//! * `items` (MapOfKeyUsages) — The usage data, as daily logs of used and remaining quotas,
//!   over the specified time interval indexed over the API keys in a usage plan. For example,
//!   {..., "values" : { "{api_key}" : [ [0, 100], [10, 90], [100, 10]]}, where {api_key}
//!   stands for an API key value and the daily log entry is of the format [used quota,
//!   remaining quota].
//! * `position` (String) — The current pagination position in the paged result set.
//! * `startDate` (String) — The starting date of the usage data.
//! * `usagePlanId` (String) — The plan Id associated with this usage data.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The submitted request is not valid, for example, the
//!   input is incomplete or incorrect. See the accompanying error message for details.
//! * `NotFoundException` (HTTP 404) — The requested resource is not found. Make sure that the
//!   request URI is correct.
//! * `TooManyRequestsException` (HTTP 429) — The request has reached its throttling limit.
//!   Retry after the specified time period.
//! * `UnauthorizedException` (HTTP 401) — The request is denied because the caller has
//!   insufficient permissions.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetUsageError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetUsageError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.apigateway#GetUsage")]
#[derive(Debug)]
pub enum GetUsageErrorModeled {
    BadRequestException(BadRequestException),
    NotFoundException(NotFoundException),
    TooManyRequestsException(TooManyRequestsException),
    UnauthorizedException(UnauthorizedException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_usage::GetUsageError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetUsageError = crate::scaffolded::Scaffolded<GetUsageErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetUsageErrorModeled> for GetUsageError {
    fn from(modeled: GetUsageErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetUsage` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetUsage<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetUsage<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The API keys one read of a plan's usage data reports, in the order the plan's
/// associations were added in — the sequence a page of the report is cut out of.
///
/// Each association is joined against the API key record it names
/// (`ComAmazonawsApigatewayBackplanecontrolserviceState::usage_plan_key`), so a key the
/// account no longer holds is in no report: the usage data is "indexed over the API keys in
/// a usage plan" (`Usage$items`), which is the keys the plan meters and that still exist.
fn metered_key_ids(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    usage_plan_id: &str,
) -> Vec<String> {
    let Some(plan) = store.usage_plans.get(usage_plan_id) else {
        return Vec::new();
    };
    let mut ordered: Vec<(u64, &str)> = plan
        .keys
        .values()
        .filter(|entry| store.usage_plan_key(usage_plan_id, &entry.key_id).is_some())
        .map(|entry| (entry.sequence, entry.key_id.as_str()))
        .collect();
    ordered.sort_unstable();
    ordered
        .into_iter()
        .map(|(_, key_id)| key_id.to_string())
        .collect()
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetUsage<Eng>
{
    type Input = GetUsageInput;
    type Output = Paged<GetUsageOutput>;
    type Error = GetUsageError;

    const NAME: &'static str = "GetUsage";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_USAGE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_USAGE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(crate::state::state_unavailable)?;
        // A report of usage the data plane already recorded, so this takes a SHARED read
        // of the store and writes nothing: a read of the usage data must leave the quota
        // exactly as the next request will meet it.
        let store = state.lock();
        // The plan is the resource the request's URI addresses, so it is resolved first,
        // the way the path itself reads: a plan the store does not hold meters nothing and
        // has no usage data to report.
        if !store.usage_plans.contains_key(&input.usage_plan_id) {
            return Err(GetUsageErrorModeled::NotFoundException(NotFoundException {
                message: Some(crate::state::NO_SUCH_USAGE_PLAN.to_string()),
            })
            .into());
        }
        // `keyId` is "the Id of the API key associated with the resultant usage data", so
        // it NARROWS the report to one key the plan meters, and an id naming no such key
        // names no usage data of this plan: it is refused as the API key identifier it is,
        // rather than served a report of every key or an empty one.
        let metered = metered_key_ids(&store, &input.usage_plan_id);
        let selected = match input.key_id.as_deref() {
            None => metered,
            Some(key_id) if metered.iter().any(|metered| metered == key_id) => {
                vec![key_id.to_string()]
            }
            Some(_) => {
                return Err(GetUsageErrorModeled::NotFoundException(NotFoundException {
                    message: Some(crate::state::NO_SUCH_API_KEY.to_string()),
                })
                .into())
            }
        };
        // The interval the report covers: both dates must name a day, in order, and no
        // wider than one report may cover — the one rule both reads of the usage data
        // admit an interval by (`crate::state::usage_interval`).
        let (from, through) = crate::state::usage_interval(&input.start_date, &input.end_date)
            .map_err(|message| {
                GetUsageErrorModeled::BadRequestException(BadRequestException {
                    message: Some(message),
                })
            })?;
        // What is indexed is the keys that HAVE usage data over those days. A key the plan
        // meters but that sent nothing in the interval has no daily log to report, so the
        // report indexes nothing for it — not a log of zeroes: the member is "the usage
        // data ... indexed over the API keys in a usage plan", and a key with no usage data
        // over the interval is not among them.
        let ordered: Vec<String> = selected
            .into_iter()
            .filter(|key_id| store.has_usage_data(&input.usage_plan_id, key_id, from, through))
            .collect();
        // The usage data is paged over the KEYS it is indexed by, which is what `position`
        // and `limit` walk here; `page_of` applies the documented default and maximum page
        // size and refuses a cursor naming no metered key of this plan.
        let page = crate::state::page_of(&ordered, input.position.as_deref(), input.limit)
            .map_err(|message| {
                GetUsageErrorModeled::BadRequestException(BadRequestException {
                    message: Some(message),
                })
            })?;
        // Each key's daily log of used and remaining quota, from the one accounting the
        // data plane writes it into: "daily logs of used and remaining quotas, over the
        // specified time interval indexed over the API keys in a usage plan", each entry
        // "of the format [used quota, remaining quota]" (`Usage$items`).
        let items = page
            .ids
            .iter()
            .map(|key_id| {
                (
                    key_id.clone(),
                    store.daily_usage(&input.usage_plan_id, key_id, from, through),
                )
            })
            .collect();
        Ok(Paged::new(
            GetUsageOutput {
                usage_plan_id: Some(input.usage_plan_id),
                // The days the report COVERS, rendered from the interval itself rather
                // than echoed from the request, so the dates and the entries under each
                // key are the same interval in the same spelling.
                start_date: Some(crate::state::usage_date_string(from)),
                end_date: Some(crate::state::usage_date_string(through)),
                // The report's index of keys, written whether or not it indexes anything:
                // `Usage$items` is the map the days are reported UNDER, not a page of a
                // collection, so a plan whose keys sent nothing over the interval is
                // reported as the empty index rather than no index at all.
                items: Some(items),
                position: page.next_position.clone(),
            },
            page.echoed_limit(),
        ))
    }
}
