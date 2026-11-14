// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-georoutes-routesservice service.
//!
//! This module carries BOTH the per-service state struct and the deterministic routing
//! model the handlers compute their answers from. `src/lib.rs` is generated and `src/ops/`
//! takes one file per operation, so this is the one module every handler of this service
//! can share: the cost function lives here exactly once, which is why a route matrix cell,
//! a direct route and an optimized-waypoint connection for the same pair of positions
//! cannot disagree.
//!
//! That cost function is exactly this: a leg's DISTANCE is the geodesic (haversine) length
//! of the geometry the response carries for it, and its DURATION is that distance over the
//! travel mode's speed ([`ComAmazonawsGeoroutesRoutesserviceState::travel_mode_speeds`])
//! scaled by the routing objective's factor ([`ObjectiveFactors::speed`]). The posted speed
//! of a road CLASS is deliberately NOT part of it: a class decides which travel modes may
//! use a road and ranks the roads a route travelled for `MajorRoadLabels`, so two legs of
//! equal length take equal time whatever classes they crossed. Costing each sampled
//! sub-segment at its own posted speed would be a per-segment cost model, which this slice
//! does not carry.
//!
//! The network has COVERAGE where a territory of the world is mapped and nowhere else
//! ([`is_mapped_territory`]): a grid cell in the open sea materialises no road, so a
//! position there snaps to nothing and a leg whose corridor crosses it is not carried by
//! the network ([`CachedLeg::connected`]). That is the one place real geography enters
//! this model; within a mapped territory the grid below is uniform.
//!
//! The network also carries the manoeuvre data a leg's TRAVEL STEPS are derived from. A
//! road is a run of the grid with an orientation and a grid index ([`RoadSegment`]), two
//! roads of different orientations meet in a [`Junction`] with the angle between their own
//! directions ([`TravelledRoad::junction_with`]), and a leg's geometry is a sequence of
//! [`PathVertex`] each carrying the road it is on — so [`leg_runs`] cuts the leg into
//! [`LegRun`]s at the junctions where its road changes, each run with its share of the
//! leg's distance and duration and the angle it turned through. No step boundary, turn
//! angle or road name is chosen: each is read off the network.
//!
//! Nothing in here reads a clock or a random source except [`now`], so a request repeated
//! with the same inputs computes the same distances, durations and geometry.

/// Whether the caller's own identity-based IAM policies let a request perform the action it
/// asks for — the decision every operation of this service takes before it computes
/// anything. It is a fact of the CREDENTIAL the request presented rather than of the road
/// model, so it lives beside this module rather than inside it.
pub mod identity_authorization;

use std::collections::HashMap;

use std::sync::OnceLock;
use std::time::SystemTime;

use aws_smithy_types::DateTime;
use country_boundaries::{CountryBoundaries, LatLon, BOUNDARIES_ODBL_360X180};

/// The instant type this service computes its departure and arrival times with: the
/// framework's own timestamp, already a dependency of every service crate, so no handler
/// needs a second calendar implementation.
pub type Instant = DateTime;

/// A WGS 84 position in the order the model carries it: `[longitude, latitude]`.
pub type Position = [f64; 2];

/// The inclusive bounds of a WGS 84 position. Positions outside them are refused with a
/// `ValidationException`; the model's `Position` shape only pins the list length.
pub const LONGITUDE_BOUNDS: (f64, f64) = (-180.0, 180.0);
/// See [`LONGITUDE_BOUNDS`].
pub const LATITUDE_BOUNDS: (f64, f64) = (-90.0, 90.0);

/// Upper bound of the model's `DistanceMeters` / `DurationSeconds` ranges (`0..4294967295`).
pub const MAX_DISTANCE_OR_DURATION: i64 = 4_294_967_295;

/// Mean Earth radius in metres, for the haversine distance every cost in this service is
/// derived from.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Metres per degree of latitude (mean meridional degree), used for the local planar
/// projection that snaps a position onto a road segment.
const METRES_PER_DEGREE_LAT: f64 = 110_574.0;
/// Metres per degree of longitude at the equator; scaled by `cos(latitude)` in use.
const METRES_PER_DEGREE_LON_EQUATOR: f64 = 111_320.0;

/// Spacing of the synthetic road grid, in degrees: every integer multiple of this in
/// longitude is a meridian road (running north-south) and every multiple in latitude is a
/// parallel road (running east-west). ~55 m, so any position is within ~28 m of a road.
const ROAD_SPACING_DEG: f64 = 0.0005;

/// Side, in degrees, of the grid cell materialised into
/// [`ComAmazonawsGeoroutesRoutesserviceState::road_network`] at a time. One cell carries
/// `2 * (CELL_DEG / ROAD_SPACING_DEG + 1)` segments.
const CELL_DEG: f64 = 0.002;

/// Cap on the grid cells materialised at once. Reaching it drops the materialised
/// network, which the next request regenerates from the same deterministic rule: the
/// network is a cache of a pure function of the position, so dropping it cannot change an
/// answer, and nothing holds a reference into it across a call.
const MAX_MATERIALISED_CELLS: usize = 4096;

/// The mapped territories of the world, read once from the boundary data the
/// `country-boundaries` crate embeds and shared by every request.
///
/// A road network exists where a territory is mapped and nowhere else: a position in
/// international waters lies in no territory, so the grid below materialises no road for
/// it. `None` when the data cannot be read at all, which leaves coverage unknown rather
/// than empty — see [`is_mapped_territory`].
fn mapped_territories() -> Option<&'static CountryBoundaries> {
    static TERRITORIES: OnceLock<Option<CountryBoundaries>> = OnceLock::new();
    TERRITORIES
        .get_or_init(|| CountryBoundaries::from_reader(BOUNDARIES_ODBL_360X180).ok())
        .as_ref()
}

/// Whether the road network covers `pos`: whether the GRID CELL holding it lies in a
/// mapped territory. A cell that lies in none — the open sea — materialises no road, so
/// nothing snaps to a position in it and no route travels through it.
///
/// The question is asked of the CELL, never of the bare position, so every reader of
/// coverage answers alike: a position that snaps onto a road cannot also be one a leg
/// reports leaving the network at, which is what keeps an endpoint's own `NoMatch` and a
/// pair's `NoRoute` apart.
///
/// With the boundary data unreadable the coverage of a position is UNKNOWN, and the
/// network answers for it as it does for mapped land: an emulator that could not read its
/// own data would otherwise report every position of the world as unroutable.
pub fn is_mapped_territory(pos: Position) -> bool {
    let Some(territories) = mapped_territories() else {
        return true;
    };
    let cell = cell_of(pos);
    let centre = [
        (cell.0 as f64 + 0.5) * CELL_DEG,
        (cell.1 as f64 + 0.5) * CELL_DEG,
    ];
    LatLon::new(centre[1], centre[0]).is_ok_and(|at| !territories.ids(at).is_empty())
}

/// Every 10th meridian road is a footpath: pedestrians only.
const FOOTPATH_EVERY: i64 = 10;
/// Every 20th parallel road is a motorway: no pedestrians, and the fastest road around, so
/// it is what `MajorRoadLabels` reports for a route that uses one.
const MOTORWAY_EVERY: i64 = 20;

/// Posted speeds of the three road classes of the synthetic network, in km/h. These rank
/// the roads a route travelled for `MajorRoadLabels` (and name the fastest class); the
/// duration of a leg comes from the travel mode's speed, not from these — see the module
/// header.
const MOTORWAY_SPEED_KPH: f64 = 90.0;
const STREET_SPEED_KPH: f64 = 50.0;
const FOOTPATH_SPEED_KPH: f64 = 5.0;

/// Target spacing of the samples a geometry of this service is built from. A leg is
/// sampled every ~250 m (at least two samples, at most [`MAX_PATH_SEGMENTS`]) and every
/// interior sample is snapped onto the road network, so the returned geometry follows the
/// roads rather than the straight line between the two positions. It is also the spacing
/// the vertices of a computed area are placed at where the request asks for no other.
pub const SAMPLE_SPACING_M: f64 = 250.0;
/// Upper bound on the samples per leg, so a long leg stays a bounded response.
const MAX_PATH_SEGMENTS: usize = 24;

/// Lateral offset, in metres, of the `FastestRoute` path at the midpoint of each leg: the
/// detour onto the through-roads it trades distance for.
const FASTEST_DETOUR_M: f64 = 400.0;

/// Lateral offset, in metres, of alternative route *k* at the midpoint of each leg: an
/// alternative bows away from the direct path and is recomputed from the roads it then
/// passes, so its distance, duration and geometry all genuinely differ.
const ALTERNATIVE_OFFSET_M: f64 = 400.0;

/// Most alternatives this service can compute distinct routes for, which is the upper
/// bound of the model's own `MaxAlternatives` range (`0..5`).
pub const MAX_ALTERNATIVES: i32 = 5;

/// Cap on the memo of computed routes. Reaching it clears the memo: the memo only ever
/// saves recomputation of a pure function, so dropping it cannot change an answer.
const MAX_MEMOISED_ROUTES: usize = 4096;

/// `SnapRadius`'s documented default, in metres ("Default value: 300" on
/// `SnapToRoadsRequest$SnapRadius`), applied when the request leaves it at the model's
/// `@default` of 0.
pub const DEFAULT_SNAP_RADIUS_M: i64 = 300;

/// The upper bound of `SnapToRoadsRequest$SnapRadius`'s modeled range (`0..10000`).
pub const MAX_SNAP_RADIUS_M: i64 = 10_000;

/// The documented default of `LegGeometryFormat` / `SnappedGeometryFormat`.
pub const DEFAULT_GEOMETRY_FORMAT: &str = "FlexiblePolyline";

/// The base pricing TIER a geo-routes query is charged in.
///
/// `PricingBucket` is "the pricing bucket for which the query is charged at"
/// (`CalculateRoutesResponse$PricingBucket`), `@required` and bound to the
/// `x-amz-geo-pricing-bucket` header on every response of this service. The model
/// enumerates no values, and recorded behavior is a tier token shared by the Location
/// Service APIs — the sibling places service reports the same `Core` tier
/// (`PRICING_BUCKET_CORE` in `service-com-amazonaws-geoplaces-placesservice`). So the
/// bucket names the TIER the call is charged in and never the request class: the route and
/// matrix queries are charged in this base tier.
pub const PRICING_BUCKET_CORE: &str = "Core";

/// The higher pricing TIER a geo-routes query is charged in.
///
/// Which tier a query falls in is a property of the OPERATION that serves it and not of
/// the request: recorded behavior answers every isoline, every waypoint-optimization and
/// every road-snapping query with this tier, however the request varies, and the route and
/// matrix queries with [`PRICING_BUCKET_CORE`]. The two are the tokens recorded behavior
/// carries; the model enumerates none, so nothing beyond the per-operation tier is read off
/// a request.
pub const PRICING_BUCKET_ADVANCED: &str = "Advanced";

/// Precision of the Flexible Polyline encoding: 6 decimal digits, the default of the
/// `aws-geospatial/polyline` library the model's `LegGeometryFormat` documentation points
/// at.
pub const POLYLINE_PRECISION: u32 = 6;

/// Version of the Flexible Polyline format this service emits, carried as the first value
/// of the header: the format defines `FORMAT_VERSION = 1`.
const POLYLINE_FORMAT_VERSION: u64 = 1;

/// The 64 characters of the Flexible Polyline encoding, in value order, as the format
/// specifies (URL-safe base64 order: `A`-`Z`, `a`-`z`, `0`-`9`, `-`, `_`).
const POLYLINE_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The travel modes this service's cost function knows, i.e. the union of the travel-mode
/// enums the five operations bind (`RouteTravelMode` is the widest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TravelMode {
    Car,
    Truck,
    Pedestrian,
    Scooter,
    Transit,
    Intermodal,
}

impl TravelMode {
    /// The mode named by a model enum value, or `None` for a value no travel-mode enum in
    /// this model declares.
    pub fn from_model(value: &str) -> Option<Self> {
        match value {
            "Car" => Some(Self::Car),
            "Truck" => Some(Self::Truck),
            "Pedestrian" => Some(Self::Pedestrian),
            "Scooter" => Some(Self::Scooter),
            "Transit" => Some(Self::Transit),
            "Intermodal" => Some(Self::Intermodal),
            _ => None,
        }
    }

    /// The mode named by a value of the narrower travel-mode enums, or `None` for a value
    /// `accepted` does not list. `accepted` is the generated list of that enum's wire
    /// values (`ROUTE_MATRIX_TRAVEL_MODE_VALUES` and friends), so the set a handler takes
    /// cannot drift from the model.
    pub fn from_model_values(value: &str, accepted: &[&str]) -> Option<Self> {
        accepted
            .contains(&value)
            .then(|| Self::from_model(value))
            .flatten()
    }

    /// This mode as the model's enum value.
    pub fn as_model(self) -> &'static str {
        match self {
            Self::Car => "Car",
            Self::Truck => "Truck",
            Self::Pedestrian => "Pedestrian",
            Self::Scooter => "Scooter",
            Self::Transit => "Transit",
            Self::Intermodal => "Intermodal",
        }
    }

    /// Whether this mode travels on foot, which is what decides the leg type and which
    /// roads of the network it may use.
    pub fn is_pedestrian(self) -> bool {
        matches!(self, Self::Pedestrian)
    }
}

/// `RoutingObjective` / `WaypointOptimizationSequencingObjective`: the trade-off between
/// travel time and physical distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingObjective {
    FastestRoute,
    ShortestRoute,
}

impl RoutingObjective {
    /// The objective named by a model enum value, or `None` for a value
    /// `ROUTING_OBJECTIVE_VALUES` does not list.
    pub fn from_model(value: &str) -> Option<Self> {
        if !crate::ops::types::ROUTING_OBJECTIVE_VALUES.contains(&value) {
            return None;
        }
        match value {
            "FastestRoute" => Some(Self::FastestRoute),
            "ShortestRoute" => Some(Self::ShortestRoute),
            _ => None,
        }
    }
}

/// The multipliers an objective applies to the cost of a leg.
///
/// The road-grid path between two positions is the shortest one this network offers, so
/// `ShortestRoute` travels exactly it, at the local-street pace of `speed`;
/// `FastestRoute` detours `detour_m` sideways onto the through-roads and travels at the
/// mode's full speed. The detour lengthens the GEOMETRY the response carries, so the
/// reported distance stays the length of that geometry. The two numbers are parameters of
/// this emulator's synthetic model — AWS publishes neither — and they are held in state so
/// every handler applies the same pair.
#[derive(Debug, Clone, Copy)]
pub struct ObjectiveFactors {
    /// How far the fastest path bows off the direct one, in metres, at the midpoint of
    /// each leg.
    pub detour_m: f64,
    /// Multiplier on the travel mode's speed.
    pub speed: f64,
}

/// The class of a road in the synthetic network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadClass {
    Motorway,
    Street,
    Footpath,
}

/// Which way a run of the grid points. It is what the angle between two runs — and so the
/// manoeuvre at the junction they meet in — is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadOrientation {
    /// A road of constant longitude, running north-south.
    Meridian,
    /// A road of constant latitude, running east-west.
    Parallel,
}

impl RoadOrientation {
    /// Bearing of the run's forward direction, in degrees clockwise from north: a meridian
    /// road points north and a parallel road east.
    pub fn forward_bearing_deg(self) -> f64 {
        match self {
            Self::Meridian => 0.0,
            Self::Parallel => 90.0,
        }
    }

    /// Which coordinate of a position runs ALONG this road: latitude for a meridian road,
    /// longitude for a parallel one.
    fn axis(self) -> usize {
        match self {
            Self::Meridian => 1,
            Self::Parallel => 0,
        }
    }
}

/// Where two roads of the network meet: the point they cross in and the angle between them.
///
/// A junction is what makes a step boundary a manoeuvre rather than a chosen point: the
/// two roads genuinely cross here, and the angle is measured from their own directions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Junction {
    /// The crossing point, `[longitude, latitude]`.
    pub position: Position,
    /// Angle between the two roads' forward directions, in degrees in `[0, 180]`.
    pub crossing_angle_deg: f64,
}

/// One segment of the synthetic road network: a straight run of one grid road across one
/// materialised cell.
#[derive(Debug, Clone)]
pub struct RoadSegment {
    /// Stable identifier, derived from the road's grid index and the cell.
    pub id: String,
    /// `[longitude, latitude]` endpoints of the run, in order.
    pub polyline: Vec<Position>,
    /// Posted speed of the road, in km/h.
    pub speed_kph: f64,
    /// Name of the road, as `MajorRoadLabels` reports it.
    pub road_label: String,
    /// The travel modes allowed on the road.
    pub allows: Vec<TravelMode>,
    /// The road's class.
    pub class: RoadClass,
    /// Which way the run points.
    pub orientation: RoadOrientation,
    /// The road's grid index: which multiple of [`ROAD_SPACING_DEG`] the road sits at.
    /// With [`RoadSegment::orientation`] it identifies the ROAD, which the per-cell
    /// [`RoadSegment::id`] does not — one road crosses as many cells as it spans.
    pub grid_index: i64,
}

impl RoadSegment {
    /// Whether `mode` may travel on this road.
    pub fn allows(&self, mode: TravelMode) -> bool {
        self.allows.contains(&mode)
    }

    /// This run's road, as a travel step of a leg reports it.
    pub fn travelled(&self) -> TravelledRoad {
        TravelledRoad {
            label: self.road_label.clone(),
            class: self.class,
            speed_kph: self.speed_kph,
            orientation: self.orientation,
            grid_index: self.grid_index,
        }
    }
}

/// The road a stretch of a leg travels on: what a travel step reports as its road, and
/// what the manoeuvre at its start is classified from.
#[derive(Debug, Clone, PartialEq)]
pub struct TravelledRoad {
    /// Name of the road.
    pub label: String,
    /// The road's class.
    pub class: RoadClass,
    /// Posted speed of the road, in km/h.
    pub speed_kph: f64,
    /// Which way the road runs.
    pub orientation: RoadOrientation,
    /// The road's grid index.
    pub grid_index: i64,
}

impl TravelledRoad {
    /// Whether this is the same ROAD as `other`. A road runs across every cell it spans,
    /// so its identity is its orientation and grid index rather than the per-cell segment
    /// a position snapped onto — otherwise a leg crossing a cell boundary would read as
    /// changing road where it travels straight on.
    pub fn is_same_road(&self, other: &Self) -> bool {
        self.orientation == other.orientation && self.grid_index == other.grid_index
    }

    /// Which TOPOLOGY SEGMENT of this road `position` lies on: a linear stretch of road
    /// between two junctions.
    ///
    /// The roads crossing this one sit at every multiple of [`ROAD_SPACING_DEG`] along its
    /// own axis ([`Self::junction_with`]), so its junctions cut it into the intervals of
    /// that spacing and the segment is the interval `position` falls in. Two positions
    /// report the same segment of the same road exactly when no junction of the network
    /// lies between them.
    pub fn topology_segment(&self, position: Position) -> i64 {
        (position[self.orientation.axis()] / ROAD_SPACING_DEG).floor() as i64
    }

    /// The junction this road and `other` meet in: the point they cross and the angle
    /// between their forward directions. `None` for two roads of the same orientation,
    /// which run parallel and so never meet.
    ///
    /// Every road of the grid sits at a multiple of [`ROAD_SPACING_DEG`] along its own axis
    /// and runs the whole way along the other, so a meridian road and a parallel road cross
    /// in exactly one point: the meridian's longitude and the parallel's latitude. The
    /// per-cell segments the network materialises are pieces of those roads, not their
    /// extent, so the junction is read off the roads rather than off the pieces a leg
    /// happened to snap onto.
    pub fn junction_with(&self, other: &Self) -> Option<Junction> {
        if self.orientation == other.orientation {
            return None;
        }
        let (meridian, parallel) = match self.orientation {
            RoadOrientation::Meridian => (self, other),
            RoadOrientation::Parallel => (other, self),
        };
        Some(Junction {
            position: [
                meridian.grid_index as f64 * ROAD_SPACING_DEG,
                parallel.grid_index as f64 * ROAD_SPACING_DEG,
            ],
            crossing_angle_deg: turn_between_deg(
                self.orientation.forward_bearing_deg(),
                other.orientation.forward_bearing_deg(),
            )
            .abs(),
        })
    }

    /// The route number this road is designated by, where the network designates one.
    ///
    /// A motorway of this network is a numbered road — it is named after its number, and
    /// it is the class `MajorRoadLabels` reports for a route that travels one. A street
    /// and a footpath carry a name only, so they have no number to report: `None` is "this
    /// road is unnumbered", not "the number is unknown".
    pub fn route_number(&self) -> Option<String> {
        matches!(self.class, RoadClass::Motorway).then(|| motorway_number(self.grid_index))
    }

    /// Bearing travel takes along this road to get from `from` to `to`, in degrees
    /// clockwise from north: the road's own forward bearing, reversed where travel runs
    /// against it.
    ///
    /// `None` where the two positions do not differ along the road's axis, which leaves the
    /// direction travel takes along it undetermined.
    pub fn travel_bearing_deg(&self, from: Position, to: Position) -> Option<f64> {
        let axis = self.orientation.axis();
        let delta = to[axis] - from[axis];
        if delta.abs() < POSITION_EPSILON {
            return None;
        }
        let forward = self.orientation.forward_bearing_deg();
        Some(if delta > 0.0 {
            forward
        } else {
            (forward + 180.0).rem_euclid(360.0)
        })
    }
}

/// The type of a route leg this service computes, as `RouteLegType` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegType {
    Vehicle,
    Pedestrian,
}

impl LegType {
    /// The leg type a travel mode produces.
    pub fn for_mode(mode: TravelMode) -> Self {
        if mode.is_pedestrian() {
            Self::Pedestrian
        } else {
            Self::Vehicle
        }
    }

    /// This leg type as the model's enum value.
    pub fn as_model(self) -> &'static str {
        match self {
            Self::Vehicle => "Vehicle",
            Self::Pedestrian => "Pedestrian",
        }
    }
}

/// One vertex of a leg's geometry: the position, and the road the leg travels there.
#[derive(Debug, Clone)]
pub struct PathVertex {
    /// `[longitude, latitude]` of the vertex.
    pub position: Position,
    /// The road the leg is on here. A leg's two submitted endpoints are not snapped
    /// points, so they carry the road of the nearest snapped vertex: the traveller departs
    /// on the road the leg's first snapped point is on and arrives on the road of its last.
    pub road: Option<TravelledRoad>,
    /// Whether this vertex is itself a point snapped onto the network, as against an
    /// endpoint that inherited its road.
    pub snapped: bool,
    /// The junction the previous vertex's road and this vertex's road meet in, when the
    /// road changes here and the two roads cross.
    pub junction: Option<Junction>,
}

impl PathVertex {
    /// A vertex off the road network: a leg's submitted endpoint, or a sample the network
    /// held no road for.
    fn off_network(position: Position) -> Self {
        Self {
            position,
            road: None,
            snapped: false,
            junction: None,
        }
    }
}

/// One run of a leg: the stretch travelled on a single road, which is what one travel step
/// of the leg reports.
#[derive(Debug, Clone)]
pub struct LegRun {
    /// Offset into the leg's geometry of the vertex the run starts at.
    pub geometry_offset: usize,
    /// The run's share of the leg's distance, in metres.
    pub distance_m: i64,
    /// The run's share of the leg's duration, in seconds.
    pub duration_s: i64,
    /// The road travelled, when the run runs on the network.
    pub road: Option<TravelledRoad>,
    /// Signed angle the leg turns through to enter this run, in degrees in `[-180, 180]`:
    /// positive turns right. It is the angle between the two ROADS where they meet in a
    /// junction, signed by the bearings travel takes along them, and the angle between
    /// those bearings where the two roads never cross. `None` for the first run of a leg,
    /// which turns off nothing, and for a boundary neither run gives a direction for.
    pub turn_angle_deg: Option<f64>,
    /// The junction the previous run and this one meet in, when the two roads cross.
    pub junction: Option<Junction>,
}

/// One leg of a computed route: the section from one waypoint to the next.
#[derive(Debug, Clone)]
pub struct CachedLeg {
    pub start: Position,
    pub end: Position,
    pub distance_m: i64,
    pub duration_s: i64,
    /// The leg's geometry, following the road network, each vertex with the road it is on;
    /// at least two vertices.
    pub path: Vec<PathVertex>,
    /// The runs of the leg, in travel order: the manoeuvres its travel steps report.
    pub runs: Vec<LegRun>,
    pub travel_mode: TravelMode,
    pub leg_type: LegType,
    /// Whether the network carries this leg the whole way: false when the corridor between
    /// its endpoints leaves the mapped territory the network has roads in, so the leg has
    /// no road to travel there. The distance and duration are still computed — a caller
    /// that has nowhere to report "no route" needs an answer — so the flag is what an
    /// answer with a channel for it reports on.
    pub connected: bool,
}

/// The positions of a path, in order.
pub fn path_positions(path: &[PathVertex]) -> Vec<Position> {
    path.iter().map(|vertex| vertex.position).collect()
}

/// The runs of a leg whose geometry is `path`, with each run's share of the leg's
/// `distance_m` and `duration_s`.
///
/// A new run starts wherever the road changes, so a run is a stretch of ONE road and a run
/// boundary is a junction of two. A run's distance is its share of the leg's geometry, and
/// its duration that same share of the leg's duration — the cost function is the leg's
/// distance over one speed, so a share of the distance is that share of the time. Both are
/// rounded CUMULATIVELY, so the runs of a leg sum to exactly the leg's own distance and
/// duration rather than to it plus a rounding error per run.
pub fn leg_runs(path: &[PathVertex], distance_m: i64, duration_s: i64) -> Vec<LegRun> {
    if path.is_empty() {
        return Vec::new();
    }
    let last = path.len() - 1;

    // Length of the geometry up to each vertex, which is what the share of the leg a run
    // covers is read off.
    let mut cumulative = Vec::with_capacity(path.len());
    let mut travelled = 0.0;
    cumulative.push(travelled);
    for pair in path.windows(2) {
        travelled += haversine_m(pair[0].position, pair[1].position);
        cumulative.push(travelled);
    }
    let total = cumulative[last];
    // A leg with no length at all (two coincident positions) is divided by vertex count,
    // so the shares still run from 0 to 1 and the runs still sum to the leg.
    let share = |index: usize| -> f64 {
        if total > 0.0 {
            cumulative[index] / total
        } else {
            index as f64 / last.max(1) as f64
        }
    };

    // A run starts at the leg's first vertex and wherever the road changes.
    let mut starts = vec![0_usize];
    for index in 1..path.len() {
        let changed = match (&path[index - 1].road, &path[index].road) {
            (Some(previous), Some(current)) => !previous.is_same_road(current),
            (None, None) => false,
            _ => true,
        };
        if changed {
            starts.push(index);
        }
    }

    // The bearing travel takes along each run: the road's own direction where the run has
    // an extent along it, and otherwise the direction of the hop leaving it.
    let bearing = |start: usize, road_end: usize| -> Option<f64> {
        let from = path[start].position;
        if let Some(bearing) = path[start]
            .road
            .as_ref()
            .and_then(|road| road.travel_bearing_deg(from, path[road_end].position))
        {
            return Some(bearing);
        }
        planar_bearing_deg(from, path.get(road_end + 1)?.position)
    };
    let bearings: Vec<Option<f64>> = starts
        .iter()
        .enumerate()
        .map(|(ordinal, &start)| {
            let road_end = match starts.get(ordinal + 1) {
                // The next run starts at the vertex the road changes in, so the vertices on
                // THIS run's road end one before it.
                Some(&next) => next.saturating_sub(1).max(start),
                None => last,
            };
            bearing(start, road_end)
        })
        .collect();

    starts
        .iter()
        .enumerate()
        .map(|(ordinal, &start)| {
            // The run runs to where the next one starts, so the runs of a leg tile its
            // whole geometry; the last one runs to the leg's end.
            let end = starts.get(ordinal + 1).copied().unwrap_or(last);
            let turn_angle_deg = match (
                ordinal.checked_sub(1).and_then(|previous| bearings[previous]),
                bearings[ordinal],
            ) {
                (Some(entered), Some(left)) => {
                    let travelled = turn_between_deg(entered, left);
                    // Where the road the leg leaves and the road it joins meet in a
                    // junction, the leg turns through the angle BETWEEN THOSE ROADS, and
                    // the bearings decide only which way round it turns. Only a boundary
                    // with no junction — two roads that never cross — is left with the
                    // angle between the bearings alone.
                    Some(match path[start].junction {
                        Some(junction) if travelled < 0.0 => -junction.crossing_angle_deg,
                        Some(junction) => junction.crossing_angle_deg,
                        None => travelled,
                    })
                }
                _ => None,
            };
            LegRun {
                geometry_offset: start,
                distance_m: scaled_share(share(end), distance_m) - scaled_share(share(start), distance_m),
                duration_s: scaled_share(share(end), duration_s) - scaled_share(share(start), duration_s),
                road: path[start].road.clone(),
                turn_angle_deg,
                junction: path[start].junction,
            }
        })
        .collect()
}

/// `share` of `total`, rounded: the cumulative value a run's distance or duration is the
/// difference of.
fn scaled_share(share: f64, total: i64) -> i64 {
    clamp_cost(share.clamp(0.0, 1.0) * total as f64)
}

/// A computed route: its legs plus the totals and road labels derived from them.
#[derive(Debug, Clone)]
pub struct CachedRoute {
    pub legs: Vec<CachedLeg>,
    pub distance_m: i64,
    pub duration_s: i64,
    /// The fastest roads the route travels on, at most two (the length `MajorRoadLabels`
    /// allows). The ROAD is carried rather than its name alone, so a caller can label it
    /// by its route number where the network numbers it.
    pub major_roads: Vec<TravelledRoad>,
    /// Whether the route travels on a controlled-access highway (a motorway of the
    /// network), which is what an `Avoid.ControlledAccessHighways` request is answered
    /// against.
    pub uses_controlled_access_highway: bool,
    /// Whether EVERY leg of the route is carried by the network ([`CachedLeg::connected`]).
    pub connected: bool,
}

/// A position snapped onto the road network.
#[derive(Debug, Clone)]
pub struct SnappedPosition {
    /// The point on the road nearest the submitted position, at the coordinate precision
    /// the network's geometry carries ([`round_to_network_precision`]).
    pub position: Position,
    /// Distance from the submitted position to [`SnappedPosition::position`], in metres.
    pub offset_m: f64,
    /// The road snapped to.
    pub road: TravelledRoad,
    /// Distance from the submitted position to the nearest point of the nearest OTHER
    /// road of the network, in metres, where there is one the same traveller may use.
    ///
    /// It is what makes a match CERTAIN or ambiguous: a position far nearer its matched
    /// road than any other lies on that road, while one as near a second road could be on
    /// either, and no offset alone tells the two cases apart.
    pub runner_up_offset_m: Option<f64>,
}

/// Per-service state for com-amazonaws-georoutes-routesservice. A handler reaches it through [`service_state`].
///
/// It is the whole routing model: the road network every geometry and snap is computed
/// against, the per-mode speeds and per-objective factors that are the single cost
/// function, the pricing bucket the responses report, and a memo of the routes already
/// computed.
#[derive(Debug)]
pub struct ComAmazonawsGeoroutesRoutesserviceState {
    /// Segments of the synthetic road network materialised so far. Grown on demand, one
    /// grid cell at a time, from a deterministic rule — so the same position always sees
    /// the same roads however many requests came before.
    pub road_network: Vec<RoadSegment>,
    /// Indices into [`Self::road_network`] by grid cell, so a snap searches one cell.
    road_cells: HashMap<(i64, i64), Vec<usize>>,
    /// Travel speed per mode, in metres per second: the single cost function shared by
    /// CalculateRoutes, CalculateRouteMatrix and OptimizeWaypoints.
    pub travel_mode_speeds: HashMap<TravelMode, f64>,
    /// Multipliers applied for `FastestRoute` versus `ShortestRoute`.
    pub routing_objective_factors: HashMap<RoutingObjective, ObjectiveFactors>,
    /// The value returned in the `x-amz-geo-pricing-bucket` header, by the operation that
    /// answers the request: the tier is per-operation, so it is held per-operation and
    /// read through [`Self::pricing_bucket`].
    pub pricing_buckets: HashMap<&'static str, String>,
    /// Memo of computed routes, keyed by [`route_key`].
    pub calculated_routes: HashMap<String, CachedRoute>,
}

impl Default for ComAmazonawsGeoroutesRoutesserviceState {
    fn default() -> Self {
        // Metres per second. Car 50 km/h, Truck 40 km/h, Scooter 30 km/h, Pedestrian
        // 5 km/h, Transit 35 km/h and Intermodal 30 km/h: the parameters of this
        // emulator's model, held here so every handler costs a leg identically.
        let travel_mode_speeds = HashMap::from([
            (TravelMode::Car, 13.9),
            (TravelMode::Truck, 11.1),
            (TravelMode::Scooter, 8.3),
            (TravelMode::Pedestrian, 1.4),
            (TravelMode::Transit, 9.7),
            (TravelMode::Intermodal, 8.3),
        ]);
        let routing_objective_factors = HashMap::from([
            (
                RoutingObjective::FastestRoute,
                ObjectiveFactors {
                    detour_m: FASTEST_DETOUR_M,
                    speed: 1.0,
                },
            ),
            (
                RoutingObjective::ShortestRoute,
                ObjectiveFactors {
                    detour_m: 0.0,
                    speed: 0.75,
                },
            ),
        ]);
        Self {
            road_network: Vec::new(),
            road_cells: HashMap::new(),
            travel_mode_speeds,
            routing_objective_factors,
            pricing_buckets: HashMap::from([
                ("CalculateIsolines", PRICING_BUCKET_ADVANCED.to_string()),
                ("CalculateRouteMatrix", PRICING_BUCKET_CORE.to_string()),
                ("CalculateRoutes", PRICING_BUCKET_CORE.to_string()),
                ("OptimizeWaypoints", PRICING_BUCKET_ADVANCED.to_string()),
                ("SnapToRoads", PRICING_BUCKET_ADVANCED.to_string()),
            ]),
            calculated_routes: HashMap::new(),
        }
    }
}

impl ComAmazonawsGeoroutesRoutesserviceState {
    /// The pricing tier the queries `operation` serves are charged in, as its response
    /// reports it in `PricingBucket`. An operation no tier is held for is charged in the
    /// base tier rather than left without the `@required` member.
    pub fn pricing_bucket(&self, operation: &str) -> String {
        self.pricing_buckets
            .get(operation)
            .cloned()
            .unwrap_or_else(|| PRICING_BUCKET_CORE.to_string())
    }

    /// Travel speed of `mode`, in metres per second.
    pub fn speed_mps(&self, mode: TravelMode) -> f64 {
        self.travel_mode_speeds
            .get(&mode)
            .copied()
            .unwrap_or(1.4)
            .max(f64::MIN_POSITIVE)
    }

    /// The factors `objective` applies.
    pub fn factors(&self, objective: RoutingObjective) -> ObjectiveFactors {
        self.routing_objective_factors
            .get(&objective)
            .copied()
            .unwrap_or(ObjectiveFactors {
                detour_m: 0.0,
                speed: 1.0,
            })
    }

    /// Materialise the grid cell holding `pos`, if it is not materialised yet.
    ///
    /// A cell outside every mapped territory carries NO road: the network covers mapped
    /// land, so the open sea holds nothing to snap to or travel on.
    fn materialise_cell(&mut self, pos: Position) {
        let cell = cell_of(pos);
        if self.road_cells.contains_key(&cell) {
            return;
        }
        if self.road_cells.len() >= MAX_MATERIALISED_CELLS {
            self.road_network.clear();
            self.road_cells.clear();
        }
        let (lon_min, lat_min) = (cell.0 as f64 * CELL_DEG, cell.1 as f64 * CELL_DEG);
        let (lon_max, lat_max) = (lon_min + CELL_DEG, lat_min + CELL_DEG);
        if !is_mapped_territory(pos) {
            self.road_cells.insert(cell, Vec::new());
            return;
        }
        let mut indices = Vec::new();

        // Meridian roads: constant longitude, spanning the cell north to south.
        let first = (lon_min / ROAD_SPACING_DEG).floor() as i64;
        let last = (lon_max / ROAD_SPACING_DEG).ceil() as i64;
        for i in first..=last {
            let lon = i as f64 * ROAD_SPACING_DEG;
            let class = if i.rem_euclid(FOOTPATH_EVERY) == 0 {
                RoadClass::Footpath
            } else {
                RoadClass::Street
            };
            indices.push(self.road_network.len());
            self.road_network.push(RoadSegment {
                id: format!("m{i}@{},{}", cell.0, cell.1),
                polyline: vec![[lon, lat_min], [lon, lat_max]],
                speed_kph: speed_of(class),
                road_label: label_of(class, true, i),
                allows: allows_of(class),
                class,
                orientation: RoadOrientation::Meridian,
                grid_index: i,
            });
        }

        // Parallel roads: constant latitude, spanning the cell west to east.
        let first = (lat_min / ROAD_SPACING_DEG).floor() as i64;
        let last = (lat_max / ROAD_SPACING_DEG).ceil() as i64;
        for j in first..=last {
            let lat = j as f64 * ROAD_SPACING_DEG;
            let class = if j.rem_euclid(MOTORWAY_EVERY) == 0 {
                RoadClass::Motorway
            } else {
                RoadClass::Street
            };
            indices.push(self.road_network.len());
            self.road_network.push(RoadSegment {
                id: format!("p{j}@{},{}", cell.0, cell.1),
                polyline: vec![[lon_min, lat], [lon_max, lat]],
                speed_kph: speed_of(class),
                road_label: label_of(class, false, j),
                allows: allows_of(class),
                class,
                orientation: RoadOrientation::Parallel,
                grid_index: j,
            });
        }

        self.road_cells.insert(cell, indices);
    }

    /// Snap `pos` onto the nearest road of the network that `mode` may travel on.
    ///
    /// `None` where the network holds no road `mode` may use near `pos`: the position lies
    /// outside every mapped territory, so there is no road network there at all. A caller
    /// still has to answer for it rather than inventing a position.
    pub fn snap(&mut self, pos: Position, mode: TravelMode) -> Option<SnappedPosition> {
        self.materialise_cell(pos);
        let cell = cell_of(pos);
        let candidates = self.road_cells.get(&cell)?;
        let mut best: Option<SnappedPosition> = None;
        for &index in candidates {
            let Some(segment) = self.road_network.get(index) else {
                continue;
            };
            if !segment.allows(mode) {
                continue;
            }
            let (position, offset_m) = nearest_point_on_polyline(pos, &segment.polyline);
            if best.as_ref().is_none_or(|current| offset_m < current.offset_m) {
                best = Some(SnappedPosition {
                    // The point is read off the network, so it is carried at the
                    // network's own precision rather than with the artefacts of the
                    // projection that located it.
                    position: round_to_network_precision(position),
                    offset_m,
                    road: segment.travelled(),
                    runner_up_offset_m: None,
                });
            }
        }
        let mut best = best?;
        // The nearest road that is not the matched one, which is the candidate the match
        // was chosen OVER. A second run of the very same road is not another candidate:
        // one road carries one match however many cells of it were materialised.
        for &index in self.road_cells.get(&cell).into_iter().flatten() {
            let Some(segment) = self.road_network.get(index) else {
                continue;
            };
            if !segment.allows(mode) || segment.travelled().is_same_road(&best.road) {
                continue;
            }
            let (_, offset_m) = nearest_point_on_polyline(pos, &segment.polyline);
            if best.runner_up_offset_m.is_none_or(|current| offset_m < current) {
                best.runner_up_offset_m = Some(offset_m);
            }
        }
        Some(best)
    }

    /// The route from `positions[0]` through every following position, costed by this
    /// service's one cost function.
    ///
    /// `variant` 0 is the direct route; 1, 2, … are the alternatives, each bowing further
    /// off the direct path. The answer is memoised in [`Self::calculated_routes`], so the
    /// same request twice — and a matrix cell beside the direct route for that pair —
    /// return the very same numbers.
    pub fn compute_route(
        &mut self,
        positions: &[Position],
        mode: TravelMode,
        objective: RoutingObjective,
        variant: u32,
    ) -> CachedRoute {
        let key = route_key(positions, mode, objective, variant);
        if let Some(hit) = self.calculated_routes.get(&key) {
            return hit.clone();
        }

        let factors = self.factors(objective);
        let speed = self.speed_mps(mode) * factors.speed.max(f64::MIN_POSITIVE);
        let mut legs: Vec<CachedLeg> = Vec::new();
        // (road, times traversed), in first-seen order.
        let mut roads: Vec<(TravelledRoad, usize)> = Vec::new();
        let mut uses_controlled_access_highway = false;

        for pair in positions.windows(2) {
            let (path, carried) = self.path_between(pair[0], pair[1], mode, objective, variant);
            // The roads the route travelled, counted per SNAPPED sample: an endpoint that
            // inherited the road of its neighbour is the same travel on that road, not a
            // second traversal of it.
            for road in path
                .iter()
                .filter(|vertex| vertex.snapped)
                .filter_map(|vertex| vertex.road.as_ref())
            {
                uses_controlled_access_highway |= road.class == RoadClass::Motorway;
                match roads.iter_mut().find(|(seen, _)| seen.label == road.label) {
                    Some(entry) => entry.1 += 1,
                    None => roads.push((road.clone(), 1)),
                }
            }
            // The distance IS the length of the geometry this leg reports, so a client
            // that measures the returned line gets the number beside it.
            let distance_m = clamp_cost(polyline_length_m(&path_positions(&path)));
            let duration_s = clamp_cost(distance_m as f64 / speed);
            legs.push(CachedLeg {
                start: pair[0],
                end: pair[1],
                distance_m,
                duration_s,
                runs: leg_runs(&path, distance_m, duration_s),
                path,
                travel_mode: mode,
                leg_type: LegType::for_mode(mode),
                connected: carried,
            });
        }

        // `MajorRoadLabels` is what "differentiates the current route from the
        // alternatives": the fastest roads the route actually travels on, most-travelled
        // first among equals. The model allows at most two.
        roads.sort_by(|a, b| {
            b.0.speed_kph
                .total_cmp(&a.0.speed_kph)
                .then(b.1.cmp(&a.1))
                .then_with(|| a.0.label.cmp(&b.0.label))
        });
        let major_roads = roads.into_iter().take(2).map(|(road, _)| road).collect();

        let route = CachedRoute {
            distance_m: clamp_cost(legs.iter().map(|leg| leg.distance_m as f64).sum()),
            duration_s: clamp_cost(legs.iter().map(|leg| leg.duration_s as f64).sum()),
            connected: legs.iter().all(|leg| leg.connected),
            legs,
            major_roads,
            uses_controlled_access_highway,
        };

        if self.calculated_routes.len() >= MAX_MEMOISED_ROUTES {
            self.calculated_routes.clear();
        }
        self.calculated_routes.insert(key, route.clone());
        route
    }

    /// The geometry of one leg: each vertex with the road the leg travels there, and
    /// whether the network carried the leg the whole way.
    ///
    /// The straight line from `from` to `to` is sampled every [`SAMPLE_SPACING_M`], each
    /// interior sample is pushed sideways by the objective's detour plus
    /// `variant * ALTERNATIVE_OFFSET_M` (zero for the shortest direct route) and then
    /// snapped onto the nearest road `mode` may use, so the returned geometry runs along
    /// the road network and its length is the leg's distance. Where a sample lands on a
    /// different road from the one before it, the two runs' own junction is recorded on the
    /// vertex: that is the manoeuvre boundary the leg's travel steps are cut at.
    ///
    /// A sample that snaps onto no road at all is outside the network's coverage, and a
    /// leg with one is NOT carried by the network: nothing of this model connects its
    /// endpoints by road.
    fn path_between(
        &mut self,
        from: Position,
        to: Position,
        mode: TravelMode,
        objective: RoutingObjective,
        variant: u32,
    ) -> (Vec<PathVertex>, bool) {
        let straight_m = haversine_m(from, to);
        // A leg between two positions that are the same place has nothing to travel: it
        // reports that pair as its geometry and costs zero, rather than wandering onto the
        // road network and back.
        if positions_equal(from, to) {
            return (
                vec![PathVertex::off_network(from), PathVertex::off_network(to)],
                is_mapped_territory(from),
            );
        }
        let steps = ((straight_m / SAMPLE_SPACING_M).ceil() as usize).clamp(2, MAX_PATH_SEGMENTS);
        let bow_m = self.factors(objective).detour_m + f64::from(variant) * ALTERNATIVE_OFFSET_M;
        let mut path = vec![PathVertex::off_network(from)];
        let mut previous_road: Option<TravelledRoad> = None;
        let mut carried = is_mapped_territory(from) && is_mapped_territory(to);
        for step in 1..steps {
            let t = step as f64 / steps as f64;
            let mut sample = interpolate(from, to, t);
            if bow_m > 0.0 {
                // A half sine, so the bow leaves and rejoins the direct path at the ends.
                sample = offset_sideways(from, to, sample, bow_m * (std::f64::consts::PI * t).sin());
            }
            match self.snap(sample, mode) {
                Some(snapped) => {
                    // A run boundary is a junction of two roads: where the road changes,
                    // the two roads' own crossing is the point they meet in and the angle
                    // between them is what the manoeuvre there is read from.
                    let junction = previous_road
                        .as_ref()
                        .filter(|previous| !previous.is_same_road(&snapped.road))
                        .and_then(|previous| previous.junction_with(&snapped.road));
                    previous_road = Some(snapped.road.clone());
                    path.push(PathVertex {
                        position: snapped.position,
                        road: Some(snapped.road),
                        snapped: true,
                        junction,
                    });
                }
                None => {
                    carried = false;
                    path.push(PathVertex::off_network(sample));
                }
            }
        }
        path.push(PathVertex::off_network(to));
        path.dedup_by(|a, b| positions_equal(a.position, b.position));
        if path.len() < 2 {
            path.push(PathVertex::off_network(to));
        }
        inherit_endpoint_roads(&mut path);
        (path, carried)
    }
}

/// A leg's submitted endpoints are not snapped points, so they carry no road of their own:
/// the traveller departs on the road the leg's first snapped vertex is on and arrives on
/// the road of its last. Without it a leg's first and last step would be a run of no road
/// rather than the stretch on those roads.
fn inherit_endpoint_roads(path: &mut [PathVertex]) {
    if let Some(first) = path.iter().position(|vertex| vertex.road.is_some()) {
        let road = path[first].road.clone();
        for vertex in &mut path[..first] {
            vertex.road = road.clone();
        }
    }
    if let Some(last) = path.iter().rposition(|vertex| vertex.road.is_some()) {
        let road = path[last].road.clone();
        for vertex in &mut path[last + 1..] {
            vertex.road = road.clone();
        }
    }
}

/// Posted speed of a road class, in km/h.
fn speed_of(class: RoadClass) -> f64 {
    match class {
        RoadClass::Motorway => MOTORWAY_SPEED_KPH,
        RoadClass::Street => STREET_SPEED_KPH,
        RoadClass::Footpath => FOOTPATH_SPEED_KPH,
    }
}

/// The modes a road class carries. A motorway carries no pedestrian; a footpath carries
/// nothing else.
fn allows_of(class: RoadClass) -> Vec<TravelMode> {
    match class {
        RoadClass::Motorway => vec![
            TravelMode::Car,
            TravelMode::Truck,
            TravelMode::Scooter,
            TravelMode::Transit,
            TravelMode::Intermodal,
        ],
        RoadClass::Street => vec![
            TravelMode::Car,
            TravelMode::Truck,
            TravelMode::Scooter,
            TravelMode::Pedestrian,
            TravelMode::Transit,
            TravelMode::Intermodal,
        ],
        RoadClass::Footpath => vec![TravelMode::Pedestrian],
    }
}

/// The route number designating a motorway of the network at grid index `index`: the
/// network numbers its motorways, which is what [`TravelledRoad::route_number`] reports
/// and what [`label_of`] names a motorway after.
///
/// A designation is what a client puts on a highway shield, so it is built the way a road
/// designation is: a prefix, the road's own number, and the hemisphere letter that tells
/// the two roads of one number apart. The grid index it is derived from is an internal
/// coordinate of this network — its SIGN says which side of the equator the road is on and
/// its multiple of [`MOTORWAY_EVERY`] counts the motorways from there — so neither the
/// sign nor the spacing reaches the designation.
fn motorway_number(index: i64) -> String {
    format!(
        "M{}{}",
        (index / MOTORWAY_EVERY).unsigned_abs(),
        hemisphere(false, index)
    )
}

/// The name of the road at grid index `index`.
///
/// Like [`motorway_number`], a name carries the road's number and the side of the origin
/// it runs on, never the signed grid index itself: a negative numeral is an artefact of
/// this network's coordinates and not something a client could render or match a road by.
fn label_of(class: RoadClass, meridian: bool, index: i64) -> String {
    let number = index.unsigned_abs();
    let side = hemisphere(meridian, index);
    match (class, meridian) {
        (RoadClass::Motorway, _) => format!("Motorway {}", motorway_number(index)),
        (RoadClass::Footpath, _) => format!("Footpath F{number}{side}"),
        (RoadClass::Street, true) => format!("Meridian Street {number}{side}"),
        (RoadClass::Street, false) => format!("Parallel Avenue {number}{side}"),
    }
}

/// Which side of the prime meridian a meridian road runs on, or which side of the equator
/// a parallel road runs on: the letter a road designation carries instead of the sign of
/// its grid index.
fn hemisphere(meridian: bool, index: i64) -> &'static str {
    match (meridian, index < 0) {
        (true, true) => "W",
        (true, false) => "E",
        (false, true) => "S",
        (false, false) => "N",
    }
}

/// The grid cell a position falls in.
fn cell_of(pos: Position) -> (i64, i64) {
    (
        (pos[0] / CELL_DEG).floor() as i64,
        (pos[1] / CELL_DEG).floor() as i64,
    )
}

/// Metres per degree of longitude and of latitude at `latitude`.
fn metres_per_degree(latitude: f64) -> (f64, f64) {
    (
        METRES_PER_DEGREE_LON_EQUATOR * latitude.to_radians().cos().abs().max(1e-6),
        METRES_PER_DEGREE_LAT,
    )
}

/// Great-circle distance between two positions, in metres.
pub fn haversine_m(a: Position, b: Position) -> f64 {
    let (lat1, lat2) = (a[1].to_radians(), b[1].to_radians());
    let d_lat = lat2 - lat1;
    let d_lon = (b[0] - a[0]).to_radians();
    let h = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().clamp(0.0, 1.0).asin()
}

/// Length of a polyline, in metres: the sum of the haversine distance over its segments.
pub fn polyline_length_m(polyline: &[Position]) -> f64 {
    polyline
        .windows(2)
        .map(|pair| haversine_m(pair[0], pair[1]))
        .sum()
}

/// The position a fraction `t` of the way from `from` to `to`, interpolated linearly in
/// degrees (the legs this service costs are short enough for the difference from a
/// great-circle interpolation to stay below the geometry's 6-digit precision).
fn interpolate(from: Position, to: Position, t: f64) -> Position {
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
    ]
}

/// `sample` moved `offset_m` metres perpendicular to the `from`-`to` direction, kept
/// inside the WGS 84 bounds.
fn offset_sideways(from: Position, to: Position, sample: Position, offset_m: f64) -> Position {
    let (mx, my) = metres_per_degree(sample[1]);
    let (dx, dy) = ((to[0] - from[0]) * mx, (to[1] - from[1]) * my);
    let norm = (dx * dx + dy * dy).sqrt();
    if norm < f64::EPSILON {
        return sample;
    }
    // Rotate the direction by 90 degrees, in metres, then back into degrees.
    [
        (sample[0] + (-dy / norm) * offset_m / mx).clamp(LONGITUDE_BOUNDS.0, LONGITUDE_BOUNDS.1),
        (sample[1] + (dx / norm) * offset_m / my).clamp(LATITUDE_BOUNDS.0, LATITUDE_BOUNDS.1),
    ]
}

/// The point of `polyline` nearest `pos`, and its distance from `pos` in metres.
fn nearest_point_on_polyline(pos: Position, polyline: &[Position]) -> (Position, f64) {
    let mut best = (pos, f64::INFINITY);
    for pair in polyline.windows(2) {
        let candidate = nearest_point_on_segment(pos, pair[0], pair[1]);
        let distance = haversine_m(pos, candidate);
        if distance < best.1 {
            best = (candidate, distance);
        }
    }
    if best.1.is_finite() {
        best
    } else {
        let end = polyline.first().copied().unwrap_or(pos);
        (end, haversine_m(pos, end))
    }
}

/// The point of the segment `a`-`b` nearest `pos`, projected in the local planar frame at
/// `pos`'s latitude.
fn nearest_point_on_segment(pos: Position, a: Position, b: Position) -> Position {
    let (mx, my) = metres_per_degree(pos[1]);
    let (ax, ay) = (a[0] * mx, a[1] * my);
    let (bx, by) = (b[0] * mx, b[1] * my);
    let (px, py) = (pos[0] * mx, pos[1] * my);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    if len2 < f64::EPSILON {
        return a;
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    [(ax + dx * t) / mx, (ay + dy * t) / my]
}

/// `position` at the coordinate precision the network's geometry carries
/// ([`POLYLINE_PRECISION`] digits, ~0.1 m).
///
/// A position READ OFF the network is located by projecting into metres and back, so the
/// arithmetic leaves digits far below that precision on it. They are not part of the
/// network: the same road vertex reached two ways would otherwise be reported as two
/// places. A position the CALLER supplied is never put through this — it is echoed as it
/// was sent.
pub fn round_to_network_precision(position: Position) -> Position {
    let Ok(digits) = i32::try_from(POLYLINE_PRECISION) else {
        return position;
    };
    let scale = 10_f64.powi(digits);
    [
        (position[0] * scale).round() / scale,
        (position[1] * scale).round() / scale,
    ]
}

/// The scale, in metres at `latitude`, on which a match onto this network is uncertain:
/// half the spacing between neighbouring roads, which is the farthest a position inside
/// the network's coverage can be from the nearest road along one axis.
///
/// A match whose offset is this much is as far from its road as a position can be, and a
/// second road no further away than this is as good a candidate as the matched one.
pub fn snap_ambiguity_m(latitude: f64) -> f64 {
    let (metres_per_lon, metres_per_lat) = metres_per_degree(latitude);
    (ROAD_SPACING_DEG * metres_per_lon.min(metres_per_lat) / 2.0).max(f64::MIN_POSITIVE)
}

/// The path ALONG THE ROADS from one matched position to another: the geometry a traveller
/// following the network covers between the two points.
///
/// Both ends lie on a road of the network, so the path runs on roads the whole way and
/// never cuts across the blocks between them:
///
/// * on one road, the stretch of that road between them;
/// * on two roads that cross, along the first to the junction the two meet in
///   ([`TravelledRoad::junction_with`]) and along the second from there;
/// * on two roads of the same orientation, which never meet, along the first to the
///   nearest road crossing both, along that one, and along the second — the corners are
///   junctions of the network, not chosen points.
pub fn road_path_between(from: &SnappedPosition, to: &SnappedPosition) -> Vec<Position> {
    let (start, end) = (from.position, to.position);
    let mut path = vec![start];
    if !from.road.is_same_road(&to.road) {
        match from.road.junction_with(&to.road) {
            Some(junction) => path.push(round_to_network_precision(junction.position)),
            None => {
                // The two roads run parallel. The axis they both run along is where a
                // crossing road sits at every multiple of the grid spacing, so the one
                // nearest the far end connects them with no detour past it.
                let axis = from.road.orientation.axis();
                let crossing = (end[axis] / ROAD_SPACING_DEG).round() * ROAD_SPACING_DEG;
                let mut leave = start;
                leave[axis] = crossing;
                let mut join = end;
                join[axis] = crossing;
                path.push(round_to_network_precision(leave));
                path.push(round_to_network_precision(join));
            }
        }
    }
    path.push(end);
    path.dedup_by(|a, b| positions_equal(*a, *b));
    path
}

/// Whether two positions are the same at the 6-digit precision the geometry carries.
fn positions_equal(a: Position, b: Position) -> bool {
    (a[0] - b[0]).abs() < POSITION_EPSILON && (a[1] - b[1]).abs() < POSITION_EPSILON
}

/// Two positions closer than this in a coordinate are the same place at the 6-digit
/// precision the geometry carries, so nothing is read off the difference between them.
const POSITION_EPSILON: f64 = 1e-7;

/// Bearing from `from` to `to`, in degrees clockwise from north, in the local planar frame
/// between them. `None` when the two positions are the same place, which has no bearing.
pub fn planar_bearing_deg(from: Position, to: Position) -> Option<f64> {
    let (mx, my) = metres_per_degree((from[1] + to[1]) / 2.0);
    let (dx, dy) = ((to[0] - from[0]) * mx, (to[1] - from[1]) * my);
    if dx.hypot(dy) < f64::EPSILON {
        return None;
    }
    Some(dx.atan2(dy).to_degrees().rem_euclid(360.0))
}

/// The position `distance_m` metres from `from` along `bearing_deg`, the bearing measured
/// clockwise from north in the local planar frame at `from` — the inverse of
/// [`planar_bearing_deg`], kept inside the WGS 84 bounds.
pub fn position_at_bearing(from: Position, bearing_deg: f64, distance_m: f64) -> Position {
    let (mx, my) = metres_per_degree(from[1]);
    let radians = bearing_deg.to_radians();
    [
        (from[0] + distance_m * radians.sin() / mx).clamp(LONGITUDE_BOUNDS.0, LONGITUDE_BOUNDS.1),
        (from[1] + distance_m * radians.cos() / my).clamp(LATITUDE_BOUNDS.0, LATITUDE_BOUNDS.1),
    ]
}

/// The turn from bearing `entered` to bearing `left`, in degrees in `(-180, 180]`:
/// positive to the right (clockwise), negative to the left.
pub fn turn_between_deg(entered: f64, left: f64) -> f64 {
    let delta = (left - entered).rem_euclid(360.0);
    if delta > 180.0 {
        delta - 360.0
    } else {
        delta
    }
}

/// Round a cost into the model's `0..4294967295` range.
pub fn clamp_cost(value: f64) -> i64 {
    if !value.is_finite() || value < 0.0 {
        return 0;
    }
    (value.round() as i64).clamp(0, MAX_DISTANCE_OR_DURATION)
}

/// The memo key of a route: the positions at the geometry's precision, the travel mode,
/// the objective and the alternative index.
pub fn route_key(
    positions: &[Position],
    mode: TravelMode,
    objective: RoutingObjective,
    variant: u32,
) -> String {
    let mut key = String::new();
    for position in positions {
        key.push_str(&format!("{:.6},{:.6};", position[0], position[1]));
    }
    key.push_str(mode.as_model());
    key.push('/');
    key.push_str(match objective {
        RoutingObjective::FastestRoute => "FastestRoute",
        RoutingObjective::ShortestRoute => "ShortestRoute",
    });
    key.push('/');
    key.push_str(&variant.to_string());
    key
}

/// A position read off a model member, or the sentence for the rule it broke.
///
/// The sentence carries NO member path: which member was refused is reported by the
/// caller in the `ValidationException`'s `fieldList`, and the sentence states the rule —
/// the coordinate component, the degree domain it has to lie in, and the value the
/// request set.
pub fn validate_position(value: &[f64]) -> Result<Position, String> {
    let [lon, lat] = match value {
        [lon, lat] => [*lon, *lat],
        other => {
            return Err(format!(
                "A position must be set to 2 values, [longitude, latitude], but was set to {} values.",
                other.len()
            ))
        }
    };
    check_degrees("Longitude", LONGITUDE_BOUNDS, lon)?;
    check_degrees("Latitude", LATITUDE_BOUNDS, lat)?;
    Ok([lon, lat])
}

/// One coordinate against its degree domain: `Ok` inside it, else the sentence for a
/// coordinate outside it.
fn check_degrees(component: &str, bounds: (f64, f64), value: f64) -> Result<(), String> {
    if value.is_finite() && value >= bounds.0 && value <= bounds.1 {
        return Ok(());
    }
    Err(must_be_between(component, bounds, "degrees", value))
}

/// The sentence for a value outside the degree domain a coordinate component lies in:
/// fixed text names the component, both bounds and the unit they are in, and the only part
/// taken from the request is the value it set.
fn must_be_between(
    component: &str,
    bounds: (f64, f64),
    unit: &str,
    value: impl std::fmt::Display,
) -> String {
    format!(
        "{component} must be between {} and {} {unit}, but was set to {value}.",
        bounds.0, bounds.1
    )
}

/// The sentence for a rule no model can express: a set of otherwise-optional members of
/// which a request must set at least one. The members are the only variable part — listed
/// bare, which is how a recorded refusal of this class states them — and the member the
/// refusal names in its `fieldList` is the first of them.
///
/// The sibling sentences of this service's other field rules state their member lists
/// QUOTED ([`only_one_of`], [`only_one_of_within`]): each class is worded as this service
/// words that class.
pub fn at_least_one_of(members: &[&str]) -> String {
    format!(
        "At least one of the following fields must be set: {}.",
        members.join(", ")
    )
}

/// The sentence for the opposite rule, equally inexpressible: a set of members that are
/// ALTERNATIVES, of which a request set more than one. The whole exclusive set is stated,
/// not the subset the request happened to populate, because the set is a property of the
/// shape the members belong to and is what a caller needs to pick one of.
pub fn only_one_of(members: &[&str]) -> String {
    format!(
        "Only one of the following fields may be set: {}.",
        quoted_list(members)
    )
}

/// [`only_one_of`] for alternatives that are members of a nested structure rather than of
/// the request itself: the container is named, so the member names read against the shape
/// that declares them.
pub fn only_one_of_within(container: &str, members: &[&str]) -> String {
    format!(
        "Only one of the following fields may be set within '{container}': {}.",
        quoted_list(members)
    )
}

/// Member names as a refusal ENUMERATES them: each quoted, separated by commas —
/// `'A'`, `'A', 'B'`. Used where the sentence lists a set rather than reading as prose.
pub fn quoted_list(members: &[&str]) -> String {
    quoted(members).join(", ")
}

/// Member names as a refusal lists them: each quoted, separated by commas, with the last
/// one joined by `and` — `'A'`, `'A' and 'B'`, `'A', 'B' and 'C'`.
pub fn quoted_members(members: &[&str]) -> String {
    let quoted = quoted(members);
    match quoted.split_last() {
        None => String::new(),
        Some((last, &[])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Each member single-quoted, the one way a refusal of this service quotes a member name.
fn quoted(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| format!("'{member}'")).collect()
}

/// `'{member}' {rule}` — the shape of a refusal that names the member at fault. The
/// member is single-quoted and spelled the way the refusal's `fieldList` names it: the
/// model's own spelling, with the position of the offending element where it is one.
pub fn member_rule(member: &str, rule: &str) -> String {
    format!("'{member}' {rule}")
}

/// A value below the minimum of the range its member allows.
pub fn must_not_be_less_than(member: &str, minimum: impl std::fmt::Display) -> String {
    member_rule(member, &format!("must not be less than {minimum}"))
}

/// A value above the maximum of the range its member allows.
pub fn must_not_be_greater_than(member: &str, maximum: impl std::fmt::Display) -> String {
    member_rule(member, &format!("must not be greater than {maximum}"))
}

/// A numeric member against the range it allows, stated by the bound the value broke and
/// never quoting the value back.
///
/// Bounds are taken as `f64` so one rule covers an integral member and a floating-point
/// one; an integral bound renders without a decimal point. A value that is not a number at
/// all cannot be inside the range, so it is reported against the maximum.
pub fn check_range(member: &str, bounds: (f64, f64), value: f64) -> Result<(), String> {
    if value.is_finite() && value >= bounds.0 && value <= bounds.1 {
        return Ok(());
    }
    if value < bounds.0 {
        return Err(must_not_be_less_than(member, bounds.0));
    }
    Err(must_not_be_greater_than(member, bounds.1))
}

/// A value shorter than the length its member allows. Worded as a LENGTH rather than in
/// entries, because the same rule applies to a string.
pub fn must_have_length_at_least(member: &str, minimum: impl std::fmt::Display) -> String {
    member_rule(member, &format!("must have length at least {minimum}"))
}

/// A value longer than the length its member allows.
pub fn must_have_length_at_most(member: &str, maximum: impl std::fmt::Display) -> String {
    member_rule(member, &format!("must have length at most {maximum}"))
}

/// A `@required` member the request left out.
///
/// Ends in a period, as the sibling Amazon Location service's recorded refusal of an absent
/// required member does; the bound sentences above carry none, which is how their own
/// recordings read.
pub fn must_not_be_null(member: &str) -> String {
    member_rule(member, "must not be null.")
}

/// A value that does not satisfy the `@pattern` its member declares, naming the modelled
/// expression itself so the caller can see what the value had to look like.
pub fn must_match_regex(member: &str, pattern: &str) -> String {
    member_rule(member, &format!("must match regex {pattern}"))
}

/// A value outside the set an enumerated member declares. The accepted set is taken from
/// the model — the generated value list, or the trait the boundary walk reported — so it
/// cannot drift from what the member actually admits.
pub fn must_be_one_of<S: AsRef<str>>(member: &str, accepted: &[S]) -> String {
    let accepted: Vec<&str> = accepted.iter().map(AsRef::as_ref).collect();
    member_rule(member, &format!("must be one of: {}", accepted.join(", ")))
}

/// A collection member against the length it allows, stated by the bound it broke. Worded
/// as a LENGTH rather than in entries, because the same rule applies to a string.
pub fn check_length(member: &str, bounds: (usize, usize), length: usize) -> Result<(), String> {
    if length < bounds.0 {
        return Err(must_have_length_at_least(member, bounds.0));
    }
    if length > bounds.1 {
        return Err(must_have_length_at_most(member, bounds.1));
    }
    Ok(())
}

/// The member a path from the framework's constraint walk names, spelled the way the MODEL
/// spells it — which is how every refusal of this service names a member.
///
/// The walk reports a path in lowerCamel with a collection step spelled `<1-based
/// position>.member` (`maxAlternatives`, `thresholds.time`, `waypoints.2.member.position`),
/// while this service names the member as the model declares it and locates an element by
/// its own 0-based subscript (`MaxAlternatives`, `Thresholds.Time`,
/// `Waypoints[1].Position`) — the spelling its handlers already use in a `fieldList` entry,
/// so one rule reads alike whether it was caught at the boundary or inside a handler.
///
/// Capitalising each segment recovers the model's spelling because every member this
/// service declares is PascalCase in the model and lowerCamel on the wire, which is the
/// regular mapping the path builder applies.
pub fn member_of_path(path: &str) -> String {
    let mut named: Vec<String> = Vec::new();
    let mut pending_position: Option<usize> = None;
    for segment in path.split('.') {
        // A collection step is the pair `<position>.member`: the position subscripts the
        // segment before it rather than becoming a segment of its own.
        if let Ok(position) = segment.parse::<usize>() {
            pending_position = Some(position);
            continue;
        }
        if segment == "member" {
            if let (Some(position), Some(last)) = (pending_position.take(), named.last_mut()) {
                last.push_str(&format!("[{}]", position.saturating_sub(1)));
            }
            continue;
        }
        pending_position = None;
        let mut characters = segment.chars();
        if let Some(first) = characters.next() {
            named.push(first.to_ascii_uppercase().to_string() + characters.as_str());
        }
    }
    if named.is_empty() {
        path.to_string()
    } else {
        named.join(".")
    }
}

/// This service's wording for ONE modelled constraint the framework found violated before
/// any handler ran: the member it names, and the rule the member's own trait states.
///
/// The rejected value is never quoted back: a refusal of this service states the RULE, and
/// which member broke it, so the sentence is the same for every value that breaks it.
pub fn violation_message(violation: &::smithy_server_runtime::Violation) -> String {
    use ::smithy_server_runtime::ViolationKind;

    let member = member_of_path(&violation.path);
    match &violation.kind {
        ViolationKind::MissingRequiredMember => must_not_be_null(&member),
        ViolationKind::MinLength(minimum) => must_have_length_at_least(&member, minimum),
        ViolationKind::MaxLength(maximum) => must_have_length_at_most(&member, maximum),
        ViolationKind::MinValue(minimum) => must_not_be_less_than(&member, minimum),
        ViolationKind::MaxValue(maximum) => must_not_be_greater_than(&member, maximum),
        ViolationKind::Pattern(pattern) => must_match_regex(&member, pattern),
        ViolationKind::EnumValue(accepted) => must_be_one_of(&member, accepted),
        ViolationKind::UniqueItems => member_rule(&member, "must not contain duplicate values"),
        // `ViolationKind` is `#[non_exhaustive]`: a class this service does not word yet
        // still names its member rather than falling back to a generic rendering that
        // spells the member a way no caller of this service ever sees.
        _ => member_rule(&member, "must satisfy the constraints the model declares"),
    }
}

/// This service's wording for every modelled constraint one refused request violated.
///
/// Each violation is stated in full and the sentences joined, rather than summarised behind
/// a count: a request that broke two rules has broken both, and a caller fixing it needs
/// each member named.
pub fn violations_message(violations: &[::smithy_server_runtime::Violation]) -> String {
    violations
        .iter()
        .map(violation_message)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Which of a `TravelModeOptions` structure's per-mode option blocks the request set.
///
/// One entry per member the operation's own options structure DECLARES, in the model's
/// order, paired with whether the request set it. Each handler builds this from its own
/// generated options type, so the set a rule is checked over cannot drift from the model.
pub type DeclaredTravelModeOptions<'a> = [(&'a str, bool)];

/// The option block of a `TravelModeOptions` structure that BELONGS to `mode`: the block
/// whose member names that mode. At most one can, since a structure declares each member
/// once; `None` where the structure declares no block for the selected mode (a matrix is
/// modelled with no pedestrian options at all).
pub fn block_of_mode<'a>(mode: &str, declared: &DeclaredTravelModeOptions<'a>) -> Option<&'a str> {
    declared
        .iter()
        .map(|(member, _)| *member)
        .find(|member| *member == mode)
}

/// The per-mode option block of a `TravelModeOptions` structure belongs to ONE travel mode:
/// the member whose name is that mode. A request may therefore set only a block the service
/// permits for the `TravelMode` it selected, and setting any other is refused — the options
/// would otherwise be silently dropped and the answer computed for a different vehicle than
/// the constraints describe.
///
/// `permitted` is the set of blocks the operation allows for the selected mode (normally the
/// block that names it, per [`block_of_mode`]); `declared` carries every block the
/// operation's options structure declares and whether the request set it. `Err` is the pair
/// a `ValidationException` is built from: the member path its `fieldList` names, and the
/// sentence. Which sentence it is depends on whether the mode permits any block at all:
/// where it does, the refusal states the blocks that may be set; where it permits none,
/// there is no set to state, so the refusal names the block that must not be set instead.
pub fn check_travel_mode_options(
    mode: &str,
    permitted: &[&str],
    declared: &DeclaredTravelModeOptions<'_>,
) -> Result<(), (String, String)> {
    let Some((offender, _)) = declared
        .iter()
        .find(|(member, is_set)| *is_set && !permitted.contains(member))
    else {
        return Ok(());
    };
    let member = format!("{TRAVEL_MODE_OPTIONS_MEMBER}.{offender}");
    let message = if permitted.is_empty() {
        format!("'{member}' must not be set when 'TravelMode' has value {mode}.")
    } else {
        format!(
            "Only the following fields may be set within '{TRAVEL_MODE_OPTIONS_MEMBER}' when \
             'TravelMode' has value {mode}: {}.",
            quoted_list(permitted)
        )
    };
    Err((member, message))
}

/// The member every operation of this service carries its per-mode options under.
const TRAVEL_MODE_OPTIONS_MEMBER: &str = "TravelModeOptions";

/// The geometry format a request asked for, or the documented default. `Err` names the
/// member for a value `GeometryFormat` does not declare, read from the generated
/// `GEOMETRY_FORMAT_VALUES` so the accepted set cannot drift from the model (the boundary
/// validator refuses those first; this is the belt).
pub fn geometry_format(member: &str, requested: Option<&String>) -> Result<String, String> {
    let accepted = crate::ops::types::GEOMETRY_FORMAT_VALUES;
    match requested.map(String::as_str) {
        None => Ok(DEFAULT_GEOMETRY_FORMAT.to_string()),
        Some(format) if accepted.contains(&format) => Ok(format.to_string()),
        Some(_) => Err(must_be_one_of(member, accepted)),
    }
}

/// `positions` encoded as a Flexible Polyline, the wire form of the `FlexiblePolyline`
/// geometry format, at this service's [`POLYLINE_PRECISION`]. `None` when a coordinate
/// cannot be carried at that precision.
pub fn encode_flexible_polyline(positions: &[Position]) -> Option<String> {
    encode_flexible_polyline_at(positions, POLYLINE_PRECISION)
}

/// `positions` encoded as a Flexible Polyline at `precision` decimal digits.
///
/// The Flexible Polyline format (HERE's published 2D/3D line encoding, which the model's
/// `FlexiblePolyline` geometry format names): a header of two unsigned varints — the
/// format version, then `thirdDimPrecision << 7 | thirdDim << 4 | precision2d`, here with
/// no third dimension — followed, for each position in order, by the latitude delta and
/// then the longitude delta from the previous position, each scaled by `10^precision`,
/// rounded to an integer and written as a zig-zag signed varint in the format's 64-character
/// alphabet. Deltas make a line of nearby positions short, which is the point of the format.
///
/// `None` when a scaled coordinate does not fit an `i64`, i.e. the caller handed in a
/// position outside the WGS 84 bounds this service validates before it ever gets here.
pub fn encode_flexible_polyline_at(positions: &[Position], precision: u32) -> Option<String> {
    if precision > 15 {
        return None;
    }
    let scale = 10_f64.powi(i32::try_from(precision).ok()?);
    let mut out = String::new();
    encode_unsigned_varint(POLYLINE_FORMAT_VERSION, &mut out);
    // No third dimension and so no third-dimension precision: the service returns 2D lines.
    encode_unsigned_varint(u64::from(precision), &mut out);

    let mut last_lat: i64 = 0;
    let mut last_lon: i64 = 0;
    for position in positions {
        let lat = scaled_coordinate(position[1], scale)?;
        let lon = scaled_coordinate(position[0], scale)?;
        encode_signed_varint(lat.checked_sub(last_lat)?, &mut out);
        encode_signed_varint(lon.checked_sub(last_lon)?, &mut out);
        last_lat = lat;
        last_lon = lon;
    }
    Some(out)
}

/// `value` scaled by `scale` and rounded to the nearest integer, half away from zero —
/// the rounding the format's reference implementation uses. `None` when the result is not
/// an `i64`.
fn scaled_coordinate(value: f64, scale: f64) -> Option<i64> {
    let scaled = (value * scale).round();
    if !scaled.is_finite() || scaled.abs() >= 9.0e18 {
        return None;
    }
    Some(scaled as i64)
}

/// `value` as a zig-zag signed varint: the sign moves into the low bit so that a small
/// negative delta stays short.
fn encode_signed_varint(value: i64, out: &mut String) {
    let shifted = (value as u64) << 1;
    let zigzag = if value < 0 { !shifted } else { shifted };
    encode_unsigned_varint(zigzag, out);
}

/// `value` as an unsigned varint in the format's alphabet: five bits per character, low
/// bits first, with bit 6 set on every character but the last.
fn encode_unsigned_varint(mut value: u64, out: &mut String) {
    while value > 0x1F {
        let index = ((value & 0x1F) | 0x20) as usize;
        out.push(char::from(POLYLINE_ALPHABET[index]));
        value >>= 5;
    }
    out.push(char::from(POLYLINE_ALPHABET[value as usize]));
}

/// A position as the generated members carry it.
pub fn position_to_vec(position: Position) -> Vec<f64> {
    vec![position[0], position[1]]
}

/// A polyline as a `LineString` member.
pub fn line_string(positions: &[Position]) -> Vec<Vec<f64>> {
    positions.iter().copied().map(position_to_vec).collect()
}

/// The current instant, for a request that asks to depart now and for the default
/// departure of one that names no time at all.
pub fn now() -> Instant {
    Instant::from(SystemTime::now())
}

/// An RFC 3339 timestamp as the model's `TimestampWithTimezoneOffset` carries it, or a
/// message naming the member.
///
/// Both forms the pattern admits name an instant and both are accepted: a UTC instant, and
/// a LOCAL time with an explicit UTC offset, which names the instant its local time is that
/// offset ahead of.
pub fn parse_timestamp(member: &str, value: &str) -> Result<Instant, String> {
    let parsed = ::chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
        format!("{member} must be a timestamp of the form YYYY-MM-DDThh:mm:ss.sssZ or YYYY-MM-DDThh:mm:ss.sss+hh:mm; got {value}")
    })?;
    Ok(Instant::from_secs_and_nanos(
        parsed.timestamp(),
        parsed.timestamp_subsec_nanos().min(999_999_999),
    ))
}

/// `instant` as a `TimestampWithTimezoneOffset` in UTC, truncated to whole seconds (the
/// durations this service computes are whole seconds). `None` only for an instant the
/// format cannot carry.
///
/// A time the REQUEST stated goes through [`UtcOffset`] instead, so it is reported the way
/// the request stated it.
pub fn format_timestamp(instant: Instant) -> Option<String> {
    UtcOffset::UTC.timestamp(instant)
}

/// How a reported timestamp is stated: at a fixed UTC offset, or in UTC with the format's
/// `Z` designator.
///
/// `TimestampWithTimezoneOffset` carries either a UTC instant or a local time with an
/// explicit offset — "Time format: `YYYY-MM-DDThh:mm:ss.sssZ |
/// YYYY-MM-DDThh:mm:ss.sss+hh:mm`" — and the times these operations report are the times
/// the REQUEST pinned the calculation to. So a reported timestamp is stated the way the
/// request stated its own: a caller that pinned a local departure reads that same local
/// time back, rather than the same instant re-expressed in UTC, and can tell which time
/// zone the answer describes. An instant that came from this service's clock was stated
/// nowhere, so it is reported in UTC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UtcOffset(Option<::chrono::FixedOffset>);

impl UtcOffset {
    /// UTC, written with the format's `Z` designator: what an instant this service read off
    /// its own clock is reported at.
    pub const UTC: Self = Self(None);

    /// How the request stated `timestamp`: at the UTC offset it carries, or in UTC for a
    /// value written with the `Z` designator and for a request that named no timestamp at
    /// all.
    pub fn stated_in(timestamp: Option<&str>) -> Self {
        let Some(value) = timestamp else {
            return Self::UTC;
        };
        if value.ends_with('Z') || value.ends_with('z') {
            return Self::UTC;
        }
        Self(
            ::chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|parsed| *parsed.offset()),
        )
    }

    /// `instant` as a `TimestampWithTimezoneOffset` stated this way: the same instant,
    /// written as the local time of this offset and labelled with it, or in UTC with the
    /// `Z` designator. Truncated to whole seconds, the resolution of every duration this
    /// service computes. `None` only for an instant the format cannot carry.
    pub fn timestamp(self, instant: Instant) -> Option<String> {
        let utc = ::chrono::DateTime::from_timestamp(instant.secs(), 0)?;
        Some(match self.0 {
            None => utc.to_rfc3339_opts(::chrono::SecondsFormat::Secs, true),
            Some(offset) => utc
                .with_timezone(&offset)
                .to_rfc3339_opts(::chrono::SecondsFormat::Secs, false),
        })
    }
}

/// `instant` moved by `seconds`, which may be negative.
pub fn shift_seconds(instant: Instant, seconds: i64) -> Instant {
    Instant::from_secs_and_nanos(instant.secs().saturating_add(seconds), instant.subsec_nanos())
}

/// Whole seconds from `from` to `to`, negative when `to` is the earlier instant. The
/// subsecond parts are dropped: every duration this service computes is whole seconds.
pub fn seconds_between(from: Instant, to: Instant) -> i64 {
    to.secs().saturating_sub(from.secs())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGeoroutesRoutesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGeoroutesRoutesserviceState>)
}
