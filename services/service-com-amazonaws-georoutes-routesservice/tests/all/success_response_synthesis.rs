// SPDX-License-Identifier: CC0-1.0

//! How the successful answers of this service are SYNTHESIZED, over the classes of request
//! each rule covers.
//!
//! The rules under test here are:
//!
//! * `PricingBucket` is the tier the OPERATION's queries are charged in: one value per
//!   operation, the same for every request that operation serves;
//! * a position READ OFF the road network is reported at the network's own coordinate
//!   precision, while a position the CALLER supplied is echoed exactly as submitted;
//! * `SnapToRoads` scores a match on the match itself — how far the point moved and
//!   whether a second road was as good a candidate — and never on the `SnapRadius` the
//!   caller allowed;
//! * `SnappedGeometry` runs ALONG the roads the trace matched onto, so every vertex is on
//!   a road and every segment follows one;
//! * `OptimizeWaypoints` answers the order whose WHOLE journey is cheapest, by the same
//!   cost table `CalculateRouteMatrix` reports for those stops;
//! * a road of the network is identified by a designation a client can render, never by an
//!   internal signed index.
//!
//! Each table spans what its rule generalises over (operation, request shape, coordinate
//! precision, hemisphere, travel mode, objective, stop count, radius) rather than
//! restating one recorded request.

use serde_json::{json, Value};

use crate::routing::{
    great_circle_m, int, position, post, routes_service, MATRIX, OPTIMIZE, ROUTES, SNAP,
};

/// The path `CalculateIsolines` is bound to.
const ISOLINES: &str = "/v2/isolines";

/// Spacing of the synthetic road grid, in degrees, as documented on `ROAD_SPACING_DEG` in
/// `src/state.rs`.
const ROAD_SPACING_DEG: f64 = 0.0005;

/// Precision of the coordinates the network's geometry carries, in decimal digits, as
/// documented on `POLYLINE_PRECISION` in `src/state.rs`.
const NETWORK_PRECISION_DIGITS: i32 = 6;

/// Whether `coordinate` is a multiple of the grid spacing, i.e. whether it is the fixed
/// coordinate of a road of the network.
fn on_a_road_line(coordinate: f64) -> bool {
    let multiples = coordinate / ROAD_SPACING_DEG;
    (multiples - multiples.round()).abs() * ROAD_SPACING_DEG <= 1e-9
}

/// Whether `value` carries no more than the network's coordinate precision, which is what
/// a position read off the network must be reported at: rounding it to that precision
/// leaves it unchanged.
fn at_network_precision(value: f64) -> bool {
    let scale = 10_f64.powi(NETWORK_PRECISION_DIGITS);
    (value * scale).round() / scale == value
}

/// The farthest a match may move a position inside the network's coverage, in metres: half
/// the grid spacing, in the one axis the match moves it along, at the most metres a degree
/// of this grid is worth anywhere on Earth (~111.3 km per degree at the equator).
const MAX_MATCH_OFFSET_M: f64 = ROAD_SPACING_DEG / 2.0 * 111_320.0;

// ---------------------------------------------------------------------------
// The pricing tier of an operation
// ---------------------------------------------------------------------------

/// `PricingBucket` ("the pricing bucket for which the query is charged at", `@required`
/// and bound to `x-amz-geo-pricing-bucket`) names the tier the OPERATION is charged in:
/// the route and matrix queries in the base tier, the isoline, waypoint-optimization and
/// road-snapping queries in the advanced one. The model enumerates no values, so the two
/// tokens come from recorded behavior; what this table pins is that each operation
/// reports ITS OWN tier and reports it for every request it serves.
#[tokio::test]
async fn each_operation_is_charged_in_its_own_pricing_tier() {
    let router = routes_service();
    let core = service_com_amazonaws_georoutes_routesservice::state::PRICING_BUCKET_CORE;
    let advanced = service_com_amazonaws_georoutes_routesservice::state::PRICING_BUCKET_ADVANCED;
    let near = [-123.1, 49.28];
    let far = [-123.12, 49.26];
    let trace = json!([
        { "Position": [-123.100_25, 49.280_25] },
        { "Position": [-123.100_75, 49.280_75] }
    ]);

    // Rows: every implemented operation, twice, with requests differing in what the
    // response varies over (travel mode, waypoints, matrix size, radius, geometry format).
    let cases: [(&str, &str, &str, Value); 10] = [
        ("a direct route", ROUTES, core, json!({ "Origin": near, "Destination": far })),
        (
            "a walking route through a waypoint",
            ROUTES,
            core,
            json!({
                "Origin": near,
                "Destination": far,
                "Waypoints": [{ "Position": [-123.11, 49.27] }],
                "TravelMode": "Pedestrian",
            }),
        ),
        (
            "a 1x1 matrix",
            MATRIX,
            core,
            json!({ "Origins": [{ "Position": near }], "Destinations": [{ "Position": far }] }),
        ),
        (
            "a 2x1 truck matrix",
            MATRIX,
            core,
            json!({
                "Origins": [{ "Position": near }, { "Position": [-123.2, 49.3] }],
                "Destinations": [{ "Position": far }],
                "TravelMode": "Truck",
            }),
        ),
        (
            "a snap at the default radius",
            SNAP,
            advanced,
            json!({ "TracePoints": trace, "SnapRadius": 0 }),
        ),
        (
            "a snap at 10 km, as a line string",
            SNAP,
            advanced,
            json!({ "TracePoints": trace, "SnapRadius": 10_000, "SnappedGeometryFormat": "Simple" }),
        ),
        (
            "an optimization with no waypoints",
            OPTIMIZE,
            advanced,
            json!({ "Origin": near, "Destination": far }),
        ),
        (
            "an optimization over two waypoints, shortest first",
            OPTIMIZE,
            advanced,
            json!({
                "Origin": near,
                "Waypoints": [
                    { "Id": "alpha", "Position": [-123.11, 49.27] },
                    { "Id": "bravo", "Position": [-123.05, 49.25] },
                ],
                "OptimizeSequencingFor": "ShortestRoute",
            }),
        ),
        (
            "an isoline bounded by a distance",
            ISOLINES,
            advanced,
            json!({
                "Origin": near,
                "Thresholds": { "Distance": [2_000] },
                "IsolineGranularity": { "MaxPoints": 31 },
            }),
        ),
        (
            "isolines bounded by two walking times, towards a destination",
            ISOLINES,
            advanced,
            json!({
                "Destination": far,
                "Thresholds": { "Time": [300, 600] },
                "TravelMode": "Pedestrian",
                "IsolineGranularity": { "MaxPoints": 31 },
            }),
        ),
    ];

    assert_ne!(
        core, advanced,
        "the two tiers of this service must be distinct tokens, got {core:?} twice"
    );
    for (case, path, expected, body) in cases {
        let (status, answer, bucket) = post(&router, path, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            bucket.as_deref(),
            Some(expected),
            "case {case:?}: expected the {expected:?} pricing tier, got {bucket:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Positions: the caller's, and the network's
// ---------------------------------------------------------------------------

/// A position READ OFF the road network is reported at the network's coordinate precision
/// and lies on a road of it; a position the CALLER supplied is echoed exactly as it was
/// submitted, however many digits it carried.
///
/// A matched position is the submitted one moved onto the network, so it "may differ from
/// the requested" position — and does differ whenever the submitted position lay on no road
/// of the network, by no more than the farthest a position inside the network's coverage can
/// be from the nearest road.
///
/// Rows span the precision of the submitted coordinates (4 to 7 decimals, and coordinates
/// already on a road line), both hemispheres and both signs of longitude, and the three
/// operations that report a matched position for a position the request supplied.
#[tokio::test]
async fn a_position_of_the_network_is_reported_at_the_networks_precision() {
    let router = routes_service();
    // name, the two positions submitted.
    let cases: [(&str, [f64; 2], [f64; 2]); 5] = [
        ("four decimals, off the roads", [-123.1003, 49.2803], [-123.1203, 49.2603]),
        (
            "seven decimals, off the roads",
            [-123.100_180_7, 49.280_180_7],
            [-123.120_220_3, 49.260_220_3],
        ),
        ("on a road line in both coordinates", [-123.1, 49.28], [-123.12, 49.26]),
        (
            "the southern hemisphere, six decimals",
            [151.210_183, -33.870_183],
            [151.200_227, -33.860_227],
        ),
        (
            "east of the prime meridian, seven decimals",
            [2.294_481_3, 48.858_370_1],
            [2.300_000_7, 48.860_000_3],
        ),
    ];

    for (case, first, second) in cases {
        let (status, snapped, _) = post(
            &router,
            SNAP,
            &json!({
                "TracePoints": [{ "Position": first }, { "Position": second }],
                "SnapRadius": 10_000,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {snapped:?}");
        let points = snapped["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: SnappedTracePoints is required, got {snapped:?}"))
            .clone();
        for (index, submitted) in [first, second].into_iter().enumerate() {
            let case = &format!("{case} / trace point {index}");
            let point = &points[index];
            assert_eq!(
                position(&point["OriginalPosition"], case),
                submitted,
                "case {case:?}: OriginalPosition is the submitted position, verbatim, got {point:?}"
            );
            let matched = position(&point["SnappedPosition"], case);
            assert!(
                matched.iter().copied().all(at_network_precision),
                "case {case:?}: a matched position carries the network's precision, got {matched:?}"
            );
            assert!(
                matched.iter().copied().any(on_a_road_line),
                "case {case:?}: a matched position lies on a road of the network, got {matched:?}"
            );
        }

        let (status, route, _) = post(
            &router,
            ROUTES,
            &json!({ "Origin": first, "Destination": second }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {route:?}");
        let details = &route["Routes"][0]["Legs"][0]["VehicleLegDetails"];
        for (which, submitted) in [("Departure", first), ("Arrival", second)] {
            let case = &format!("{case} / {which}");
            let place = &details[which]["Place"];
            assert_eq!(
                position(&place["OriginalPosition"], case),
                submitted,
                "case {case:?}: OriginalPosition is the submitted position, verbatim, got {place:?}"
            );
            let reported = position(&place["Position"], case);
            assert!(
                reported.iter().copied().all(at_network_precision),
                "case {case:?}: Position is read off the network, so it carries the network's \
                 precision, got {reported:?}"
            );
            assert!(
                reported.iter().copied().any(on_a_road_line),
                "case {case:?}: Position lies on a road of the network, got {reported:?}"
            );
        }

        // An isoline is grown from the anchor matched onto the network, and reports that
        // matched point for whichever of `Origin` and `Destination` the request set: "the
        // actual point on the road network used for calculations, which may differ from the
        // requested origin if Origin was not directly on a road".
        for (member, reported_member, submitted) in [
            ("Origin", "SnappedOrigin", first),
            ("Destination", "SnappedDestination", second),
        ] {
            let case = &format!("{case} / an isoline from {member}");
            let (status, answer, _) = post(
                &router,
                ISOLINES,
                &json!({
                    member: submitted,
                    "Thresholds": { "Distance": [1_000] },
                    "IsolineGranularity": { "MaxPoints": 31 },
                }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
            let matched = position(&answer[reported_member], case);
            assert!(
                matched.iter().copied().all(at_network_precision),
                "case {case:?}: {reported_member} is read off the network, so it carries the \
                 network's precision, got {matched:?}"
            );
            assert!(
                matched.iter().copied().any(on_a_road_line),
                "case {case:?}: {reported_member} lies on a road of the network, got {matched:?}"
            );
            let offset_m = great_circle_m(submitted, matched);
            assert!(
                offset_m <= MAX_MATCH_OFFSET_M,
                "case {case:?}: the anchor is matched onto a road near it, so the match moved it \
                 at most {MAX_MATCH_OFFSET_M} m, got {offset_m} m to {matched:?}"
            );
            if submitted.iter().copied().any(on_a_road_line) {
                assert_eq!(
                    matched, submitted,
                    "case {case:?}: the submitted position is already on a road, so the point \
                     used for calculations is that position, got {matched:?}"
                );
            } else {
                assert_ne!(
                    matched, submitted,
                    "case {case:?}: the submitted position lies on no road, so the point used for \
                     calculations differs from it, got {matched:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The confidence of a match
// ---------------------------------------------------------------------------

/// One `Confidence` of a snapped trace point, as the service reports it.
async fn confidence_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    point: [f64; 2],
    radius: i64,
) -> f64 {
    let (status, answer, _) = post(
        router,
        SNAP,
        &json!({
            // A trace is two points at least; the second is far enough away not to be the
            // one under test.
            "TracePoints": [{ "Position": point }, { "Position": [point[0] + 0.004, point[1] + 0.004] }],
            "SnapRadius": radius,
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
    answer["SnappedTracePoints"][0]["Confidence"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: Confidence is required, got {answer:?}"))
}

/// `Confidence` ("Confidence value for the correctness of this point match", `@range 0..1`)
/// is scored on the MATCH: a point nearer its road is matched more certainly than one
/// further off, and a point lying as near a second road the traveller may use could be on
/// either of them, so that match is less certain than one with no other road near. The
/// radius the request allowed says which points may be matched at all, so it cannot move
/// the score of a match that was made.
///
/// Rows span the offset from the matched road, at every radius the service accepts; the
/// comparison below spans the other thing the score is read from, how near the next
/// candidate road is.
#[tokio::test]
async fn a_match_is_scored_on_the_match_and_not_on_the_requested_radius() {
    let router = routes_service();
    // Longitude -123.095 is an odd multiple of ten grid steps, where the network puts a
    // footpath no car may travel, so the nearest road a car can be matched to in that axis
    // is a grid step and a half away and these rows differ only in the LATITUDE offset from
    // the parallel road at 49.28. The offsets run from none to a quarter of the grid
    // spacing, which is as far as a point on this network can be from the road nearest it.
    let on_a_road = [-123.095, 49.28];
    let cases: [(&str, f64); 4] = [
        ("on the road", 0.0),
        ("a tenth of the spacing off the road", 0.000_05),
        ("a fifth of the spacing off the road", 0.000_1),
        ("as far off the road as a point can be", 0.000_2),
    ];

    let mut by_offset: Vec<(&str, f64)> = Vec::new();
    for (case, offset_deg) in cases {
        let point = [on_a_road[0], on_a_road[1] + offset_deg];
        let mut radii: Vec<(i64, f64)> = Vec::new();
        for radius in [50_i64, 300, 1_000, 10_000] {
            let confidence = confidence_of(&router, case, point, radius).await;
            assert!(
                (0.0..=1.0).contains(&confidence),
                "case {case:?} / radius {radius}: Confidence must be in 0..1, got {confidence}"
            );
            assert!(
                confidence > 0.0,
                "case {case:?} / radius {radius}: a matched point carries some confidence, got {confidence}"
            );
            radii.push((radius, confidence));
        }
        for (radius, confidence) in &radii[1..] {
            assert_eq!(
                *confidence, radii[0].1,
                "case {case:?}: the same match must score the same at {radius} m as at {} m, \
                 got {confidence} against {}",
                radii[0].0, radii[0].1
            );
        }
        by_offset.push((case, radii[0].1));
    }

    // The further the point had to move to reach its road, the less certain the match.
    for pair in by_offset.windows(2) {
        let ((nearer, higher), (further, lower)) = (pair[0], pair[1]);
        assert!(
            higher > lower,
            "case {further:?}: a point further from its road than {nearer:?} is matched less \
             certainly, got {lower} against {higher}"
        );
    }

    // Longitude -123.0945 carries a meridian street, so a point a metre east of it sits at
    // a crossing of two roads a car may travel and could be on either: the same offset
    // from its road as the first row above, but an ambiguous match.
    let at_a_crossing = confidence_of(&router, "at a crossing", [-123.094_49, 49.28], 300).await;
    assert!(
        at_a_crossing < by_offset[0].1,
        "a point at a crossing of two roads is matched less certainly than one with no other \
         road near it, got {at_a_crossing} against {}",
        by_offset[0].1
    );
    assert!(
        at_a_crossing > 0.0,
        "a point at a crossing is still matched, so it carries some confidence, got {at_a_crossing}"
    );
}

// ---------------------------------------------------------------------------
// The snapped geometry
// ---------------------------------------------------------------------------

/// `SnappedGeometry` is the geometry of the ROADS the trace was matched onto: every vertex
/// of it lies on a road of the network and every segment of it follows one road, so a
/// client drawing the line stays on the network instead of cutting across the blocks
/// between the matched points.
///
/// Rows span the ways two consecutive matched points can sit on the network — the same
/// road, two crossing roads, two roads that never meet — and the length of the trace.
#[tokio::test]
async fn the_snapped_geometry_runs_along_the_roads_the_trace_matched() {
    let router = routes_service();
    // name, the trace, whether consecutive matched points lie on different roads (so the
    // line has to turn, and carries more vertices than there are matched points).
    let cases: [(&str, Vec<[f64; 2]>, bool); 5] = [
        (
            // Both on the parallel road at 49.28, 110 m apart along it.
            "two points on one road",
            vec![[-123.1, 49.28], [-123.0985, 49.28]],
            false,
        ),
        (
            // One on a parallel road, one on a meridian road: they cross.
            "two points on crossing roads",
            vec![[-123.1, 49.2803], [-123.1015, 49.281]],
            true,
        ),
        (
            // Both on meridian roads of different longitude: they never meet.
            "two points on roads that never meet",
            vec![[-123.1, 49.2803], [-123.102, 49.2817]],
            true,
        ),
        (
            "a five-point trace across the grid",
            vec![
                [-123.100_18, 49.280_18],
                [-123.100_92, 49.280_93],
                [-123.101_67, 49.281_71],
                [-123.102_44, 49.282_39],
                [-123.103_11, 49.283_05],
            ],
            true,
        ),
        (
            "a trace in the southern hemisphere",
            vec![[151.210_18, -33.870_18], [151.211_12, -33.869_21], [151.212_07, -33.868_33]],
            true,
        ),
    ];

    for (case, trace, turns) in cases {
        let body = json!({
            "TracePoints": trace.iter().map(|position| json!({ "Position": position })).collect::<Vec<Value>>(),
            "SnapRadius": 300,
            "SnappedGeometryFormat": "Simple",
        });
        let (status, answer, _) = post(&router, SNAP, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let matched: Vec<[f64; 2]> = answer["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: SnappedTracePoints is required, got {answer:?}"))
            .iter()
            .map(|point| position(&point["SnappedPosition"], case))
            .collect();
        let line: Vec<[f64; 2]> = answer["SnappedGeometry"]["LineString"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the snapped line string is required, got {answer:?}"))
            .iter()
            .map(|vertex| position(vertex, case))
            .collect();

        assert_eq!(
            line.first().copied(),
            matched.first().copied(),
            "case {case:?}: the line starts at the first matched point, got {line:?}"
        );
        assert_eq!(
            line.last().copied(),
            matched.last().copied(),
            "case {case:?}: the line ends at the last matched point, got {line:?}"
        );
        for vertex in &line {
            assert!(
                vertex.iter().copied().any(on_a_road_line),
                "case {case:?}: every vertex of the line is on a road of the network, got {vertex:?}"
            );
        }
        // One road of this network runs at a constant longitude or at a constant latitude,
        // so a segment that follows one road moves in exactly one coordinate.
        for pair in line.windows(2) {
            let moved: Vec<usize> = (0..2)
                .filter(|axis| (pair[0][*axis] - pair[1][*axis]).abs() > 1e-9)
                .collect();
            assert_eq!(
                moved.len(), 1,
                "case {case:?}: every segment of the line follows one road, so it moves in one \
                 coordinate only, got {pair:?}"
            );
        }
        if turns {
            assert!(
                line.len() > matched.len(),
                "case {case:?}: the matched points are on different roads, so the line turns \
                 between them, got {} vertices for {} matched points",
                line.len(),
                matched.len()
            );
        }
        // Following the roads is never shorter than the straight line between the points.
        let along_roads: f64 = line.windows(2).map(|pair| great_circle_m(pair[0], pair[1])).sum();
        let straight: f64 = matched.windows(2).map(|pair| great_circle_m(pair[0], pair[1])).sum();
        assert!(
            along_roads >= straight - 0.5,
            "case {case:?}: the line along the roads ({along_roads:.1} m) cannot be shorter than \
             the chords between the matched points ({straight:.1} m)"
        );
    }
}

// ---------------------------------------------------------------------------
// The optimized order
// ---------------------------------------------------------------------------

/// Every order of `stops`, as lists of indexes into it.
fn permutations(stops: &[usize]) -> Vec<Vec<usize>> {
    if stops.len() <= 1 {
        return vec![stops.to_vec()];
    }
    let mut orders = Vec::new();
    for (index, stop) in stops.iter().enumerate() {
        let mut rest: Vec<usize> = stops.to_vec();
        rest.remove(index);
        for mut order in permutations(&rest) {
            order.insert(0, *stop);
            orders.push(order);
        }
    }
    orders
}

/// `OptimizeWaypoints` "calculates the optimal order to travel between a set of waypoints
/// to minimize either the travel time or the distance travelled during the journey", so the
/// order it answers with is the one whose WHOLE journey — origin, every waypoint, and the
/// destination where the request names one — costs least in what the objective minimizes.
/// Nothing cheaper exists, which this test checks against the service's own cost table:
/// `CalculateRouteMatrix` over the same stops, which reports the cost of every hop.
///
/// Rows span the number of waypoints, both objectives, whether the journey ends at a
/// destination (the case where the cheapest first hop is the wrong one), and the travel
/// mode.
#[tokio::test]
async fn the_optimized_order_is_the_cheapest_order_of_the_whole_journey() {
    let router = routes_service();
    // name, origin, waypoints, destination, travel mode, objective.
    type OrderCase = (
        &'static str,
        [f64; 2],
        Vec<[f64; 2]>,
        Option<[f64; 2]>,
        &'static str,
        &'static str,
    );
    let cases: [OrderCase; 5] = [
        (
            "three waypoints, fastest, open-ended",
            [-123.1, 49.28],
            vec![[-123.112, 49.272], [-123.085, 49.262], [-123.104, 49.291]],
            None,
            "Car",
            "FastestRoute",
        ),
        (
            // The nearest waypoint lies the wrong way for the journey: taking it first is
            // the cheapest hop and leaves the dearest remainder.
            "two waypoints and a destination, fastest",
            [-123.1, 49.28],
            vec![[-123.091, 49.28], [-123.111, 49.28]],
            Some([-123.03, 49.28]),
            "Car",
            "FastestRoute",
        ),
        (
            "four waypoints, shortest, open-ended",
            [-123.1, 49.28],
            vec![
                [-123.112, 49.272],
                [-123.085, 49.262],
                [-123.104, 49.291],
                [-123.093, 49.286],
            ],
            None,
            "Car",
            "ShortestRoute",
        ),
        (
            "four waypoints and a destination, shortest, by truck",
            [-123.1, 49.28],
            vec![
                [-123.112, 49.272],
                [-123.085, 49.262],
                [-123.104, 49.291],
                [-123.093, 49.286],
            ],
            Some([-123.07, 49.3]),
            "Truck",
            "ShortestRoute",
        ),
        (
            "three waypoints and a destination in the southern hemisphere, fastest",
            [151.21, -33.87],
            vec![[151.222, -33.862], [151.196, -33.879], [151.204, -33.855]],
            Some([151.24, -33.85]),
            "Scooter",
            "FastestRoute",
        ),
    ];

    for (case, origin, waypoints, destination, travel_mode, objective) in cases {
        // Stop 0 is the origin, 1..=n the waypoints, and the last the destination where
        // there is one: the indexing the cost table below is read with.
        let stops: Vec<[f64; 2]> = std::iter::once(origin)
            .chain(waypoints.iter().copied())
            .chain(destination)
            .collect();
        let endpoints: Vec<Value> = stops.iter().map(|position| json!({ "Position": position })).collect();
        let (status, matrix, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": endpoints,
                "Destinations": endpoints,
                "TravelMode": travel_mode,
                "OptimizeRoutingFor": objective,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200 from the matrix, got {status} with {matrix:?}");
        // What this objective minimizes, hop by hop.
        let member = if objective == "ShortestRoute" { "Distance" } else { "Duration" };
        let cost = |from: usize, to: usize| -> i64 {
            let entry = &matrix["RouteMatrix"][from][to];
            assert!(
                entry["Error"].is_null(),
                "case {case:?}: the hop {from} -> {to} must be routable, got {entry:?}"
            );
            int(entry, member, case)
        };

        let mut body = json!({
            "Origin": origin,
            "OriginOptions": { "Id": "origin" },
            "Waypoints": waypoints
                .iter()
                .enumerate()
                .map(|(index, position)| json!({ "Id": format!("w{index}"), "Position": position }))
                .collect::<Vec<Value>>(),
            "TravelMode": travel_mode,
            "OptimizeSequencingFor": objective,
        });
        if let Some(destination) = destination {
            body["Destination"] = json!(destination);
            body["DestinationOptions"] = json!({ "Id": "destination" });
        }
        let (status, answer, _) = post(&router, OPTIMIZE, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        // The cheapest journey any order of the waypoints makes, by the same cost table.
        let last = stops.len() - 1;
        let total_of = |order: &[usize]| -> i64 {
            let mut total = 0;
            let mut from = 0;
            for &stop in order {
                total += cost(from, stop);
                from = stop;
            }
            if destination.is_some() {
                total += cost(from, last);
            }
            total
        };
        let orders = permutations(&(1..=waypoints.len()).collect::<Vec<usize>>());
        let cheapest = orders
            .iter()
            .map(|order| total_of(order))
            .min()
            .unwrap_or_else(|| panic!("case {case:?}: there is always at least one order"));

        // The order the service answered with, as stop indexes.
        let visited: Vec<String> = answer["OptimizedWaypoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: OptimizedWaypoints is required, got {answer:?}"))
            .iter()
            .map(|waypoint| waypoint["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        let answered: Vec<usize> = visited
            .iter()
            .filter_map(|id| id.strip_prefix('w'))
            .filter_map(|index| index.parse::<usize>().ok())
            .map(|index| index + 1)
            .collect();
        assert_eq!(
            answered.len(),
            waypoints.len(),
            "case {case:?}: every waypoint must be sequenced once, got {visited:?}"
        );
        assert_eq!(
            total_of(&answered),
            cheapest,
            "case {case:?}: the answered order {visited:?} costs {} where the cheapest order of \
             the journey costs {cheapest}",
            total_of(&answered)
        );

        // And the totals the answer reports are that same journey: the objective's total
        // over the connections it carries.
        let reported = if objective == "ShortestRoute" {
            int(&answer, "Distance", case)
        } else {
            int(&answer["TimeBreakdown"], "TravelDuration", case)
        };
        assert_eq!(
            reported, cheapest,
            "case {case:?}: the reported {member} of the journey must be the cheapest order's \
             {cheapest}, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// How a road is identified
// ---------------------------------------------------------------------------

/// Every text this service reports a road by is a designation a client can render or match
/// on: the road's number and the side of the origin it runs on. The grid index the network
/// identifies a road by internally is signed, and a negative numeral is not part of any
/// road's name, so no reported text carries one.
///
/// Rows span the four quadrants of the world — both signs of longitude and of latitude —
/// because the sign of the index is exactly what used to reach the text, and the travel
/// modes that decide which classes of road are travelled.
#[tokio::test]
async fn a_road_is_reported_by_a_designation_and_never_by_a_signed_index() {
    let router = routes_service();
    // name, origin, destination, travel mode.
    let cases: [(&str, [f64; 2], [f64; 2], &str); 5] = [
        ("west and north of the origin", [-123.1, 49.28], [-123.08, 49.29], "Car"),
        ("west and south of the origin", [-70.65, -33.45], [-70.63, -33.44], "Car"),
        ("east and north of the origin", [2.294, 48.858], [2.314, 48.868], "Truck"),
        ("east and south of the origin", [151.21, -33.87], [151.23, -33.86], "Scooter"),
        ("on foot, west and north", [-123.1, 49.28], [-123.097, 49.283], "Pedestrian"),
    ];

    for (case, origin, destination, travel_mode) in cases {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": origin,
                "Destination": destination,
                "TravelMode": travel_mode,
                "TravelStepType": "TurnByTurn",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let mut texts: Vec<String> = Vec::new();
        for label in answer["Routes"][0]["MajorRoadLabels"].as_array().unwrap_or(&Vec::new()) {
            for member in ["RoadName", "RouteNumber"] {
                if let Some(text) = label[member]["Value"].as_str() {
                    texts.push(text.to_string());
                }
            }
        }
        let details = &answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["TravelSteps"];
        let steps = details
            .as_array()
            .cloned()
            .or_else(|| {
                answer["Routes"][0]["Legs"][0]["PedestrianLegDetails"]["TravelSteps"]
                    .as_array()
                    .cloned()
            })
            .unwrap_or_default();
        for step in &steps {
            for road in ["CurrentRoad", "NextRoad"] {
                for name in step[road]["RoadName"].as_array().unwrap_or(&Vec::new()) {
                    if let Some(text) = name["Value"].as_str() {
                        texts.push(text.to_string());
                    }
                }
            }
        }

        assert!(
            !texts.is_empty(),
            "case {case:?}: a route over the grid travels named roads, got {answer:?}"
        );
        for text in &texts {
            assert!(
                !text.contains('-'),
                "case {case:?}: a road designation carries no negative numeral, got {text:?}"
            );
            assert!(
                text.ends_with(['N', 'S', 'E', 'W']),
                "case {case:?}: a road designation names the side of the origin it runs on, got {text:?}"
            );
        }
    }
}
