// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the geo-routes computation operations.
//!
//! Every test drives the service the way the emulator does: a buffered HTTP request
//! through a [`MultiServiceRouter`] with this service mounted on a shared engine, so the
//! request walks the real restJson1 codec, the model's own constraint validation and the
//! handler.
//!
//! The suites are table-driven on purpose: the rule under test is never "this recorded
//! request gives this recorded number" but an invariant over a class of requests — one
//! leg per stop, a matrix cell that agrees with the direct route for its pair, a snapped
//! point within the radius it was allowed, totals that are the sum of their parts — so
//! each table spans the inputs the rule generalises over (waypoint count, travel mode,
//! objective, geometry format, radius, positions).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over one shared engine — what
/// `core/router/src/lib.rs` builds for the whole emulator, narrowed to the service under
/// test.
pub(crate) fn routes_service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, not by a service; these tests drive the
    // router directly, so they turn the check off rather than signing each request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_georoutes_routesservice::EmulatedService::new(engine),
    );
    router
}

/// POST `body` to `path` on `router`, with the SigV4 credential scope that routes a
/// request to the `geo-routes` signing name.
pub(crate) async fn post(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value, Option<String>) {
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-west-2/geo-routes/aws4_request, \
             SignedHeaders=host, Signature=deadbeef",
        )
        .header("content-type", "application/json")
        .body(Bytes::from(serde_json::to_vec(body).expect("serialize the request body")))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let pricing_bucket = response
        .headers()
        .get("x-amz-geo-pricing-bucket")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed, pricing_bucket)
}

/// Great-circle distance in metres, by the spherical law of cosines — deliberately a
/// different formula from the haversine the service computes with, so these tests check
/// the answer rather than restate the implementation.
pub(crate) fn great_circle_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (lat1, lat2) = (a[1].to_radians(), b[1].to_radians());
    let delta_lon = (b[0] - a[0]).to_radians();
    let cosine = lat1.sin() * lat2.sin() + lat1.cos() * lat2.cos() * delta_lon.cos();
    6_371_008.8 * cosine.clamp(-1.0, 1.0).acos()
}

/// `[longitude, latitude]` read off a response member.
pub(crate) fn position(value: &Value, case: &str) -> [f64; 2] {
    let values = value
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a position array, got {value:?}"));
    assert_eq!(
        values.len(),
        2,
        "case {case:?}: expected a position of 2 values, got {value:?}"
    );
    [
        values[0].as_f64().unwrap_or_else(|| panic!("case {case:?}: longitude is not a number: {value:?}")),
        values[1].as_f64().unwrap_or_else(|| panic!("case {case:?}: latitude is not a number: {value:?}")),
    ]
}

/// An integer member of a response object.
pub(crate) fn int(value: &Value, member: &str, case: &str) -> i64 {
    value[member]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: {member} is missing or not an integer in {value:?}"))
}

pub(crate) const ROUTES: &str = "/v2/routes";
pub(crate) const MATRIX: &str = "/v2/route-matrix";
pub(crate) const SNAP: &str = "/v2/snap-to-roads";
pub(crate) const OPTIMIZE: &str = "/v2/optimize-waypoints";
const ISOLINES: &str = "/v2/isolines";

// ---------------------------------------------------------------------------
// CalculateRoutes
// ---------------------------------------------------------------------------

/// One leg per pair of consecutive stops, whatever the waypoint count, travel mode,
/// objective or geometry format: the legs are derived from the submitted sequence, their
/// details match the leg type, and the summary carries a non-zero distance and duration.
#[tokio::test]
async fn calculate_routes_returns_one_leg_per_stop_for_every_shape_of_request() {
    let router = routes_service();
    let origin = [-123.1, 49.28];
    let destination = [-123.12, 49.26];
    let waypoint_pool = [[-123.11, 49.275], [-123.105, 49.27], [-123.115, 49.265]];

    // Rows differ along what the rule generalises over: how many waypoints, which travel
    // mode (so which leg type and details), which objective and which geometry format.
    /// name, waypoint count, travel mode, routing objective, leg geometry format.
    type RouteCase = (
        &'static str,
        usize,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
    );
    let cases: [RouteCase; 8] = [
        ("no waypoints, car, default format", 0, "Car", None, None),
        ("one waypoint, car", 1, "Car", None, None),
        ("two waypoints, truck, simple", 2, "Truck", None, Some("Simple")),
        ("three waypoints, scooter", 3, "Scooter", None, None),
        ("one waypoint, pedestrian", 1, "Pedestrian", None, None),
        ("two waypoints, pedestrian, shortest", 2, "Pedestrian", Some("ShortestRoute"), None),
        ("no waypoints, car, shortest, simple", 0, "Car", Some("ShortestRoute"), Some("Simple")),
        ("three waypoints, truck, fastest, flexible", 3, "Truck", Some("FastestRoute"), Some("FlexiblePolyline")),
    ];

    for (case, waypoint_count, travel_mode, objective, format) in cases {
        let waypoints: Vec<Value> = waypoint_pool[..waypoint_count]
            .iter()
            .map(|position| json!({ "Position": position }))
            .collect();
        let mut body = json!({
            "Origin": origin,
            "Destination": destination,
            "TravelMode": travel_mode,
            "Waypoints": waypoints,
        });
        if let Some(objective) = objective {
            body["OptimizeRoutingFor"] = json!(objective);
        }
        if let Some(format) = format {
            body["LegGeometryFormat"] = json!(format);
        }

        let (status, answer, bucket) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert!(
            bucket.as_deref().is_some_and(|bucket| !bucket.is_empty()),
            "case {case:?}: expected the x-amz-geo-pricing-bucket header, got {bucket:?}"
        );
        assert_eq!(
            answer["LegGeometryFormat"],
            json!(format.unwrap_or("FlexiblePolyline")),
            "case {case:?}: LegGeometryFormat must echo the effective format, got {:?}",
            answer["LegGeometryFormat"]
        );
        assert!(
            answer["Notices"].is_array(),
            "case {case:?}: Notices is required, got {:?}",
            answer["Notices"]
        );

        let routes = answer["Routes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Routes is required, got {answer:?}"));
        assert_eq!(routes.len(), 1, "case {case:?}: expected one route, got {}", routes.len());
        let route = &routes[0];
        let legs = route["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {route:?}"));
        assert_eq!(
            legs.len(),
            waypoint_count + 1,
            "case {case:?}: {waypoint_count} waypoints must give {} legs, got {}",
            waypoint_count + 1,
            legs.len()
        );
        assert!(
            route["MajorRoadLabels"].is_array(),
            "case {case:?}: MajorRoadLabels is required, got {route:?}"
        );

        let distance = int(&route["Summary"], "Distance", case);
        let duration = int(&route["Summary"], "Duration", case);
        let straight = great_circle_m(origin, destination);
        assert!(
            distance as f64 >= straight * 0.5,
            "case {case:?}: the summary distance must be of the order of the geodesic {straight:.0} m, got {distance}"
        );
        assert!(
            duration > 0,
            "case {case:?}: the summary duration must be positive, got {duration}"
        );

        let expected_type = if travel_mode == "Pedestrian" { "Pedestrian" } else { "Vehicle" };
        for (index, leg) in legs.iter().enumerate() {
            assert_eq!(
                leg["TravelMode"], json!(travel_mode),
                "case {case:?}: leg {index} must carry the requested travel mode, got {:?}",
                leg["TravelMode"]
            );
            assert_eq!(
                leg["Type"], json!(expected_type),
                "case {case:?}: leg {index} type must match the travel mode, got {:?}",
                leg["Type"]
            );
            let details = if expected_type == "Pedestrian" {
                &leg["PedestrianLegDetails"]
            } else {
                &leg["VehicleLegDetails"]
            };
            assert!(
                details.is_object(),
                "case {case:?}: leg {index} must carry the details of its type, got {leg:?}"
            );
            assert!(
                details["Departure"]["Place"]["Position"].is_array()
                    && details["Arrival"]["Place"]["Position"].is_array(),
                "case {case:?}: leg {index} must carry its departure and arrival places, got {details:?}"
            );

            match format {
                Some("Simple") => {
                    let line = leg["Geometry"]["LineString"].as_array().unwrap_or_else(|| {
                        panic!("case {case:?}: leg {index} must carry a LineString, got {:?}", leg["Geometry"])
                    });
                    assert!(
                        line.len() >= 2,
                        "case {case:?}: leg {index} LineString needs 2 positions at least, got {}",
                        line.len()
                    );
                }
                _ => {
                    let polyline = leg["Geometry"]["Polyline"].as_str().unwrap_or_else(|| {
                        panic!("case {case:?}: leg {index} must carry a Polyline, got {:?}", leg["Geometry"])
                    });
                    assert!(
                        !polyline.is_empty(),
                        "case {case:?}: leg {index} Polyline must not be empty"
                    );
                }
            }
        }

        // The first leg leaves the origin and the last one reaches the destination,
        // whatever happens in between. The place reports the SUBMITTED position in
        // `OriginalPosition` ("Position provided in the request"); `Position` is that
        // position matched onto the road network, which `the_places_of_a_leg_report_the_
        // submitted_position_and_its_match` pins.
        let first = &legs[0];
        let last = &legs[legs.len() - 1];
        let place = |leg: &Value, which: &str| -> [f64; 2] {
            let details = if expected_type == "Pedestrian" {
                &leg["PedestrianLegDetails"]
            } else {
                &leg["VehicleLegDetails"]
            };
            position(&details[which]["Place"]["OriginalPosition"], case)
        };
        assert_eq!(
            place(first, "Departure"), origin,
            "case {case:?}: the first leg must depart the submitted origin"
        );
        assert_eq!(
            place(last, "Arrival"), destination,
            "case {case:?}: the last leg must arrive at the submitted destination"
        );
    }
}

/// The two geometry formats describe the same line: decoding the Flexible Polyline of a
/// leg gives back the `Simple` line string of the same request.
/// The Flexible Polyline the service returns decodes back to the `LineString` it returns
/// for the same request: the two geometry formats are two renderings of one line.
///
/// Decoded by the reader below — written from the format's specification and independent
/// of the encoder under test — and the encoder is pinned against the format's own
/// published example in `the_published_flexible_polyline_example_round_trips`.
#[tokio::test]
async fn the_flexible_polyline_of_a_leg_decodes_to_its_simple_line_string() {
    let router = routes_service();
    let cases = [
        ("short hop", [-123.1, 49.28], [-123.105, 49.277]),
        ("longer hop", [-123.1, 49.28], [-123.16, 49.24]),
        ("southern hemisphere", [151.21, -33.87], [151.19, -33.85]),
    ];

    for (case, origin, destination) in cases {
        let request = |format: &str| {
            json!({
                "Origin": origin,
                "Destination": destination,
                "LegGeometryFormat": format,
            })
        };
        let (_, simple, _) = post(&router, ROUTES, &request("Simple")).await;
        let (_, flexible, _) = post(&router, ROUTES, &request("FlexiblePolyline")).await;

        let line = simple["Routes"][0]["Legs"][0]["Geometry"]["LineString"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a LineString, got {simple:?}"))
            .iter()
            .map(|value| position(value, case))
            .collect::<Vec<_>>();
        let encoded = flexible["Routes"][0]["Legs"][0]["Geometry"]["Polyline"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Polyline, got {flexible:?}"));

        let coordinates = decode_flexible_polyline(encoded)
            .unwrap_or_else(|| panic!("case {case:?}: the polyline must decode, got {encoded:?}"));
        assert_eq!(
            coordinates.len(),
            line.len(),
            "case {case:?}: the polyline must carry the same positions as the line string, got {} against {}",
            coordinates.len(),
            line.len()
        );
        for (index, ((latitude, longitude), expected)) in coordinates.iter().zip(line.iter()).enumerate() {
            assert!(
                (longitude - expected[0]).abs() < 1e-5 && (latitude - expected[1]).abs() < 1e-5,
                "case {case:?}: position {index} must round-trip, got [{longitude}, {latitude}] against {expected:?}"
            );
        }
    }
}

/// One row of [`the_published_flexible_polyline_example_encodes_to_its_published_string`]:
/// the case name, the precision, the line, and the published encoding where the format
/// publishes one.
type PolylineCase = (&'static str, u32, Vec<[f64; 2]>, Option<&'static str>);

/// The encoder is pinned to the Flexible Polyline format itself, not to this service: the
/// rows are the example published with the format (the 4-position line at precision 5
/// encoding to `BFoz5xJ67i1B1B7PzIhaxL7Y`, from the flexible-polyline specification's
/// README), plus the degenerate lines around it, so a change to the codec that still
/// round-trips through the reader below cannot pass unnoticed.
#[test]
fn the_published_flexible_polyline_example_encodes_to_its_published_string() {
    // Positions are [longitude, latitude] on the wire; the published example lists them
    // latitude-first, so they are transposed here.
    let published: Vec<[f64; 2]> = vec![
        [8.6982122, 50.1022829],
        [8.6956695, 50.1020076],
        [8.6914960, 50.1006313],
        [8.6875156, 50.0987800],
    ];
    let cases: [PolylineCase; 5] = [
        ("the published example at precision 5", 5, published.clone(), Some("BFoz5xJ67i1B1B7PzIhaxL7Y")),
        ("the empty line at precision 5", 5, Vec::new(), Some("BF")),
        ("a single position at precision 6", 6, vec![[8.6982122, 50.1022829]], None),
        ("the published example at precision 6", 6, published.clone(), None),
        (
            "a line crossing the antimeridian and the equator",
            6,
            vec![[179.999999, -0.000001], [-179.999999, 0.000001]],
            None,
        ),
    ];

    for (case, precision, positions, expected) in cases {
        let encoded = encode(&positions, precision)
            .unwrap_or_else(|| panic!("case {case:?}: the positions must encode, got None"));
        if let Some(expected) = expected {
            assert_eq!(
                encoded, expected,
                "case {case:?}: expected the published encoding {expected:?}, got {encoded:?}"
            );
        }
        let decoded = decode_flexible_polyline(&encoded)
            .unwrap_or_else(|| panic!("case {case:?}: {encoded:?} must decode"));
        assert_eq!(
            decoded.len(),
            positions.len(),
            "case {case:?}: expected {} positions back, got {}",
            positions.len(),
            decoded.len()
        );
        let tolerance = 10_f64.powi(-i32::try_from(precision).expect("a small precision")) * 0.75;
        for (index, ((latitude, longitude), original)) in decoded.iter().zip(positions.iter()).enumerate() {
            assert!(
                (longitude - original[0]).abs() <= tolerance && (latitude - original[1]).abs() <= tolerance,
                "case {case:?}: position {index} must round-trip within {tolerance}, got [{longitude}, {latitude}] against {original:?}"
            );
        }
    }
}

/// The service's own encoder, reached directly so the codec can be checked against the
/// format's published vector rather than only through a response.
fn encode(positions: &[[f64; 2]], precision: u32) -> Option<String> {
    service_com_amazonaws_georoutes_routesservice::state::encode_flexible_polyline_at(positions, precision)
}

/// The 64 characters of the Flexible Polyline encoding, in value order, from the format's
/// specification.
const POLYLINE_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A Flexible Polyline read back as `(latitude, longitude)` pairs — written from the
/// format's specification (header of two unsigned varints, then zig-zag varint deltas,
/// latitude first, five bits per character with bit 6 as the continuation flag), so it is
/// an independent reader rather than the encoder run backwards. `None` when the string is
/// not a 2D polyline of this format.
fn decode_flexible_polyline(encoded: &str) -> Option<Vec<(f64, f64)>> {
    let mut values = Vec::new();
    let mut accumulator: u64 = 0;
    let mut shift = 0_u32;
    for byte in encoded.bytes() {
        let digit = POLYLINE_ALPHABET.iter().position(|candidate| *candidate == byte)? as u64;
        accumulator |= (digit & 0x1F).checked_shl(shift)?;
        if digit & 0x20 == 0 {
            values.push(accumulator);
            accumulator = 0;
            shift = 0;
        } else {
            shift += 5;
        }
    }

    let mut values = values.into_iter();
    // The format's version, then the header: precision in the low nibble, the third
    // dimension in bits 4..7 (absent here) and its precision above it.
    if values.next()? != 1 {
        return None;
    }
    let header = values.next()?;
    if (header >> 4) & 0x07 != 0 {
        return None;
    }
    let scale = 10_f64.powi(i32::try_from(header & 0x0F).ok()?);

    let mut out = Vec::new();
    let (mut latitude, mut longitude) = (0_i64, 0_i64);
    loop {
        let Some(delta_latitude) = values.next() else {
            return Some(out);
        };
        let delta_longitude = values.next()?;
        latitude = latitude.checked_add(zigzag(delta_latitude))?;
        longitude = longitude.checked_add(zigzag(delta_longitude))?;
        out.push((latitude as f64 / scale, longitude as f64 / scale));
    }
}

/// A zig-zag varint read back as a signed integer: the low bit carries the sign.
fn zigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// The same request twice gives the same route: the model is deterministic and the memo
/// in state cannot change an answer.
#[tokio::test]
async fn an_identical_request_returns_an_identical_route() {
    let router = routes_service();
    let cases = [
        ("no waypoints", vec![]),
        ("one waypoint", vec![json!({ "Position": [-123.11, 49.27] })]),
        (
            "two waypoints",
            vec![
                json!({ "Position": [-123.11, 49.27] }),
                json!({ "Position": [-123.115, 49.265] }),
            ],
        ),
    ];

    for (case, waypoints) in cases {
        let body = json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.12, 49.26],
            "Waypoints": waypoints,
            "DepartureTime": "2026-04-22T17:57:24Z",
        });
        let (_, first, _) = post(&router, ROUTES, &body).await;
        let (_, second, _) = post(&router, ROUTES, &body).await;
        assert_eq!(
            first["Routes"], second["Routes"],
            "case {case:?}: the same request must compute the same route, got {:?} then {:?}",
            first["Routes"], second["Routes"]
        );
    }
}

/// Alternatives are genuinely different routes, and there are never more than were asked
/// for.
#[tokio::test]
async fn max_alternatives_bounds_the_routes_and_each_one_differs() {
    let router = routes_service();
    for requested in [0_i64, 1, 2, 3, 5] {
        let case = format!("MaxAlternatives {requested}");
        let body = json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.16, 49.24],
            "MaxAlternatives": requested,
        });
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let routes = answer["Routes"].as_array().expect("Routes is required");
        assert!(
            routes.len() <= (requested as usize) + 1,
            "case {case:?}: at most {} routes may be returned, got {}",
            requested + 1,
            routes.len()
        );
        let distances: Vec<i64> = routes
            .iter()
            .map(|route| int(&route["Summary"], "Distance", &case))
            .collect();
        let mut unique = distances.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            distances.len(),
            "case {case:?}: every alternative must be a different route, got distances {distances:?}"
        );
    }

    // The model pins the count to 0..5, so a sixth alternative is refused at the
    // boundary.
    let (status, answer, _) = post(
        &router,
        ROUTES,
        &json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.16, 49.24],
            "MaxAlternatives": 6,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"MaxAlternatives 6\": past the modeled range must be refused, got {status} with {answer:?}"
    );
}

/// A `PassThrough` waypoint "is passed through and doesn't split the route into different
/// legs": it is reported inside the leg that passes it instead.
#[tokio::test]
async fn a_pass_through_waypoint_does_not_split_the_route_into_legs() {
    let router = routes_service();
    let cases: [(&str, [bool; 2], usize, usize); 4] = [
        ("both stops", [false, false], 3, 0),
        ("first passes through", [true, false], 2, 1),
        ("second passes through", [false, true], 2, 1),
        ("both pass through", [true, true], 1, 2),
    ];

    for (case, pass_through, expected_legs, expected_pass_through) in cases {
        let body = json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.12, 49.26],
            "Waypoints": [
                json!({ "Position": [-123.105, 49.275], "PassThrough": pass_through[0] }),
                json!({ "Position": [-123.11, 49.27], "PassThrough": pass_through[1] }),
            ],
        });
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let legs = answer["Routes"][0]["Legs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Legs is required, got {answer:?}"));
        assert_eq!(
            legs.len(), expected_legs,
            "case {case:?}: expected {expected_legs} legs, got {}",
            legs.len()
        );
        let passed: usize = legs
            .iter()
            .map(|leg| {
                leg["VehicleLegDetails"]["PassThroughWaypoints"]
                    .as_array()
                    .map_or(0, Vec::len)
            })
            .sum();
        assert_eq!(
            passed, expected_pass_through,
            "case {case:?}: expected {expected_pass_through} pass-through waypoints across the legs, got {passed}"
        );
    }
}

/// The leg clock walks forward from `DepartureTime`, and an `ArrivalTime` request lands
/// exactly on the arrival it asked for.
#[tokio::test]
async fn the_leg_times_follow_the_time_the_request_pins() {
    let router = routes_service();
    let waypoints = json!([{ "Position": [-123.11, 49.27] }]);

    let (status, departing, _) = post(
        &router,
        ROUTES,
        &json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.12, 49.26],
            "Waypoints": waypoints,
            "DepartureTime": "2026-04-22T17:57:24Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "a DepartureTime request must succeed, got {status} with {departing:?}");
    let legs = departing["Routes"][0]["Legs"].as_array().expect("Legs is required");
    assert_eq!(
        legs[0]["VehicleLegDetails"]["Departure"]["Time"],
        json!("2026-04-22T17:57:24Z"),
        "case \"departure\": the first leg must depart at the requested time, got {:?}",
        legs[0]["VehicleLegDetails"]["Departure"]["Time"]
    );
    let mut previous = String::new();
    for (index, leg) in legs.iter().enumerate() {
        let departure = leg["VehicleLegDetails"]["Departure"]["Time"]
            .as_str()
            .unwrap_or_else(|| panic!("leg {index} must carry a departure time"))
            .to_string();
        let arrival = leg["VehicleLegDetails"]["Arrival"]["Time"]
            .as_str()
            .unwrap_or_else(|| panic!("leg {index} must carry an arrival time"))
            .to_string();
        assert!(
            departure < arrival,
            "case \"departure\": leg {index} must arrive after it departs, got {departure} then {arrival}"
        );
        assert!(
            previous <= departure,
            "case \"departure\": leg {index} must depart no earlier than the leg before, got {previous} then {departure}"
        );
        previous = arrival;
    }

    let (status, arriving, _) = post(
        &router,
        ROUTES,
        &json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.12, 49.26],
            "Waypoints": waypoints,
            "ArrivalTime": "2026-04-22T18:57:24Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "an ArrivalTime request must succeed, got {status} with {arriving:?}");
    let legs = arriving["Routes"][0]["Legs"].as_array().expect("Legs is required");
    assert_eq!(
        legs[legs.len() - 1]["VehicleLegDetails"]["Arrival"]["Time"],
        json!("2026-04-22T18:57:24Z"),
        "case \"arrival\": the last leg must arrive at the requested time, got {:?}",
        legs[legs.len() - 1]["VehicleLegDetails"]["Arrival"]["Time"]
    );

    // The clock also has to hold at the published bounds of `StopDuration` (0..49999) and
    // past them.
    for (case, stop_duration, expected) in [
        ("no stop", 0_i64, 200_u16),
        ("at the stop maximum", 49_999, 200),
        ("past the stop maximum", 50_000, 400),
    ] {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.12, 49.26],
                "Waypoints": [{ "Position": [-123.11, 49.27], "StopDuration": stop_duration }],
                "ArrivalTime": "2026-04-22T18:57:24Z",
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
        if expected == 200 {
            assert_eq!(
                answer["Routes"][0]["Legs"][1]["VehicleLegDetails"]["Arrival"]["Time"],
                json!("2026-04-22T18:57:24Z"),
                "case {case:?}: the arrival must still land on the requested time, got {answer:?}"
            );
        }
    }
}

/// The leg summary summarises the leg that was computed, so every leg carries it whatever
/// the request asked for; a `LegAdditionalFeature` this slice does not compute is still
/// declined rather than answered with an empty value.
#[tokio::test]
async fn the_leg_summary_is_reported_for_every_leg() {
    let router = routes_service();
    // Rows span the feature list: none at all, the summary itself, the step instructions
    // (which ask for other detail entirely), and a feature the road model has no data for.
    let cases: [(&str, Value, u16); 5] = [
        ("no features", json!([]), 200),
        ("summary asked for", json!(["Summary"]), 200),
        ("the step instructions instead", json!(["TravelStepInstructions"]), 200),
        ("a feature this slice does not compute", json!(["TypicalDuration"]), 501),
        ("summary beside one it does not compute", json!(["Summary", "Tolls"]), 501),
    ];

    for (case, features, expected) in cases {
        let body = json!({
            "Origin": [-123.1, 49.28],
            "Destination": [-123.12, 49.26],
            "LegAdditionalFeatures": features,
        });
        let (status, answer, _) = post(&router, ROUTES, &body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
        if expected != 200 {
            continue;
        }
        let summary = &answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Summary"];
        assert!(
            summary.is_object(),
            "case {case:?}: the leg summary is reported for every leg, got {summary:?}"
        );
        assert!(
            int(&summary["Overview"], "Distance", case) > 0
                && int(&summary["Overview"], "Duration", case) > 0,
            "case {case:?}: the leg summary must carry the computed distance and duration, got {summary:?}"
        );
        assert_eq!(
            int(&summary["TravelOnly"], "Duration", case),
            int(&summary["Overview"], "Duration", case),
            "case {case:?}: nothing but travel happens on a leg with no stop at its end, got {summary:?}"
        );
    }
}

/// An avoidance this road model cannot satisfy is reported with the model's own violation
/// notice, which is what "a violated avoidance produces a notice for the violation"
/// means; an avoidance of something the network does not carry is satisfied silently.
#[tokio::test]
async fn a_violated_avoidance_is_reported_as_a_notice() {
    let router = routes_service();
    // A run along a trunk parallel of the network — a controlled-access road — taken as the
    // shortest (direct) route, so the computed path stays on it: avoiding controlled-access
    // highways cannot be satisfied for it.
    let crossing = json!({
        "Origin": [-123.1, 49.3],
        "Destination": [-123.0, 49.3],
        "OptimizeRoutingFor": "ShortestRoute",
        "Avoid": { "ControlledAccessHighways": true },
    });
    let (status, answer, _) = post(&router, ROUTES, &crossing).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let notices = answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Notices"]
        .as_array()
        .unwrap_or_else(|| panic!("the leg notices are required, got {answer:?}"));
    assert_eq!(
        notices.len(), 1,
        "case \"violated\": the violated avoidance must be reported once, got {notices:?}"
    );
    assert_eq!(
        notices[0]["Code"], json!("ViolatedAvoidControlledAccessHighway"),
        "case \"violated\": the model's own code must name the violation, got {notices:?}"
    );

    // Nothing on this network is a toll road or a ferry, so avoiding them is satisfied and
    // there is nothing to report.
    let (status, satisfied, _) = post(
        &router,
        ROUTES,
        &json!({
            "Origin": [-123.1, 49.3],
            "Destination": [-123.0, 49.3],
            "OptimizeRoutingFor": "ShortestRoute",
            "Avoid": { "TollRoads": true, "Ferries": true },
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {satisfied:?}");
    assert!(
        satisfied["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Notices"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "case \"satisfied\": an avoidance the network satisfies reports nothing, got {:?}",
        satisfied["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Notices"]
    );
}

/// The reported distance is the length of the geometry the same response carries: a client
/// that measures the returned line gets the number beside it.
#[tokio::test]
async fn the_reported_distance_is_the_length_of_the_returned_geometry() {
    let router = routes_service();
    let cases: [(&str, [f64; 2], [f64; 2], &str); 5] = [
        ("short, fastest", [-123.1, 49.28], [-123.105, 49.277], "FastestRoute"),
        ("short, shortest", [-123.1, 49.28], [-123.105, 49.277], "ShortestRoute"),
        ("long, fastest", [-123.1, 49.28], [-123.2, 49.2], "FastestRoute"),
        ("long, shortest", [-123.1, 49.28], [-123.2, 49.2], "ShortestRoute"),
        ("southern hemisphere", [151.21, -33.87], [151.19, -33.85], "FastestRoute"),
    ];

    for (case, origin, destination, objective) in cases {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": origin,
                "Destination": destination,
                "OptimizeRoutingFor": objective,
                "LegGeometryFormat": "Simple",
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let line: Vec<[f64; 2]> = answer["Routes"][0]["Legs"][0]["Geometry"]["LineString"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a LineString, got {answer:?}"))
            .iter()
            .map(|value| position(value, case))
            .collect();
        let measured: f64 = line.windows(2).map(|pair| great_circle_m(pair[0], pair[1])).sum();
        let reported = int(&answer["Routes"][0]["Summary"], "Distance", case) as f64;
        assert!(
            (measured - reported).abs() <= measured.max(1.0) * 0.01,
            "case {case:?}: the reported distance must be the length of the geometry {measured:.1} m, got {reported}"
        );
    }
}

/// A route between two positions that are the same place costs nothing and carries that
/// pair as its geometry.
#[tokio::test]
async fn a_route_between_coincident_positions_costs_nothing() {
    let router = routes_service();
    let cases = [
        ("northern hemisphere", [-123.1, 49.28]),
        ("southern hemisphere", [151.21, -33.87]),
        ("on the equator", [0.0, 0.0]),
    ];
    for (case, position) in cases {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({ "Origin": position, "Destination": position, "LegGeometryFormat": "Simple" }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let summary = &answer["Routes"][0]["Summary"];
        assert_eq!(
            int(summary, "Distance", case), 0,
            "case {case:?}: a route to the same place covers no distance, got {summary:?}"
        );
        assert_eq!(
            int(summary, "Duration", case), 0,
            "case {case:?}: a route to the same place takes no time, got {summary:?}"
        );
        let line = answer["Routes"][0]["Legs"][0]["Geometry"]["LineString"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a LineString, got {answer:?}"));
        assert_eq!(
            line.len(), 2,
            "case {case:?}: the geometry is the submitted pair, got {line:?}"
        );
    }
}

/// A longer separation costs more: distance and duration rise with the geodesic between
/// the two positions, so they are computed from the geometry rather than fixed.
#[tokio::test]
async fn the_cost_of_a_route_rises_with_the_distance_between_its_positions() {
    let router = routes_service();
    let origin = [-123.1, 49.28];
    let destinations = [
        ("1 km east", [-123.086, 49.28]),
        ("3 km east", [-123.059, 49.28]),
        ("8 km east", [-123.0, 49.28]),
    ];

    let mut last: Option<(String, i64, i64)> = None;
    for (case, destination) in destinations {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({ "Origin": origin, "Destination": destination }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let summary = &answer["Routes"][0]["Summary"];
        let (distance, duration) = (int(summary, "Distance", case), int(summary, "Duration", case));
        if let Some((previous_case, previous_distance, previous_duration)) = &last {
            assert!(
                distance > *previous_distance,
                "case {case:?}: must be further than {previous_case:?}, got {distance} against {previous_distance}"
            );
            assert!(
                duration > *previous_duration,
                "case {case:?}: must take longer than {previous_case:?}, got {duration} against {previous_duration}"
            );
        }
        last = Some((case.to_string(), distance, duration));
    }
}

/// The travel mode's speed is what turns a distance into a duration, so a slower mode
/// takes longer over the same pair of positions.
#[tokio::test]
async fn a_slower_travel_mode_takes_longer_over_the_same_pair() {
    let router = routes_service();
    // Ordered fastest to slowest by the speeds held in state.
    let modes = ["Car", "Truck", "Scooter", "Pedestrian"];
    let mut last: Option<(&str, i64)> = None;
    for mode in modes {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.12, 49.26],
                "TravelMode": mode,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {mode:?}: expected 200, got {status} with {answer:?}");
        let duration = int(&answer["Routes"][0]["Summary"], "Duration", mode);
        if let Some((previous_mode, previous_duration)) = last {
            assert!(
                duration >= previous_duration,
                "case {mode:?}: must not be quicker than {previous_mode:?}, got {duration} against {previous_duration}"
            );
        }
        last = Some((mode, duration));
    }
}

/// `ShortestRoute` is never the longer way and `FastestRoute` is never the slower one: the
/// objective is applied, not ignored. Where the graph of a pair offers a trade at all — a
/// quicker way through it that is not also its shortest — the trade is both ways round: the
/// quicker way costs distance and the shorter way costs time.
#[tokio::test]
async fn the_routing_objective_trades_distance_against_time() {
    let router = routes_service();
    let cases: [(&str, [f64; 2], [f64; 2]); 4] = [
        ("short pair", [-123.1, 49.28], [-123.12, 49.26]),
        ("long pair", [-123.1, 49.28], [-123.2, 49.2]),
        ("a pair across a county", [-123.1, 49.28], [-123.6, 49.9]),
        (
            "a pair on no road of its own",
            [-123.1037, 49.2831],
            [-123.2411, 49.1877],
        ),
    ];
    let mut traded = Vec::new();
    for (case, origin, destination) in cases {
        let ask = |objective: &str| {
            json!({ "Origin": origin, "Destination": destination, "OptimizeRoutingFor": objective })
        };
        let (_, fastest, _) = post(&router, ROUTES, &ask("FastestRoute")).await;
        let (_, shortest, _) = post(&router, ROUTES, &ask("ShortestRoute")).await;
        let fast = &fastest["Routes"][0]["Summary"];
        let short = &shortest["Routes"][0]["Summary"];
        let (fast_distance, fast_duration) = (int(fast, "Distance", case), int(fast, "Duration", case));
        let (short_distance, short_duration) = (int(short, "Distance", case), int(short, "Duration", case));
        assert!(
            short_distance <= fast_distance,
            "case {case:?}: ShortestRoute must not be the longer one, got {short_distance} against {fast_distance}"
        );
        assert!(
            fast_duration <= short_duration,
            "case {case:?}: FastestRoute must not be the slower one, got {fast_duration} against {short_duration}"
        );
        if fast_duration < short_duration {
            // The quicker way is quicker because it travels faster roads, which is what the
            // objective asked for; it is never also the shorter one, which is what the
            // other objective asks for.
            traded.push(case);
        }
    }
    assert!(
        !traded.is_empty(),
        "the objective must reach the route where the graph offers a quicker way than the \
         shortest one, and no pair of the table traded"
    );
}

// ---------------------------------------------------------------------------
// CalculateRouteMatrix
// ---------------------------------------------------------------------------

/// The matrix has one row per origin and one entry per destination, and every entry
/// agrees with the direct route for its pair — both are costed by the one cost function.
#[tokio::test]
async fn every_matrix_entry_agrees_with_the_direct_route_for_its_pair() {
    let router = routes_service();
    let pool = [
        [-123.1, 49.28],
        [-123.2, 49.3],
        [-123.05, 49.25],
        [-123.12, 49.26],
    ];
    let cases: [(&str, usize, usize, &str, &str); 5] = [
        ("1x1 car fastest", 1, 1, "Car", "FastestRoute"),
        ("2x1 car fastest", 2, 1, "Car", "FastestRoute"),
        ("2x3 truck fastest", 2, 3, "Truck", "FastestRoute"),
        ("3x2 pedestrian shortest", 3, 2, "Pedestrian", "ShortestRoute"),
        ("4x4 scooter shortest", 4, 4, "Scooter", "ShortestRoute"),
    ];

    for (case, origin_count, destination_count, travel_mode, objective) in cases {
        let origins: Vec<Value> = pool[..origin_count]
            .iter()
            .map(|position| json!({ "Position": position }))
            .collect();
        let destinations: Vec<Value> = pool[..destination_count]
            .iter()
            .rev()
            .map(|position| json!({ "Position": position }))
            .collect();
        let (status, answer, bucket) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": origins,
                "Destinations": destinations,
                "TravelMode": travel_mode,
                "OptimizeRoutingFor": objective,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert!(
            bucket.is_some(),
            "case {case:?}: expected the x-amz-geo-pricing-bucket header"
        );
        assert_eq!(
            answer["RoutingBoundary"], json!({ "Unbounded": true }),
            "case {case:?}: the default routing boundary is Unbounded set to true, got {:?}",
            answer["RoutingBoundary"]
        );
        assert_eq!(
            answer["ErrorCount"], json!(0),
            "case {case:?}: every pair is routable here, got {:?}",
            answer["ErrorCount"]
        );

        let matrix = answer["RouteMatrix"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: RouteMatrix is required, got {answer:?}"));
        assert_eq!(
            matrix.len(), origin_count,
            "case {case:?}: expected one row per origin, got {}",
            matrix.len()
        );
        for (i, row) in matrix.iter().enumerate() {
            let row = row
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: row {i} must be a list, got {row:?}"));
            assert_eq!(
                row.len(), destination_count,
                "case {case:?}: row {i} must have one entry per destination, got {}",
                row.len()
            );
            for (j, entry) in row.iter().enumerate() {
                assert!(
                    entry["Error"].is_null(),
                    "case {case:?}: entry [{i}][{j}] must not carry an error, got {entry:?}"
                );
                let origin = position(&origins[i]["Position"], case);
                let destination = position(&destinations[j]["Position"], case);
                let (_, direct, _) = post(
                    &router,
                    ROUTES,
                    &json!({
                        "Origin": origin,
                        "Destination": destination,
                        "TravelMode": travel_mode,
                        "OptimizeRoutingFor": objective,
                    }),
                )
                .await;
                let summary = &direct["Routes"][0]["Summary"];
                assert_eq!(
                    int(entry, "Distance", case),
                    int(summary, "Distance", case),
                    "case {case:?}: entry [{i}][{j}] distance must match the direct route, got {entry:?} against {summary:?}"
                );
                assert_eq!(
                    int(entry, "Duration", case),
                    int(summary, "Duration", case),
                    "case {case:?}: entry [{i}][{j}] duration must match the direct route, got {entry:?} against {summary:?}"
                );
            }
        }
    }
}

/// A pair outside the routing boundary is not routed: its entry carries the unqualified
/// out-of-bounds code, whichever endpoint the boundary left out, and `ErrorCount` counts
/// exactly those entries.
#[tokio::test]
async fn matrix_entries_outside_the_routing_boundary_carry_an_error() {
    let router = routes_service();
    let near = [-123.1, 49.28];
    let far = [-100.0, 40.0];

    // Rows span the boundary geometries the model offers, and both sides of the
    // boundary.
    let cases: [(&str, Value, Value, Value, &str, i64); 5] = [
        (
            "circle, destination outside",
            json!({ "Geometry": { "Circle": { "Center": near, "Radius": 5000.0 } } }),
            json!([{ "Position": near }]),
            json!([{ "Position": near }, { "Position": far }]),
            "OutOfBounds",
            1,
        ),
        (
            "circle, origin outside",
            json!({ "Geometry": { "Circle": { "Center": near, "Radius": 5000.0 } } }),
            json!([{ "Position": far }]),
            json!([{ "Position": near }]),
            "OutOfBounds",
            1,
        ),
        (
            "bounding box, destination outside",
            json!({ "Geometry": { "BoundingBox": [-123.3, 49.1, -123.0, 49.4] } }),
            json!([{ "Position": near }]),
            json!([{ "Position": far }, { "Position": near }]),
            "OutOfBounds",
            1,
        ),
        (
            "polygon, destination outside",
            json!({
                "Geometry": {
                    "Polygon": [[
                        [-123.3, 49.1], [-123.0, 49.1], [-123.0, 49.4], [-123.3, 49.4], [-123.3, 49.1]
                    ]]
                }
            }),
            json!([{ "Position": near }]),
            json!([{ "Position": far }]),
            "OutOfBounds",
            1,
        ),
        (
            "unbounded, nothing outside",
            json!({ "Unbounded": true }),
            json!([{ "Position": near }]),
            json!([{ "Position": far }]),
            "",
            0,
        ),
    ];

    for (case, boundary, origins, destinations, code, expected_errors) in cases {
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": origins,
                "Destinations": destinations,
                "RoutingBoundary": boundary,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["ErrorCount"], json!(expected_errors),
            "case {case:?}: expected {expected_errors} error entries, got {:?} in {:?}",
            answer["ErrorCount"], answer["RouteMatrix"]
        );
        let errors: Vec<&Value> = answer["RouteMatrix"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: RouteMatrix is required, got {answer:?}"))
            .iter()
            .flat_map(|row| row.as_array().map(Vec::as_slice).unwrap_or_default())
            .filter(|entry| !entry["Error"].is_null())
            .collect();
        assert_eq!(
            errors.len() as i64, expected_errors,
            "case {case:?}: ErrorCount must equal the entries carrying an error, got {errors:?}"
        );
        for entry in errors {
            assert_eq!(
                entry["Error"], json!(code),
                "case {case:?}: expected the error code {code}, got {entry:?}"
            );
            assert_eq!(
                int(entry, "Distance", case), 0,
                "case {case:?}: an unroutable entry carries no distance, got {entry:?}"
            );
        }
    }
}

/// `AutoCircle` is a request-only setting: the circle derived from the submitted
/// waypoints comes back as `Circle`, and it covers them.
#[tokio::test]
async fn an_auto_circle_boundary_is_reported_as_the_circle_it_derived() {
    let router = routes_service();
    let cases: [(&str, i64, i64, Vec<[f64; 2]>); 3] = [
        ("no margin", 0, 0, vec![[-123.1, 49.28], [-123.2, 49.3]]),
        ("with margin", 1_000, 0, vec![[-123.1, 49.28], [-123.2, 49.3]]),
        ("capped radius", 0, 500, vec![[-123.1, 49.28], [-123.4, 49.5]]),
    ];

    for (case, margin, max_radius, waypoints) in cases {
        let origins: Vec<Value> = waypoints.iter().map(|p| json!({ "Position": p })).collect();
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": origins,
                "Destinations": [{ "Position": waypoints[0] }],
                "RoutingBoundary": {
                    "Geometry": { "AutoCircle": { "Margin": margin, "MaxRadius": max_radius } }
                },
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let circle = &answer["RoutingBoundary"]["Geometry"]["Circle"];
        assert!(
            circle.is_object(),
            "case {case:?}: the derived Circle must be reported, got {:?}",
            answer["RoutingBoundary"]
        );
        assert!(
            answer["RoutingBoundary"]["Geometry"]["AutoCircle"].is_null(),
            "case {case:?}: AutoCircle is a request-only setting, got {:?}",
            answer["RoutingBoundary"]
        );
        let center = position(&circle["Center"], case);
        let radius = circle["Radius"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: Radius is required, got {circle:?}"));
        let reach = waypoints
            .iter()
            .map(|waypoint| great_circle_m(center, *waypoint))
            .fold(0.0_f64, f64::max);
        if max_radius == 0 {
            assert!(
                radius + 1.0 >= reach,
                "case {case:?}: the derived circle must reach the furthest waypoint {reach:.0} m away, got {radius}"
            );
        } else {
            assert!(
                radius <= (max_radius + margin) as f64,
                "case {case:?}: the derived radius must honour MaxRadius plus Margin, got {radius}"
            );
        }
    }
}

/// The documented matrix limits are enforced, and they depend on the routing boundary.
#[tokio::test]
async fn the_matrix_size_limits_depend_on_the_routing_boundary() {
    let router = routes_service();
    let position = [-123.1, 49.28];
    let list = |count: usize| -> Value {
        Value::Array((0..count).map(|_| json!({ "Position": position })).collect())
    };

    // With `Unbounded` the documented limits are 15 origins, 100 destinations and 100
    // pairs; a geometry raises them to 500 / 500 / 160,000.
    let cases: [(&str, Value, usize, usize, u16); 6] = [
        ("unbounded at the origin limit", json!({ "Unbounded": true }), 15, 1, 200),
        ("unbounded past the origin limit", json!({ "Unbounded": true }), 16, 1, 400),
        ("unbounded at the pair limit", json!({ "Unbounded": true }), 10, 10, 200),
        ("unbounded past the pair limit", json!({ "Unbounded": true }), 11, 10, 400),
        (
            "geometry past the unbounded origin limit",
            json!({ "Geometry": { "Circle": { "Center": position, "Radius": 50000.0 } } }),
            16,
            1,
            200,
        ),
        (
            "geometry past the unbounded pair limit",
            json!({ "Geometry": { "Circle": { "Center": position, "Radius": 50000.0 } } }),
            11,
            10,
            200,
        ),
    ];

    for (case, boundary, origins, destinations, expected) in cases {
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": list(origins),
                "Destinations": list(destinations),
                "RoutingBoundary": boundary,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
        if expected == 400 {
            assert_eq!(
                answer["__type"], json!("ValidationException"),
                "case {case:?}: a refused size is a ValidationException, got {answer:?}"
            );
        }
    }
}

/// A routing boundary that sets more than one thing, or nothing this service can read, is
/// refused rather than silently ignored.
#[tokio::test]
async fn a_contradictory_routing_boundary_is_refused() {
    let router = routes_service();
    let position = [-123.1, 49.28];
    let cases: [(&str, Value, u16); 9] = [
        (
            "geometry and unbounded together",
            json!({ "Unbounded": true, "Geometry": { "Circle": { "Center": position, "Radius": 1000.0 } } }),
            400,
        ),
        (
            "two geometries",
            json!({ "Geometry": { "Circle": { "Center": position, "Radius": 1000.0 }, "BoundingBox": [-124.0, 49.0, -123.0, 50.0] } }),
            400,
        ),
        (
            "radius past the documented maximum",
            json!({ "Geometry": { "Circle": { "Center": position, "Radius": 200001.0 } } }),
            400,
        ),
        (
            "auto circle past the documented maximum",
            json!({ "Geometry": { "AutoCircle": { "Margin": 100000, "MaxRadius": 150000 } } }),
            400,
        ),
        (
            "an unclosed linear ring",
            json!({ "Geometry": { "Polygon": [[[-124.0, 49.0], [-123.0, 49.0], [-123.0, 50.0], [-124.0, 50.0]]] } }),
            400,
        ),
        (
            "no geometry with Unbounded off",
            json!({ "Unbounded": false }),
            400,
        ),
        (
            "a linear ring past the accepted maximum",
            {
                let mut ring: Vec<Value> = (0..1_001)
                    .map(|index| json!([-124.0 + (index as f64) * 0.0001, 49.0]))
                    .collect();
                ring.push(json!([-124.0, 49.0]));
                json!({ "Geometry": { "Polygon": [ring] } })
            },
            400,
        ),
        (
            "a bounding box whose north edge is south of its south edge",
            json!({ "Geometry": { "BoundingBox": [-124.0, 50.0, -123.0, 49.0] } }),
            400,
        ),
        (
            "a bounding box that wraps the antimeridian",
            json!({ "Geometry": { "BoundingBox": [179.0, 49.0, -179.0, 50.0] } }),
            200,
        ),
    ];

    for (case, boundary, expected) in cases {
        let (status, answer, _) = post(
            &router,
            MATRIX,
            &json!({
                "Origins": [{ "Position": position }],
                "Destinations": [{ "Position": position }],
                "RoutingBoundary": boundary,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// SnapToRoads
// ---------------------------------------------------------------------------

/// One snapped point per trace point, at the same index, with the submitted position
/// echoed, the snapped position inside the radius the request allowed, and a confidence
/// in 0..1 derived from the offset.
#[tokio::test]
async fn snap_to_roads_answers_one_point_per_trace_point_within_the_radius() {
    let router = routes_service();
    // Deliberately half a grid step off the road network in both axes, so every point
    // has to move to snap.
    let pool = [
        [-123.10025, 49.28025],
        [-123.10075, 49.28075],
        [-123.10125, 49.28125],
        [-123.10175, 49.28175],
        [-123.10225, 49.28225],
    ];
    let cases: [(&str, usize, i64, &str, Option<&str>); 6] = [
        ("two points, default radius", 2, 0, "Car", None),
        ("three points, 50 m", 3, 50, "Car", None),
        ("three points, 300 m", 3, 300, "Car", None),
        ("five points, 10 km, simple", 5, 10_000, "Car", Some("Simple")),
        ("three points, pedestrian", 3, 300, "Pedestrian", None),
        ("two points, truck", 2, 300, "Truck", None),
    ];

    for (case, count, radius, travel_mode, format) in cases {
        let trace_points: Vec<Value> = pool[..count]
            .iter()
            .map(|position| json!({ "Position": position }))
            .collect();
        let mut body = json!({
            "TracePoints": trace_points,
            "SnapRadius": radius,
            "TravelMode": travel_mode,
        });
        if let Some(format) = format {
            body["SnappedGeometryFormat"] = json!(format);
        }

        let (status, answer, bucket) = post(&router, SNAP, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert!(bucket.is_some(), "case {case:?}: expected the pricing bucket header");
        assert_eq!(
            answer["SnappedGeometryFormat"],
            json!(format.unwrap_or("FlexiblePolyline")),
            "case {case:?}: the effective geometry format must be echoed, got {:?}",
            answer["SnappedGeometryFormat"]
        );
        assert!(
            answer["Notices"].as_array().is_some_and(Vec::is_empty),
            "case {case:?}: nothing here is worth a notice, got {:?}",
            answer["Notices"]
        );
        assert!(
            answer["SnappedGeometry"].is_object(),
            "case {case:?}: the snapped geometry must be reported, got {answer:?}"
        );

        let snapped = answer["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: SnappedTracePoints is required, got {answer:?}"));
        assert_eq!(
            snapped.len(), count,
            "case {case:?}: expected one snapped point per trace point, got {}",
            snapped.len()
        );
        // The radius the service applies: 0 means the documented default of 300 m.
        let allowed = if radius == 0 { 300.0 } else { radius as f64 };
        for (index, point) in snapped.iter().enumerate() {
            assert_eq!(
                position(&point["OriginalPosition"], case), pool[index],
                "case {case:?}: point {index} must echo the submitted position, got {point:?}"
            );
            let offset = great_circle_m(pool[index], position(&point["SnappedPosition"], case));
            assert!(
                offset <= allowed,
                "case {case:?}: point {index} must snap within {allowed} m, got {offset:.1} m"
            );
            assert!(
                offset > 0.0,
                "case {case:?}: point {index} is off the road network, so it must move, got {offset:.1} m"
            );
            let confidence = point["Confidence"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: point {index} must carry a confidence, got {point:?}"));
            assert!(
                (0.0..=1.0).contains(&confidence),
                "case {case:?}: point {index} confidence must be in 0..1, got {confidence}"
            );
            assert!(
                confidence > 0.0,
                "case {case:?}: point {index} matched a road, so its confidence must be positive, got {confidence}"
            );
        }
    }
}

/// Confidence is a property of the MATCH and not of the request: the same trace point
/// matched onto the same road is reported with the same confidence however much slack
/// `SnapRadius` left around it, and a point the matcher had to move further, or that lies
/// as near a second road, is reported with less of it.
#[tokio::test]
async fn the_confidence_of_a_match_does_not_follow_the_snap_radius() {
    let router = routes_service();
    let trace_points = json!([
        { "Position": [-123.10025, 49.28025] },
        { "Position": [-123.10075, 49.28075] }
    ]);
    let mut first: Option<(i64, f64)> = None;
    for radius in [50_i64, 100, 300, 1_000, 10_000] {
        let (status, answer, _) = post(
            &router,
            SNAP,
            &json!({ "TracePoints": trace_points, "SnapRadius": radius }),
        )
        .await;
        assert_eq!(status, 200, "case {radius:?}: expected 200, got {status} with {answer:?}");
        let confidence = answer["SnappedTracePoints"][0]["Confidence"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {radius:?}: expected a confidence, got {answer:?}"));
        if let Some((first_radius, first_confidence)) = first {
            assert_eq!(
                confidence, first_confidence,
                "case {radius:?}: the same match as at {first_radius} m must score the same, got {confidence} against {first_confidence}"
            );
        }
        first = first.or(Some((radius, confidence)));
    }
}

/// A trace point with no road inside the radius is not matched: it keeps the position it
/// was submitted with, scores no confidence, and its index is listed under a
/// `TracePointsNotMatched` notice.
#[tokio::test]
async fn a_trace_point_with_no_road_in_range_is_reported_unmatched() {
    let router = routes_service();
    let pool = [[-123.10025, 49.28025], [-123.10075, 49.28075]];
    for radius in [1_i64, 2, 5] {
        let (status, answer, _) = post(
            &router,
            SNAP,
            &json!({
                "TracePoints": [{ "Position": pool[0] }, { "Position": pool[1] }],
                "SnapRadius": radius,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {radius:?}: expected 200, got {status} with {answer:?}");
        let snapped = answer["SnappedTracePoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {radius:?}: SnappedTracePoints is required, got {answer:?}"));
        for (index, point) in snapped.iter().enumerate() {
            assert_eq!(
                position(&point["SnappedPosition"], "unmatched"), pool[index],
                "case {radius:?}: an unmatched point keeps its submitted position, got {point:?}"
            );
            assert_eq!(
                point["Confidence"], json!(0.0),
                "case {radius:?}: an unmatched point scores no confidence, got {point:?}"
            );
        }
        let notices = answer["Notices"]
            .as_array()
            .unwrap_or_else(|| panic!("case {radius:?}: Notices is required, got {answer:?}"));
        assert_eq!(
            notices.len(), 1,
            "case {radius:?}: expected one notice, got {notices:?}"
        );
        assert_eq!(
            notices[0]["Code"], json!("TracePointsNotMatched"),
            "case {radius:?}: expected the TracePointsNotMatched code, got {:?}",
            notices[0]
        );
        assert_eq!(
            notices[0]["TracePointIndexes"], json!([0, 1]),
            "case {radius:?}: the notice must name every unmatched index, got {:?}",
            notices[0]
        );
        assert!(
            answer["SnappedGeometry"].is_null(),
            "case {radius:?}: with nothing matched there is no snapped line to draw, got {:?}",
            answer["SnappedGeometry"]
        );
    }
}

// ---------------------------------------------------------------------------
// OptimizeWaypoints
// ---------------------------------------------------------------------------

/// The optimized sequence is a permutation of exactly the submitted ids, origin first and
/// destination last, its clock runs forward, and the totals are the sum over the
/// connections.
#[tokio::test]
async fn optimize_waypoints_sequences_the_submitted_ids_and_sums_its_connections() {
    let router = routes_service();
    let pool: [(&str, [f64; 2]); 4] = [
        ("bravo", [-123.2, 49.3]),
        ("alpha", [-123.11, 49.27]),
        ("charlie", [-123.05, 49.25]),
        ("delta", [-123.3, 49.35]),
    ];
    let cases: [(&str, usize, bool, &str); 6] = [
        ("no waypoints, with destination", 0, true, "FastestRoute"),
        ("one waypoint, with destination", 1, true, "FastestRoute"),
        ("three waypoints, with destination", 3, true, "FastestRoute"),
        ("three waypoints, shortest", 3, true, "ShortestRoute"),
        ("four waypoints, no destination", 4, false, "FastestRoute"),
        ("two waypoints, no destination", 2, false, "ShortestRoute"),
    ];

    for (case, waypoint_count, with_destination, objective) in cases {
        let waypoints: Vec<Value> = pool[..waypoint_count]
            .iter()
            .map(|(id, position)| json!({ "Id": id, "Position": position }))
            .collect();
        let mut body = json!({
            "Origin": [-123.1, 49.28],
            "OriginOptions": { "Id": "start" },
            "Waypoints": waypoints,
            "OptimizeSequencingFor": objective,
            "DepartureTime": "2026-04-22T08:00:00Z",
        });
        if with_destination {
            body["Destination"] = json!([-123.12, 49.26]);
            body["DestinationOptions"] = json!({ "Id": "finish" });
        }

        let (status, answer, bucket) = post(&router, OPTIMIZE, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert!(bucket.is_some(), "case {case:?}: expected the pricing bucket header");

        let optimized = answer["OptimizedWaypoints"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: OptimizedWaypoints is required, got {answer:?}"));
        let mut expected_ids: Vec<String> = vec!["start".to_string()];
        expected_ids.extend(pool[..waypoint_count].iter().map(|(id, _)| (*id).to_string()));
        if with_destination {
            expected_ids.push("finish".to_string());
        }
        let mut got_ids: Vec<String> = optimized
            .iter()
            .map(|waypoint| {
                waypoint["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: every optimized waypoint carries an Id, got {waypoint:?}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            got_ids.first().map(String::as_str), Some("start"),
            "case {case:?}: the origin comes first, got {got_ids:?}"
        );
        if with_destination {
            assert_eq!(
                got_ids.last().map(String::as_str), Some("finish"),
                "case {case:?}: the destination comes last, got {got_ids:?}"
            );
        }
        got_ids.sort();
        let mut sorted_expected = expected_ids.clone();
        sorted_expected.sort();
        assert_eq!(
            got_ids, sorted_expected,
            "case {case:?}: the sequence must be a permutation of the submitted ids, got {got_ids:?} against {sorted_expected:?}"
        );

        let connections = answer["Connections"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Connections is required, got {answer:?}"));
        assert_eq!(
            connections.len(), optimized.len() - 1,
            "case {case:?}: one connection per consecutive pair, got {} for {} waypoints",
            connections.len(),
            optimized.len()
        );

        let mut distance_total = 0;
        let mut duration_total = 0;
        let mut travel_total = 0;
        for (index, connection) in connections.iter().enumerate() {
            assert_eq!(
                connection["From"], optimized[index]["Id"],
                "case {case:?}: connection {index} must start where the sequence is, got {connection:?}"
            );
            assert_eq!(
                connection["To"], optimized[index + 1]["Id"],
                "case {case:?}: connection {index} must end at the next waypoint, got {connection:?}"
            );
            let travel = int(connection, "TravelDuration", case);
            assert!(
                travel > 0,
                "case {case:?}: connection {index} must take time to travel, got {connection:?}"
            );
            distance_total += int(connection, "Distance", case);
            duration_total += travel + int(connection, "RestDuration", case) + int(connection, "WaitDuration", case);
            travel_total += travel;
        }
        assert_eq!(
            int(&answer, "Distance", case), distance_total,
            "case {case:?}: Distance must be the sum over the connections, got {:?}",
            answer["Distance"]
        );
        assert_eq!(
            int(&answer, "Duration", case), duration_total,
            "case {case:?}: Duration must be the sum over the connections, got {:?}",
            answer["Duration"]
        );
        assert_eq!(
            int(&answer["TimeBreakdown"], "TravelDuration", case), travel_total,
            "case {case:?}: the time breakdown must account for the travel, got {:?}",
            answer["TimeBreakdown"]
        );
        assert!(
            answer["ImpedingWaypoints"].as_array().is_some_and(Vec::is_empty),
            "case {case:?}: nothing impedes this sequence, got {:?}",
            answer["ImpedingWaypoints"]
        );

        // The clock starts at the requested departure and only moves forward.
        assert_eq!(
            optimized[0]["DepartureTime"], json!("2026-04-22T08:00:00Z"),
            "case {case:?}: the origin departs at the requested time, got {:?}",
            optimized[0]["DepartureTime"]
        );
        let mut previous = String::new();
        for (index, waypoint) in optimized.iter().enumerate() {
            let departure = waypoint["DepartureTime"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: waypoint {index} must carry a departure time"))
                .to_string();
            assert!(
                previous <= departure,
                "case {case:?}: waypoint {index} must depart after the one before, got {previous} then {departure}"
            );
            assert_eq!(
                waypoint["ArrivalTime"].is_string(), index > 0,
                "case {case:?}: only the origin has no arrival time, got {waypoint:?}"
            );
            previous = departure;
        }
    }
}

/// A made-up id for a waypoint the caller left unnamed never collides with an id the
/// caller did supply: a connection names its endpoints by id, so every stop must be
/// distinguishable.
#[tokio::test]
async fn an_unnamed_waypoint_is_identified_by_its_index() {
    let router = routes_service();
    let (status, answer, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Position": [-123.2, 49.3] },
                { "Id": "named", "Position": [-123.11, 49.27] },
                { "Position": [-123.05, 49.25] }
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let mut ids: Vec<&str> = answer["OptimizedWaypoints"]
        .as_array()
        .expect("OptimizedWaypoints is required")
        .iter()
        .filter_map(|waypoint| waypoint["Id"].as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids, vec!["0", "2", "Origin", "named"],
        "an unnamed waypoint takes its index as its id, got {ids:?}"
    );

    // The caller used "0" and "Origin" itself, so the stops that need a made-up id get a
    // different one and nothing is refused.
    let (status, answer, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Position": [-123.2, 49.3] },
                { "Id": "0", "Position": [-123.11, 49.27] },
                { "Id": "Origin", "Position": [-123.05, 49.25] }
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ids the caller did not duplicate must not be refused, got {status} with {answer:?}"
    );
    let ids: Vec<&str> = answer["OptimizedWaypoints"]
        .as_array()
        .expect("OptimizedWaypoints is required")
        .iter()
        .filter_map(|waypoint| waypoint["Id"].as_str())
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(), ids.len(),
        "every stop must carry a distinct id, got {ids:?}"
    );
}

/// Two stops cannot share one id: a connection names its endpoints by id, so the answer
/// would be ambiguous.
#[tokio::test]
async fn duplicate_waypoint_ids_are_refused() {
    let router = routes_service();
    let cases: [(&str, Value, u16); 3] = [
        (
            "two waypoints with one id",
            json!([{ "Id": "same", "Position": [-123.2, 49.3] }, { "Id": "same", "Position": [-123.05, 49.25] }]),
            400,
        ),
        (
            "a waypoint sharing the origin id",
            json!([{ "Id": "start", "Position": [-123.2, 49.3] }]),
            400,
        ),
        (
            "distinct ids",
            json!([{ "Id": "one", "Position": [-123.2, 49.3] }, { "Id": "two", "Position": [-123.05, 49.25] }]),
            200,
        ),
    ];

    for (case, waypoints, expected) in cases {
        let (status, answer, _) = post(
            &router,
            OPTIMIZE,
            &json!({
                "Origin": [-123.1, 49.28],
                "OriginOptions": { "Id": "start" },
                "Waypoints": waypoints,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
    }
}

/// Service time, an appointment and a driver's rest cycle are all accounted for: the
/// waiting, resting and service seconds come out of the request's own numbers.
#[tokio::test]
async fn the_time_breakdown_accounts_for_service_rest_and_waiting() {
    let router = routes_service();

    let (status, serviced, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "a", "Position": [-123.11, 49.27], "ServiceDuration": 600 },
                { "Id": "b", "Position": [-123.05, 49.25], "ServiceDuration": 300 }
            ],
            "DepartureTime": "2026-04-22T08:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "a serviced sequence must succeed, got {status} with {serviced:?}");
    assert_eq!(
        int(&serviced["TimeBreakdown"], "ServiceDuration", "service"),
        900,
        "case \"service\": the service durations must be accounted for, got {:?}",
        serviced["TimeBreakdown"]
    );

    // An appointment far in the future is waited out at that waypoint.
    let (status, appointed, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "a", "Position": [-123.11, 49.27], "AppointmentTime": "2026-04-22T10:00:00Z" }
            ],
            "DepartureTime": "2026-04-22T08:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "an appointed sequence must succeed, got {status} with {appointed:?}");
    let wait = int(&appointed["Connections"][0], "WaitDuration", "appointment");
    assert!(
        wait > 0,
        "case \"appointment\": arriving early means waiting for the appointment, got {wait}"
    );
    assert_eq!(
        appointed["OptimizedWaypoints"][1]["DepartureTime"], json!("2026-04-22T10:00:00Z"),
        "case \"appointment\": the waypoint is left at the appointment time, got {:?}",
        appointed["OptimizedWaypoints"][1]
    );
    assert!(
        appointed["ImpedingWaypoints"].as_array().is_some_and(Vec::is_empty),
        "case \"appointment\": an appointment reached in time impedes nothing, got {:?}",
        appointed["ImpedingWaypoints"]
    );

    // An appointment already past cannot be met, so the waypoint impedes the sequence.
    let (status, missed, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "a", "Position": [-123.11, 49.27], "AppointmentTime": "2026-04-22T07:00:00Z" }
            ],
            "DepartureTime": "2026-04-22T08:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "a missed appointment still answers, got {status} with {missed:?}");
    assert_eq!(
        missed["ImpedingWaypoints"][0]["FailedConstraints"][0]["Constraint"],
        json!("AppointmentTime"),
        "case \"missed appointment\": the missed constraint must be reported, got {:?}",
        missed["ImpedingWaypoints"]
    );
    assert_eq!(
        int(&missed["Connections"][0], "WaitDuration", "missed appointment"), 0,
        "case \"missed appointment\": a past appointment is not a negative wait, got {:?}",
        missed["Connections"]
    );

    // A rest cycle whose work duration is shorter than the first hop owes a rest.
    let (status, rested, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "a", "Position": [-123.3, 49.4] },
                { "Id": "b", "Position": [-123.5, 49.5] }
            ],
            "Driver": {
                "RestCycles": {
                    "ShortCycle": { "WorkDuration": 60, "RestDuration": 900 },
                    "LongCycle": { "WorkDuration": 36000, "RestDuration": 39600 }
                }
            },
            "DepartureTime": "2026-04-22T08:00:00Z",
        }),
    )
    .await;
    assert_eq!(status, 200, "a rest schedule must be honoured, got {status} with {rested:?}");
    assert_eq!(
        int(&rested["Connections"][0], "RestDuration", "rest"), 900,
        "case \"rest\": a hop past the short work duration owes the short rest, got {:?}",
        rested["Connections"]
    );
    assert_eq!(
        int(&rested["TimeBreakdown"], "RestDuration", "rest"),
        int(&rested["Connections"][0], "RestDuration", "rest")
            + int(&rested["Connections"][1], "RestDuration", "rest"),
        "case \"rest\": the breakdown must account for every rest, got {:?}",
        rested["TimeBreakdown"]
    );
}

/// The documented waypoint maximum is enforced, and it is the lower one when any waypoint
/// uses a constraint — the sequencer costs every remaining candidate at every step, so the
/// list length bounds the work of a request.
#[tokio::test]
async fn the_documented_waypoint_maximum_is_enforced() {
    let router = routes_service();
    let waypoints = |count: usize, constrained: bool| -> Value {
        Value::Array(
            (0..count)
                .map(|index| {
                    let position = [-123.1 - (index as f64) * 0.001, 49.28 + (index as f64) * 0.001];
                    if constrained {
                        json!({ "Id": format!("w{index}"), "Position": position, "ServiceDuration": 60 })
                    } else {
                        json!({ "Id": format!("w{index}"), "Position": position })
                    }
                })
                .collect(),
        )
    };
    // Documented: "Maximum 50 waypoints per request / Maximum 20 waypoints when using
    // constraints".
    let cases: [(&str, usize, bool, u16); 6] = [
        ("at the plain maximum", 50, false, 200),
        ("past the plain maximum", 51, false, 400),
        ("at the constrained maximum", 20, true, 200),
        ("past the constrained maximum", 21, true, 400),
        ("below the constrained maximum, constrained", 19, true, 200),
        ("between the maximums, constrained", 30, true, 400),
    ];

    for (case, count, constrained, expected) in cases {
        let (status, answer, _) = post(
            &router,
            OPTIMIZE,
            &json!({ "Origin": [-123.1, 49.28], "Waypoints": waypoints(count, constrained) }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
        if expected == 400 {
            assert_eq!(
                answer["__type"], json!("ValidationException"),
                "case {case:?}: an oversized list is a ValidationException, got {answer:?}"
            );
        }
    }

    // CalculateRoutes documents its own, lower maximum: "Max length: 23".
    let route_waypoints = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({ "Position": [-123.1 - (index as f64) * 0.001, 49.28] }))
                .collect(),
        )
    };
    for (case, count, expected) in [
        ("at the route maximum", 23_usize, 200_u16),
        ("past the route maximum", 24, 400),
    ] {
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": [-123.1, 49.28],
                "Destination": [-123.2, 49.28],
                "Waypoints": route_waypoints(count),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
    }
}

/// A `Before` entry that names no other waypoint is refused, rather than reported as a
/// constraint the sequencer failed.
#[tokio::test]
async fn a_before_entry_that_names_no_waypoint_is_refused() {
    let router = routes_service();
    let cases: [(&str, Value, u16); 4] = [
        ("past the end", json!([2]), 400),
        ("negative", json!([-1]), 400),
        ("its own index", json!([0]), 400),
        ("the other waypoint", json!([1]), 200),
    ];

    for (case, before, expected) in cases {
        let (status, answer, _) = post(
            &router,
            OPTIMIZE,
            &json!({
                "Origin": [-123.1, 49.28],
                "Waypoints": [
                    { "Id": "a", "Position": [-123.2, 49.3], "Before": before },
                    { "Id": "b", "Position": [-123.05, 49.25] }
                ],
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
    }
}

/// A `Before` constraint is honoured where it can be, and reported when the constraints
/// cannot all hold.
#[tokio::test]
async fn a_before_constraint_is_honoured_and_a_cycle_is_reported() {
    let router = routes_service();
    // Waypoint 0 is much further away, so cost alone would visit it last; its `Before`
    // requires waypoint 1 to come after it, which the sequence must satisfy.
    let (status, answer, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "far", "Position": [-123.5, 49.6], "Before": [1] },
                { "Id": "near", "Position": [-123.105, 49.277] }
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let ids: Vec<&str> = answer["OptimizedWaypoints"]
        .as_array()
        .expect("OptimizedWaypoints is required")
        .iter()
        .filter_map(|waypoint| waypoint["Id"].as_str())
        .collect();
    assert_eq!(
        ids, vec!["Origin", "far", "near"],
        "case \"honoured\": the sequence must visit far before near, got {ids:?}"
    );
    assert!(
        answer["ImpedingWaypoints"].as_array().is_some_and(Vec::is_empty),
        "case \"honoured\": a satisfied constraint impedes nothing, got {:?}",
        answer["ImpedingWaypoints"]
    );

    // Two waypoints that each require the other to come after them cannot both be
    // satisfied, so the sequence falls back to cost and reports the broken constraint.
    let (status, answer, _) = post(
        &router,
        OPTIMIZE,
        &json!({
            "Origin": [-123.1, 49.28],
            "Waypoints": [
                { "Id": "a", "Position": [-123.5, 49.6], "Before": [1] },
                { "Id": "b", "Position": [-123.105, 49.277], "Before": [0] }
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let impeding = answer["ImpedingWaypoints"]
        .as_array()
        .expect("ImpedingWaypoints is required");
    assert_eq!(
        impeding.len(), 1,
        "case \"cycle\": exactly the waypoint whose Before the order breaks is reported, got {impeding:?}"
    );
    assert_eq!(
        impeding[0]["FailedConstraints"][0]["Constraint"], json!("Before"),
        "case \"cycle\": the failed constraint must be named, got {impeding:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals and the operations this slice does not compute
// ---------------------------------------------------------------------------

/// A position outside the WGS 84 bounds is refused with a 400 `ValidationException`,
/// wherever it appears; a position on the bounds is accepted.
#[tokio::test]
async fn a_position_outside_the_wgs84_bounds_is_refused_wherever_it_appears() {
    let router = routes_service();
    let good = [-123.1, 49.28];
    // Rows span every operation's position members, every bound, and one position on each
    // published bound.
    let cases: [(&str, &str, Value, u16); 14] = [
        (
            "routes origin longitude",
            ROUTES,
            json!({ "Origin": [-999.0, 49.0], "Destination": good }),
            400,
        ),
        (
            "routes origin latitude",
            ROUTES,
            json!({ "Origin": [-123.1, 99.0], "Destination": good }),
            400,
        ),
        (
            "routes destination longitude",
            ROUTES,
            json!({ "Origin": good, "Destination": [180.001, 49.0] }),
            400,
        ),
        (
            "routes destination latitude",
            ROUTES,
            json!({ "Origin": good, "Destination": [-123.1, -90.5] }),
            400,
        ),
        (
            "routes waypoint position",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "Waypoints": [{ "Position": [200.0, 49.0] }] }),
            400,
        ),
        (
            "routes on the longitude bound",
            ROUTES,
            json!({ "Origin": [-180.0, 49.0], "Destination": [180.0, 49.0] }),
            200,
        ),
        (
            "routes on the latitude bound",
            ROUTES,
            json!({ "Origin": [-123.1, -90.0], "Destination": [-123.1, 90.0] }),
            200,
        ),
        (
            "matrix origin",
            MATRIX,
            json!({ "Origins": [{ "Position": [-181.0, 49.0] }], "Destinations": [{ "Position": good }] }),
            400,
        ),
        (
            "matrix destination",
            MATRIX,
            json!({ "Origins": [{ "Position": good }], "Destinations": [{ "Position": [-123.1, 90.0001] }] }),
            400,
        ),
        (
            "matrix boundary circle centre",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "RoutingBoundary": { "Geometry": { "Circle": { "Center": [-123.1, 91.0], "Radius": 1000.0 } } }
            }),
            400,
        ),
        (
            "snap trace point",
            SNAP,
            json!({ "TracePoints": [{ "Position": good }, { "Position": [0.0, -91.0] }], "SnapRadius": 300 }),
            400,
        ),
        (
            "optimize origin",
            OPTIMIZE,
            json!({ "Origin": [181.0, 49.0] }),
            400,
        ),
        (
            "optimize waypoint",
            OPTIMIZE,
            json!({ "Origin": good, "Waypoints": [{ "Position": [-123.1, 90.1] }] }),
            400,
        ),
        (
            "optimize destination",
            OPTIMIZE,
            json!({ "Origin": good, "Destination": [-190.0, 49.0] }),
            400,
        ),
    ];

    for (case, path, body, expected) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
        if expected == 400 {
            assert_eq!(
                answer["__type"], json!("ValidationException"),
                "case {case:?}: a refused position is a ValidationException, got {answer:?}"
            );
            assert!(
                answer["message"].as_str().is_some_and(|message| !message.is_empty()),
                "case {case:?}: the refusal must say what was wrong, got {answer:?}"
            );
        }
    }
}

/// Members that each pin the route's clock, or that name a value no enum declares, are
/// refused rather than quietly ignored.
#[tokio::test]
async fn contradictory_or_unknown_members_are_refused() {
    let router = routes_service();
    let good = [-123.1, 49.28];
    let cases: [(&str, &str, Value, u16); 15] = [
        (
            "departure and arrival together",
            ROUTES,
            json!({
                "Origin": good,
                "Destination": good,
                "DepartureTime": "2026-04-22T17:57:24Z",
                "ArrivalTime": "2026-04-22T18:57:24Z"
            }),
            400,
        ),
        (
            "depart now and a departure time",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "DepartNow": true, "DepartureTime": "2026-04-22T17:57:24Z" }),
            400,
        ),
        (
            "depart now alone",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "DepartNow": true }),
            200,
        ),
        (
            "a negative alternative count",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "MaxAlternatives": -1 }),
            400,
        ),
        (
            "a travel mode no enum declares",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "TravelMode": "Hovercraft" }),
            400,
        ),
        (
            "a travel mode the matrix does not offer",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "TravelMode": "Transit"
            }),
            400,
        ),
        (
            "depart now and a departure time on the matrix",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "DepartNow": true,
                "DepartureTime": "2026-04-22T17:57:24Z"
            }),
            400,
        ),
        (
            "a negative service duration",
            OPTIMIZE,
            json!({
                "Origin": good,
                "Waypoints": [{ "Id": "a", "Position": [-123.2, 49.3], "ServiceDuration": -1 }]
            }),
            400,
        ),
        (
            "a heading past the modeled range",
            OPTIMIZE,
            json!({
                "Origin": good,
                "Waypoints": [{ "Id": "a", "Position": [-123.2, 49.3], "Heading": 360.5 }]
            }),
            400,
        ),
        (
            "a heading on the modeled bound",
            OPTIMIZE,
            json!({
                "Origin": good,
                "Waypoints": [{ "Id": "a", "Position": [-123.2, 49.3], "Heading": 360.0 }]
            }),
            200,
        ),
        (
            "a negative stop duration on a route",
            ROUTES,
            json!({
                "Origin": good,
                "Destination": good,
                "Waypoints": [{ "Position": [-123.2, 49.3], "StopDuration": -5 }]
            }),
            400,
        ),
        (
            "a negative snap radius",
            SNAP,
            json!({
                "TracePoints": [{ "Position": good }, { "Position": [-123.11, 49.27] }],
                "SnapRadius": -1
            }),
            400,
        ),
        (
            "a snap radius past the modeled range",
            SNAP,
            json!({
                "TracePoints": [{ "Position": good }, { "Position": [-123.11, 49.27] }],
                "SnapRadius": 10001
            }),
            400,
        ),
        (
            "a snap radius on the modeled bound",
            SNAP,
            json!({
                "TracePoints": [{ "Position": good }, { "Position": [-123.11, 49.27] }],
                "SnapRadius": 10000
            }),
            200,
        ),
        (
            "a single trace point",
            SNAP,
            json!({ "TracePoints": [{ "Position": good }], "SnapRadius": 300 }),
            400,
        ),
    ];

    for (case, path, body, expected) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {answer:?}"
        );
    }
}

/// Every operation of this service reports the pricing bucket HELD IN STATE FOR IT in the
/// required `PricingBucket` member, which the model binds to the
/// `x-amz-geo-pricing-bucket` HTTP header (`@httpHeader` on
/// `CalculateRoutesResponse$PricingBucket` and its four siblings), and reports the same
/// value however the request varies — the tier is a property of the operation, not of the
/// request, so nothing in the body carries it and nothing in the request moves it.
#[tokio::test]
async fn every_operation_reports_the_pricing_bucket_held_in_state() {
    let router = routes_service();
    // The mapping every handler reads, taken from the state struct itself rather than
    // restated here, so a change to it cannot leave this test asserting a stale literal.
    let state =
        service_com_amazonaws_georoutes_routesservice::ComAmazonawsGeoroutesRoutesserviceState::default();
    let near = [-123.1, 49.28];
    let far = [-123.12, 49.26];

    // Rows span the five implemented operations and, within each, requests that differ in
    // the dimensions the response varies over (waypoints, size, radius, travel mode,
    // threshold).
    let cases: [(&str, &str, &str, Value); 10] = [
        (
            "a direct route",
            ROUTES,
            "CalculateRoutes",
            json!({ "Origin": near, "Destination": far }),
        ),
        (
            "a time-based isoline",
            ISOLINES,
            "CalculateIsolines",
            json!({ "Origin": near, "Thresholds": { "Time": [300] } }),
        ),
        (
            "a distance-based isoline around a destination",
            ISOLINES,
            "CalculateIsolines",
            json!({ "Destination": far, "Thresholds": { "Distance": [2000] } }),
        ),
        (
            "a route through a waypoint, on foot",
            ROUTES,
            "CalculateRoutes",
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
            "CalculateRouteMatrix",
            json!({ "Origins": [{ "Position": near }], "Destinations": [{ "Position": far }] }),
        ),
        (
            "a 2x2 truck matrix",
            MATRIX,
            "CalculateRouteMatrix",
            json!({
                "Origins": [{ "Position": near }, { "Position": [-123.2, 49.3] }],
                "Destinations": [{ "Position": far }, { "Position": [-123.05, 49.25] }],
                "TravelMode": "Truck",
            }),
        ),
        (
            "a snap at the default radius",
            SNAP,
            "SnapToRoads",
            json!({
                "TracePoints": [{ "Position": [-123.10025, 49.28025] }, { "Position": [-123.10125, 49.28125] }],
                "SnapRadius": 0,
            }),
        ),
        (
            "a snap at 10 km, simple geometry",
            SNAP,
            "SnapToRoads",
            json!({
                "TracePoints": [{ "Position": [-123.10025, 49.28025] }, { "Position": [-123.10075, 49.28075] }],
                "SnapRadius": 10_000,
                "SnappedGeometryFormat": "Simple",
            }),
        ),
        (
            "an optimization with no waypoints",
            OPTIMIZE,
            "OptimizeWaypoints",
            json!({ "Origin": near, "Destination": far }),
        ),
        (
            "an optimization over three waypoints",
            OPTIMIZE,
            "OptimizeWaypoints",
            json!({
                "Origin": near,
                "Destination": far,
                "Waypoints": [
                    { "Id": "bravo", "Position": [-123.2, 49.3] },
                    { "Id": "alpha", "Position": [-123.11, 49.27] },
                    { "Id": "charlie", "Position": [-123.05, 49.25] },
                ],
            }),
        ),
    ];

    for (case, path, operation, body) in cases {
        let expected = state.pricing_bucket(operation);
        let (status, answer, bucket) = post(&router, path, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            bucket.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the x-amz-geo-pricing-bucket header to carry the bucket held in state for {operation} ({expected:?}), got {bucket:?}"
        );
        assert!(
            answer["PricingBucket"].is_null(),
            "case {case:?}: PricingBucket is header-bound, so the body must not repeat it, got {:?}",
            answer["PricingBucket"]
        );
    }
}

/// What this slice cannot compute answers 501 `NotImplemented` — never an empty 200.
#[tokio::test]
async fn what_cannot_be_computed_answers_501() {
    let router = routes_service();
    let good = [-123.1, 49.28];
    let area = json!({ "Geometry": { "BoundingBox": [-124.0, 49.0, -123.0, 50.0] } });
    let cases: [(&str, &str, Value); 14] = [
        (
            "a transit route",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "TravelMode": "Transit" }),
        ),
        (
            "an intermodal route",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "TravelMode": "Intermodal" }),
        ),
        (
            "a rest profile",
            OPTIMIZE,
            json!({ "Origin": good, "Driver": { "RestProfile": { "Profile": "EU" } } }),
        ),
        (
            "an area to avoid on an isoline",
            ISOLINES,
            json!({
                "Origin": good,
                "Thresholds": { "Distance": [1000] },
                "Avoid": { "Areas": [area.clone()] }
            }),
        ),
        (
            "controlled-access highways avoided on an isoline",
            ISOLINES,
            json!({
                "Origin": good,
                "Thresholds": { "Time": [600] },
                "Avoid": { "ControlledAccessHighways": true }
            }),
        ),
        (
            "a country exclusion",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "Exclude": { "Countries": ["CAN"] } }),
        ),
        (
            "span features",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "SpanAdditionalFeatures": ["SpeedLimit"] }),
        ),
        (
            "a country exclusion on the matrix",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "Exclude": { "Countries": ["CAN"] }
            }),
        ),
        (
            "a country exclusion on the optimizer",
            OPTIMIZE,
            json!({ "Origin": good, "Exclude": { "Countries": ["CAN"] } }),
        ),
        (
            "waypoint access hours",
            OPTIMIZE,
            json!({
                "Origin": good,
                "Waypoints": [{
                    "Id": "a",
                    "Position": [-123.2, 49.3],
                    "AccessHours": {
                        "From": { "DayOfWeek": "Monday", "TimeOfDay": "08:00:00Z" },
                        "To": { "DayOfWeek": "Monday", "TimeOfDay": "17:00:00Z" }
                    }
                }]
            }),
        ),
        (
            "an area to avoid on a route",
            ROUTES,
            json!({ "Origin": good, "Destination": good, "Avoid": { "Areas": [area.clone()] } }),
        ),
        (
            "an area to avoid on the matrix",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "Avoid": { "Areas": [area.clone()] }
            }),
        ),
        (
            "controlled-access highways avoided on the matrix",
            MATRIX,
            json!({
                "Origins": [{ "Position": good }],
                "Destinations": [{ "Position": good }],
                "Avoid": { "ControlledAccessHighways": true }
            }),
        ),
        (
            "controlled-access highways avoided on the optimizer",
            OPTIMIZE,
            json!({ "Origin": good, "Avoid": { "ControlledAccessHighways": true } }),
        ),
    ];

    for (case, path, body) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501 NotImplemented, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["__type"], json!("NotImplemented"),
            "case {case:?}: a 501 carries the NotImplemented code, got {answer:?}"
        );
    }
}
