// SPDX-License-Identifier: CC0-1.0

//! What `CalculateIsolines` reports about the area it computed.
//!
//! The rule under test is never "this recorded request gives this recorded ring" but
//! invariants over classes of requests, each asserted over a table that spans what it
//! generalises over (threshold type and value, how many thresholds and in which order,
//! travel mode, routing objective, granularity, geometry format, and whether the area is
//! measured from an origin or towards a destination):
//!
//! * a vertex of an isoline is reachable within that isoline's threshold and a point well
//!   beyond it is not — checked by asking `CalculateRoutes` for the very route, so the two
//!   operations are held to the one cost function between them;
//! * the farther a travel mode travels in a second, the farther a time-based isoline for it
//!   reaches, while a distance-based isoline does not grow with speed;
//! * a request's thresholds and the answer's isolines correspond one for one, in order, and a
//!   larger threshold reaches at least as far as a smaller one;
//! * the granularity asked for is what decides how many points a boundary carries;
//! * the area is measured from, and the snapped point reported for, whichever of `Origin`
//!   and `Destination` the request set — and a request that sets neither, or both, or
//!   thresholds of two types, is refused by the member and the rule it broke.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::routing::{great_circle_m, int, position, post, routes_service, ROUTES};

/// The path `CalculateIsolines` is bound to.
const ISOLINES: &str = "/v2/isolines";

/// A position in mapped territory, where the road network has coverage.
const GOOD: [f64; 2] = [-123.1, 49.28];
/// A second one, on a different landmass.
const SEATTLE: [f64; 2] = [-122.3321, 47.6062];
/// The middle of the North Pacific: no territory is mapped there, so no road network exists
/// to reach anything from.
const PACIFIC: [f64; 2] = [-140.0, 40.0];

/// A granularity that asks for the fewest points an isoline is drawn with, which is what
/// keeps the number of routes a table row checks small.
fn fewest_points() -> Value {
    json!({ "MaxPoints": 31 })
}

/// The ring of the first geometry of isoline `index`, read off a `Simple` answer.
fn ring(answer: &Value, index: usize, case: &str) -> Vec<[f64; 2]> {
    let isoline = &answer["Isolines"][index];
    let geometries = isoline["Geometries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Geometries is required, got {isoline:?}"));
    assert_eq!(
        geometries.len(),
        1,
        "case {case:?}: one connected reachable area is one geometry, got {geometries:?}"
    );
    let positions = geometries[0]["Polygon"][0]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a Simple geometry carries a Polygon, got {geometries:?}"));
    assert!(
        positions.len() >= 4,
        "case {case:?}: a linear ring carries at least 4 positions, got {}",
        positions.len()
    );
    let ring: Vec<[f64; 2]> = positions.iter().map(|value| position(value, case)).collect();
    assert_eq!(
        ring.first(),
        ring.last(),
        "case {case:?}: a linear ring is closed by repeating its first position, got {ring:?}"
    );
    ring
}

/// The farthest a ring reaches from `centre`, in metres.
fn reach_m(centre: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    ring.iter()
        .map(|vertex| great_circle_m(centre, *vertex))
        .fold(0.0_f64, f64::max)
}

/// `position` moved away from `centre` by `factor` times its offset from it.
fn pushed_out(centre: [f64; 2], position: [f64; 2], factor: f64) -> [f64; 2] {
    [
        centre[0] + (position[0] - centre[0]) * factor,
        centre[1] + (position[1] - centre[1]) * factor,
    ]
}

/// Which cost a threshold bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Distance,
    Time,
}

impl Kind {
    /// The member a request carries this kind of threshold under.
    fn request_member(self) -> &'static str {
        match self {
            Self::Distance => "Distance",
            Self::Time => "Time",
        }
    }

    /// The member an isoline reports this kind of threshold in.
    fn response_member(self) -> &'static str {
        match self {
            Self::Distance => "DistanceThreshold",
            Self::Time => "TimeThreshold",
        }
    }

    /// The cost of a route this kind of threshold bounds, out of `(Distance, Duration)`.
    fn cost_of(self, route: (i64, i64)) -> i64 {
        match self {
            Self::Distance => route.0,
            Self::Time => route.1,
        }
    }
}

/// A request body asking for isolines around `anchor` at every threshold in `values`.
fn isoline_request(anchor_member: &str, anchor: [f64; 2], kind: Kind, values: &[i64]) -> Value {
    json!({
        anchor_member: anchor,
        "Thresholds": { kind.request_member(): values },
        "IsolineGeometryFormat": "Simple",
        "IsolineGranularity": fewest_points(),
    })
}

/// The cost `CalculateRoutes` reports for travelling from `from` to `to`: `(Distance,
/// Duration)`. The travel mode and the routing objective are the isoline's own, since they
/// are what the cost depends on.
async fn route_cost(
    router: &MultiServiceRouter,
    case: &str,
    from: [f64; 2],
    to: [f64; 2],
    travel_mode: &str,
    objective: Option<&str>,
) -> (i64, i64) {
    let mut body = json!({ "Origin": from, "Destination": to, "TravelMode": travel_mode });
    if let Some(objective) = objective {
        body["OptimizeRoutingFor"] = json!(objective);
    }
    let (status, answer, _) = post(router, ROUTES, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the route to a reported position must be served, got {status} with {answer:?}"
    );
    let summary = &answer["Routes"][0]["Summary"];
    (int(summary, "Distance", case), int(summary, "Duration", case))
}

/// The farthest the isoline for one threshold reaches from the snapped origin, in metres.
async fn reach_of(router: &MultiServiceRouter, kind: Kind, threshold: i64, travel_mode: &str) -> f64 {
    let case = format!("{travel_mode} within {threshold} bounded by {kind:?}");
    let mut body = isoline_request("Origin", GOOD, kind, &[threshold]);
    body["TravelMode"] = json!(travel_mode);
    let (status, answer, _) = post(router, ISOLINES, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {answer:?}"
    );
    let centre = position(&answer["SnappedOrigin"], &case);
    reach_m(centre, &ring(&answer, 0, &case))
}

/// Every vertex of an isoline is a point a route reaches within that isoline's threshold,
/// and a point well beyond the boundary is not — so an isoline and a route for the same
/// travel mode and objective cannot disagree about what is reachable.
///
/// Rows differ along what the rule generalises over: which cost the threshold bounds, how
/// large it is, the travel mode, the routing objective, how precisely the boundary was asked
/// to be resolved, and where the anchor is.
#[tokio::test]
async fn a_vertex_of_an_isoline_is_reachable_within_its_threshold_and_a_point_past_it_is_not() {
    let router = routes_service();
    // case, anchor, kind, threshold, travel mode, OptimizeRoutingFor, OptimizeIsolineFor.
    type Case = (
        &'static str,
        [f64; 2],
        Kind,
        i64,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
    );
    let cases: [Case; 7] = [
        ("ten minutes by car", GOOD, Kind::Time, 600, "Car", None, None),
        (
            "half an hour on foot",
            GOOD,
            Kind::Time,
            1_800,
            "Pedestrian",
            None,
            None,
        ),
        (
            "five kilometres by car",
            GOOD,
            Kind::Distance,
            5_000,
            "Car",
            None,
            None,
        ),
        (
            "twenty kilometres by truck",
            SEATTLE,
            Kind::Distance,
            20_000,
            "Truck",
            None,
            None,
        ),
        (
            "twenty minutes by scooter",
            SEATTLE,
            Kind::Time,
            1_200,
            "Scooter",
            None,
            None,
        ),
        (
            "ten minutes by car on the shortest route",
            GOOD,
            Kind::Time,
            600,
            "Car",
            Some("ShortestRoute"),
            None,
        ),
        (
            "five kilometres by car, resolved accurately",
            GOOD,
            Kind::Distance,
            5_000,
            "Car",
            None,
            Some("AccurateCalculation"),
        ),
    ];

    for (case, anchor, kind, threshold, travel_mode, objective, precision) in cases {
        let mut body = isoline_request("Origin", anchor, kind, &[threshold]);
        body["TravelMode"] = json!(travel_mode);
        if let Some(objective) = objective {
            body["OptimizeRoutingFor"] = json!(objective);
        }
        if let Some(precision) = precision {
            body["OptimizeIsolineFor"] = json!(precision);
        }
        let (status, answer, bucket) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an isoline on the network must be served, got {status} with {answer:?}"
        );
        assert!(
            bucket.is_some_and(|bucket| !bucket.is_empty()),
            "case {case:?}: the pricing bucket is header-bound and required"
        );
        let centre = position(&answer["SnappedOrigin"], case);
        let ring = ring(&answer, 0, case);
        assert_eq!(
            int(&answer["Isolines"][0], kind.response_member(), case),
            threshold,
            "case {case:?}: an isoline reports the threshold it was computed for"
        );

        // The rule holds for every vertex; the ones checked are spread around the ring, so
        // one row asks for a handful of routes rather than one per point.
        let mut checked = 0_usize;
        for vertex in ring.iter().take(ring.len() - 1).step_by(5) {
            let radius_m = great_circle_m(centre, *vertex);
            // A bearing the network carries nowhere has nothing to check past: its vertex is
            // the anchor itself.
            if radius_m < 100.0 {
                continue;
            }
            checked += 1;
            let inside = kind.cost_of(route_cost(&router, case, centre, *vertex, travel_mode, objective).await);
            // The search resolves the boundary to a fraction of the threshold's reach, so a
            // vertex may sit a hair outside it; a quarter past the boundary cannot.
            let slack = threshold / 100 + 5;
            assert!(
                inside <= threshold + slack,
                "case {case:?}: a vertex {radius_m:.0} m from the anchor must be reachable within \
                 the threshold {threshold}, got {inside}"
            );
            let beyond_position = pushed_out(centre, *vertex, 1.25);
            let beyond =
                kind.cost_of(route_cost(&router, case, centre, beyond_position, travel_mode, objective).await);
            assert!(
                beyond > threshold,
                "case {case:?}: a point a quarter past a vertex {radius_m:.0} m out must cost more \
                 than the threshold {threshold}, got {beyond}"
            );
        }
        assert!(
            checked >= 3,
            "case {case:?}: the area reachable on this network must have vertices away from the \
             anchor, got {checked} of the {} vertices sampled",
            ring.len() / 5
        );
    }
}

/// A time-based isoline reaches as far as the travel mode travels in the time, so the faster
/// the mode the farther the area reaches; a distance-based isoline is bounded by distance
/// alone, so it does not grow with the mode's speed.
#[tokio::test]
async fn a_faster_travel_mode_reaches_farther_in_the_same_time_but_not_in_the_same_distance() {
    let router = routes_service();
    // The modes `IsolineTravelMode` declares, slowest first by the speeds this service costs
    // them at (`ComAmazonawsGeoroutesRoutesserviceState::travel_mode_speeds`).
    const SLOWEST_FIRST: [&str; 4] = ["Pedestrian", "Scooter", "Truck", "Car"];

    // A time threshold: each mode must reach strictly farther than the one before it.
    let mut previous: Option<(&str, f64)> = None;
    for travel_mode in SLOWEST_FIRST {
        let reach = reach_of(&router, Kind::Time, 1_800, travel_mode).await;
        if let Some((slower, slower_reach)) = previous {
            assert!(
                reach > slower_reach,
                "case {travel_mode:?}: a faster mode reaches farther in the same time than \
                 {slower:?}, got {reach:.0} m against {slower_reach:.0} m"
            );
        }
        previous = Some((travel_mode, reach));
    }

    // A distance threshold: the bound is the same for every mode, so none reaches past the
    // threshold itself and none appreciably farther than another.
    let threshold: i64 = 10_000;
    let mut reaches: Vec<(&str, f64)> = Vec::new();
    for travel_mode in SLOWEST_FIRST {
        let reach = reach_of(&router, Kind::Distance, threshold, travel_mode).await;
        assert!(
            reach <= threshold as f64,
            "case {travel_mode:?}: a distance-bounded area cannot reach past its threshold \
             {threshold} m, got {reach:.0} m"
        );
        reaches.push((travel_mode, reach));
    }
    let (slowest_mode, slowest) = reaches[0];
    for (travel_mode, reach) in &reaches[1..] {
        let ratio = reach / slowest.max(1.0);
        assert!(
            (0.7..1.4).contains(&ratio),
            "case {travel_mode:?}: a distance-bounded area does not grow with the mode's speed, \
             got {reach:.0} m against {slowest:.0} m for {slowest_mode:?}"
        );
    }
}

/// One isoline per threshold, in the order the request listed them, each reporting its own
/// threshold — and a larger threshold reaches at least as far as a smaller one, while two
/// equal thresholds reach exactly as far as each other.
#[tokio::test]
async fn the_isolines_answer_the_requested_thresholds_one_for_one_and_in_order() {
    let router = routes_service();
    // case, kind, the thresholds as the request lists them.
    let cases: [(&str, Kind, Vec<i64>); 5] = [
        ("one time threshold", Kind::Time, vec![600]),
        ("three ascending time thresholds", Kind::Time, vec![300, 600, 1_200]),
        ("two descending time thresholds", Kind::Time, vec![1_200, 300]),
        (
            "five distance thresholds, unsorted",
            Kind::Distance,
            vec![4_000, 1_000, 20_000, 8_000, 2_000],
        ),
        ("two equal distance thresholds", Kind::Distance, vec![5_000, 5_000]),
    ];

    for (case, kind, thresholds) in cases {
        let body = isoline_request("Origin", GOOD, kind, &thresholds);
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let isolines = answer["Isolines"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Isolines is required, got {answer:?}"));
        assert_eq!(
            isolines.len(),
            thresholds.len(),
            "case {case:?}: one isoline per requested threshold, got {isolines:?}"
        );
        let centre = position(&answer["SnappedOrigin"], case);
        let mut reaches = Vec::new();
        for (index, threshold) in thresholds.iter().enumerate() {
            assert_eq!(
                int(&isolines[index], kind.response_member(), case),
                *threshold,
                "case {case:?}: isoline {index} answers the threshold at that position of the request"
            );
            reaches.push(reach_m(centre, &ring(&answer, index, case)));
        }
        for (index, threshold) in thresholds.iter().enumerate() {
            for (other, other_threshold) in thresholds.iter().enumerate() {
                if threshold == other_threshold {
                    // The same threshold computes the same area, whatever order it is in.
                    assert!(
                        (reaches[index] - reaches[other]).abs() < 1.0,
                        "case {case:?}: equal thresholds {threshold} reach equally far, got \
                         {:.0} m against {:.0} m",
                        reaches[index],
                        reaches[other]
                    );
                    continue;
                }
                if threshold > other_threshold {
                    continue;
                }
                // The smaller threshold's area is the nested one. The boundary is resolved
                // to a fraction of each threshold's own reach, so the comparison carries
                // that resolution as its slack rather than demanding exactness.
                assert!(
                    reaches[index] <= reaches[other] * 1.05 + 50.0,
                    "case {case:?}: threshold {threshold} must not reach farther than \
                     {other_threshold}, got {:.0} m against {:.0} m",
                    reaches[index],
                    reaches[other]
                );
            }
        }
    }
}

/// An isoline reports the threshold it WAS computed for and nothing about the kind of
/// threshold the request never asked for: `DistanceThreshold` is the distance "used to
/// calculate this isoline, if distance-based thresholds were specified in the request" and
/// `TimeThreshold` the time "used to calculate this isoline, if time-based thresholds were
/// specified in the request"
/// (`smithy_query(query="shape",shape="Isoline",
/// model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")`),
/// so the member of the other kind does not apply and is ABSENT from the isoline object —
/// presence of the member is how a caller tells a distance isoline from a time one.
///
/// Rows differ along what the rule generalises over: which kind the request bounds the area
/// by, the threshold values and how many of them, whether the area is measured from an
/// origin or towards a destination, and the geometry format the answer is rendered in.
#[tokio::test]
async fn an_isoline_carries_only_the_threshold_member_of_the_kind_the_request_asked_for() {
    let router = routes_service();
    // case, anchor member, kind, the thresholds the request lists, the geometry format it
    // asks for (`None` leaves the documented default).
    type Case = (&'static str, &'static str, Kind, Vec<i64>, Option<&'static str>);
    let cases: [Case; 6] = [
        ("one distance threshold", "Origin", Kind::Distance, vec![2_000], Some("Simple")),
        (
            "two distance thresholds, default format",
            "Origin",
            Kind::Distance,
            vec![1_000, 2_000],
            None,
        ),
        ("one time threshold", "Origin", Kind::Time, vec![600], Some("Simple")),
        (
            "three time thresholds, default format",
            "Origin",
            Kind::Time,
            vec![300, 600, 1_200],
            None,
        ),
        (
            "five distance thresholds towards a destination",
            "Destination",
            Kind::Distance,
            vec![1_000, 2_000, 3_000, 4_000, 5_000],
            Some("Simple"),
        ),
        (
            "a time threshold towards a destination",
            "Destination",
            Kind::Time,
            vec![900],
            None,
        ),
    ];

    for (case, anchor_member, kind, thresholds, format) in cases {
        let mut body = isoline_request(anchor_member, GOOD, kind, &thresholds);
        match format {
            Some(format) => body["IsolineGeometryFormat"] = json!(format),
            // `isoline_request` asks for `Simple`; removing it leaves the default.
            None => {
                body.as_object_mut()
                    .expect("a request body is an object")
                    .remove("IsolineGeometryFormat");
            }
        }
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let isolines = answer["Isolines"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Isolines is required, got {answer:?}"));
        assert_eq!(
            isolines.len(),
            thresholds.len(),
            "case {case:?}: one isoline per requested threshold, got {isolines:?}"
        );
        let inapplicable = match kind {
            Kind::Distance => Kind::Time,
            Kind::Time => Kind::Distance,
        };
        for (index, threshold) in thresholds.iter().enumerate() {
            let isoline = isolines[index]
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an isoline is an object, got {isolines:?}"));
            let reported = isoline.get(kind.response_member());
            assert_eq!(
                reported.and_then(Value::as_i64),
                Some(*threshold),
                "case {case:?}: isoline {index} reports {} = {threshold}, got {reported:?}",
                kind.response_member()
            );
            assert!(
                !isoline.contains_key(inapplicable.response_member()),
                "case {case:?}: isoline {index} was not computed for a {inapplicable:?} \
                 threshold, so {} must be absent, got {:?}",
                inapplicable.response_member(),
                isoline.get(inapplicable.response_member())
            );
        }
    }
}

/// How many points a boundary carries is what `IsolineGranularity` asks for: `MaxPoints`
/// caps them, a finer `MaxResolution` asks for more of them, and neither drops a ring below
/// the fewest points a caller may ask for.
#[tokio::test]
async fn the_granularity_asked_for_decides_how_many_points_a_boundary_carries() {
    let router = routes_service();
    // `IsolineGranularityOptions$MaxPoints` has a modeled minimum of 31
    // (`smithy_query(query="shape", shape="com.amazonaws.georoutes#IsolineGranularityOptions")`),
    // so no isoline is drawn with fewer points than that.
    const FEWEST_POINTS: usize = 31;
    // case, the granularity asked for, the cap it sets on the points.
    type Case = (&'static str, Value, Option<usize>);
    let cases: [Case; 7] = [
        ("no granularity at all", Value::Null, None),
        (
            "the fewest points a caller may ask for",
            json!({ "MaxPoints": 31 }),
            Some(31),
        ),
        ("sixty points", json!({ "MaxPoints": 60 }), Some(60)),
        ("two hundred points", json!({ "MaxPoints": 200 }), Some(200)),
        (
            "a five-kilometre resolution",
            json!({ "MaxResolution": 5_000 }),
            None,
        ),
        (
            "a one-kilometre resolution",
            json!({ "MaxResolution": 1_000 }),
            None,
        ),
        (
            "a two-hundred-metre resolution",
            json!({ "MaxResolution": 200 }),
            None,
        ),
    ];

    let mut counts: Vec<(&str, usize)> = Vec::new();
    for (case, granularity, cap) in cases {
        let mut body = json!({
            "Origin": GOOD,
            "Thresholds": { "Time": [600] },
            "IsolineGeometryFormat": "Simple",
        });
        if !granularity.is_null() {
            body["IsolineGranularity"] = granularity;
        }
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let points = ring(&answer, 0, case).len();
        assert!(
            points >= FEWEST_POINTS,
            "case {case:?}: an isoline carries at least {FEWEST_POINTS} points, got {points}"
        );
        if let Some(cap) = cap {
            assert!(
                points <= cap,
                "case {case:?}: MaxPoints is a maximum, got {points} points"
            );
        }
        counts.push((case, points));
    }

    // More points asked for is more points drawn, and a finer resolution is more points
    // drawn: both series run over the rows above in the order they ask for more detail.
    for series in [[1_usize, 2, 3], [4, 5, 6]] {
        for pair in series.windows(2) {
            let (coarse, coarse_count) = counts[pair[0]];
            let (fine, fine_count) = counts[pair[1]];
            assert!(
                fine_count > coarse_count,
                "case {fine:?}: a finer granularity draws more points than {coarse:?}, got \
                 {fine_count} against {coarse_count}"
            );
        }
    }
}

/// The area is measured from whichever of `Origin` and `Destination` the request set, and
/// the answer reports the snapped point for that member and for no other: a
/// destination-based request reports where the DESTINATION was matched onto the network.
#[tokio::test]
async fn the_snapped_point_reported_is_the_anchor_the_request_named() {
    let router = routes_service();
    // case, the member carrying the anchor, the anchor itself.
    let cases: [(&str, &str, [f64; 2]); 4] = [
        ("an origin on the network", "Origin", GOOD),
        ("a destination on the network", "Destination", GOOD),
        ("an origin on another landmass", "Origin", SEATTLE),
        ("a destination on another landmass", "Destination", SEATTLE),
    ];

    for (case, member, anchor) in cases {
        let body = isoline_request(member, anchor, Kind::Time, &[600]);
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let (reported, absent) = if member == "Origin" {
            ("SnappedOrigin", "SnappedDestination")
        } else {
            ("SnappedDestination", "SnappedOrigin")
        };
        assert!(
            answer[absent].is_null(),
            "case {case:?}: the request set no counterpart to {member}, so {absent} must be \
             absent, got {:?}",
            answer[absent]
        );
        let matched = position(&answer[reported], case);
        let offset_m = great_circle_m(anchor, matched);
        assert!(
            offset_m < 1_000.0,
            "case {case:?}: {reported} is the submitted position matched onto a road near it, got \
             {offset_m:.0} m away"
        );
        let reach = reach_m(matched, &ring(&answer, 0, case));
        assert!(
            reach > 100.0,
            "case {case:?}: a ten-minute drive reaches away from the anchor, got {reach:.0} m"
        );
    }

    // A position outside the mapped territory the network covers has no road to reach
    // anything from, so there is no area and no snapped point — never an invented ring.
    for member in ["Origin", "Destination"] {
        let case = format!("{member} in open water");
        let body = isoline_request(member, PACIFIC, Kind::Time, &[600]);
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        assert!(
            answer["SnappedOrigin"].is_null() && answer["SnappedDestination"].is_null(),
            "case {case:?}: no road matched the position, so neither snapped point is reported, \
             got {answer:?}"
        );
        let geometries = answer["Isolines"][0]["Geometries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Geometries is required, got {answer:?}"));
        assert!(
            geometries.is_empty(),
            "case {case:?}: nothing is reachable, so the isoline carries no geometry, got \
             {geometries:?}"
        );
    }
}

/// The geometry format decides which member of a geometry carries the boundary, and the
/// answer states the format it used — `FlexiblePolyline` where the request named none.
#[tokio::test]
async fn an_isoline_carries_its_boundary_in_the_requested_geometry_format() {
    let router = routes_service();
    // case, the format the request asked for, the format the answer must state, the member
    // that carries the ring, the member that must be absent.
    let cases: [(&str, Option<&str>, &str, &str, &str); 3] = [
        (
            "no format at all",
            None,
            "FlexiblePolyline",
            "PolylinePolygon",
            "Polygon",
        ),
        (
            "an explicit flexible polyline",
            Some("FlexiblePolyline"),
            "FlexiblePolyline",
            "PolylinePolygon",
            "Polygon",
        ),
        (
            "simple coordinates",
            Some("Simple"),
            "Simple",
            "Polygon",
            "PolylinePolygon",
        ),
    ];

    for (case, requested, expected, carried, absent) in cases {
        let mut body = json!({
            "Origin": GOOD,
            "Thresholds": { "Time": [600] },
            "IsolineGranularity": fewest_points(),
        });
        if let Some(format) = requested {
            body["IsolineGeometryFormat"] = json!(format);
        }
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["IsolineGeometryFormat"].as_str(),
            Some(expected),
            "case {case:?}: the answer states the format it used, got {answer:?}"
        );
        let geometry = &answer["Isolines"][0]["Geometries"][0];
        assert!(
            geometry[absent].is_null(),
            "case {case:?}: {absent} belongs to the other format, got {geometry:?}"
        );
        let shapes = geometry[carried]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: {carried} carries the boundary, got {geometry:?}"));
        assert_eq!(
            shapes.len(),
            1,
            "case {case:?}: one connected area is one ring, got {shapes:?}"
        );
        let connections = answer["Isolines"][0]["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Connections is required, got {answer:?}"));
        assert!(
            connections.is_empty(),
            "case {case:?}: a connection links two polygons of one isoline, and there is one, got \
             {connections:?}"
        );
    }
}

/// A request that does not say what area to compute is refused by the member and the rule it
/// broke: an area needs exactly one point to be measured from or towards, thresholds of
/// exactly one type, one time of day at most, options that name the mode selected, and a
/// position inside the coordinate domain wherever it carries one.
#[tokio::test]
async fn a_request_that_does_not_name_one_anchor_and_one_threshold_type_is_refused() {
    let router = routes_service();
    // case, body, the member the refusal names and the sentence it states — `None` where the
    // request must be served instead.
    type Case = (&'static str, Value, Option<(&'static str, &'static str)>);
    let cases: [Case; 12] = [
        (
            "neither an origin nor a destination",
            json!({ "Thresholds": { "Time": [600] } }),
            Some((
                "Origin",
                "At least one of the following fields must be set: Origin, Destination.",
            )),
        ),
        (
            "both an origin and a destination",
            json!({
                "Origin": GOOD,
                "Destination": SEATTLE,
                "Thresholds": { "Time": [600] }
            }),
            Some((
                "Destination",
                "Only one of the following fields may be set: 'Origin', 'Destination'.",
            )),
        ),
        (
            "thresholds of neither type",
            json!({ "Origin": GOOD, "Thresholds": {} }),
            Some((
                "Thresholds.Distance",
                "At least one of the following fields must be set: Thresholds.Distance, \
                 Thresholds.Time.",
            )),
        ),
        (
            "thresholds of both types",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Distance": [1_000], "Time": [600] }
            }),
            Some((
                "Thresholds",
                "Only one of the following fields may be set within 'Thresholds': 'Consumption', \
                 'Distance', 'Time'.",
            )),
        ),
        (
            // The exclusive set belongs to the container, so the sentence does not change
            // with which kinds the request populated, how many values each carries, or the
            // order they arrive in.
            "thresholds of both types, several values each, time first",
            json!({
                "Destination": GOOD,
                "Thresholds": { "Time": [300, 600, 900], "Distance": [1_000, 2_000] }
            }),
            Some((
                "Thresholds",
                "Only one of the following fields may be set within 'Thresholds': 'Consumption', \
                 'Distance', 'Time'.",
            )),
        ),
        (
            // The same rule between the two anchors, which are members of the request
            // itself rather than of a nested structure, so there is no container to name.
            "both a destination and an origin, with distance thresholds",
            json!({
                "Destination": GOOD,
                "Origin": SEATTLE,
                "Thresholds": { "Distance": [2_000] }
            }),
            Some((
                "Destination",
                "Only one of the following fields may be set: 'Origin', 'Destination'.",
            )),
        ),
        (
            "a time of day set twice",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "DepartureTime": "2024-04-22T17:57:24Z",
                "DepartNow": true
            }),
            Some((
                "DepartureTime",
                "DepartureTime, ArrivalTime and DepartNow each set the time of the calculation, so \
                 at most one may be supplied",
            )),
        ),
        (
            "truck options for a car",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "TravelMode": "Car",
                "TravelModeOptions": { "Truck": { "GrossWeight": 10_000 } }
            }),
            Some((
                "TravelModeOptions.Truck",
                "Only the following fields may be set within 'TravelModeOptions' when 'TravelMode' \
                 has value Car: 'Car'.",
            )),
        ),
        (
            "car options for a pedestrian",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "TravelMode": "Pedestrian",
                "TravelModeOptions": { "Car": {} }
            }),
            Some((
                "TravelModeOptions.Car",
                "'TravelModeOptions.Car' must not be set when 'TravelMode' has value Pedestrian.",
            )),
        ),
        (
            "a side-of-street position outside the coordinate domain",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "OriginOptions": { "SideOfStreet": { "Position": [200.0, 49.0] } }
            }),
            Some((
                "OriginOptions.SideOfStreet.Position",
                "Longitude must be between -180 and 180 degrees, but was set to 200.",
            )),
        ),
        (
            "an origin and one time threshold",
            json!({ "Origin": GOOD, "Thresholds": { "Time": [600] } }),
            None,
        ),
        (
            "a destination and five distance thresholds",
            json!({
                "Destination": GOOD,
                "Thresholds": { "Distance": [1_000, 2_000, 3_000, 4_000, 5_000] },
                "IsolineGranularity": { "MaxPoints": 31 }
            }),
            None,
        ),
    ];

    for (case, body, refused) in cases {
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        let Some((member, expected)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: a request that names one anchor and one threshold type must be \
                 served, got {status} with {answer:?}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: a refused request is the modeled ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(expected),
            "case {case:?}: expected {expected:?}, got {:?}",
            answer["message"]
        );
        assert_eq!(
            answer["reason"].as_str(),
            Some("FieldValidationFailed"),
            "case {case:?}: a broken field rule is reported as FieldValidationFailed, got {answer:?}"
        );
        let fields = answer["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: fieldList is required, got {answer:?}"));
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: one broken rule is one fieldList entry, got {fields:?}"
        );
        assert_eq!(
            fields[0]["name"].as_str(),
            Some(member),
            "case {case:?}: expected the offending member {member:?}, got {fields:?}"
        );
        assert_eq!(
            fields[0]["message"].as_str(),
            Some(expected),
            "case {case:?}: the fieldList entry restates the message, got {fields:?}"
        );
    }
}

/// The times the answer reports are the ones the request pinned the calculation to, and only
/// for the anchor they belong to: the departure of an origin-based request, and an arrival
/// for a destination-based request that asked for one.
#[tokio::test]
async fn the_answer_reports_the_time_the_request_pinned_the_calculation_to() {
    let router = routes_service();
    let departure = "2024-04-22T17:57:24Z";
    let arrival = "2024-04-22T19:30:00Z";
    // case, the members the request adds to an isoline around an anchor, the departure the
    // answer must report, the arrival it must report.
    type Case = (&'static str, Value, Option<&'static str>, Option<&'static str>);
    let cases: [Case; 4] = [
        (
            "an origin and a departure time",
            json!({ "Origin": GOOD, "DepartureTime": departure }),
            Some(departure),
            None,
        ),
        (
            "a destination and an arrival time",
            json!({ "Destination": GOOD, "ArrivalTime": arrival }),
            None,
            Some(arrival),
        ),
        (
            "an origin and an arrival time",
            json!({ "Origin": GOOD, "ArrivalTime": arrival }),
            None,
            None,
        ),
        (
            "a destination and a departure time",
            json!({ "Destination": GOOD, "DepartureTime": departure }),
            None,
            None,
        ),
    ];

    for (case, members, expected_departure, expected_arrival) in cases {
        let mut body = json!({
            "Thresholds": { "Time": [600] },
            "IsolineGranularity": fewest_points(),
        });
        for (member, value) in members.as_object().expect("an object of members") {
            body[member] = value.clone();
        }
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["DepartureTime"].as_str(),
            expected_departure,
            "case {case:?}: expected the departure {expected_departure:?}, got {:?}",
            answer["DepartureTime"]
        );
        assert_eq!(
            answer["ArrivalTime"].as_str(),
            expected_arrival,
            "case {case:?}: expected the arrival {expected_arrival:?}, got {:?}",
            answer["ArrivalTime"]
        );
    }

    // An isoline is "the area reachable within a given threshold" at a DEPARTURE instant,
    // so an origin-based request is anchored at one however it asked: `DepartNow` and a
    // request that names no time at all are both answered with the instant the area was
    // computed for, in the UTC form the model's pattern admits. The instant comes off this
    // service's clock, so what is pinned here is the FORM, never one run's value.
    for (case, body) in [
        (
            "depart now",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "DepartNow": true,
                "IsolineGranularity": fewest_points(),
            }),
        ),
        (
            "an origin and no time at all",
            json!({
                "Origin": GOOD,
                "Thresholds": { "Time": [600] },
                "IsolineGranularity": fewest_points(),
            }),
        ),
    ] {
        let (status, answer, _) = post(&router, ISOLINES, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer:?}"
        );
        let reported = answer["DepartureTime"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the departure the area was computed for is reported, got {answer:?}"));
        assert!(
            reported.len() == 20 && reported.ends_with('Z'),
            "case {case:?}: expected an ISO-8601 instant, got {reported:?}"
        );
        assert!(
            answer["ArrivalTime"].is_null(),
            "case {case:?}: the request named no arrival, so none is reported, got {:?}",
            answer["ArrivalTime"]
        );
    }
}
