// SPDX-License-Identifier: CC0-1.0

//! The traversal is DETERMINISTIC, and it answers the same way for a pair of real places
//! wherever on the mapped world they are.
//!
//! The issue these suites were written for reports the per-cell metrics of
//! `CalculateRouteMatrix` as "a deterministic per-cell pair" of a road-network distance and
//! a duration consistent with it, so two properties belong to the traversal itself rather
//! than to any one pair: asking twice gives the same answer, and the answer is a path of the
//! road graph costed at the speeds of the roads it runs on for every pair the network
//! carries — not only for the pairs of one metropolitan grid.
//!
//! The rows span the mapped world (North America, Europe, Asia, the southern hemisphere) and
//! separations from a few kilometres to a thousand, which is what the rule generalises over.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, post, routes_service, MATRIX};

/// Posted speeds of the slowest and the fastest road a car may travel, in km/h, as
/// `src/state.rs` holds them (`STREET_SPEED_KPH`, `MOTORWAY_SPEED_KPH`).
const STREET_SPEED_KPH: f64 = 50.0;
const MOTORWAY_SPEED_KPH: f64 = 110.0;

/// Metres per second in one km/h.
const KPH_TO_MPS: f64 = 1_000.0 / 3_600.0;

/// Pairs of real places, each pair on one landmass so the network carries it.
const PAIRS: [(&str, [f64; 2], [f64; 2]); 6] = [
    ("Seattle to Tacoma", [-122.3493, 47.6205], [-122.4443, 47.2529]),
    ("Seattle to Portland", [-122.3321, 47.6062], [-122.6784, 45.5152]),
    ("Paris to Orleans", [2.3522, 48.8566], [1.909, 47.9029]),
    ("Madrid to Barcelona", [-3.7038, 40.4168], [2.1734, 41.3851]),
    ("Tokyo to Nagoya", [139.6917, 35.6895], [136.9066, 35.1815]),
    ("Sydney to Canberra", [151.2093, -33.8688], [149.1300, -35.2809]),
];

/// Every pair is answered with a path of the graph — never shorter than the rectilinear
/// journey the grid forces — costed between the slowest and the fastest road a car may
/// travel.
#[tokio::test]
async fn a_pair_anywhere_on_the_network_is_a_graph_path_at_a_road_speed() {
    let router = routes_service();
    // A match moves a position at most half the grid spacing in each axis, at either end:
    // that is the only part of a reported distance that is not travel along a road.
    let slack_m = 4.0 * 0.000_5 * 111_320.0;

    for (pair, origin, destination) in PAIRS {
        let cell = cell(&router, origin, destination, pair).await;
        let distance = int(&cell, "Distance", pair);
        let duration = int(&cell, "Duration", pair);
        let rectilinear = great_circle_m(origin, [origin[0], destination[1]])
            + great_circle_m([origin[0], destination[1]], destination)
                .min(great_circle_m(origin, [destination[0], origin[1]]));
        assert!(
            distance as f64 + slack_m >= rectilinear,
            "case {pair:?}: the roads run north-south and east-west, so the route covers the \
             {rectilinear:.0} m rectilinear journey at least, got {distance}"
        );
        assert!(
            duration > 0,
            "case {pair:?}: travelling {distance} m takes time, got {duration}"
        );
        let implied = distance as f64 / duration as f64;
        let (slowest, fastest) = (
            STREET_SPEED_KPH * KPH_TO_MPS - 0.5,
            MOTORWAY_SPEED_KPH * KPH_TO_MPS + 0.5,
        );
        assert!(
            (slowest..=fastest).contains(&implied),
            "case {pair:?}: {distance} m in {duration} s implies {implied:.2} m/s, outside the \
             {slowest:.2}..{fastest:.2} m/s the roads a car may travel are posted at"
        );
    }
}

/// The same pair asked for twice is answered with the same numbers, and the answer does not
/// depend on which pairs were asked for before it: the traversal is a function of the pair,
/// the travel mode and the objective, and the memo it is kept in cannot change it.
#[tokio::test]
async fn the_same_pair_is_answered_with_the_same_metrics_however_often_it_is_asked() {
    let router = routes_service();

    let mut first_answers: Vec<(&str, i64, i64)> = Vec::new();
    for (pair, origin, destination) in PAIRS {
        let cell = cell(&router, origin, destination, pair).await;
        first_answers.push((pair, int(&cell, "Distance", pair), int(&cell, "Duration", pair)));
    }
    // Round two in the reverse order, on the same service: every pair has been computed
    // once, and several others have been computed since.
    for (pair, origin, destination) in PAIRS.iter().rev() {
        let cell = cell(&router, *origin, *destination, pair).await;
        let expected = first_answers
            .iter()
            .find(|(name, _, _)| name == pair)
            .copied()
            .unwrap_or_else(|| panic!("case {pair:?}: the first round answered every pair"));
        assert_eq!(
            (*pair, int(&cell, "Distance", pair), int(&cell, "Duration", pair)),
            expected,
            "case {pair:?}: the same pair must be answered with the same metrics, got {cell:?}"
        );
    }
}

/// The single cell of a 1x1 car matrix for `origin` -> `destination`, which must be routed.
async fn cell(
    router: &smithy_server_runtime::MultiServiceRouter,
    origin: [f64; 2],
    destination: [f64; 2],
    case: &str,
) -> Value {
    let (status, answer, _) = post(
        router,
        MATRIX,
        &json!({
            "Origins": [{ "Position": origin }],
            "Destinations": [{ "Position": destination }],
            "TravelMode": "Car",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
    let cell = answer["RouteMatrix"][0][0].clone();
    assert!(
        cell["Error"].is_null(),
        "case {case:?}: the pair is on one landmass, so the network routes it, got {cell:?}"
    );
    cell
}
