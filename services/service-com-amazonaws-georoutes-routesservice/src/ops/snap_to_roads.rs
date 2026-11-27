// SPDX-License-Identifier: CC0-1.0

//! @op SnapToRoads
//!
//! Each trace point is snapped onto the nearest road of the network in [`crate::state`]
//! that the requested travel mode may use. The answer carries one snapped point per
//! submitted trace point, at the same index, with the submitted position echoed as
//! `OriginalPosition`, the point on the road as `SnappedPosition` and a `Confidence` read
//! off the match itself ([`match_confidence`]): how far the point had to move to reach its
//! road, weighed against how far the nearest road it was NOT matched onto lies — so a point
//! already on a road matches with the range's own maximum, and a point that could as well
//! have been on the road beside it scores near the minimum however little it moved.
//!
//! A trace is matched as a WHOLE, not as independent points. A point with no road of its
//! own inside `SnapRadius` — one whose nearest usable road is further away than that, and
//! one in the open sea, where [`crate::state`] materialises no road at all — is placed on
//! the stretch of the matched path its neighbours in the trace establish, which is a road
//! of the network and so a position a client can use. Placing it there moves it further
//! than the radius the request allowed, so its index is listed under the notice code for a
//! point moved by a large distance. Only a trace that matched no road anywhere has no path
//! to place such a point on: its points keep the positions they were submitted at, score no
//! confidence, and their indexes are listed under the code for a point that was not
//! matched. So the notice and `SnappedTracePoints` always agree — the "not matched" code is
//! reported for a point, and only for a point, the answer reports no road position for.
//!
//! A notice carries the code of the condition and the trace-point indexes it holds for,
//! and nothing else: see [`RoadSnapNotice`] for the `Title` the model marks `@required`
//! and this service does not put on the wire.
//!
//! `SnappedGeometry` is the line along the ROADS the matched points were matched onto,
//! from the first of them to the last ([`matched_geometry`]); a point with no road of its
//! own contributes no vertex to it — it is placed ON that line rather than bending it — and
//! a trace with fewer than two matched points reports no geometry at all.
//!
//! `TracePoints[].Timestamp`, `Speed` and `Heading` are read but have nothing to act on:
//! the network carries no one-way or direction data for a heading to disambiguate and no
//! time-varying road data for a timestamp or a speed to select, so the snap of a point
//! depends on its position and the travel mode alone. `TravelModeOptions` is the same —
//! the model carries no vehicle restriction for a truck's dimensions or cargo to meet.
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
//! # Modeled contract for `SnapToRoads`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! SnapToRoads matches GPS trace to roads most likely traveled on. For more information,
//! see Snap to Roads in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `SnapRadius` (DistanceMeters, range 0..4294967295, default 0) **required** — The radius
//!   around the provided tracepoint that is considered for snapping. Unit: meters Default
//!   value: 300
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — Chooses
//!   what the returned SnappedGeometry format should be. Default value: FlexiblePolyline
//! * `TracePoints` (RoadSnapTracePointList) **required** — List of trace points to be snapped
//!   onto the road network.
//! * `TravelMode` (RoadSnapTravelMode, one of: Car | Pedestrian | Scooter | Truck) —
//!   Specifies the mode of transport when calculating a route. Used in estimating the speed
//!   of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (RoadSnapTravelModeOptions) — Travel mode related options for the
//!   provided travel mode.
//!
//! ## Output
//!
//! * `Notices` (RoadSnapNoticeList) **required** — Notices are additional information
//!   returned that indicate issues that occurred during route calculation.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `SnappedGeometry` (RoadSnapSnappedGeometry) — The interpolated geometry for the snapped
//!   route onto the road network.
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required**
//!   — Specifies the format of the geometry returned for each leg of the route.
//! * `SnappedTracePoints` (RoadSnapSnappedTracePointList) **required** — The trace points
//!   snapped onto the road network.
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

use crate::state::{self, Position, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SnapToRoadsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SnapToRoadsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#SnapToRoads")]
#[derive(Debug)]
pub enum SnapToRoadsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::snap_to_roads::SnapToRoadsError`). The wrapper is kept because it is
/// also the only place a request this operation models no error for could be refused;
/// every refusal below uses the model's own `ValidationException`.
pub type SnapToRoadsError = crate::scaffolded::Scaffolded<SnapToRoadsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SnapToRoadsErrorModeled> for SnapToRoadsError {
    fn from(modeled: SnapToRoadsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SnapToRoads` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SnapToRoads<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SnapToRoads<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SnapToRoads<Eng>
{
    type Input = SnapToRoadsInput;
    type Output = SnapToRoadsOutput;
    type Error = SnapToRoadsError;

    const NAME: &'static str = "SnapToRoads";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_OUTPUT_SCHEMA;
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

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, ROAD_SNAP_TRAVEL_MODE_VALUES).ok_or_else(|| {
                invalid(
                    "TravelMode",
                    state::must_be_one_of("TravelMode", ROAD_SNAP_TRAVEL_MODE_VALUES),
                )
            })?,
        };
        let geometry_format = state::geometry_format("SnappedGeometryFormat", input.snapped_geometry_format.as_ref())
            .map_err(|message| invalid("SnappedGeometryFormat", message))?;

        // A per-mode option block belongs to the one travel mode its member names, so a
        // request may set only the block the `TravelMode` it selected names; this
        // operation's options structure declares a block for one of the four modes its
        // travel-mode enum admits, and a mode with none permits no block at all.
        let declared = declared_travel_mode_options(input.travel_mode_options.as_ref());
        let permitted: Vec<&str> = state::block_of_mode(travel_mode.as_model(), &declared)
            .into_iter()
            .collect();
        state::check_travel_mode_options(travel_mode.as_model(), &permitted, &declared)
            .map_err(|(member, message)| invalid(&member, message))?;

        // `SnapRadius` narrows the range of the shape it targets: `DistanceMeters` is
        // `0..4294967295` and the member is `0..10000`, so the boundary validator enforces
        // the TARGET's wider range and the member's own upper bound is the handler's to
        // apply. The model's `@default` is 0, which is no radius at all; the documented
        // default the service applies is 300 metres, so a radius outside the member's range
        // is refused rather than read as "unset".
        state::check_range(
            "SnapRadius",
            (0.0, state::MAX_SNAP_RADIUS_M as f64),
            input.snap_radius as f64,
        )
        .map_err(|message| invalid("SnapRadius", message))?;
        let snap_radius_m = if input.snap_radius == 0 {
            state::DEFAULT_SNAP_RADIUS_M as f64
        } else {
            input.snap_radius as f64
        };

        // The documented list bounds ("List of trace points to be snapped onto the road
        // network", 2..5000 on `TracePoints`). They are enforced HERE rather than relied on
        // from the boundary validator, so the per-request work below is bounded whatever
        // the generated validator does with the member's length.
        state::check_length(
            "TracePoints",
            (MIN_TRACE_POINTS, MAX_TRACE_POINTS),
            input.trace_points.len(),
        )
        .map_err(|message| invalid("TracePoints", message))?;
        let mut positions = Vec::with_capacity(input.trace_points.len());
        for (index, trace_point) in input.trace_points.iter().enumerate() {
            let member = format!("TracePoints[{index}].Position");
            positions.push(
                state::validate_position(&trace_point.position)
                    .map_err(|message| invalid(&member, message))?,
            );
        }

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Snapping runs in chunks: the lock on the shared state is taken per chunk and
        // released between chunks, where the handler yields — so a long trace neither
        // holds the lock for the whole request nor leaves a client's cancellation or the
        // server's deadline nowhere to land.
        let mut snapped: Vec<Option<state::SnappedPosition>> = Vec::with_capacity(positions.len());
        for chunk in positions.chunks(SNAP_CHUNK) {
            {
                let mut state = state_handle.lock();
                snapped.extend(chunk.iter().map(|position| state.snap(*position, travel_mode)));
            }
            tokio::task::yield_now().await;
        }
        let pricing_bucket = state_handle.lock().pricing_bucket(Self::NAME);

        // The road each point matched ONTO, where the network held one inside the radius
        // the request allowed: a road outside that radius is not considered, which is what
        // `SnapRadius` ("the radius around the provided tracepoint that is considered for
        // snapping") says.
        let matched: Vec<Option<state::SnappedPosition>> = snapped
            .into_iter()
            .map(|candidate| candidate.filter(|candidate| candidate.offset_m <= snap_radius_m))
            .collect();
        // The matched points in order, each with the road it matched onto: the snapped
        // geometry is the stretch of ROAD between consecutive ones, so the road is needed
        // beside the position.
        let matched_path: Vec<state::SnappedPosition> = matched.iter().flatten().cloned().collect();
        // The matched points that bracket each point of the trace, which is the stretch of
        // the matched path a point with no road of its own belongs to ([`place`]).
        let bracketing = bracketing_matches(&matched);

        let mut snapped_trace_points = Vec::with_capacity(positions.len());
        // The indexes the answer reports a notice for, kept apart by WHICH notice: one set
        // was placed on the trace's path from further away than the request's radius, the
        // other could not be placed on a road at all.
        let mut moved_by_large_distance: Vec<i32> = Vec::new();
        let mut not_matched: Vec<i32> = Vec::new();
        for (index, ((position, matched_here), (before, after))) in
            positions.iter().zip(&matched).zip(&bracketing).enumerate()
        {
            let bracket = |at: Option<usize>| at.and_then(|at| matched.get(at)?.as_ref());
            let placed = place(
                *position,
                matched_here.as_ref(),
                bracket(*before),
                bracket(*after),
                snap_radius_m,
            );
            let (snapped_position, confidence) = match placed {
                Placement::OnRoad {
                    position: on_road,
                    offset_m,
                    runner_up_m,
                }
                | Placement::Moved {
                    position: on_road,
                    offset_m,
                    runner_up_m,
                } => (
                    state::position_to_vec(on_road),
                    match_confidence(*position, offset_m, runner_up_m),
                ),
                // A point the answer reports no road position for is reported where it was
                // submitted — the request's own coordinate, carried as received — and
                // scores nothing: the notice is what tells it apart from a road fix.
                Placement::Unplaced => (state::echoed_position(*position), 0.0),
            };
            if let Ok(index) = i32::try_from(index) {
                match placed {
                    Placement::OnRoad { .. } => {}
                    Placement::Moved { .. } => moved_by_large_distance.push(index),
                    Placement::Unplaced => not_matched.push(index),
                }
            }
            snapped_trace_points.push(RoadSnapSnappedTracePoint {
                confidence,
                original_position: state::echoed_position(*position),
                snapped_position,
            });
        }

        // The notices, in the order the model's own enum declares their codes. Each carries
        // the code of the condition and the indexes it holds for and nothing else.
        // `TracePointIndexes` carries at most `MAX_NOTICE_INDEXES` entries, so a trace with
        // more flagged points than that is reported over as many notices of the same code
        // as it takes: every flagged index is reported, and no notice breaks the modeled
        // length.
        let mut notices = Vec::new();
        for (code, indexes) in [
            (MOVED_BY_LARGE_DISTANCE, &moved_by_large_distance),
            (NOT_MATCHED, &not_matched),
        ] {
            for indexes in indexes.chunks(MAX_NOTICE_INDEXES) {
                notices.push(RoadSnapNotice {
                    code: code.to_string(),
                    trace_point_indexes: indexes.to_vec(),
                });
            }
        }

        let snapped_geometry = snapped_geometry(&matched_geometry(&matched_path), &geometry_format)?;
        Ok(SnapToRoadsOutput {
            notices,
            pricing_bucket,
            snapped_geometry,
            snapped_geometry_format: geometry_format,
            snapped_trace_points,
        })
    }
}

/// The bounds of the `TracePoints` list. The Amazon Location Service API reference states
/// for `SnapToRoads` request parameter `TracePoints`: "Array Members: Minimum number of 2
/// items. Maximum number of 5000 items." — a trace is a sequence of positions travelled
/// through, so a single point is not one. This crate's Smithy model omits the `@length`
/// trait on `RoadSnapTracePointList`, so the framework's constraint walk does not enforce
/// it and this handler does; the upper bound also bounds the work one request can ask for.
const MIN_TRACE_POINTS: usize = 2;
const MAX_TRACE_POINTS: usize = 5_000;

/// How many trace points are snapped under one acquisition of the shared state lock.
const SNAP_CHUNK: usize = 256;

/// Most indexes one notice may carry (`RoadSnapNotice$TracePointIndexes` is `@length`
/// 1..1000).
const MAX_NOTICE_INDEXES: usize = 1_000;

/// The `RoadSnapNoticeCode` for a trace point the matcher had to move further than the
/// radius the request allowed to put it on a road: it had no road of its own inside that
/// radius, so it was placed on the stretch of matched path its neighbours establish.
const MOVED_BY_LARGE_DISTANCE: &str = "TracePointsMovedByLargeDistance";

/// The `RoadSnapNoticeCode` for a trace point that was not matched to a road at all: the
/// trace it belongs to matched none either, so there was no path to place it on.
const NOT_MATCHED: &str = "TracePointsNotMatched";

/// Where one trace point ended up, which is what decides the position the answer reports
/// for it and the notice, if any, that names its index.
enum Placement {
    /// On a road no further from the submitted position than the radius the request
    /// allowed: the match itself, which is worth no notice. `runner_up_m` is how far the
    /// nearest road the match was chosen OVER lies, where the network offered one.
    OnRoad {
        position: Position,
        offset_m: f64,
        runner_up_m: Option<f64>,
    },
    /// On a road further away than that radius — the point had none of its own inside it,
    /// so it was placed on the stretch of matched path its neighbours establish.
    Moved {
        position: Position,
        offset_m: f64,
        runner_up_m: Option<f64>,
    },
    /// On no road at all: the point had none inside the radius and the trace matched none
    /// to place it on, so the answer reports it where it was submitted.
    Unplaced,
}

/// Where `position` is placed, given the road it matched onto if any and the matched
/// points that bracket it in the trace.
///
/// A trace is matched as a whole: a point with no road of its own inside `radius_m` is
/// still a point of a trace that runs along roads, so it is placed on the stretch of that
/// path its neighbours in the trace establish ([`state::road_path_between`]) — a position
/// on a road of the network, carried at this API's own coordinate precision like every
/// other position this service resolved. A point with a neighbour on one side only is
/// placed on the road that neighbour matched; only a trace that matched nothing at all
/// leaves a point unplaced.
///
/// The stretch weighed is the ONE between the two neighbours, so the work per point is
/// fixed whatever the trace's length.
fn place(
    position: Position,
    matched: Option<&state::SnappedPosition>,
    before: Option<&state::SnappedPosition>,
    after: Option<&state::SnappedPosition>,
    radius_m: f64,
) -> Placement {
    let (on_road, offset_m, runner_up_m) = match matched {
        Some(matched) => (matched.position, matched.offset_m, matched.runner_up_m),
        None => {
            let stretch: Vec<Position> = match (before, after) {
                (Some(before), Some(after)) => state::road_path_between(before, after),
                (Some(neighbour), None) | (None, Some(neighbour)) => vec![neighbour.position],
                (None, None) => return Placement::Unplaced,
            };
            let (on_path, offset_m) = state::nearest_point_on_polyline(position, &stretch);
            // The point was placed on the ONE stretch its neighbours establish rather than
            // chosen among the roads near it, so there is no runner-up to weigh it against.
            (state::at_api_precision(on_path), offset_m, None)
        }
    };
    if offset_m <= radius_m {
        Placement::OnRoad {
            position: on_road,
            offset_m,
            runner_up_m,
        }
    } else {
        Placement::Moved {
            position: on_road,
            offset_m,
            runner_up_m,
        }
    }
}

/// For every point of the trace, the nearest point AT OR BEFORE it that matched a road of
/// its own and the nearest one AT OR AFTER it — the two ends of the stretch of matched path
/// it belongs to. Either is `None` where the trace matched nothing on that side.
///
/// Two linear passes over the trace, so a long trace costs no more per point than a short
/// one.
fn bracketing_matches(
    matched: &[Option<state::SnappedPosition>],
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut before: Option<usize> = None;
    let nearest_before: Vec<Option<usize>> = matched
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            if slot.is_some() {
                before = Some(index);
            }
            before
        })
        .collect();
    let mut after: Option<usize> = None;
    let mut nearest_after: Vec<Option<usize>> = matched
        .iter()
        .enumerate()
        .rev()
        .map(|(index, slot)| {
            if slot.is_some() {
                after = Some(index);
            }
            after
        })
        .collect();
    nearest_after.reverse();
    nearest_before.into_iter().zip(nearest_after).collect()
}

/// How certain the placement of `position` on a road `offset_m` metres away is, with the
/// nearest road that placement was chosen OVER `runner_up_m` metres away, as `Confidence`
/// ("Confidence value for the correctness of this point match", a `@range 0..1` double)
/// reports it.
///
/// A match names a ROAD, so how correct it is turns on TWO things, and both are read off
/// the match itself:
///
/// * its RESIDUAL — how far the point had to move to reach the road. A point already on a
///   road was observed where the road is, so its match is certain and scores the range's
///   own maximum, and the further off a point lay the less of an explanation its road is.
/// * how much better that road explains the point than the next road does. The residual
///   alone cannot say: a point 7 m from its road and 70 m from any other can only have
///   been on the first, while the same point with a second road 8 m away could have been
///   on either, and the two are not matches of equal correctness. So the residual is
///   weighed on the scale the CANDIDATES set — half the distance to the runner-up — which
///   is why one answer's points differ in confidence by more than their residuals differ:
///   a point on an isolated carriageway is a sure match and one in a dense crossing is a
///   guess between roads.
///
/// The score therefore falls off smoothly as the point sits further from its own road and
/// as the roads it was not matched onto close in, reaching the range's minimum where the
/// point lies as near the runner-up as its own road. That is what lets a client threshold
/// on it: two points whose candidates lie alike score alike, and of two points with the
/// same residual the one with a nearer rival always scores less.
///
/// Where the network offered NO second road the choice was not between roads at all, so
/// the residual is weighed on the widest a match inside this coverage can be out by
/// ([`state::snap_ambiguity_m`]).
///
/// The radius the request allowed is deliberately NOT part of it: `SnapRadius` says which
/// roads are considered for a point, so scoring against it would report the same match as
/// confident or not depending on how much slack the caller left, and would never signal a
/// poor match at a generous radius.
fn match_confidence(position: Position, offset_m: f64, runner_up_m: Option<f64>) -> f64 {
    if offset_m <= 0.0 {
        // The point lies ON the carriageway: it was observed where the road is, whatever
        // else runs nearby.
        return 1.0;
    }
    // A Gaussian error model on the distance from the observed point to the road: the
    // likelihood that a point this far off was travelling on it rather than on the road it
    // was not matched onto. Its scale is half the distance to that runner-up, so a point
    // lying as near the runner-up as its own road is two scales out — a choice the model
    // has all but ruled out either way — while a point on its road scores 1.
    let ambiguity_m = runner_up_m.unwrap_or_else(|| state::snap_ambiguity_m(position[1]));
    let scale = (ambiguity_m / 2.0).max(f64::MIN_POSITIVE);
    (-(offset_m / scale).powi(2)).exp().clamp(0.0, 1.0)
}

/// The geometry of the snapped route: the roads the trace was matched onto, from the first
/// matched point to the last.
///
/// Between two consecutive matched points the line is the stretch of the NETWORK that
/// joins them ([`state::road_path_between`]), not the chord between them: the trace was
/// matched onto roads, so the geometry the answer carries runs along those roads and a
/// client drawing it stays on the network. A point with no road of its own contributes no
/// vertex — it is placed ON this line ([`place`]) rather than bending it towards a position
/// no road of the network passes through.
fn matched_geometry(matched: &[state::SnappedPosition]) -> Vec<Position> {
    let mut path: Vec<Position> = matched.first().map(|first| vec![first.position]).unwrap_or_default();
    for pair in matched.windows(2) {
        for vertex in state::road_path_between(&pair[0], &pair[1]) {
            if path.last() != Some(&vertex) {
                path.push(vertex);
            }
        }
    }
    path
}

/// The snapped geometry in the requested format. `None` when there is no line to draw — a
/// `LineString` carries two positions at least.
fn snapped_geometry(path: &[Position], format: &str) -> Result<Option<RoadSnapSnappedGeometry>, SnapToRoadsError> {
    if path.len() < 2 {
        return Ok(None);
    }
    if format == "Simple" {
        return Ok(Some(RoadSnapSnappedGeometry {
            line_string: Some(state::line_string(path)),
            polyline: None,
        }));
    }
    Ok(Some(RoadSnapSnappedGeometry {
        line_string: None,
        polyline: Some(
            state::encode_flexible_polyline(path)
                .ok_or_else(|| internal("the snapped geometry could not be encoded as a Flexible Polyline"))?,
        ),
    }))
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::ValidationException(ValidationException {
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
fn access_denied(message: String) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator
/// rather than of the request.
fn internal(message: &str) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// Every per-mode option block `RoadSnapTravelModeOptions` declares, in the model's order,
/// paired with whether the request set it — what [`state::check_travel_mode_options`]
/// holds against the selected `TravelMode`.
fn declared_travel_mode_options(
    options: Option<&RoadSnapTravelModeOptions>,
) -> [(&'static str, bool); 1] {
    [("Truck", options.is_some_and(|set| set.truck.is_some()))]
}
