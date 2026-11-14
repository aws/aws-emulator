// SPDX-License-Identifier: CC0-1.0

//! @op CalculateIsolines
//! The area reachable within each threshold, computed from the same road network and the
//! same cost function as every other operation of this service
//! ([`ComAmazonawsGeoroutesRoutesserviceState::compute_route`]): an isoline's boundary is
//! resolved one bearing at a time, by searching for the farthest point along that bearing
//! whose route to or from the anchor still costs no more than the threshold. A route to a
//! point ON an isoline therefore costs within that isoline's threshold and a route past it
//! costs more — an isoline and a route for the same travel mode and objective cannot
//! disagree.
//!
//! The anchor is the submitted `Origin` snapped onto the road network, or the snapped
//! `Destination` for the reversed calculation that shows "areas that could reach your
//! location": a destination-based isoline costs the route INTO the destination. Exactly one
//! of the two is the anchor — "either Origin or Destination must be provided" — so a request
//! that sets neither has nothing to calculate from, and one that sets both does not say
//! which of the two calculations it is asking for.
//!
//! Thresholds "all must be the same type", so a request carries either distance thresholds
//! or time thresholds, and the answer carries ONE isoline per threshold in the order the
//! request listed them, each reporting the threshold it was computed for.
//!
//! A boundary is a ring of vertices, wound counterclockwise as GeoJSON winds an exterior
//! ring and closed by repeating its first position. How many vertices it carries is what
//! `IsolineGranularity` asks for: `MaxResolution` is "the maximum distance in meters between
//! points along the isoline", so it is the spacing the vertices are placed at, and
//! `MaxPoints` is a maximum, so the ring carries the vertices that spacing asks for up to
//! that cap — never more than this emulator computes one boundary with, and never fewer than
//! the fewest points a caller may ask for. `OptimizeIsolineFor` is the documented trade-off
//! "between calculation speed and isoline precision", so it is what chooses how finely the
//! search resolves each bearing.
//!
//! Where the anchor matches no road of the network — it lies outside the mapped territory
//! the network covers — nothing is reachable and the isoline carries no geometry at all; and
//! a bearing whose corridor leaves that coverage is not carried by the network, which is
//! what clips an isoline at a coast. An isoline's `Connections` link the polygons of one
//! isoline to each other, and this model computes one polygon per isoline, so there is
//! nothing to link.
//!
//! `Avoid.Areas` and `Avoid.ControlledAccessHighways` answer 501, as they do on the sibling
//! operations: an avoidance area is a routing constraint this model does not compute around,
//! and a violated best-effort avoidance "produces a notice for the violation", which an
//! isoline has nowhere to carry. `Allow`, `Traffic` and `TravelModeOptions` have nothing to
//! act on — the model carries no HOV/HOT lane, traffic variation or vehicle restriction —
//! and the matching, heading and side-of-street options of `OriginOptions` /
//! `DestinationOptions` are validated but change nothing, because a position is matched onto
//! the nearest road its travel mode may use.
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
//! # Modeled contract for `CalculateIsolines`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! Calculates areas that can be reached within specified time or distance thresholds from a
//! given point. For example, you can use this operation to determine the area within a
//! 30-minute drive of a store location, find neighborhoods within walking distance of a
//! school, or identify delivery zones based on drive time. Isolines (also known as
//! isochrones for time-based calculations) are useful for various applications including:
//! Service area visualization - Show customers the area you can serve within promised
//! delivery times Site selection - Analyze potential business locations based on population
//! within travel distance Site selection - Determine areas that can be reached within
//! specified response times Route preferences such as avoiding toll roads or ferries are
//! treated as preferences rather than absolute restrictions. If a viable route cannot be
//! calculated while honoring all preferences, some may be ignored. For more information,
//! see Calculate isolines in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Allow` (IsolineAllowOptions) — Enables special road types or features that should be
//!   considered for routing even if they might be restricted by default for the selected
//!   travel mode. These include high-occupancy vehicle and toll lanes.
//! * `ArrivalTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Determine areas from which Destination can be reached by this time, taking into
//!   account predicted traffic conditions and working backward to account for congestion
//!   patterns. This attribute cannot be used together with DepartureTime or DepartNow.
//!   Specified as an ISO-8601 timestamp with timezone offset. Time format:
//!   YYYY-MM-DDThh:mm:ss.sssZ | YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples: 2020-04-22T17:57:24Z
//!   2020-04-22T17:57:24+02:00
//! * `Avoid` (IsolineAvoidanceOptions) — Specifies road types, features, or areas to avoid
//!   (if possible) when calculating reachable areas. These are treated as preferences rather
//!   than strict constraints—if a route cannot be calculated without using an avoided
//!   feature, that avoidance preference may be ignored.
//! * `DepartNow` (SensitiveBoolean) — When true, uses the current time as the departure time
//!   and takes current traffic conditions into account. This attribute cannot be used
//!   together with DepartureTime or ArrivalTime.
//! * `DepartureTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Determine areas that can be reached when departing at this time, taking into
//!   account predicted traffic conditions. This attribute cannot be used together with
//!   ArrivalTime or DepartNow. Specified as an ISO-8601 timestamp with timezone offset. Time
//!   format:YYYY-MM-DDThh:mm:ss.sssZ | YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples:
//!   2020-04-22T17:57:24Z 2020-04-22T17:57:24+02:00
//! * `Destination` (Position, len 2..2) — An optional destination point, specified as
//!   [longitude, latitude] coordinates. When provided, the service calculates areas from
//!   which this destination can be reached within the specified thresholds. This reverses the
//!   usual isoline calculation to show areas that could reach your location, rather than
//!   areas you could reach from your location. Either Origin or Destination must be provided.
//! * `DestinationOptions` (IsolineDestinationOptions) — Options that control how the
//!   destination point is matched to the road network and how routes can approach it. These
//!   options help improve travel time accuracy by accounting for real-world access to the
//!   destination.
//! * `IsolineGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — The format
//!   of the returned IsolineGeometry. Default value:FlexiblePolyline
//! * `IsolineGranularity` (IsolineGranularityOptions) — Controls the detail level of the
//!   generated isolines. Higher granularity produces smoother shapes but requires more
//!   processing time and results in larger responses.
//! * `Key` (ApiKey, len ..1000) — An Amazon Location Service API Key with access to this
//!   action. If omitted, the request must be signed using Signature Version 4.
//! * `OptimizeIsolineFor` (IsolineOptimizationObjective, one of: AccurateCalculation |
//!   BalancedCalculation | FastCalculation) — Controls the trade-off between calculation
//!   speed and isoline precision. Choose FastCalculation for quicker results with less
//!   detail, AccurateCalculation for more precise results, or BalancedCalculation for a
//!   middle ground. Default value: BalancedCalculation
//! * `OptimizeRoutingFor` (RoutingObjective, one of: FastestRoute | ShortestRoute) —
//!   Determines whether routes prioritize shortest travel time (FastestRoute) or shortest
//!   physical distance (ShortestRoute) when calculating reachable areas. Default value:
//!   FastestRoute
//! * `Origin` (Position, len 2..2) — The starting point for isoline calculations, specified
//!   as [longitude, latitude] coordinates. For example, this could be a store location,
//!   service center, or any point from which you want to calculate reachable areas. Either
//!   Origin or Destination must be provided.
//! * `OriginOptions` (IsolineOriginOptions) — Options that control how the origin point is
//!   matched to the road network and how routes can depart from it. These options help
//!   improve travel time accuracy by accounting for real-world access from the origin.
//! * `Thresholds` (IsolineThresholds) **required** — The distance or time thresholds used to
//!   determine reachable areas. You can specify up to five thresholds (which all must be the
//!   same type) to calculate multiple isolines in a single request. For example, to determine
//!   the areas that are reachable within 10 and 20 minutes of the origin, specify time
//!   thresholds of 600 and 1200 seconds. You incur a calculation charge for each threshold.
//!   Using a large number of thresholds in a request can lead to unexpected charges. For more
//!   information, see Routes pricing in the Amazon Location Service Developer Guide.
//! * `Traffic` (IsolineTrafficOptions) — Configures how real-time and historical traffic data
//!   affects isoline calculations. Traffic patterns can significantly impact reachable areas,
//!   especially during peak hours.
//! * `TravelMode` (IsolineTravelMode, one of: Car | Pedestrian | Scooter | Truck) — The mode
//!   of transportation to use for calculations. This affects which road types or features can
//!   be used, estimated speed, and the traffic levels that are applied. Car—Standard
//!   passenger vehicle routing using roads accessible to cars Pedestrian—Walking routes using
//!   pedestrian paths, sidewalks, and crossings Scooter—Light two-wheeled vehicle routing
//!   using roads and paths accessible to scooters Truck—Commercial truck routing considering
//!   vehicle dimensions, weight restrictions, and hazardous material regulations The mode
//!   Scooter also applies to motorcycles; set this to Scooter when calculating isolines for
//!   motorcycles. Default value: Car
//! * `TravelModeOptions` (IsolineTravelModeOptions) — Additional attributes that refine how
//!   reachable areas are calculated based on specific vehicle characteristics. These options
//!   help produce more accurate results by accounting for real-world constraints and
//!   capabilities. For example: For trucks (Truck), specify dimensions, weight limits, and
//!   hazardous cargo restrictions to ensure isolines only include roads that can physically
//!   and legally accommodate the vehicle For cars (Car), set maximum speed capabilities or
//!   indicate high-occupancy vehicle eligibility to better estimate reachable areas For
//!   scooters (Scooter), specify engine type and speed limitations to more accurately model
//!   their travel capabilities Without these options, calculations …
//!
//! ## Output
//!
//! * `ArrivalTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Time of arrival at the destination, used for traffic calculations. This attribute
//!   is returned only if the Destination and ArrivalTime attributes were provided in the
//!   request. Time format: YYYY-MM-DDThh:mm:ss.sssZ | YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples:
//!   2020-04-22T17:57:24Z 2020-04-22T17:57:24+02:00
//! * `DepartureTime` (TimestampWithTimezoneOffset, pattern
//!   ^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?
//!   …) — Time of departure from the origin, used for traffic calculations. This attribute is
//!   returned when Origin was provided in the request and either a specific departure time
//!   was requested (DepartureTime) or DepartNow was set to true. Time format:
//!   YYYY-MM-DDThh:mm:ss.sssZ | YYYY-MM-DDThh:mm:ss.sss+hh:mm Examples: 2020-04-22T17:57:24Z
//!   2020-04-22T17:57:24+02:00
//! * `IsolineGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required**
//!   — The format of the returned geometries, matching the format specified in the request.
//!   Either FlexiblePolyline for compact encoding or Simple for GeoJSON-compatible
//!   coordinates. Default value:FlexiblePolyline
//! * `Isolines` (IsolineList, len 1..5) **required** — Reachable areas, or isolines, for each
//!   threshold specified in the request.
//! * `PricingBucket` (String) **required** — The pricing bucket applied to this calculation.
//!   Different buckets apply based on the travel mode and thresholds used.
//! * `SnappedDestination` (Position, len 2..2) — The actual point on the road network used
//!   for calculations, which may differ from the requested destination if Destination was not
//!   directly on a road.
//! * `SnappedOrigin` (Position, len 2..2) — The actual point on the road network used for
//!   calculations, which may differ from the requested origin if Origin was not directly on a
//!   road.
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

use crate::state::{
    self, CachedRoute, ComAmazonawsGeoroutesRoutesserviceState, Instant, Position, RoutingObjective,
    TravelMode,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CalculateIsolinesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CalculateIsolinesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#CalculateIsolines")]
#[derive(Debug)]
pub enum CalculateIsolinesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::calculate_isolines::CalculateIsolinesError`). The wrapper is kept because
/// it is also where the two requests this road model cannot compute an area for are
/// declined; every refusal of a bad request below uses the model's own `ValidationException`.
pub type CalculateIsolinesError = crate::scaffolded::Scaffolded<CalculateIsolinesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CalculateIsolinesErrorModeled> for CalculateIsolinesError {
    fn from(modeled: CalculateIsolinesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CalculateIsolines` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CalculateIsolines<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CalculateIsolines<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CalculateIsolines<Eng>
{
    type Input = CalculateIsolinesInput;
    type Output = CalculateIsolinesOutput;
    type Error = CalculateIsolinesError;

    const NAME: &'static str = "CalculateIsolines";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ISOLINES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ISOLINES_OUTPUT_SCHEMA;
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

        // An avoidance area is a routing constraint this model does not compute around, and
        // an avoidance it can violate "produces a notice for the violation" that an isoline
        // has nowhere to carry, so both are declined rather than silently ignored. The rest
        // of `Avoid` is satisfied trivially: this network carries no toll road, ferry,
        // tunnel, dirt road, car shuttle train, seasonal closure or zone.
        if let Some(avoid) = input.avoid.as_ref() {
            if avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "CalculateIsolines with Avoid.Areas",
                ));
            }
            if avoid.controlled_access_highways == Some(true) {
                return Err(crate::scaffolded::Scaffolded::not_implemented(
                    "CalculateIsolines with Avoid.ControlledAccessHighways",
                ));
            }
        }

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, ISOLINE_TRAVEL_MODE_VALUES).ok_or_else(|| {
                invalid(
                    "TravelMode",
                    state::must_be_one_of("TravelMode", ISOLINE_TRAVEL_MODE_VALUES),
                )
            })?,
        };
        let objective = match input.optimize_routing_for.as_deref() {
            // `Default value: FastestRoute`.
            None => RoutingObjective::FastestRoute,
            Some(value) => RoutingObjective::from_model(value).ok_or_else(|| {
                invalid(
                    "OptimizeRoutingFor",
                    state::must_be_one_of("OptimizeRoutingFor", ROUTING_OBJECTIVE_VALUES),
                )
            })?,
        };
        let precision = match input.optimize_isoline_for.as_deref() {
            // `Default value: BalancedCalculation`.
            None => Precision::Balanced,
            Some(value) => Precision::from_model(value).ok_or_else(|| {
                invalid(
                    "OptimizeIsolineFor",
                    state::must_be_one_of("OptimizeIsolineFor", ISOLINE_OPTIMIZATION_OBJECTIVE_VALUES),
                )
            })?,
        };
        let geometry_format = state::geometry_format("IsolineGeometryFormat", input.isoline_geometry_format.as_ref())
            .map_err(|message| invalid("IsolineGeometryFormat", message))?;

        // A per-mode option block belongs to the one travel mode its member names, so a
        // request may set only the block the `TravelMode` it selected names. This
        // operation's options structure declares no block for every mode its travel-mode
        // enum admits, so a mode with none permits no block at all.
        let declared = declared_travel_mode_options(input.travel_mode_options.as_ref());
        let permitted: Vec<&str> = state::block_of_mode(travel_mode.as_model(), &declared)
            .into_iter()
            .collect();
        state::check_travel_mode_options(travel_mode.as_model(), &permitted, &declared)
            .map_err(|(member, message)| invalid(&member, message))?;

        let timing = Timing::resolve(&input)?;
        let anchor = Anchor::resolve(&input)?;
        let thresholds = resolve_thresholds(&input.thresholds)?;
        check_granularity(input.isoline_granularity.as_ref())?;
        if let Some(options) = input.origin_options.as_ref() {
            check_point_options("OriginOptions", options.heading, options.side_of_street.as_ref())?;
        }
        if let Some(options) = input.destination_options.as_ref() {
            check_point_options(
                "DestinationOptions",
                options.heading,
                options.side_of_street.as_ref(),
            )?;
        }

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // The anchor is matched onto the road network once: it is the point every route of
        // every threshold's search starts from or ends at, and the point the answer reports
        // as the snapped origin or destination.
        let (matched, pricing_bucket) = {
            let mut state = state_handle.lock();
            (
                state.snap(anchor.position, travel_mode).map(|snapped| snapped.position),
                state.pricing_bucket(Self::NAME),
            )
        };

        // One isoline per threshold, in the order the request listed them. Each boundary is
        // resolved under ONE acquisition of the shared state lock, which is released between
        // thresholds, and the handler yields there — so a five-threshold request neither
        // holds the lock for the whole request nor leaves a cancellation nowhere to land.
        let mut isolines = Vec::with_capacity(thresholds.len());
        for threshold in &thresholds {
            let ring = match matched {
                // No road of the network near the anchor: nothing is reachable from it, and
                // an invented ring would be absence dressed as an area.
                None => Vec::new(),
                Some(centre) => {
                    let mut state = state_handle.lock();
                    let search = Search {
                        threshold: *threshold,
                        mode: travel_mode,
                        objective,
                        outward: anchor.outward,
                        max_radius_m: threshold.max_radius_m(&state, travel_mode),
                        steps: precision.steps(),
                    };
                    let vertices = ring_vertices(input.isoline_granularity.as_ref(), search.max_radius_m);
                    reachable_ring(&mut state, centre, search, vertices)
                }
            };
            isolines.push(Isoline {
                // A connection links two polygons of ONE isoline; this model computes a
                // single polygon per isoline, so it has nothing to link.
                connections: Vec::new(),
                distance_threshold: threshold.distance_threshold(),
                geometries: isoline_geometries(&ring, &geometry_format)?,
                time_threshold: threshold.time_threshold(),
            });
            tokio::task::yield_now().await;
        }

        let matched_position = matched.map(state::position_to_vec);
        let (snapped_origin, snapped_destination) = if anchor.outward {
            (matched_position, None)
        } else {
            (None, matched_position)
        };
        Ok(CalculateIsolinesOutput {
            // The arrival is "returned only if the Destination and ArrivalTime attributes
            // were provided in the request". The departure is reported for every
            // origin-anchored request: it is "time of departure from the origin, used for
            // traffic calculations", so it is the instant the area was computed for, which
            // is the current one for a request that named no time.
            arrival_time: timing.reported_arrival(anchor.outward)?,
            departure_time: timing.reported_departure(anchor.outward)?,
            isoline_geometry_format: geometry_format,
            isolines,
            pricing_bucket,
            snapped_destination,
            snapped_origin,
        })
    }
}

/// Where the reachable area is measured from, and which way travel runs.
#[derive(Debug, Clone, Copy)]
struct Anchor {
    /// The submitted `Origin` or `Destination`.
    position: Position,
    /// Whether travel runs OUT of the anchor (an `Origin`) or INTO it (a `Destination`).
    outward: bool,
}

impl Anchor {
    /// The anchor a request named. "Either Origin or Destination must be provided", and the
    /// two ask for opposite calculations — areas reachable FROM a point against areas that
    /// can reach it — so a request names exactly one of them.
    fn resolve(input: &CalculateIsolinesInput) -> Result<Self, CalculateIsolinesError> {
        match (input.origin.as_deref(), input.destination.as_deref()) {
            (None, None) => Err(invalid(
                ORIGIN_MEMBER,
                state::at_least_one_of(&ANCHOR_MEMBERS),
            )),
            (Some(_), Some(_)) => Err(invalid(
                DESTINATION_MEMBER,
                state::only_one_of(&ANCHOR_MEMBERS),
            )),
            (Some(origin), None) => Ok(Self {
                position: state::validate_position(origin).map_err(|message| invalid(ORIGIN_MEMBER, message))?,
                outward: true,
            }),
            (None, Some(destination)) => Ok(Self {
                position: state::validate_position(destination)
                    .map_err(|message| invalid(DESTINATION_MEMBER, message))?,
                outward: false,
            }),
        }
    }
}

/// The two members a reachable area can be measured from, in the order this operation's own
/// documentation names them ("Either Origin or Destination must be provided"), which is the
/// order a refusal that lists them states them in.
const ANCHOR_MEMBERS: [&str; 2] = ["Origin", "Destination"];
const ORIGIN_MEMBER: &str = ANCHOR_MEMBERS[0];
const DESTINATION_MEMBER: &str = ANCHOR_MEMBERS[1];

/// The two members a threshold can be given under, in the order the model declares them.
const THRESHOLD_MEMBERS: [&str; 2] = ["Thresholds.Distance", "Thresholds.Time"];

/// The member a request carries its thresholds under: the container whose members are the
/// alternatives, which is what a refusal of more than one of them at once names.
const THRESHOLDS_MEMBER: &str = "Thresholds";

/// Every threshold kind the thresholds container holds, in the order this service
/// enumerates them. The set belongs to the CONTAINER rather than to the request, so a
/// refusal states all of them — including the consumption kind this revision of the
/// structure does not declare — and a caller discovering the rule from the refusal learns
/// the whole set it must pick exactly one of.
const THRESHOLD_KINDS: [&str; 3] = ["Consumption", "Distance", "Time"];

/// Which instant the calculation is anchored at, and the UTC offset the request stated it
/// at — the offset the answer reports that same instant back at.
#[derive(Debug, Clone, Copy)]
enum Timing {
    /// A departure: the `DepartureTime` the request named, or the current instant for
    /// `DepartNow` and for a request that named no time at all.
    Departure(Instant, state::UtcOffset),
    /// An arrival: `ArrivalTime`.
    Arrival(Instant, state::UtcOffset),
}

impl Timing {
    /// The instant a request anchored the calculation at. `DepartureTime`, `ArrivalTime`
    /// and `DepartNow` each pin it and each "cannot be used together with" the others, so
    /// naming more than one is contradictory and refused.
    ///
    /// A request that names NONE of them is anchored at the current instant: reachability
    /// is computed for a departure time, and the one a request that asks for no other
    /// departs at is now — which is exactly what `DepartNow` asks for explicitly.
    fn resolve(input: &CalculateIsolinesInput) -> Result<Self, CalculateIsolinesError> {
        let depart_now = input.depart_now == Some(true);
        let named = usize::from(input.departure_time.is_some())
            + usize::from(input.arrival_time.is_some())
            + usize::from(depart_now);
        if named > 1 {
            return Err(invalid(
                "DepartureTime",
                "DepartureTime, ArrivalTime and DepartNow each set the time of the calculation, so at most one may be supplied"
                    .to_string(),
            ));
        }
        if let Some(departure) = &input.departure_time {
            return Ok(Self::Departure(
                state::parse_timestamp("DepartureTime", departure)
                    .map_err(|message| invalid("DepartureTime", message))?,
                state::UtcOffset::stated_in(Some(departure)),
            ));
        }
        if let Some(arrival) = &input.arrival_time {
            return Ok(Self::Arrival(
                state::parse_timestamp("ArrivalTime", arrival).map_err(|message| invalid("ArrivalTime", message))?,
                state::UtcOffset::stated_in(Some(arrival)),
            ));
        }
        // The instant comes from this service's clock rather than from the request, so
        // there is no offset it was stated at and it is reported in UTC.
        Ok(Self::Departure(state::now(), state::UtcOffset::UTC))
    }

    /// The departure time the answer reports: the instant the calculation departs the
    /// origin at, at the offset the request stated it at.
    ///
    /// An origin-anchored request that pinned no instant of its own still departs at one —
    /// the current one, which is what `DepartNow` asks for explicitly — so a departure is
    /// reported for every such request. A request anchored at a destination departs from no
    /// one place, and one that pinned an ARRIVAL is reporting that arrival instead.
    fn reported_departure(self, outward: bool) -> Result<Option<String>, CalculateIsolinesError> {
        match self {
            Self::Departure(instant, offset) if outward => Ok(Some(
                offset
                    .timestamp(instant)
                    .ok_or_else(|| internal("the departure time is not representable"))?,
            )),
            _ => Ok(None),
        }
    }

    /// The arrival time the answer reports: the instant the request arrives by, for a
    /// request that gave a `Destination` and asked for one, at the offset the request
    /// stated it at.
    fn reported_arrival(self, outward: bool) -> Result<Option<String>, CalculateIsolinesError> {
        match self {
            Self::Arrival(instant, offset) if !outward => Ok(Some(
                offset
                    .timestamp(instant)
                    .ok_or_else(|| internal("the arrival time is not representable"))?,
            )),
            _ => Ok(None),
        }
    }
}

/// One threshold of a request: the distance or the time a reachable area is bounded by.
#[derive(Debug, Clone, Copy)]
enum Threshold {
    Distance(i64),
    Time(i64),
}

impl Threshold {
    /// The cost a route must not exceed to lie inside this threshold.
    fn limit(self) -> i64 {
        match self {
            Self::Distance(metres) => metres,
            Self::Time(seconds) => seconds,
        }
    }

    /// The cost of `route` that this threshold bounds.
    fn cost_of(self, route: &CachedRoute) -> i64 {
        match self {
            Self::Distance(_) => route.distance_m,
            Self::Time(_) => route.duration_s,
        }
    }

    /// How far from the anchor this threshold can possibly reach, in metres.
    ///
    /// A route's distance is the length of the path it travels along the roads, which is
    /// never shorter than the straight line between its endpoints, and its duration is that
    /// path at the speed of the roads it runs on — never above the travel mode's own top
    /// speed. So a distance threshold reaches at most its own metres, and a time threshold
    /// at most what the mode covers in its seconds. That makes this the upper bound the
    /// search starts from, and it is read off the same speeds the cost function uses.
    fn max_radius_m(
        self,
        state: &ComAmazonawsGeoroutesRoutesserviceState,
        mode: TravelMode,
    ) -> f64 {
        match self {
            Self::Distance(metres) => metres as f64,
            Self::Time(seconds) => seconds as f64 * state.speed_mps(mode),
        }
    }

    /// `Isoline$DistanceThreshold`: the distance this isoline was computed for, and NONE
    /// for a time-based one — the member reports the distance "used to calculate this
    /// isoline, if distance-based thresholds were specified in the request", so it does not
    /// apply to an isoline computed for a time and is absent from it.
    fn distance_threshold(self) -> Option<i64> {
        match self {
            Self::Distance(metres) => Some(metres),
            Self::Time(_) => None,
        }
    }

    /// `Isoline$TimeThreshold`, the mirror of [`Self::distance_threshold`].
    fn time_threshold(self) -> Option<i64> {
        match self {
            Self::Distance(_) => None,
            Self::Time(seconds) => Some(seconds),
        }
    }
}

/// The thresholds a request asked for, in the order it listed them.
///
/// "You can specify up to five thresholds (which all must be the same type)": the model
/// carries the count (`@length 1..5` on each list) and the per-threshold bounds, but not
/// that the two lists are alternatives, so that rule is enforced here. A request that sets
/// neither list asks for no area at all.
fn resolve_thresholds(thresholds: &IsolineThresholds) -> Result<Vec<Threshold>, CalculateIsolinesError> {
    let distance = thresholds.distance.as_deref().unwrap_or_default();
    let time = thresholds.time.as_deref().unwrap_or_default();
    if !distance.is_empty() && !time.is_empty() {
        return Err(invalid(
            THRESHOLDS_MEMBER,
            state::only_one_of_within(THRESHOLDS_MEMBER, &THRESHOLD_KINDS),
        ));
    }
    if distance.is_empty() && time.is_empty() {
        return Err(invalid(
            THRESHOLD_MEMBERS[0],
            state::at_least_one_of(&THRESHOLD_MEMBERS),
        ));
    }
    let mut resolved = Vec::with_capacity(distance.len() + time.len());
    for (index, metres) in distance.iter().enumerate() {
        let member = format!("{}[{index}]", THRESHOLD_MEMBERS[0]);
        state::check_range(&member, (0.0, MAX_DISTANCE_THRESHOLD_M as f64), *metres as f64)
            .map_err(|message| invalid(&member, message))?;
        resolved.push(Threshold::Distance(*metres));
    }
    for (index, seconds) in time.iter().enumerate() {
        let member = format!("{}[{index}]", THRESHOLD_MEMBERS[1]);
        state::check_range(&member, (0.0, MAX_TIME_THRESHOLD_S as f64), *seconds as f64)
            .map_err(|message| invalid(&member, message))?;
        resolved.push(Threshold::Time(*seconds));
    }
    Ok(resolved)
}

/// The modeled ranges of a threshold: `DistanceThresholdList`'s member narrows
/// `DistanceMeters` to `0..300000` and `TimeThresholdList`'s narrows `DurationSeconds` to
/// `0..10800`. The framework refuses a value outside them before this handler runs; these
/// are the belt, and they state the bounds a threshold is read against.
const MAX_DISTANCE_THRESHOLD_M: i64 = 300_000;
const MAX_TIME_THRESHOLD_S: i64 = 10_800;

/// The granularity a request asked for, against the bounds the model declares on it: the
/// framework's constraint walk refuses a value outside them first, and this is the belt.
fn check_granularity(granularity: Option<&IsolineGranularityOptions>) -> Result<(), CalculateIsolinesError> {
    let Some(granularity) = granularity else {
        return Ok(());
    };
    if let Some(max_points) = granularity.max_points {
        let member = "IsolineGranularity.MaxPoints";
        state::check_range(
            member,
            (MIN_RING_POSITIONS as f64, f64::from(i32::MAX)),
            f64::from(max_points),
        )
        .map_err(|message| invalid(member, message))?;
    }
    let member = "IsolineGranularity.MaxResolution";
    state::check_range(
        member,
        (0.0, state::MAX_DISTANCE_OR_DURATION as f64),
        granularity.max_resolution as f64,
    )
    .map_err(|message| invalid(member, message))
}

/// The options that control how a point is matched onto the road network, against the
/// bounds the model declares on them.
///
/// `Heading` carries a `@default` of 0, which the framework's constraint walk does not hold
/// to the member's `@range`, so the handler does; `SideOfStreet.Position` is a position like
/// any other and is held to the WGS 84 domain no model expresses. Neither changes the answer
/// — a point is matched onto the nearest road its travel mode may use — but a value outside
/// a declared bound is refused rather than served as if it were inside it.
fn check_point_options(
    member: &str,
    heading: f64,
    side_of_street: Option<&IsolineSideOfStreetOptions>,
) -> Result<(), CalculateIsolinesError> {
    let heading_member = format!("{member}.Heading");
    state::check_range(&heading_member, HEADING_BOUNDS, heading)
        .map_err(|message| invalid(&heading_member, message))?;
    if let Some(side_of_street) = side_of_street {
        let position_member = format!("{member}.SideOfStreet.Position");
        state::validate_position(&side_of_street.position)
            .map_err(|message| invalid(&position_member, message))?;
    }
    Ok(())
}

/// The modeled range of `Heading`, in degrees clockwise from north.
const HEADING_BOUNDS: (f64, f64) = (0.0, 360.0);

/// How finely the search resolves the boundary along one bearing: `OptimizeIsolineFor` is
/// the documented trade-off "between calculation speed and isoline precision", so it is
/// what chooses the number of halvings the search takes. Each halving doubles the
/// precision, so the boundary lands within `2^-steps` of the threshold's reach: about 3%
/// for a fast calculation, 0.8% for a balanced one and 0.2% for an accurate one.
#[derive(Debug, Clone, Copy)]
enum Precision {
    Fast,
    Balanced,
    Accurate,
}

impl Precision {
    /// The precision named by a value of `IsolineOptimizationObjective`, or `None` for one
    /// the generated value list does not declare.
    fn from_model(value: &str) -> Option<Self> {
        if !ISOLINE_OPTIMIZATION_OBJECTIVE_VALUES.contains(&value) {
            return None;
        }
        match value {
            "FastCalculation" => Some(Self::Fast),
            "BalancedCalculation" => Some(Self::Balanced),
            "AccurateCalculation" => Some(Self::Accurate),
            _ => None,
        }
    }

    /// Halvings of the radius interval the search takes per bearing.
    fn steps(self) -> u32 {
        match self {
            Self::Fast => 5,
            Self::Balanced => 7,
            Self::Accurate => 9,
        }
    }
}

/// What resolving one isoline's boundary needs: the threshold it is bounded by, how travel
/// is costed, which way travel runs and how far and finely to search.
#[derive(Debug, Clone, Copy)]
struct Search {
    threshold: Threshold,
    mode: TravelMode,
    objective: RoutingObjective,
    /// Whether travel runs out of the anchor or into it.
    outward: bool,
    max_radius_m: f64,
    steps: u32,
}

/// Fewest and most positions a ring of this service carries.
///
/// `IsolineGranularityOptions$MaxPoints` has a modeled minimum of 31, so 31 points is the
/// fewest a caller may ask an isoline to be drawn with and so the fewest one is drawn with.
/// The maximum is this emulator's own bound on the work one boundary costs — AWS publishes
/// none — and it is a legitimate answer to a larger `MaxPoints` because that member is a
/// MAXIMUM: "the maximum number of points used to define each isoline".
const MIN_RING_POSITIONS: usize = 31;
const MAX_RING_POSITIONS: usize = 300;

/// How many DISTINCT vertices a boundary is resolved at, for the granularity a request
/// asked for and the farthest the threshold can reach.
///
/// `MaxResolution` is "the maximum distance in meters between points along the isoline", so
/// it is the spacing the vertices are placed at; with none asked for they are placed at the
/// spacing this service samples a geometry at. The boundary lies within the circle of
/// `max_radius_m`, so its length is at most that circle's circumference and the vertices
/// that spacing asks for are that circumference over it — plus the one position that closes
/// the ring.
fn ring_vertices(granularity: Option<&IsolineGranularityOptions>, max_radius_m: f64) -> usize {
    let spacing = granularity
        .map(|granularity| granularity.max_resolution)
        .filter(|resolution| *resolution > 0)
        .map_or(state::SAMPLE_SPACING_M, |resolution| resolution as f64);
    let asked = (2.0 * std::f64::consts::PI * max_radius_m / spacing.max(1.0)).ceil();
    let asked = if asked.is_finite() && asked >= 0.0 {
        asked as usize
    } else {
        MAX_RING_POSITIONS
    };
    let allowed = granularity
        .and_then(|granularity| granularity.max_points)
        .and_then(|points| usize::try_from(points).ok())
        .unwrap_or(MAX_RING_POSITIONS)
        .clamp(MIN_RING_POSITIONS, MAX_RING_POSITIONS);
    asked.saturating_add(1).clamp(MIN_RING_POSITIONS, allowed) - 1
}

/// The boundary of the area reachable within `search.threshold` around `anchor`: one
/// position per bearing, closed by repeating the first.
///
/// The bearings run COUNTERCLOCKWISE — this service measures a bearing clockwise from north,
/// so the ring is walked in decreasing bearing — because that is how GeoJSON winds the
/// exterior ring of a polygon.
fn reachable_ring(
    state: &mut ComAmazonawsGeoroutesRoutesserviceState,
    anchor: Position,
    search: Search,
    vertices: usize,
) -> Vec<Position> {
    let mut ring = Vec::with_capacity(vertices + 1);
    for index in 0..vertices {
        let bearing_deg = -360.0 * index as f64 / vertices as f64;
        let radius_m = boundary_radius_m(state, anchor, bearing_deg, search);
        ring.push(state::position_at_bearing(anchor, bearing_deg, radius_m));
    }
    if let Some(first) = ring.first().copied() {
        ring.push(first);
    }
    ring
}

/// How far along `bearing_deg` the area reaches: the greatest radius the search resolves
/// whose route between that point and `anchor` the network carries at no more than the
/// threshold's cost.
///
/// The cost comes from [`ComAmazonawsGeoroutesRoutesserviceState::compute_route`] — the one
/// cost function of this service — so the boundary is where a route to it stops fitting the
/// threshold, and a route the network does not carry the whole way (a corridor leaving the
/// mapped territory) is not reachable at any cost, which is what clips a boundary at a
/// coast. The search starts at the farthest the threshold could possibly reach and halves
/// the interval `search.steps` times; the anchor itself costs nothing, so there is always a
/// radius inside the threshold to return.
fn boundary_radius_m(
    state: &mut ComAmazonawsGeoroutesRoutesserviceState,
    anchor: Position,
    bearing_deg: f64,
    search: Search,
) -> f64 {
    let mut within_threshold = |radius_m: f64| -> bool {
        let candidate = state::position_at_bearing(anchor, bearing_deg, radius_m);
        let positions = if search.outward {
            [anchor, candidate]
        } else {
            [candidate, anchor]
        };
        let route = state.compute_route(&positions, search.mode, search.objective, 0);
        route.connected && search.threshold.cost_of(&route) <= search.threshold.limit()
    };
    if within_threshold(search.max_radius_m) {
        return search.max_radius_m;
    }
    let (mut inside, mut outside) = (0.0_f64, search.max_radius_m);
    for _ in 0..search.steps {
        let middle = inside + (outside - inside) / 2.0;
        if within_threshold(middle) {
            inside = middle;
        } else {
            outside = middle;
        }
    }
    inside
}

/// The geometry of one isoline, in the requested format: the polygon its boundary ring
/// encloses, or no geometry at all where nothing is reachable.
fn isoline_geometries(
    ring: &[Position],
    format: &str,
) -> Result<Vec<IsolineShapeGeometry>, CalculateIsolinesError> {
    // A linear ring carries four positions at least, so a ring shorter than that encloses
    // no area and there is no polygon to report.
    if ring.len() < MIN_RING_LENGTH {
        return Ok(Vec::new());
    }
    if format == "Simple" {
        return Ok(vec![IsolineShapeGeometry {
            polygon: Some(vec![state::line_string(ring)]),
            polyline_polygon: None,
        }]);
    }
    Ok(vec![IsolineShapeGeometry {
        polygon: None,
        polyline_polygon: Some(vec![state::encode_flexible_polyline(ring)
            .ok_or_else(|| internal("the isoline geometry could not be encoded as a Flexible Polyline"))?]),
    }])
}

/// The modeled minimum length of a `LinearRing`: four positions, the fewest that close a
/// ring around an area.
const MIN_RING_LENGTH: usize = 4;

/// Every per-mode option block `IsolineTravelModeOptions` declares, in the model's order,
/// paired with whether the request set it — what [`state::check_travel_mode_options`] holds
/// against the selected `TravelMode`.
fn declared_travel_mode_options(options: Option<&IsolineTravelModeOptions>) -> [(&'static str, bool); 3] {
    [
        ("Car", options.is_some_and(|set| set.car.is_some())),
        ("Scooter", options.is_some_and(|set| set.scooter.is_some())),
        ("Truck", options.is_some_and(|set| set.truck.is_some())),
    ]
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> CalculateIsolinesError {
    CalculateIsolinesErrorModeled::ValidationException(ValidationException {
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
fn access_denied(message: String) -> CalculateIsolinesError {
    CalculateIsolinesErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator rather
/// than of the request.
fn internal(message: &str) -> CalculateIsolinesError {
    CalculateIsolinesErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}
