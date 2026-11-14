// SPDX-License-Identifier: CC0-1.0

//! What a route metric of this service IS: the length of a path the road network offers,
//! and the time travelling that path takes at the speeds of the roads it runs on.
//!
//! `RouteMatrixEntry.Distance`/`Duration` and `RouteSummary.Distance`/`Duration` are "the
//! total distance of travel for the route" and "the expected duration of travel"
//! (`smithy_query(query="shape",shape="RouteMatrixEntry")`); the model fixes no routing
//! algorithm, so what these suites pin is the rule a road answer follows, over a class of
//! requests rather than one recorded pair:
//!
//! * a route travels ALONG roads, and the roads of this network run north-south or
//!   east-west, so no route is shorter than the rectilinear journey between its endpoints
//!   — strictly longer than the straight line for any pair that is not axis-aligned;
//! * a route's duration is the sum of its stretches at the posted speed of the road each
//!   one runs on, held down to the travel mode's own top speed — so the speed a journey
//!   implies is a speed of the network's roads, it is higher on the roads that are posted
//!   higher, and a journey long enough to be worth reaching the fast classes for implies
//!   more of a speed than a short one that cannot.
//!
//! The tables span what the rule generalises over: separation (hundreds of metres to
//! hundreds of kilometres), bearing (east-west, north-south, diagonal), hemisphere, the
//! class of road travelled, the travel mode and the routing objective.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, post, routes_service, MATRIX, ROUTES};

/// The grid's road spacing in degrees, as `src/state.rs` holds it (`ROAD_SPACING_DEG`): a
/// submitted position is matched onto a road at most half of it away in each axis, which is
/// the only part of a reported distance that is not travel along the roads.
const ROAD_SPACING_DEG: f64 = 0.000_5;

/// Posted speeds of the classes of road the network carries, in km/h, as `src/state.rs`
/// holds them (`MOTORWAY_SPEED_KPH`, `TRUNK_SPEED_KPH`, `STREET_SPEED_KPH`,
/// `FOOTPATH_SPEED_KPH`).
const MOTORWAY_SPEED_KPH: f64 = 110.0;
const TRUNK_SPEED_KPH: f64 = 85.0;
const STREET_SPEED_KPH: f64 = 50.0;
const FOOTPATH_SPEED_KPH: f64 = 5.0;

/// Metres per second in one km/h.
const KPH_TO_MPS: f64 = 1_000.0 / 3_600.0;

/// The fastest and the slowest road `mode` may travel, in metres per second, held down to
/// the mode's own top speed (`ComAmazonawsGeoroutesRoutesserviceState::travel_mode_speeds`
/// in `src/state.rs`): a pedestrian walks at the footpath pace on any road, a scooter and
/// an intermodal journey are held to 45 km/h, a truck to 90 and a transit vehicle to 80,
/// and a car keeps up with any road of the network.
fn speed_bounds_mps(mode: &str) -> (f64, f64) {
    let top_kph = match mode {
        "Pedestrian" => FOOTPATH_SPEED_KPH,
        "Scooter" | "Intermodal" => 45.0,
        "Transit" => 80.0,
        "Truck" => 90.0,
        _ => MOTORWAY_SPEED_KPH,
    };
    // A pedestrian may use the streets and the footpaths, every other mode the streets and
    // the numbered roads; whichever it is, no stretch is travelled slower than the slowest
    // road the mode may use or faster than the fastest.
    let slowest_kph = if mode == "Pedestrian" {
        FOOTPATH_SPEED_KPH
    } else {
        STREET_SPEED_KPH
    };
    (
        slowest_kph.min(top_kph) * KPH_TO_MPS,
        MOTORWAY_SPEED_KPH.min(top_kph) * KPH_TO_MPS,
    )
}

/// The rectilinear journey between two positions, in metres: the north-south leg and the
/// east-west leg a traveller on a grid of meridian and parallel roads has to cover. It is
/// measured at the latitude where the east-west leg is SHORTEST, so it is a lower bound on
/// any staircase between the two positions however the route splits its legs up.
fn rectilinear_m(from: [f64; 2], to: [f64; 2]) -> f64 {
    let north_south = great_circle_m([from[0], from[1]], [from[0], to[1]]);
    let east_west = great_circle_m([from[0], from[1]], [to[0], from[1]])
        .min(great_circle_m([from[0], to[1]], [to[0], to[1]]));
    north_south + east_west
}

/// One pair per row, differing in separation, bearing and hemisphere. Every position is in
/// mapped territory and every pair is connected by road, so each is routed.
const PAIRS: [(&str, [f64; 2], [f64; 2]); 7] = [
    ("a few hundred metres east", [-123.1017, 49.2823], [-123.0963, 49.2823]),
    ("a kilometre north", [-123.1017, 49.2823], [-123.1017, 49.2913]),
    ("a few kilometres diagonally", [-123.1017, 49.2823], [-123.1431, 49.2487]),
    ("tens of kilometres diagonally", [-122.3321, 47.6062], [-122.1037, 47.3129]),
    ("hundreds of kilometres north to south", [-122.3321, 47.6062], [-122.6784, 45.5152]),
    ("hundreds of kilometres west to east", [-122.3321, 47.6062], [-119.8743, 47.4221]),
    ("a diagonal pair in the southern hemisphere", [151.2093, -33.8688], [150.8931, -34.0522]),
];

/// No cell of a matrix is shorter than the rectilinear journey between its pair, and the
/// speed it implies is a speed of the roads the traveller is allowed on: the two members
/// are one traversal of the graph, not two numbers.
#[tokio::test]
async fn every_cell_is_a_path_of_the_road_graph_and_the_time_it_takes_on_it() {
    let router = routes_service();
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];
    // The matching hops at the two ends are the only part of a reported distance that is
    // not travel along a road, and a match moves a position at most half the grid spacing
    // in each axis.
    let slack_m = 4.0 * ROAD_SPACING_DEG * 111_320.0;

    for mode in modes {
        for objective in ["FastestRoute", "ShortestRoute"] {
            for (pair, origin, destination) in PAIRS {
                let case = format!("{mode}, {objective}, {pair}");
                let cell = cell(&router, origin, destination, mode, objective, &case).await;
                let distance = int(&cell, "Distance", &case);
                let duration = int(&cell, "Duration", &case);
                let rectilinear = rectilinear_m(origin, destination);
                assert!(
                    distance as f64 + slack_m >= rectilinear,
                    "case {case:?}: the roads run north-south and east-west, so the route covers \
                     the {rectilinear:.0} m rectilinear journey at least, got {distance}"
                );
                assert!(
                    duration > 0,
                    "case {case:?}: travelling {distance} m takes time, got {duration}"
                );
                let implied = distance as f64 / duration as f64;
                let (slowest, fastest) = speed_bounds_mps(mode);
                assert!(
                    implied >= slowest - 0.5 && implied <= fastest + 0.5,
                    "case {case:?}: {distance} m in {duration} s implies {implied:.2} m/s, outside \
                     the {slowest:.2}..{fastest:.2} m/s the roads this mode may travel are posted at"
                );
            }
        }
    }
}

/// The hierarchy reaches the answer: for a mode that can go faster than a local street, a
/// journey long enough to be worth joining the fast classes for implies more of a speed
/// than a short one that spends its length on the local roads. For a mode held slower than
/// every road of the network, the mode's own top speed is what every journey implies,
/// however long it is.
#[tokio::test]
async fn a_journey_worth_taking_the_fast_roads_implies_more_of_a_speed_than_a_local_one() {
    let router = routes_service();
    let street = STREET_SPEED_KPH * KPH_TO_MPS;

    for mode in ["Car", "Truck", "Scooter", "Pedestrian"] {
        let (_, top) = speed_bounds_mps(mode);
        let mut speeds: Vec<(&str, f64)> = Vec::new();
        for (pair, origin, destination) in [PAIRS[0], PAIRS[3], PAIRS[4]] {
            let case = format!("{mode}, {pair}");
            let cell = cell(&router, origin, destination, mode, "FastestRoute", &case).await;
            let (distance, duration) = (int(&cell, "Distance", &case), int(&cell, "Duration", &case));
            speeds.push((pair, distance as f64 / duration as f64));
        }
        if top <= street {
            // Every road of the network is posted above this mode's own top speed, so the
            // mode is what the journey is paced by, whatever roads it travels.
            for (pair, speed) in &speeds {
                let case = format!("{mode}, {pair}");
                assert!(
                    (speed - top).abs() <= 0.2,
                    "case {case:?}: every road is posted above this mode's {top:.2} m/s, so that \
                     is the pace of any journey it makes, got {speed:.2} m/s"
                );
            }
            continue;
        }
        let (local_pair, local) = speeds[0];
        for (pair, speed) in &speeds[1..] {
            assert!(
                *speed > local,
                "case {mode:?}: {pair:?} travels the fast classes, so it must imply more of a \
                 speed than {local_pair:?}, got {speed:.2} m/s against {local:.2} m/s"
            );
        }
        // And a journey of hundreds of kilometres is not priced at a local-street pace,
        // which is the lowest speed any road this mode may use is posted at.
        let (long_pair, long) = speeds[speeds.len() - 1];
        assert!(
            long > street,
            "case {mode:?}: {long_pair:?} runs on the fast classes, so it must imply more than \
             the {street:.2} m/s a street is posted at, got {long:.2} m/s"
        );
    }
}

/// The POSTED SPEED of the road travelled is what turns a distance into a duration: three
/// runs of the same length along a motorway, a trunk road and a street take times in the
/// order of those three speeds, and the same run on foot takes the footpath pace.
///
/// Each row is a run straight along one parallel road of the grid, taken as the shortest
/// route so that the path is that road: latitude 49.4 is a multiple of 400 grid steps,
/// where the network puts a motorway, 49.45 is a multiple of 100, where it puts a trunk
/// road, and 49.4505 is neither, so it carries a street (`class_of` in `src/state.rs`).
#[tokio::test]
async fn the_duration_of_a_run_is_its_length_at_the_posted_speed_of_the_road_it_runs_on() {
    let router = routes_service();
    // name, the latitude the run follows, the speed the road is posted at.
    let runs: [(&str, f64, f64); 3] = [
        ("along a motorway", 49.4, MOTORWAY_SPEED_KPH),
        ("along a trunk road", 49.45, TRUNK_SPEED_KPH),
        ("along a street", 49.4505, STREET_SPEED_KPH),
    ];
    // A longitude between two meridian roads, so the run's endpoints match onto the
    // parallel road they sit on rather than onto a meridian crossing it.
    let (west, east) = (-123.097_23, -123.067_23);

    let mut measured: Vec<(&str, i64, i64, f64)> = Vec::new();
    for (case, latitude, posted_kph) in runs {
        let cell = cell(
            &router,
            [west, latitude],
            [east, latitude],
            "Car",
            "ShortestRoute",
            case,
        )
        .await;
        let (distance, duration) = (int(&cell, "Distance", case), int(&cell, "Duration", case));
        let implied = distance as f64 / duration as f64;
        assert!(
            (implied - posted_kph * KPH_TO_MPS).abs() <= 1.0,
            "case {case:?}: a run along this road is travelled at the {posted_kph} km/h it is \
             posted at, got {distance} m in {duration} s, i.e. {implied:.2} m/s"
        );
        measured.push((case, distance, duration, implied));
    }

    // The three runs cover the same ground to within the scale of their own latitudes, so
    // what differs between them is the road.
    let shortest = measured.iter().map(|run| run.1).min().unwrap_or_default();
    let longest = measured.iter().map(|run| run.1).max().unwrap_or_default();
    assert!(
        (longest - shortest) <= 10,
        "the three runs must cover the same distance for the comparison to be about the road, \
         got {measured:?}"
    );
    for pair in measured.windows(2) {
        let ((quicker, _, quicker_duration, _), (slower, _, slower_duration, _)) = (pair[0], pair[1]);
        assert!(
            quicker_duration < slower_duration,
            "case {slower:?}: a road posted lower takes longer over the same ground than \
             {quicker:?}, got {slower_duration} s against {quicker_duration} s"
        );
    }

    // The travel mode holds the speed down where the mode is the slower of the two: on foot
    // the same run takes the footpath pace whatever the road is posted at.
    for (case, latitude, _) in runs {
        let case = &format!("{case} on foot");
        let cell = cell(
            &router,
            [west, latitude],
            [east, latitude],
            "Pedestrian",
            "ShortestRoute",
            case,
        )
        .await;
        let (distance, duration) = (int(&cell, "Distance", case), int(&cell, "Duration", case));
        let implied = distance as f64 / duration as f64;
        assert!(
            (implied - FOOTPATH_SPEED_KPH * KPH_TO_MPS).abs() <= 0.2,
            "case {case:?}: a walker covers the ground at walking pace whatever the road is \
             posted at, got {distance} m in {duration} s, i.e. {implied:.2} m/s"
        );
    }
}

/// One traversal serves every operation that reports a route metric: the matrix cell for a
/// pair and the direct route for that pair are the same path, so they report the same
/// distance and the same duration.
#[tokio::test]
async fn the_matrix_cell_and_the_direct_route_report_the_same_traversal() {
    let router = routes_service();

    for mode in ["Car", "Pedestrian"] {
        for (pair, origin, destination) in PAIRS {
            let case = format!("{mode}, {pair}");
            let cell = cell(&router, origin, destination, mode, "FastestRoute", &case).await;
            let (status, answer, _) = post(
                &router,
                ROUTES,
                &json!({
                    "Origin": origin,
                    "Destination": destination,
                    "TravelMode": mode,
                    "OptimizeRoutingFor": "FastestRoute",
                }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
            let summary = &answer["Routes"][0]["Summary"];
            assert_eq!(
                (int(summary, "Distance", &case), int(summary, "Duration", &case)),
                (int(&cell, "Distance", &case), int(&cell, "Duration", &case)),
                "case {case:?}: the cell and the direct route are one traversal, got {summary:?} \
                 against {cell:?}"
            );
        }
    }
}

/// The single cell of a 1x1 matrix for `origin` -> `destination`, which must be routed.
async fn cell(
    router: &smithy_server_runtime::MultiServiceRouter,
    origin: [f64; 2],
    destination: [f64; 2],
    travel_mode: &str,
    objective: &str,
    case: &str,
) -> Value {
    let (status, answer, _) = post(
        router,
        MATRIX,
        &json!({
            "Origins": [{ "Position": origin }],
            "Destinations": [{ "Position": destination }],
            "TravelMode": travel_mode,
            "OptimizeRoutingFor": objective,
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
    let cell = answer["RouteMatrix"][0][0].clone();
    assert!(
        cell["Error"].is_null(),
        "case {case:?}: the pair is on the road network, so it is routed, got {cell:?}"
    );
    cell
}
