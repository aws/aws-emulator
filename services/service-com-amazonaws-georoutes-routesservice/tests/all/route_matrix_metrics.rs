// SPDX-License-Identifier: CC0-1.0

//! The metrics a `CalculateRouteMatrix` cell carries, and how they hang together.
//!
//! `RouteMatrixEntry.Distance` (metres) and `RouteMatrixEntry.Duration` (seconds) are the
//! total travel distance and the expected travel duration of one origin/destination pair
//! (`smithy_query(query="shape",shape="RouteMatrixEntry")`). The model states no routing
//! algorithm, so what can be asserted is how the two hang together, and these are the
//! invariants: a routed cell is never shorter than the straight line between its pair, its
//! duration is that distance travelled at a speed of the roads the network carries, and one
//! cost function serves the whole matrix — so a pair costs the same alone as it does inside
//! a row, and a slower traveller never implies a faster one over the same pair.
//!
//! The tables span what the rule generalises over: separation (a few hundred metres to a
//! few hundred kilometres), bearing (east-west, north-south, diagonal), travel mode and
//! routing objective.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, post, routes_service, MATRIX};

/// Posted speeds of the classes of road this service's network carries, in km/h, as
/// `src/state.rs` holds them (`FOOTPATH_SPEED_KPH`, `STREET_SPEED_KPH`,
/// `TRUNK_SPEED_KPH`, `MOTORWAY_SPEED_KPH`): nothing travels faster than the fastest road
/// it is allowed on, and a pedestrian walks at the footpath pace.
const FOOTPATH_SPEED_KPH: f64 = 5.0;
const MOTORWAY_SPEED_KPH: f64 = 110.0;

/// The fastest road `mode` may travel on, in metres per second: a pedestrian is on foot,
/// every other mode is a vehicle and the network's motorways are the fastest roads it
/// carries.
fn top_road_speed_mps(mode: &str) -> f64 {
    let kph = if mode == "Pedestrian" {
        FOOTPATH_SPEED_KPH
    } else {
        MOTORWAY_SPEED_KPH
    };
    kph * 1_000.0 / 3_600.0
}

/// One pair per row, differing in separation and bearing. Every position is in mapped
/// territory and every pair is connected by the road network, so each cell is routed.
const PAIRS: [(&str, [f64; 2], [f64; 2]); 5] = [
    ("a few hundred metres east", [-123.1, 49.28], [-123.095, 49.28]),
    ("a kilometre north", [-123.1, 49.28], [-123.1, 49.289]),
    ("a few kilometres diagonally", [-123.1, 49.28], [-123.2, 49.2]),
    ("tens of kilometres diagonally", [-122.3321, 47.6062], [-122.1, 47.3]),
    ("hundreds of kilometres", [-122.3321, 47.6062], [-122.6784, 45.5152]),
];

/// Every routed cell covers at least the straight line between its pair and takes that
/// distance at a speed of the road network: the two members are one cost, not two
/// independent numbers.
#[tokio::test]
async fn every_routed_cell_is_a_distance_and_the_time_it_takes_on_the_road_network() {
    let router = routes_service();
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];

    for mode in modes {
        for (pair, origin, destination) in PAIRS {
            let case = format!("{mode}, {pair}");
            let cell = cell(&router, origin, destination, mode, "FastestRoute", &case).await;
            let distance = int(&cell, "Distance", &case);
            let duration = int(&cell, "Duration", &case);
            let straight = great_circle_m(origin, destination);
            assert!(
                distance as f64 + 1.0 >= straight,
                "case {case:?}: a route cannot be shorter than the {straight:.0} m straight line, got {distance}"
            );
            assert!(
                duration > 0,
                "case {case:?}: travelling {distance} m takes time, got {duration}"
            );
            let implied = distance as f64 / duration as f64;
            assert!(
                implied > 0.0 && implied <= top_road_speed_mps(mode) + 0.1,
                "case {case:?}: {distance} m in {duration} s implies {implied:.1} m/s, past the {:.1} m/s the fastest road allows",
                top_road_speed_mps(mode)
            );
        }
    }
}

/// One cost function serves the whole matrix, so the same pair costs the same wherever it
/// is asked for — alone or inside a row — and a mode held slower never gets there quicker.
///
/// The speed a cell implies is NOT one number: it is the speed of the roads the pair's
/// route travels, so a pair worth taking the fast classes for implies more of a speed than
/// a short pair stuck on the local roads. What cannot vary is the pair: one pair, one cost,
/// and a slower traveller never implies a faster one over it.
#[tokio::test]
async fn one_cost_function_serves_every_cell_and_a_slower_mode_never_implies_a_faster_one() {
    let router = routes_service();
    // Ordered fastest to slowest by the top speeds this service holds per mode.
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];

    // The cell of every pair, as a 1x1 matrix and as one cell of a row of the whole table:
    // the two must agree, and the implied speed must fall inside the speeds of the network.
    let mut faster: Option<(&str, Vec<f64>)> = None;
    for mode in modes {
        let (status, row, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": [{ "Position": PAIRS[0].1 }],
                "Destinations": PAIRS.iter().map(|(_, _, to)| json!({ "Position": to })).collect::<Vec<Value>>(),
                "TravelMode": mode,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {mode:?}: expected 200, got {status} with {row:?}");

        let mut speeds: Vec<f64> = Vec::new();
        for (index, (pair, origin, destination)) in PAIRS.iter().enumerate() {
            let case = format!("{mode}, {pair}");
            let alone = cell(&router, *origin, *destination, mode, "FastestRoute", &case).await;
            // The first row of the table is this pair's origin, so the row's cell for it is
            // the same pair as the 1x1 matrix above.
            if *origin == PAIRS[0].1 {
                let inside = &row["RouteMatrix"][0][index];
                assert_eq!(
                    (int(inside, "Distance", &case), int(inside, "Duration", &case)),
                    (int(&alone, "Distance", &case), int(&alone, "Duration", &case)),
                    "case {case:?}: one pair costs the same alone as in a row, got {inside:?} against {alone:?}"
                );
            }
            let (distance, duration) = (int(&alone, "Distance", &case), int(&alone, "Duration", &case));
            assert!(
                duration > 0,
                "case {case:?}: travelling {distance} m takes time, got {duration}"
            );
            let implied = distance as f64 / duration as f64;
            assert!(
                implied > 0.0 && implied <= top_road_speed_mps(mode) + 0.1,
                "case {case:?}: {distance} m in {duration} s implies {implied:.1} m/s, past the {:.1} m/s the fastest road allows",
                top_road_speed_mps(mode)
            );
            speeds.push(implied);
        }
        if let Some((faster_mode, faster_speeds)) = &faster {
            for (index, (speed, faster_speed)) in speeds.iter().zip(faster_speeds).enumerate() {
                let case = format!("{mode}, {}", PAIRS[index].0);
                assert!(
                    *speed <= faster_speed + 0.1,
                    "case {case:?}: must not imply a higher speed than {faster_mode:?} over the same pair, \
                     got {speed:.2} m/s against {faster_speed:.2} m/s"
                );
            }
        }
        faster = Some((mode, speeds));
    }
}

/// `ShortestRoute` asks for the shorter way and `FastestRoute` for the quicker one, and
/// neither is worse than the other in its own currency: the objective reaches the per-cell
/// cost rather than being read and dropped.
#[tokio::test]
async fn the_routing_objective_reaches_every_matrix_cell() {
    let router = routes_service();

    for (pair, origin, destination) in PAIRS {
        let case = format!("Car, {pair}");
        let fastest = cell(&router, origin, destination, "Car", "FastestRoute", &case).await;
        let shortest = cell(&router, origin, destination, "Car", "ShortestRoute", &case).await;
        assert!(
            int(&shortest, "Distance", &case) <= int(&fastest, "Distance", &case),
            "case {case:?}: ShortestRoute must not be the longer one, got {shortest:?} against {fastest:?}"
        );
        assert!(
            int(&fastest, "Duration", &case) <= int(&shortest, "Duration", &case),
            "case {case:?}: FastestRoute must not be the slower one, got {fastest:?} against {shortest:?}"
        );
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
