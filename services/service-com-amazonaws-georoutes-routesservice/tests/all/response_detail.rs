// SPDX-License-Identifier: CC0-1.0

//! What a `CalculateRoutes` answer says about the request it answers.
//!
//! The rules under test are not "this request gives these numbers" but invariants over
//! classes of requests:
//!
//! * a place reports the SUBMITTED position in `OriginalPosition` ("Position provided in
//!   the request") at full double precision, and the position MATCHED onto the road
//!   network in `Position` — the same match `SnapToRoads` reports for that position, so
//!   the two members differ by the snap offset;
//! * a leg's `Summary` summarises the leg that was computed, so every leg carries it;
//! * a travel step carries the members the model requires, the `ExitNumber` list the
//!   service computes, and narration only where the request asked for it;
//! * a major road label identifies a road the way the road IS identified — by its route
//!   number where the network numbers it, by its name otherwise — and declares the
//!   language it is rendered in;
//! * `PricingBucket` is the pricing TIER a call is charged in, never a request or
//!   operation class.
//!
//! Every table spans what its rule generalises over: whether the submitted position lies
//! on the network, its precision, the travel mode (so leg type and which roads may be
//! travelled), the hemisphere, the step detail, the requested languages and the feature
//! list.

use serde_json::{json, Value};

use crate::routing::{great_circle_m, int, position, post, routes_service, ROUTES, SNAP};

/// Spacing of the synthetic road grid, in degrees, as documented on `ROAD_SPACING_DEG` in
/// `src/state.rs`: every multiple of it in longitude is a meridian road and every multiple
/// in latitude is a parallel road. A point ON the network therefore has one coordinate at
/// a multiple of this.
const ROAD_SPACING_DEG: f64 = 0.0005;

/// Tolerance, in degrees, for reading a coordinate as sitting on a grid line — the
/// `POSITION_EPSILON` of `src/state.rs`, below which two positions are the same place.
const GRID_EPSILON_DEG: f64 = 1e-7;

/// Whether `position` lies on a road of the grid: a meridian road fixes its longitude and
/// a parallel road its latitude, so one of its two coordinates is a multiple of the
/// spacing.
fn lies_on_the_grid(position: [f64; 2]) -> bool {
    position.iter().any(|coordinate| {
        let multiples = coordinate / ROAD_SPACING_DEG;
        (multiples - multiples.round()).abs() * ROAD_SPACING_DEG <= GRID_EPSILON_DEG
    })
}

/// The leg details of whichever leg type `leg` is.
fn leg_details(leg: &Value, case: &str) -> Value {
    for member in ["VehicleLegDetails", "PedestrianLegDetails"] {
        if leg[member].is_object() {
            return leg[member].clone();
        }
    }
    panic!("case {case:?}: the leg carries no details of its type: {leg:?}")
}

/// A direct route request between two positions.
fn route_request(origin: [f64; 2], destination: [f64; 2], travel_mode: &str) -> Value {
    json!({
        "Origin": origin,
        "Destination": destination,
        "TravelMode": travel_mode,
    })
}

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

/// `OriginalPosition` is the submitted position, verbatim; `Position` is where that
/// position was matched onto the road network.
///
/// Rows span what the rule generalises over: a position already on a road, positions well
/// off one, a position given at 7-decimal precision (which must come back un-rounded),
/// both leg types, and both hemispheres. `moves_at_least_m` is 0 for a row submitted on a
/// road — there the two positions legitimately coincide — and positive for one that has to
/// be matched.
#[tokio::test]
async fn the_places_of_a_leg_report_the_submitted_position_and_its_match() {
    let router = routes_service();
    /// name, origin, destination, travel mode, how far each position must be matched.
    type PlaceCase = (&'static str, [f64; 2], [f64; 2], &'static str, f64);
    let cases: [PlaceCase; 6] = [
        // -123.1 and 49.28 are both multiples of the grid spacing, so both endpoints are
        // already on a road and nothing has to move.
        ("both endpoints on a road", [-123.1, 49.28], [-123.12, 49.26], "Car", 0.0),
        (
            "both endpoints off the roads",
            [-123.100_18, 49.280_18],
            [-123.120_22, 49.260_22],
            "Car",
            5.0,
        ),
        (
            "seven decimals of precision, off the roads",
            [-123.100_180_7, 49.280_180_7],
            [-123.120_220_3, 49.260_220_3],
            "Truck",
            5.0,
        ),
        (
            "a pedestrian leg, off the roads",
            [-123.100_18, 49.280_18],
            [-123.102_22, 49.278_22],
            "Pedestrian",
            5.0,
        ),
        (
            "a scooter leg in the southern hemisphere",
            [151.210_18, -33.870_18],
            [151.200_22, -33.860_22],
            "Scooter",
            5.0,
        ),
        (
            // Longitude -123.100_18 lies between meridian roads; latitude 49.28 is a
            // parallel road a car may travel, so only one coordinate has to move.
            "a position on a parallel road but not a meridian one",
            [-123.100_18, 49.28],
            [-123.12, 49.26],
            "Car",
            0.0,
        ),
    ];

    for (case, origin, destination, travel_mode, moves_at_least_m) in cases {
        let (status, answer, _) = post(&router, ROUTES, &route_request(origin, destination, travel_mode)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
            .clone();
        let details = leg_details(&legs[0], case);

        // The same two positions, matched by SnapToRoads: the one operation of this
        // service whose whole job is the match, driven at a radius wide enough that
        // neither point can go unmatched. A leg's `Position` must be that same match.
        let snap = json!({
            "TracePoints": [{ "Position": origin }, { "Position": destination }],
            "TravelMode": travel_mode,
            "SnapRadius": 10_000,
        });
        let (snap_status, snapped, _) = post(&router, SNAP, &snap).await;
        assert_eq!(
            snap_status, 200,
            "case {case:?}: expected 200 from SnapToRoads, got {snap_status} with {snapped:?}"
        );
        let snapped_points = snapped["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: SnappedTracePoints is required, got {snapped:?}"))
            .clone();

        for (which, submitted, point) in [
            ("Departure", origin, &snapped_points[0]),
            ("Arrival", destination, &snapped_points[1]),
        ] {
            let case = &format!("{case} / {which}");
            let place = &details[which]["Place"];
            let original = position(&place["OriginalPosition"], case);
            let reported = position(&place["Position"], case);
            let matched = position(&point["SnappedPosition"], case);

            assert_eq!(
                original, submitted,
                "case {case:?}: OriginalPosition is the position provided in the request, un-rounded, got {original:?}"
            );
            assert!(
                (reported[0] - matched[0]).abs() <= GRID_EPSILON_DEG
                    && (reported[1] - matched[1]).abs() <= GRID_EPSILON_DEG,
                "case {case:?}: Position must be the position matched onto the road network, \
                 which SnapToRoads reports as {matched:?}, got {reported:?}"
            );
            assert!(
                lies_on_the_grid(reported),
                "case {case:?}: Position is a point of the road network, got {reported:?}"
            );

            let offset = great_circle_m(original, reported);
            if moves_at_least_m > 0.0 {
                assert!(
                    offset >= moves_at_least_m,
                    "case {case:?}: a position off the network is matched onto it, so Position \
                     must differ from OriginalPosition by at least {moves_at_least_m} m, got {offset:.3} m"
                );
            } else {
                assert!(
                    offset <= 0.05,
                    "case {case:?}: a position already on a road must not be moved, got {offset:.3} m"
                );
            }
        }
    }
}

/// A pass-through waypoint reports the same two positions under the same rule: it is a
/// place of the leg it falls inside, not a leg of its own.
#[tokio::test]
async fn a_pass_through_place_reports_the_submitted_position_and_its_match() {
    let router = routes_service();
    // Rows differ in how many pass-through waypoints the leg carries and how far off the
    // network they sit.
    let cases: [(&str, Vec<[f64; 2]>); 3] = [
        ("one waypoint on a road", vec![[-123.105, 49.27]]),
        ("one waypoint off the roads", vec![[-123.105_18, 49.274_22]]),
        (
            "two waypoints off the roads",
            vec![[-123.104_18, 49.276_22], [-123.112_18, 49.267_22]],
        ),
    ];

    for (case, through) in cases {
        let mut body = route_request([-123.1, 49.28], [-123.12, 49.26], "Car");
        body["Waypoints"] = Value::Array(
            through
                .iter()
                .map(|position| json!({ "Position": position, "PassThrough": true }))
                .collect(),
        );
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let details = leg_details(&answer["Routes"][0]["Legs"][0], case);
        let waypoints = details["PassThroughWaypoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: PassThroughWaypoints is required, got {details:?}"))
            .clone();
        assert_eq!(
            waypoints.len(),
            through.len(),
            "case {case:?}: every pass-through waypoint is reported, got {waypoints:?}"
        );

        for (index, submitted) in through.iter().enumerate() {
            let case = &format!("{case} / waypoint {index}");
            let place = &waypoints[index]["Place"];
            assert_eq!(
                position(&place["OriginalPosition"], case),
                *submitted,
                "case {case:?}: OriginalPosition is the submitted position, got {place:?}"
            );
            let reported = position(&place["Position"], case);
            assert!(
                lies_on_the_grid(reported),
                "case {case:?}: Position is a point of the road network, got {reported:?}"
            );
            assert_eq!(
                int(place, "WaypointIndex", case),
                index as i64,
                "case {case:?}: the place names the waypoint it is, got {place:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Travel steps
// ---------------------------------------------------------------------------

/// A travel step carries what the model REQUIRES of it (`Duration`, `Type`), the
/// `ExitNumber` list the service computes, and — only where the request asked for the
/// per-step narration — `Instruction`, `CurrentRoad` and `NextRoad`.
///
/// Rows span the step detail (none, both values of `RouteTravelStepType`) and the travel
/// mode, i.e. both step shapes the model declares; none of them asks for a leg feature.
#[tokio::test]
async fn a_step_asked_for_no_detail_carries_only_what_the_model_requires() {
    let router = routes_service();
    /// name, origin, destination, travel mode, TravelStepType.
    type StepCase = (&'static str, [f64; 2], [f64; 2], &'static str, Option<&'static str>);
    let cases: [StepCase; 5] = [
        ("car, no step type", [-123.1, 49.28], [-123.12, 49.26], "Car", None),
        ("car, basic steps", [-123.1, 49.28], [-123.12, 49.26], "Car", Some("Default")),
        (
            "car, turn by turn",
            [-123.1, 49.28],
            [-123.12, 49.26],
            "Car",
            Some("TurnByTurn"),
        ),
        (
            "a pedestrian leg",
            [-123.1, 49.28],
            [-123.103, 49.277],
            "Pedestrian",
            Some("Default"),
        ),
        (
            "a truck leg in the southern hemisphere",
            [151.21, -33.87],
            [151.2, -33.86],
            "Truck",
            None,
        ),
    ];

    for (case, origin, destination, travel_mode, step_type) in cases {
        let mut body = route_request(origin, destination, travel_mode);
        if let Some(step_type) = step_type {
            body["TravelStepType"] = json!(step_type);
        }
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        for (index, leg) in answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"))
            .iter()
            .enumerate()
        {
            let details = leg_details(leg, case);
            let steps = details["TravelSteps"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: TravelSteps is required, got {details:?}"))
                .clone();
            assert!(
                !steps.is_empty(),
                "case {case:?}: leg {index} travels, so it reports steps"
            );

            for (ordinal, step) in steps.iter().enumerate() {
                let case = &format!("{case} / leg {index} / step {ordinal}");
                assert!(
                    step["Type"].as_str().is_some_and(|value| !value.is_empty()),
                    "case {case:?}: Type is required, got {step:?}"
                );
                assert!(
                    step["Duration"].as_i64().is_some(),
                    "case {case:?}: Duration is required, got {step:?}"
                );
                assert_eq!(
                    step["ExitNumber"].as_array(),
                    Some(&Vec::new()),
                    "case {case:?}: ExitNumber is reported as the empty list where the step takes \
                     no numbered exit, got {:?}",
                    step["ExitNumber"]
                );
                for member in ["Instruction", "CurrentRoad", "NextRoad"] {
                    assert!(
                        step[member].is_null(),
                        "case {case:?}: {member} is narration the request did not ask for, got {:?}",
                        step[member]
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Major road labels
// ---------------------------------------------------------------------------

/// A major road label carries exactly one of the two members `RouteMajorRoadLabel`
/// declares, and it is the one the road HAS: the `RouteNumber` of a numbered road, the
/// `RoadName` of an unnumbered one. Either member declares the language it is rendered in.
///
/// Rows span both variants — the numbered roads of the grid are its motorways and trunk
/// roads, and a pedestrian may travel neither, so a walking route is labelled by name —
/// and the requested `Languages`, which is what the label's `Language` follows.
#[tokio::test]
async fn a_major_road_label_identifies_a_road_the_way_the_road_is_identified() {
    let router = routes_service();
    // name, origin, destination, travel mode, objective, requested languages, the language
    // tag the labels must carry, whether a numbered road must appear among the labels.
    type LabelCase = (
        &'static str,
        [f64; 2],
        [f64; 2],
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        &'static str,
        bool,
    );
    let cases: [LabelCase; 5] = [
        (
            // Latitude 49.3 is a trunk parallel of the grid — a numbered road — and the
            // shortest route runs straight along it.
            "a run along a numbered road",
            [-123.1, 49.3],
            [-123.0, 49.3],
            "Car",
            Some("ShortestRoute"),
            None,
            "en-us",
            true,
        ),
        (
            "the same run asked for in a regional variant",
            [-123.1, 49.3],
            [-123.0, 49.3],
            "Truck",
            Some("ShortestRoute"),
            Some("en-GB"),
            "en-GB",
            true,
        ),
        (
            "a walking route, which may not use a numbered road",
            [-123.1, 49.28],
            [-123.103, 49.277],
            "Pedestrian",
            None,
            None,
            "en-us",
            false,
        ),
        (
            "a walking route in a language the text is not held in",
            [-123.1, 49.28],
            [-123.103, 49.277],
            "Pedestrian",
            None,
            Some("ja"),
            "en-us",
            false,
        ),
        (
            // A scooter may travel the numbered roads, and the grid's trunk roads are near
            // enough anywhere that a route of a few hundred metres takes one.
            "a scooter route in the southern hemisphere",
            [151.21, -33.87],
            [151.2, -33.86],
            "Scooter",
            None,
            None,
            "en-us",
            true,
        ),
    ];

    for (case, origin, destination, travel_mode, objective, languages, tag, numbered) in cases {
        let mut body = route_request(origin, destination, travel_mode);
        if let Some(objective) = objective {
            body["OptimizeRoutingFor"] = json!(objective);
        }
        if let Some(languages) = languages {
            body["Languages"] = json!([languages]);
        }
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");

        let labels = answer["Routes"][0]["MajorRoadLabels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: MajorRoadLabels is required, got {answer:?}"))
            .clone();
        assert!(
            !labels.is_empty(),
            "case {case:?}: a route over the grid travels named roads, so it carries labels"
        );

        let mut numbered_labels = 0;
        for (index, label) in labels.iter().enumerate() {
            let case = &format!("{case} / label {index}");
            let by_number = label["RouteNumber"].is_object();
            let by_name = label["RoadName"].is_object();
            assert!(
                by_number != by_name,
                "case {case:?}: a label identifies the road one way, got {label:?}"
            );
            let identity = if by_number {
                numbered_labels += 1;
                &label["RouteNumber"]
            } else {
                &label["RoadName"]
            };
            assert!(
                identity["Value"].as_str().is_some_and(|value| !value.is_empty()),
                "case {case:?}: Value is required on the label, got {label:?}"
            );
            assert_eq!(
                identity["Language"], json!(tag),
                "case {case:?}: the label declares the language it is rendered in, got {label:?}"
            );
            if by_number {
                // The network numbers its motorways and trunk roads; the number is not the
                // road's full name, which is what the other variant carries.
                let value = identity["Value"].as_str().unwrap_or_default();
                assert!(
                    value.starts_with('M') && value.len() > 1,
                    "case {case:?}: a route number names the numbered road, got {value:?}"
                );
            }
        }
        assert_eq!(
            numbered_labels > 0, numbered,
            "case {case:?}: expected a numbered road among the labels={numbered}, got {labels:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// PricingBucket
// ---------------------------------------------------------------------------

/// `PricingBucket` is the pricing TIER the call is charged in — never the name of the
/// request class or of the operation that served it, which no caller could bucket a bill
/// by.
///
/// Rows span requests that differ in everything the response varies over (travel mode,
/// waypoints, alternatives, step detail, features), because the tier is a property of the
/// query, not of its shape.
#[tokio::test]
async fn the_pricing_bucket_is_a_tier_and_not_a_request_class() {
    let router = routes_service();
    let expected = service_com_amazonaws_georoutes_routesservice::state::PRICING_BUCKET_CORE;
    let cases: [(&str, Value); 4] = [
        ("a direct route", route_request([-123.1, 49.28], [-123.12, 49.26], "Car")),
        (
            "a truck route with travel-mode options",
            json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.12, 49.26],
                "TravelMode": "Truck",
                "TravelModeOptions": { "Truck": { "GrossWeight": 7000 } },
            }),
        ),
        (
            "a route with no alternatives asked for",
            json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.12, 49.26],
                "MaxAlternatives": 0,
            }),
        ),
        (
            "a walking route with the step instructions",
            json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.103, 49.277],
                "TravelMode": "Pedestrian",
                "TravelStepType": "TurnByTurn",
                "LegAdditionalFeatures": ["Summary", "TravelStepInstructions"],
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, answer, bucket) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            bucket.as_deref(),
            Some(expected),
            "case {case:?}: expected the {expected:?} pricing tier, got {bucket:?}"
        );
        for not_a_tier in ["RoutesRequest", "CalculateRoutes", "CalculateRoutesRequest"] {
            assert_ne!(
                bucket.as_deref(),
                Some(not_a_tier),
                "case {case:?}: the pricing bucket must not be the request or operation class"
            );
        }
    }
}
