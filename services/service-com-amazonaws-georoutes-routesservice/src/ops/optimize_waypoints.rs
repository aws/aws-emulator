// SPDX-License-Identifier: CC0-1.0

//! @op OptimizeWaypoints
//!
//! Orders the submitted waypoints so that the cost of the WHOLE journey — by the shared
//! cost function in [`crate::state`]: duration for `FastestRoute`, distance for
//! `ShortestRoute` — is the least any order of them takes, with the origin first and the
//! destination, when one is given, last. The order is searched over orders rather than
//! grown hop by hop ([`optimal_order`]), because the cheapest next hop can leave an
//! expensive remainder and the journey is what the operation minimizes. The answer carries
//! one connection per consecutive pair of the computed sequence, the clock walked forward
//! across them (travel, then the rest a `Driver.RestCycles` schedule owes, then any wait
//! for an `AppointmentTime`, then the waypoint's `ServiceDuration`), and the totals summed
//! over those connections: the overall `Duration` is every phase `TimeBreakdown` splits the
//! cycle into — travel, rest, wait and the service time spent at the waypoints.
//!
//! A request must say where the journey GOES, not only where it starts: `Origin` is the
//! model's only `@required` member, and a request that sets neither `Destination` nor a
//! non-empty `Waypoints` is refused rather than answered with a route to nowhere.
//!
//! `Before` is HONOURED where it can be: a waypoint another unvisited waypoint requires
//! to come after it is not a candidate for the next place in the order, so the computed
//! sequence satisfies the constraint whenever the constraints are satisfiable at all.
//! Constraints that name a cycle cannot all hold; the sequence then falls back to cost
//! alone and every waypoint whose `Before` the order breaks comes back as an
//! `ImpedingWaypoints` entry naming that failed constraint — the model's own channel for
//! "waypoints that caused the optimization problem to fail, and the constraints that were
//! unsatisfied".
//!
//! `Clustering` is HONOURED: the clusters the requested algorithm forms are what the
//! sequencer keeps together — the traveller leaves a cluster only once that cluster's own
//! waypoints are behind — and every optimized waypoint reports the `ClusterIndex` of the
//! cluster it was assigned to.
//! `DrivingDistance` clusters the stops that are within the configured driving distance of
//! each other, costed by the same cost function as the connections; `TopologySegment`
//! clusters the stops that snap onto one stretch of one road between two junctions of the
//! network.
//!
//! `Driver.RestProfile`, `Exclude`, `AccessHours`, `Avoid.Areas` and
//! `Avoid.ControlledAccessHighways` answer 501: the EU rest profile stands for statutory
//! driving and break limits this emulator holds no data for, a country exclusion is
//! "strictly excluded" and no road of the model carries the country it is in, an access
//! window would move the arrival, the wait and the impeding waypoints, and a violated
//! avoidance would have to be reported as a notice this answer has nowhere to carry. The
//! rest of `Avoid` is satisfied trivially
//! (no toll road, ferry, tunnel, dirt road or car shuttle train exists on this network),
//! and `Traffic` and `TravelModeOptions` have nothing to act on: the model carries no
//! traffic variation and no vehicle restriction.
//!
//! ## Authorization
//!
//! The emulator verifies every request's SIGNATURE before any handler runs, so this handler
//! checks no credential. What it does check is whether the caller MAY perform the action:
//! AWS decides a signed call against the calling principal's identity-based IAM policies at
//! the IAM layer, ahead of the service reading the request, so the handler takes that
//! decision first through [`crate::state::identity_authorization`] and answers the modelled
//! `AccessDeniedException` (HTTP 403) with IAM's own sentence for a caller no readable
//! statement grants the action to, or one an explicit `Deny` refuses. The modelled `Key`
//! (API key) member is still not consulted: this emulator holds no API-key records to
//! authorize a key against, and refusing a correctly signed request for want of one would
//! refuse a valid caller.
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
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `OptimizeWaypoints`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! OptimizeWaypoints calculates the optimal order to travel between a set of waypoints to
//! minimize either the travel time or the distance travelled during the journey, based on
//! road network restrictions and the traffic pattern data. For more information, see
//! Optimize waypoints in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Avoid` (WaypointOptimizationAvoidanceOptions) — Features that are avoided. Avoidance is
//!   on a best-case basis. If an avoidance can't be satisfied for a particular case, this
//!   setting is ignored.
//! * `Clustering` (WaypointOptimizationClusteringOptions) — Clustering allows you to specify
//!   how nearby waypoints can be clustered to improve the optimized sequence.
//! * `DepartureTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Departure time from the waypoint. Time format:YYYY-MM-DDThh:mm:ss.sssZ |
//!   YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples: 2020-04-22T17:57:24Z 2020-04-22T17:57:24+02:00
//! * `Destination` (Position, len 2..2) — The final position for the route in the World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `DestinationOptions` (WaypointOptimizationDestinationOptions) — Destination related
//!   options.
//! * `Driver` (WaypointOptimizationDriverOptions) — Driver related options.
//! * `Exclude` (WaypointOptimizationExclusionOptions) — Features to be strictly excluded
//!   while calculating the route.
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `OptimizeSequencingFor` (WaypointOptimizationSequencingObjective, one of: FastestRoute |
//!   ShortestRoute) — Specifies the optimization criteria for the calculated sequence.
//!   Default value: FastestRoute.
//! * `Origin` (Position, len 2..2) **required** — The start position for the route in World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `OriginOptions` (WaypointOptimizationOriginOptions) — Origin related options.
//! * `Traffic` (WaypointOptimizationTrafficOptions) — Traffic-related options.
//! * `TravelMode` (WaypointOptimizationTravelMode, one of: Car | Pedestrian | Scooter |
//!   Truck) — Specifies the mode of transport when calculating a route. Used in estimating
//!   the speed of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (WaypointOptimizationTravelModeOptions) — Travel mode related
//!   options for the provided travel mode.
//! * `Waypoints` (WaypointOptimizationWaypointList) — List of waypoints between the Origin
//!   and Destination, in World Geodetic System (WGS 84) format: [longitude, latitude]. The
//!   maximum number of waypoints allowed per request: Maximum 50 waypoints per request
//!   Maximum 20 waypoints when using constraints (AccessHours, AppointmentTime,
//!   ServiceDuration, Heading, SideOfStreet, Before)
//!
//! ## Output
//!
//! * `Connections` (WaypointOptimizationConnectionList) **required** — Details about the
//!   connection from one waypoint to the next, within the optimized sequence.
//! * `Distance` (DistanceMeters, range 0..4294967295, default 0) **required** — Overall
//!   distance to travel the whole sequence.
//! * `Duration` (DurationSeconds, range 0..4294967295, default 0) **required** — Overall
//!   duration to travel the whole sequence. Unit: seconds
//! * `ImpedingWaypoints` (WaypointOptimizationImpedingWaypointList) **required** — Returns
//!   waypoints that caused the optimization problem to fail, and the constraints that were
//!   unsatisfied leading to the failure.
//! * `OptimizedWaypoints` (WaypointOptimizationOptimizedWaypointList) **required** —
//!   Waypoints in the order of the optimized sequence.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `TimeBreakdown` (WaypointOptimizationTimeBreakdown) **required** — Time breakdown for
//!   the sequence.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an AWS service.

#[allow(unused_imports)]
use super::types::*;

use std::collections::HashMap;

use crate::state::{self, Instant, Position, RoadOrientation, RoutingObjective, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `OptimizeWaypointsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `OptimizeWaypointsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#OptimizeWaypoints")]
#[derive(Debug)]
pub enum OptimizeWaypointsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::optimize_waypoints::OptimizeWaypointsError`). The wrapper is kept
/// because `Driver.RestProfile`, `Exclude` and `AccessHours` are answered with its 501.
pub type OptimizeWaypointsError = crate::scaffolded::Scaffolded<OptimizeWaypointsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<OptimizeWaypointsErrorModeled> for OptimizeWaypointsError {
    fn from(modeled: OptimizeWaypointsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `OptimizeWaypoints` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct OptimizeWaypoints<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> OptimizeWaypoints<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for OptimizeWaypoints<Eng>
{
    type Input = OptimizeWaypointsInput;
    type Output = OptimizeWaypointsOutput;
    type Error = OptimizeWaypointsError;

    const NAME: &'static str = "OptimizeWaypoints";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::OPTIMIZE_WAYPOINTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::OPTIMIZE_WAYPOINTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides authorization at the IAM layer, ahead of the service looking at the
        // request's own input, so this is the FIRST thing this handler does: a caller whose
        // identity-based policies do not allow this action is refused and nothing is
        // computed for it.
        if let Some(denial) = crate::state::identity_authorization::refusal(
            &*self.engine,
            ctx,
            <Self as smithy_server_runtime::Operation>::NAME,
        ) {
            return Err(access_denied(denial));
        }

        if input.driver.as_ref().is_some_and(|driver| driver.rest_profile.is_some()) {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with Driver.RestProfile",
            ));
        }
        // "This setting mandates that the router will avoid any routes that include the
        // specified options": no road of this model carries the country it is in, so a
        // country exclusion can be neither honoured nor reported.
        if input.exclude.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with Exclude",
            ));
        }
        // An avoidance is best-effort and a violated one is reported as a notice — but this
        // answer has nowhere to carry one, so the two avoidances this road model could
        // violate are declined instead of silently violated. Everything else `Avoid` names
        // is satisfied trivially: this network carries no toll road, ferry, tunnel, dirt
        // road or car shuttle train, and the computed path never turns back on itself.
        if let Some(avoid) = input.avoid.as_ref() {
            if avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "OptimizeWaypoints with Avoid.Areas",
                ));
            }
            if avoid.controlled_access_highways == Some(true) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "OptimizeWaypoints with Avoid.ControlledAccessHighways",
                ));
            }
        }
        // An access window would move the arrival, the wait and the impeding waypoints,
        // and this slice computes none of that from it, so it is declined rather than
        // accepted and ignored.
        let access_hours_set = input
            .waypoints
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|waypoint| waypoint.access_hours.is_some())
            || input
                .destination_options
                .as_ref()
                .is_some_and(|options| options.access_hours.is_some());
        if access_hours_set {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "OptimizeWaypoints with AccessHours",
            ));
        }

        // `Origin` is the only `@required` member, so the boundary validator enforces it
        // alone; that a journey must also have somewhere to go — a `Destination`, a
        // `Waypoints` list, or both — is a rule between two optional members that no model
        // can express, so it is enforced here. A `Waypoints` list with no entry sets no
        // waypoint, so it is no more "somewhere to go" than an absent one.
        if input.destination.is_none() && input.waypoints.as_deref().unwrap_or_default().is_empty() {
            return Err(invalid(
                MUTUALLY_SUFFICIENT_MEMBERS[0],
                state::at_least_one_of(&MUTUALLY_SUFFICIENT_MEMBERS),
            ));
        }

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, WAYPOINT_OPTIMIZATION_TRAVEL_MODE_VALUES)
                .ok_or_else(|| {
                    invalid(
                        "TravelMode",
                        state::must_be_one_of("TravelMode", WAYPOINT_OPTIMIZATION_TRAVEL_MODE_VALUES),
                    )
                })?,
        };
        let objective = match input.optimize_sequencing_for.as_deref() {
            None => RoutingObjective::FastestRoute,
            Some(value) => RoutingObjective::from_model(value).ok_or_else(|| {
                invalid(
                    "OptimizeSequencingFor",
                    state::must_be_one_of(
                        "OptimizeSequencingFor",
                        WAYPOINT_OPTIMIZATION_SEQUENCING_OBJECTIVE_VALUES,
                    ),
                )
            })?,
        };
        // A per-mode option block belongs to the one travel mode its member names, so a
        // request may set only the block the `TravelMode` it selected names; this
        // operation's options structure declares a block for two of the four modes its
        // travel-mode enum admits, and a mode with none permits no block at all.
        let declared = declared_travel_mode_options(input.travel_mode_options.as_ref());
        let permitted: Vec<&str> = state::block_of_mode(travel_mode.as_model(), &declared)
            .into_iter()
            .collect();
        state::check_travel_mode_options(travel_mode.as_model(), &permitted, &declared)
            .map_err(|(member, message)| invalid(&member, message))?;

        let clustering = match input.clustering.as_ref() {
            None => None,
            Some(options) => Some(Clustering::resolve(options)?),
        };

        let departure = match &input.departure_time {
            Some(value) => {
                state::parse_timestamp("DepartureTime", value).map_err(|message| invalid("DepartureTime", message))?
            }
            None => state::now(),
        };
        // Every time the answer reports is the request's own departure moved forward, so it
        // is reported at the offset the request stated that departure at; a departure taken
        // from this service's clock was stated at none and is reported in UTC.
        let clock_offset = state::UtcOffset::stated_in(input.departure_time.as_deref());

        let submitted = input.waypoints.as_deref().unwrap_or_default();

        // The documented server-side maximum, which the member carries no modeled length
        // constraint for: "Maximum 50 waypoints per request / Maximum 20 waypoints when
        // using constraints (AccessHours, AppointmentTime, ServiceDuration, Heading,
        // SideOfStreet, Before)". It is enforced here because the sequencer below costs
        // every remaining candidate at every step, so an unbounded list would be
        // unbounded work for one request.
        let uses_constraints = submitted.iter().any(|waypoint| {
            waypoint.access_hours.is_some()
                || waypoint.appointment_time.is_some()
                || waypoint.side_of_street.is_some()
                || waypoint.service_duration != 0
                || waypoint.heading != 0.0
                || waypoint.before.as_deref().is_some_and(|before| !before.is_empty())
        });
        let maximum = if uses_constraints {
            MAX_CONSTRAINED_WAYPOINTS
        } else {
            MAX_WAYPOINTS
        };
        if submitted.len() > maximum {
            return Err(invalid(
                "Waypoints",
                format!(
                    "at most {maximum} waypoints are allowed{}; got {}",
                    if uses_constraints { " when using constraints" } else { "" },
                    submitted.len()
                ),
            ));
        }

        // `ServiceDuration` and `Heading` carry modeled ranges the boundary validator does
        // not enforce on a defaulted member, and the service duration is added to the
        // sequence's clock, so the handler checks both.
        for (index, waypoint) in submitted.iter().enumerate() {
            check_service_duration(&format!("Waypoints[{index}].ServiceDuration"), waypoint.service_duration)?;
            check_heading(&format!("Waypoints[{index}].Heading"), waypoint.heading)?;
        }
        if let Some(options) = input.destination_options.as_ref() {
            check_service_duration("DestinationOptions.ServiceDuration", options.service_duration)?;
            check_heading("DestinationOptions.Heading", options.heading)?;
        }

        // A `Before` entry names another waypoint by its index, so an index outside the
        // submitted list, or a waypoint's own, names no constraint this service can honour
        // or report, and the same index twice names one constraint twice.
        for (index, waypoint) in submitted.iter().enumerate() {
            let before = waypoint.before.as_deref().unwrap_or_default();
            for (position, entry) in before.iter().enumerate() {
                let member = format!("Waypoints[{index}].Before");
                let named = usize::try_from(*entry).ok().filter(|named| *named != index);
                if named.is_none_or(|named| named >= submitted.len()) {
                    return Err(invalid(
                        &member,
                        format!(
                            "Before must name another waypoint by its index in Waypoints (0..{}); got {entry}",
                            submitted.len()
                        ),
                    ));
                }
                if before[position + 1..].contains(entry) {
                    return Err(invalid(
                        &member,
                        format!("Before must name each waypoint at most once; {entry} is named more than once"),
                    ));
                }
            }
        }

        // Every connection names its endpoints by id, so two stops sharing one id would
        // make the answer ambiguous: a duplicate among the ids the CALLER supplied is
        // refused, and an id this service has to make up for a stop the caller left
        // unnamed is chosen so that it cannot collide with one of them. The refusal names
        // the OFFENDING stop's own `Id` member — the member that carries it, the element's
        // position in it when that member is a list, and `Id` itself — so a client can
        // point at the duplicate rather than at the whole list.
        let supplied_origin_id = input
            .origin_options
            .as_ref()
            .and_then(|options| options.id.clone());
        let supplied_destination_id = input
            .destination_options
            .as_ref()
            .and_then(|options| options.id.clone());
        let id_slots: Vec<(String, String)> = [(
            format!("{}.Id", ID_BEARING_MEMBERS[0]),
            supplied_origin_id.clone(),
        )]
        .into_iter()
        .chain(submitted.iter().enumerate().map(|(index, waypoint)| {
            (
                format!("{}[{index}].Id", ID_BEARING_MEMBERS[1]),
                waypoint.id.clone(),
            )
        }))
        .chain([(
            format!("{}.Id", ID_BEARING_MEMBERS[2]),
            supplied_destination_id.clone(),
        )])
        .filter_map(|(path, id)| id.map(|id| (path, id)))
        .collect();
        for (position, (path, id)) in id_slots.iter().enumerate() {
            if id_slots[..position].iter().any(|(_, earlier)| earlier == id) {
                return Err(invalid(path, ids_must_be_unique()));
            }
        }
        let mut taken: Vec<String> = id_slots.into_iter().map(|(_, id)| id).collect();
        let origin_id = assign_id(supplied_origin_id, "Origin", &mut taken);
        let waypoint_ids: Vec<String> = submitted
            .iter()
            .enumerate()
            .map(|(index, waypoint)| assign_id(waypoint.id.clone(), &index.to_string(), &mut taken))
            .collect();
        let destination_id = assign_id(supplied_destination_id, "Destination", &mut taken);

        // The origin, every waypoint and the destination, as the sequencer sees them.
        let origin = Stop {
            id: origin_id,
            position: state::validate_position(&input.origin)
                .map_err(|message| invalid("Origin", message))?,
            service_duration_s: 0,
            appointment: None,
            before: Vec::new(),
            waypoint_index: None,
        };
        let mut waypoints = Vec::new();
        for (index, waypoint) in submitted.iter().enumerate() {
            let member = format!("Waypoints[{index}].Position");
            waypoints.push(Stop {
                id: waypoint_ids[index].clone(),
                position: state::validate_position(&waypoint.position)
                    .map_err(|message| invalid(&member, message))?,
                service_duration_s: waypoint.service_duration,
                appointment: match &waypoint.appointment_time {
                    None => None,
                    Some(value) => {
                        let member = format!("Waypoints[{index}].AppointmentTime");
                        Some(
                            state::parse_timestamp(&member, value)
                                .map_err(|message| invalid(&member, message))?,
                        )
                    }
                },
                before: waypoint
                    .before
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|index| usize::try_from(*index).ok())
                    .collect(),
                waypoint_index: Some(index),
            });
        }
        let destination = match &input.destination {
            None => None,
            Some(position) => {
                let options = input.destination_options.as_ref();
                Some(Stop {
                    id: destination_id,
                    position: state::validate_position(position)
                        .map_err(|message| invalid("Destination", message))?,
                    service_duration_s: options.map_or(0, |options| options.service_duration),
                    appointment: match options.and_then(|options| options.appointment_time.clone()) {
                        None => None,
                        Some(value) => Some(
                            state::parse_timestamp("DestinationOptions.AppointmentTime", &value)
                                .map_err(|message| invalid("DestinationOptions.AppointmentTime", message))?,
                        ),
                    },
                    before: Vec::new(),
                    waypoint_index: None,
                })
            }
        };

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // The clusters the request asked for, by stop id. They are computed BEFORE the
        // sequencing below, because a cluster is what the sequencer keeps together, and
        // they are what every optimized waypoint reports its `ClusterIndex` from. A
        // request that asked for no clustering has none, and reports none.
        let clusters: HashMap<String, i32> = match clustering {
            None => HashMap::new(),
            Some(clustering) => {
                let stops: Vec<&Stop> = std::iter::once(&origin)
                    .chain(waypoints.iter())
                    .chain(destination.iter())
                    .collect();
                let mut state = state_handle.lock();
                clustering.assign(&mut state, &stops, travel_mode, objective)
            }
        };

        // Every stop of the journey, in the order the request named them: the origin
        // first, then the submitted waypoints, then the destination where the request
        // named one. The sequencing below reorders the WAYPOINTS between those two fixed
        // ends — the journey starts at the origin and, when there is one, ends at the
        // destination.
        let ends_at_destination = destination.is_some();
        let stops: Vec<Stop> = std::iter::once(origin)
            .chain(waypoints)
            .chain(destination)
            .collect();
        let last_stop = stops.len() - 1;

        // The cost of every hop the journey could take, from the one cost function in
        // [`crate::state`]: `hops[from][to]` is the distance and the duration of
        // travelling from one stop to another. The WHOLE table is costed before anything
        // is sequenced, because the order with the least total cost cannot be read off one
        // hop at a time. Nothing travels back to the origin and nothing travels on from
        // the destination, so those hops are not costed.
        //
        // The lock on the shared state is taken once per stop — at most `Waypoints.len()`
        // acquisitions, itself capped by the documented maximum above — and released
        // between them, where the handler yields, so a long request neither holds the lock
        // for the whole of it nor leaves a client's cancellation nowhere to land.
        let mut hops: Vec<Vec<(i64, i64)>> = vec![vec![(0, 0); stops.len()]; stops.len()];
        for from in 0..stops.len() {
            if ends_at_destination && from == last_stop {
                continue;
            }
            {
                let mut state = state_handle.lock();
                for to in 1..stops.len() {
                    if to == from {
                        continue;
                    }
                    let route =
                        state.compute_route(&[stops[from].position, stops[to].position], travel_mode, objective, 0);
                    hops[from][to] = (route.distance_m, route.duration_s);
                }
            }
            tokio::task::yield_now().await;
        }

        // What the objective minimises: the travel time for `FastestRoute`, the distance
        // travelled for `ShortestRoute`.
        let hop_costs: Vec<Vec<i64>> = hops
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(distance_m, duration_s)| match objective {
                        RoutingObjective::FastestRoute => *duration_s,
                        RoutingObjective::ShortestRoute => *distance_m,
                    })
                    .collect()
            })
            .collect();
        // `Before` names waypoints by their index in `Waypoints`, which is one less than
        // their index among the stops.
        let must_follow: Vec<Vec<usize>> = stops
            .iter()
            .map(|stop| stop.before.iter().map(|index| index + 1).collect())
            .collect();
        let cluster_of: Vec<Option<i32>> = stops
            .iter()
            .map(|stop| clusters.get(&stop.id).copied())
            .collect();

        // The stops in VISITING order, which is what every connection, optimized waypoint
        // and total below is read off.
        let visiting: Vec<usize> = std::iter::once(0)
            .chain(optimal_order(
                &hop_costs,
                &must_follow,
                &cluster_of,
                ends_at_destination,
            ))
            .chain(ends_at_destination.then_some(last_stop))
            .collect();
        let sequence: Vec<Stop> = visiting.iter().map(|index| stops[*index].clone()).collect();
        // The cost of each hop of the computed sequence, read off the same table the order
        // was chosen from, so the answer's connections cannot disagree with the choice.
        let routed: Vec<(i64, i64)> = visiting.windows(2).map(|pair| hops[pair[0]][pair[1]]).collect();
        let pricing_bucket = state_handle.lock().pricing_bucket(Self::NAME);

        let treat_service_time_as_rest = input
            .driver
            .as_ref()
            .and_then(|driver| driver.treat_service_time_as.as_deref())
            == Some("Rest");
        let rest_cycles = input.driver.as_ref().and_then(|driver| driver.rest_cycles.clone());

        let mut connections = Vec::new();
        let mut optimized = Vec::new();
        let mut impeding = Vec::new();
        let mut breakdown = WaypointOptimizationTimeBreakdown::default();
        let mut schedule = RestSchedule::default();
        let mut clock = departure;

        optimized.push(WaypointOptimizationOptimizedWaypoint {
            arrival_time: None,
            cluster_index: clusters.get(&sequence[0].id).copied(),
            departure_time: clock_offset
                .timestamp(clock)
                .ok_or_else(|| internal("the departure time is not representable"))?,
            id: sequence[0].id.clone(),
            position: state::position_to_vec(sequence[0].position),
        });

        for (index, pair) in sequence.windows(2).enumerate() {
            let (from, to) = (&pair[0], &pair[1]);
            let (distance_m, travel_duration) = routed[index];
            let rest_duration = schedule.rest_after(travel_duration, rest_cycles.as_ref());
            let arrival = state::shift_seconds(clock, travel_duration.saturating_add(rest_duration));
            // An appointment the traveller reaches early is waited out; one already past
            // is a failed `AppointmentTime` constraint, not a negative wait.
            let wait_duration: i64 = to
                .appointment
                .map_or(0, |appointment| state::seconds_between(arrival, appointment).max(0));
            let service_duration = to.service_duration_s;
            if treat_service_time_as_rest {
                schedule.rest(service_duration, rest_cycles.as_ref());
            } else {
                schedule.work(service_duration);
            }
            let departure_from_to =
                state::shift_seconds(arrival, wait_duration.saturating_add(service_duration));

            connections.push(WaypointOptimizationConnection {
                distance: distance_m,
                from: from.id.clone(),
                rest_duration,
                to: to.id.clone(),
                travel_duration,
                wait_duration,
            });
            optimized.push(WaypointOptimizationOptimizedWaypoint {
                arrival_time: clock_offset.timestamp(arrival),
                // The cluster this stop was assigned to, for a request that asked for
                // clustering; a stop of a request that did not asks for no index.
                cluster_index: clusters.get(&to.id).copied(),
                departure_time: clock_offset
                    .timestamp(departure_from_to)
                    .ok_or_else(|| internal("a departure time is not representable"))?,
                id: to.id.clone(),
                position: state::position_to_vec(to.position),
            });

            breakdown.travel_duration =
                state::clamp_cost(breakdown.travel_duration.saturating_add(travel_duration) as f64);
            breakdown.rest_duration = state::clamp_cost(breakdown.rest_duration.saturating_add(rest_duration) as f64);
            breakdown.wait_duration = state::clamp_cost(breakdown.wait_duration.saturating_add(wait_duration) as f64);
            breakdown.service_duration =
                state::clamp_cost(breakdown.service_duration.saturating_add(service_duration) as f64);

            if let Some(appointment) = to.appointment {
                if arrival > appointment {
                    impeding.push(impeding_waypoint(
                        to,
                        "AppointmentTime",
                        format!(
                            "the computed arrival is {} seconds after the appointment",
                            state::seconds_between(appointment, arrival)
                        ),
                    ));
                }
            }
            clock = departure_from_to;
        }

        // `Before` says which waypoints must be visited after this one. The sequencer
        // honours it: a waypoint another unvisited one requires to come later is not
        // eligible yet. A cyclic constraint set cannot be satisfied by any order, so the
        // sequencer falls back to cost and the constraints left unmet are reported here as
        // `ImpedingWaypoints` — which is also the check that the honoured case passes.
        let order: Vec<Option<usize>> = sequence.iter().map(|stop| stop.waypoint_index).collect();
        for (place, stop) in sequence.iter().enumerate() {
            for required_after in &stop.before {
                let later = order
                    .iter()
                    .position(|index| *index == Some(*required_after))
                    .is_some_and(|position| position > place);
                if !later {
                    impeding.push(impeding_waypoint(
                        stop,
                        "Before",
                        format!("Waypoints[{required_after}] is not sequenced after this waypoint"),
                    ));
                }
            }
        }

        Ok(OptimizeWaypointsOutput {
            distance: state::clamp_cost(
                connections
                    .iter()
                    .map(|c| c.distance)
                    .fold(0_i64, i64::saturating_add) as f64,
            ),
            // What the whole cycle TAKES, which is every phase the breakdown splits it
            // into: travelling, the rests the driver's cycles owe, the waits for an
            // appointment, and the service time spent at the waypoints. A total that left
            // the service time out would contradict the breakdown reporting it.
            duration: state::clamp_cost(
                breakdown
                    .travel_duration
                    .saturating_add(breakdown.rest_duration)
                    .saturating_add(breakdown.wait_duration)
                    .saturating_add(breakdown.service_duration) as f64,
            ),
            connections,
            impeding_waypoints: impeding,
            optimized_waypoints: optimized,
            pricing_bucket,
            time_breakdown: breakdown,
        })
    }
}

/// The documented maximum number of waypoints per request, and the lower maximum that
/// applies when any waypoint uses a constraint.
const MAX_WAYPOINTS: usize = 50;
const MAX_CONSTRAINED_WAYPOINTS: usize = 20;

/// Up to this many waypoints the visiting order is searched EXACTLY, over the subsets of
/// the waypoints already visited ([`exact_order`]). Beyond it an exact search is more work
/// than one request may ask for, so the order is grown and then improved
/// ([`improved_order`]).
const EXACT_SEQUENCING_MAX: usize = 12;

/// How many improvement passes [`improved_order`] makes over a grown order. A pass that
/// improves nothing stops the loop, so this only bounds the work of a request whose order
/// keeps improving.
const SEQUENCING_PASSES: usize = 8;

/// The order the journey visits the waypoints in: the one whose TOTAL cost is least.
///
/// `costs[from][to]` is the cost of the hop between two stops, in whatever the objective
/// minimises. Stop `0` is the origin, stops `1..` the submitted waypoints and, when
/// `ends_at_destination`, the last stop is the destination. The answer is the waypoints in
/// visiting order.
///
/// This operation "calculates the optimal order to travel between a set of waypoints to
/// minimize either the travel time or the distance travelled during the journey", so what
/// is minimised is the cost of the WHOLE journey — the origin, every waypoint and the
/// destination where there is one — and not the cost of the next hop. Taking the cheapest
/// hop at each step does not minimise the journey: a cheap first hop can leave an
/// expensive remainder, and the sequence a client dispatches its stops in would then be
/// one it could have beaten.
///
/// `must_follow[stop]` names the stops that have to be visited after that stop (`Before`)
/// and `clusters[stop]` the cluster it was assigned to; an order breaking either is not
/// considered. Constraints no order can satisfy at once — a `Before` cycle — are dropped a
/// tier at a time rather than left to make the request unanswerable, and the constraints
/// the resulting order leaves unmet are what the caller is told about through
/// `ImpedingWaypoints`.
fn optimal_order(
    costs: &[Vec<i64>],
    must_follow: &[Vec<usize>],
    clusters: &[Option<i32>],
    ends_at_destination: bool,
) -> Vec<usize> {
    let stops: Vec<usize> = (1..costs.len() - usize::from(ends_at_destination)).collect();
    if stops.len() < 2 {
        return stops;
    }
    for (honour_before, honour_clusters) in [(true, true), (false, true), (false, false)] {
        let rules = Rules {
            must_follow,
            clusters,
            stops: &stops,
            honour_before,
            honour_clusters,
        };
        let order = if stops.len() <= EXACT_SEQUENCING_MAX {
            exact_order(costs, &rules, ends_at_destination)
        } else {
            improved_order(costs, &rules, ends_at_destination)
        };
        if let Some(order) = order {
            return order.into_iter().map(|local| stops[local]).collect();
        }
    }
    stops
}

/// Which waypoint may be visited next, over the constraints the request set.
struct Rules<'a> {
    /// By stop, the stops that must be visited after it.
    must_follow: &'a [Vec<usize>],
    /// By stop, the cluster it was assigned to.
    clusters: &'a [Option<i32>],
    /// The stops being sequenced, by the LOCAL index the search uses.
    stops: &'a [usize],
    honour_before: bool,
    honour_clusters: bool,
}

impl Rules<'_> {
    /// Whether the waypoint at local index `next` may be visited now: with `visited` (a
    /// bit per local index) behind the traveller and `last` the waypoint reached last, or
    /// `None` while the traveller is still at the origin.
    fn allows(&self, visited: u64, last: Option<usize>, next: usize) -> bool {
        let stop = self.stops[next];
        if self.honour_before {
            // A waypoint another unvisited waypoint requires to come after it cannot be
            // visited yet.
            let waiting_on = self.stops.iter().enumerate().any(|(local, earlier)| {
                local != next
                    && visited & bit(local) == 0
                    && self.must_follow[*earlier].contains(&stop)
            });
            if waiting_on {
                return false;
            }
        }
        if self.honour_clusters {
            // Clustering "improves the optimized sequence": the waypoints of one cluster
            // are visited together, so the traveller leaves a cluster only once its own
            // waypoints are all behind.
            let current = match last {
                // The origin is a stop of the journey and so of a cluster too.
                None => self.clusters[0],
                Some(last) => self.clusters[self.stops[last]],
            };
            if let Some(cluster) = current {
                if self.clusters[stop] != Some(cluster) {
                    let left_behind = self.stops.iter().enumerate().any(|(local, waypoint)| {
                        local != next && visited & bit(local) == 0 && self.clusters[*waypoint] == Some(cluster)
                    });
                    if left_behind {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Whether a whole order of local indexes keeps to the rules, which is the same check
    /// walked from the origin.
    fn allows_order(&self, order: &[usize]) -> bool {
        let mut visited = 0_u64;
        let mut last = None;
        for &local in order {
            if !self.allows(visited, last, local) {
                return false;
            }
            visited |= bit(local);
            last = Some(local);
        }
        true
    }
}

/// The bit a local index occupies in a visited set. A request carries at most
/// [`MAX_WAYPOINTS`] waypoints, which is fewer than the bits of the set.
fn bit(local: usize) -> u64 {
    debug_assert!(local < u64::BITS as usize, "a visited set covers every waypoint");
    1_u64 << (local % u64::BITS as usize)
}

/// The cost of a whole order: the hops from the origin through the waypoints and on to the
/// destination where there is one.
fn order_cost(costs: &[Vec<i64>], rules: &Rules<'_>, order: &[usize], ends_at_destination: bool) -> i64 {
    let mut total: i64 = 0;
    let mut from = 0;
    for &local in order {
        let to = rules.stops[local];
        total = total.saturating_add(costs[from][to]);
        from = to;
    }
    if ends_at_destination {
        total = total.saturating_add(costs[from][costs.len() - 1]);
    }
    total
}

/// The cheapest way to have reached one waypoint: what that cost, and the waypoint visited
/// before it — `None` for the first of the order, which was reached from the origin.
type Reached = Option<(i64, Option<usize>)>;

/// The cheapest order, searched exactly: for every SET of waypoints already visited and
/// every waypoint the traveller could be at, the least cost of having visited that set and
/// stopped there. A set is reached from the sets one waypoint smaller, so each is costed
/// once however many orders lead to it, and the cheapest full set is the cheapest journey.
///
/// `None` when the rules admit no order at all.
fn exact_order(costs: &[Vec<i64>], rules: &Rules<'_>, ends_at_destination: bool) -> Option<Vec<usize>> {
    let count = rules.stops.len();
    let sets = 1_usize.checked_shl(u32::try_from(count).ok()?)?;
    // By visited set and waypoint reached last.
    let mut best: Vec<Vec<Reached>> = vec![vec![None; count]; sets];
    for first in 0..count {
        if rules.allows(0, None, first) {
            best[bit(first) as usize][first] = Some((costs[0][rules.stops[first]], None));
        }
    }
    for visited in 1..sets {
        for last in 0..count {
            if visited & bit(last) as usize == 0 {
                continue;
            }
            let Some((reached, _)) = best[visited][last] else {
                continue;
            };
            for next in 0..count {
                if visited & bit(next) as usize != 0 || !rules.allows(visited as u64, Some(last), next) {
                    continue;
                }
                let cost = reached.saturating_add(costs[rules.stops[last]][rules.stops[next]]);
                let entry = &mut best[visited | bit(next) as usize][next];
                if entry.is_none_or(|(known, _)| cost < known) {
                    *entry = Some((cost, Some(last)));
                }
            }
        }
    }

    let full = sets - 1;
    let mut end: Option<(i64, usize)> = None;
    for last in 0..count {
        let Some((reached, _)) = best[full][last] else {
            continue;
        };
        let total = if ends_at_destination {
            reached.saturating_add(costs[rules.stops[last]][costs.len() - 1])
        } else {
            reached
        };
        if end.is_none_or(|(known, _)| total < known) {
            end = Some((total, last));
        }
    }

    // Walk the choices back from the cheapest end to the first waypoint.
    let (_, mut last) = end?;
    let mut visited = full;
    let mut order = Vec::with_capacity(count);
    loop {
        order.push(last);
        let (_, previous) = best[visited][last]?;
        visited ^= bit(last) as usize;
        match previous {
            Some(previous) => last = previous,
            None => break,
        }
    }
    order.reverse();
    Some(order)
}

/// The cheapest order found by growing one and then improving it, for a request with more
/// waypoints than an exact search may be asked for.
///
/// The order is grown by taking the cheapest hop the rules allow, which is a feasible
/// order but not the cheapest one, and then improved: reversing a stretch of it is kept
/// whenever that makes the whole journey cheaper and still keeps to the rules, until a
/// pass finds no improvement. `None` when the rules admit no order at all.
fn improved_order(costs: &[Vec<i64>], rules: &Rules<'_>, ends_at_destination: bool) -> Option<Vec<usize>> {
    let count = rules.stops.len();
    let mut order: Vec<usize> = Vec::with_capacity(count);
    let mut visited = 0_u64;
    while order.len() < count {
        let last = order.last().copied();
        let next = (0..count)
            .filter(|local| visited & bit(*local) == 0 && rules.allows(visited, last, *local))
            .min_by_key(|local| {
                let from = last.map_or(0, |last| rules.stops[last]);
                costs[from][rules.stops[*local]]
            })?;
        visited |= bit(next);
        order.push(next);
    }

    let mut cost = order_cost(costs, rules, &order, ends_at_destination);
    for _ in 0..SEQUENCING_PASSES {
        let mut improved = false;
        for from in 0..count {
            for to in from + 1..count {
                let mut candidate = order.clone();
                candidate[from..=to].reverse();
                let candidate_cost = order_cost(costs, rules, &candidate, ends_at_destination);
                if candidate_cost < cost && rules.allows_order(&candidate) {
                    order = candidate;
                    cost = candidate_cost;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    Some(order)
}

/// The upper bound of the model's `DurationSeconds` range, which `ServiceDuration`
/// targets (`0..4294967295`).
const MAX_SERVICE_DURATION_S: i64 = 4_294_967_295;

/// `Heading`'s modeled range, in degrees.
const HEADING_BOUNDS: (f64, f64) = (0.0, 360.0);

/// Refuse a service duration outside the range the model declares for it.
fn check_service_duration(member: &str, seconds: i64) -> Result<(), OptimizeWaypointsError> {
    state::check_range(member, (0.0, MAX_SERVICE_DURATION_S as f64), seconds as f64)
        .map_err(|message| invalid(member, message))
}

/// Refuse a heading outside the range the model declares for it.
fn check_heading(member: &str, heading: f64) -> Result<(), OptimizeWaypointsError> {
    state::check_range(member, HEADING_BOUNDS, heading).map_err(|message| invalid(member, message))
}

/// The id of a stop: the one the caller supplied, or `preferred` made unique against the
/// ids already in use. Recorded in `taken`, so two made-up ids cannot collide either.
fn assign_id(supplied: Option<String>, preferred: &str, taken: &mut Vec<String>) -> String {
    if let Some(id) = supplied {
        return id;
    }
    let mut candidate = preferred.to_string();
    let mut suffix = 2;
    while taken.contains(&candidate) {
        candidate = format!("{preferred}-{suffix}");
        suffix += 1;
    }
    taken.push(candidate.clone());
    candidate
}

/// How the request asked nearby waypoints to be clustered, and with what.
#[derive(Debug, Clone, Copy)]
enum Clustering {
    /// "DrivingDistance assigns all the waypoints that are within driving distance of each
    /// other into a single cluster", with the distance the request configured.
    DrivingDistance { distance_m: i64 },
    /// "TopologySegment assigns all the waypoints that are within the same topology
    /// segment into a single cluster. A Topology segment is a linear stretch of road
    /// between two junctions."
    TopologySegment,
}

impl Clustering {
    /// The clustering a request's `Clustering` asks for, or a refusal naming the member it
    /// broke.
    fn resolve(options: &WaypointOptimizationClusteringOptions) -> Result<Self, OptimizeWaypointsError> {
        if !WAYPOINT_OPTIMIZATION_CLUSTERING_ALGORITHM_VALUES.contains(&options.algorithm.as_str()) {
            return Err(invalid(
                "Clustering.Algorithm",
                state::must_be_one_of(
                    "Clustering.Algorithm",
                    WAYPOINT_OPTIMIZATION_CLUSTERING_ALGORITHM_VALUES,
                ),
            ));
        }
        if options.algorithm == TOPOLOGY_SEGMENT_ALGORITHM {
            // "Driving distance options to be used when the clustering algorithm is
            // DrivingDistance": they configure the other algorithm, so they act on nothing
            // here.
            return Ok(Self::TopologySegment);
        }
        // `DrivingDistanceOptions` is the options structure "to be used when the
        // clustering algorithm is DrivingDistance". It is optional while its only member
        // is `@required` with `@default: 5` and `@range` 5..50000, so a request that sets
        // no options structure asks for that member's own default rather than for no
        // clustering. The modeled range is not enforced on a defaulted member, so it is
        // checked here.
        let member = "Clustering.DrivingDistanceOptions.DrivingDistance";
        let distance_m = options
            .driving_distance_options
            .as_ref()
            .map_or(DEFAULT_DRIVING_DISTANCE_M, |options| options.driving_distance);
        state::check_range(member, DRIVING_DISTANCE_BOUNDS, distance_m as f64)
            .map_err(|message| invalid(member, message))?;
        Ok(Self::DrivingDistance { distance_m })
    }

    /// The cluster of each stop, by stop id. The clusters are numbered in the order the
    /// stops are submitted, so the origin's cluster is the first; a stop the algorithm
    /// places in no cluster — one lying on no topology segment, because the network holds
    /// no road for it — reports no index.
    fn assign(
        self,
        state: &mut state::ComAmazonawsGeoroutesRoutesserviceState,
        stops: &[&Stop],
        mode: TravelMode,
        objective: RoutingObjective,
    ) -> HashMap<String, i32> {
        let groups: Vec<Option<usize>> = match self {
            Self::DrivingDistance { distance_m } => driving_distance_groups(state, stops, mode, objective, distance_m),
            Self::TopologySegment => topology_segment_groups(state, stops, mode),
        };
        // The group ids above are whichever representative each algorithm settled on, so
        // they are renumbered into the indices the answer reports.
        let mut numbering: Vec<usize> = Vec::new();
        let mut clusters = HashMap::new();
        for (stop, group) in stops.iter().zip(groups) {
            let Some(group) = group else {
                continue;
            };
            let index = match numbering.iter().position(|seen| *seen == group) {
                Some(index) => index,
                None => {
                    numbering.push(group);
                    numbering.len() - 1
                }
            };
            if let Ok(index) = i32::try_from(index) {
                clusters.insert(stop.id.clone(), index);
            }
        }
        clusters
    }
}

/// The `WaypointOptimizationClusteringAlgorithm` that clusters by topology segment; the
/// other value of the enum is the driving-distance one.
const TOPOLOGY_SEGMENT_ALGORITHM: &str = "TopologySegment";

/// `WaypointOptimizationDrivingDistance`'s modeled range, in metres, and the default its
/// member declares (`@range` min 5, max 50000; `@default` 5, on a `@required` member).
const DRIVING_DISTANCE_BOUNDS: (f64, f64) = (5.0, 50_000.0);
const DEFAULT_DRIVING_DISTANCE_M: i64 = 5;

/// Groups the stops that are within `distance_m` of each other BY ROAD into one cluster:
/// the relation is transitive, so two stops connected through a third belong to the same
/// cluster even where the road between them is longer than the configured distance.
///
/// The driving distance of a pair is the one the shared cost function computes for it, so
/// a cluster is what the same request's connections are costed against. A road route is
/// never shorter than the straight line between its endpoints, so a pair further apart
/// than the configured distance as the crow flies cannot be within driving distance and is
/// not routed at all. A stop the network holds no road for is within driving distance of
/// nothing, so it ends up in a cluster of its own rather than in none: the distance
/// relation places every stop, where a topology segment exists only under a road.
fn driving_distance_groups(
    state: &mut state::ComAmazonawsGeoroutesRoutesserviceState,
    stops: &[&Stop],
    mode: TravelMode,
    objective: RoutingObjective,
    distance_m: i64,
) -> Vec<Option<usize>> {
    let mut parent: Vec<usize> = (0..stops.len()).collect();
    for i in 0..stops.len() {
        for j in i + 1..stops.len() {
            if state::haversine_m(stops[i].position, stops[j].position) > distance_m as f64 {
                continue;
            }
            let route = state.compute_route(&[stops[i].position, stops[j].position], mode, objective, 0);
            if route.connected && route.distance_m <= distance_m {
                let (a, b) = (find_group(&mut parent, i), find_group(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    (0..stops.len())
        .map(|index| Some(find_group(&mut parent, index)))
        .collect()
}

/// The representative of `index`'s group, with the path to it compressed.
fn find_group(parent: &mut [usize], index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    let mut walk = index;
    while parent[walk] != root {
        let next = parent[walk];
        parent[walk] = root;
        walk = next;
    }
    root
}

/// Groups the stops that lie on the same topology segment — one stretch of one road
/// between two junctions — into one cluster. A stop the network holds no road for lies on
/// no segment, so it is in no cluster.
fn topology_segment_groups(
    state: &mut state::ComAmazonawsGeoroutesRoutesserviceState,
    stops: &[&Stop],
    mode: TravelMode,
) -> Vec<Option<usize>> {
    let mut segments: Vec<(RoadOrientation, i64, i64)> = Vec::new();
    stops
        .iter()
        .map(|stop| {
            let snapped = state.snap(stop.position, mode)?;
            let segment = (
                snapped.road.orientation,
                snapped.road.grid_index,
                snapped.road.topology_segment(snapped.position),
            );
            Some(match segments.iter().position(|seen| *seen == segment) {
                Some(index) => index,
                None => {
                    segments.push(segment);
                    segments.len() - 1
                }
            })
        })
        .collect()
}

/// One stop of the sequence: the origin, a waypoint or the destination.
#[derive(Debug, Clone)]
struct Stop {
    id: String,
    position: Position,
    service_duration_s: i64,
    appointment: Option<Instant>,
    /// `Before`: indices in `Waypoints` that must be visited after this stop.
    before: Vec<usize>,
    /// Index in the request's `Waypoints` list, for the stops that have one.
    waypoint_index: Option<usize>,
}

/// The driver's work-rest state while the sequence is walked.
///
/// "A rest needs to be taken after the short work duration. The short cycle can be
/// repeated until you hit the long work duration, at which point the long rest duration is
/// taken."
#[derive(Debug, Default)]
struct RestSchedule {
    work_since_short_rest: i64,
    work_since_long_rest: i64,
}

impl RestSchedule {
    /// Count `seconds` of work, without resting.
    fn work(&mut self, seconds: i64) {
        self.work_since_short_rest = self.work_since_short_rest.saturating_add(seconds);
        self.work_since_long_rest = self.work_since_long_rest.saturating_add(seconds);
    }

    /// Count `seconds` of rest: any rest starts the short cycle again, and a rest as long
    /// as the long cycle's own rest starts that cycle again too.
    fn rest(&mut self, seconds: i64, cycles: Option<&WaypointOptimizationRestCycles>) {
        if seconds <= 0 {
            return;
        }
        self.work_since_short_rest = 0;
        if cycles.is_some_and(|cycles| {
            cycles.long_cycle.rest_duration > 0 && seconds >= cycles.long_cycle.rest_duration
        }) {
            self.work_since_long_rest = 0;
        }
    }

    /// Work `travel` seconds, then the rest the schedule now owes, in seconds.
    fn rest_after(&mut self, travel: i64, cycles: Option<&WaypointOptimizationRestCycles>) -> i64 {
        self.work(travel);
        let Some(cycles) = cycles else {
            return 0;
        };
        if cycles.long_cycle.work_duration > 0 && self.work_since_long_rest >= cycles.long_cycle.work_duration {
            self.work_since_long_rest = 0;
            self.work_since_short_rest = 0;
            return cycles.long_cycle.rest_duration.max(0);
        }
        if cycles.short_cycle.work_duration > 0 && self.work_since_short_rest >= cycles.short_cycle.work_duration {
            self.work_since_short_rest = 0;
            return cycles.short_cycle.rest_duration.max(0);
        }
        0
    }
}

/// A waypoint the sequencer could not satisfy a constraint for.
fn impeding_waypoint(stop: &Stop, constraint: &str, reason: String) -> WaypointOptimizationImpedingWaypoint {
    WaypointOptimizationImpedingWaypoint {
        failed_constraints: vec![WaypointOptimizationFailedConstraint {
            constraint: Some(constraint.to_string()),
            reason: Some(reason),
        }],
        id: stop.id.clone(),
        position: state::position_to_vec(stop.position),
    }
}

/// The members that are each sufficient on their own to say where this journey goes.
/// `Origin` is `@required`; one of these must be set beside it.
const MUTUALLY_SUFFICIENT_MEMBERS: [&str; 2] = ["Destination", "Waypoints"];

/// The members of this operation's input that carry a stop's `Id`, in request order.
const ID_BEARING_MEMBERS: [&str; 3] = ["OriginOptions", "Waypoints", "DestinationOptions"];

/// The sentence for the rule that a stop's id must not repeat: it names the `Id` member
/// and the members an id can come from, and quotes no value out of the request — which
/// member the duplicate sits in is reported in the refusal's `fieldList` instead.
fn ids_must_be_unique() -> String {
    format!(
        "'Id' must be unique across {}.",
        state::quoted_members(&ID_BEARING_MEMBERS)
    )
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> OptimizeWaypointsError {
    OptimizeWaypointsErrorModeled::ValidationException(ValidationException {
        message: message.clone(),
        reason: "FieldValidationFailed".to_string(),
        field_list: vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }],
    })
    .into()
}

/// The model's own `AccessDeniedException` (HTTP 403) for a caller the IAM policies in
/// force do not permit to perform this action, carrying IAM's own sentence
/// ([`crate::state::identity_authorization`]).
fn access_denied(message: String) -> OptimizeWaypointsError {
    OptimizeWaypointsErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator
/// rather than of the request.
fn internal(message: &str) -> OptimizeWaypointsError {
    OptimizeWaypointsErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// Every per-mode option block `WaypointOptimizationTravelModeOptions` declares, in the
/// model's order, paired with whether the request set it — what
/// [`state::check_travel_mode_options`] holds against the selected `TravelMode`.
fn declared_travel_mode_options(
    options: Option<&WaypointOptimizationTravelModeOptions>,
) -> [(&'static str, bool); 2] {
    [
        (
            "Pedestrian",
            options.is_some_and(|set| set.pedestrian.is_some()),
        ),
        ("Truck", options.is_some_and(|set| set.truck.is_some())),
    ]
}
