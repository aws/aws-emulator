// SPDX-License-Identifier: CC0-1.0

//! @op CalculateRoutes
//!
//! Computes the route from `Origin` through `Waypoints` to `Destination` over the
//! deterministic road model in [`crate::state`]: one leg per pair of consecutive STOPS
//! (a waypoint with `PassThrough` set does not split the route, per its own
//! documentation), each leg's geometry snapped onto the road network, and its distance
//! and duration costed by the single cost function
//! [`ComAmazonawsGeoroutesRoutesserviceState::compute_route`] that CalculateRouteMatrix
//! and OptimizeWaypoints also use — so a matrix cell and this route cannot disagree.
//!
//! `TravelMode` `Transit` and `Intermodal` answer 501: a faithful answer needs transit
//! lines and timetables, which this emulator holds no data for, and the leg shapes those
//! modes return (`TransitLegDetails`, boarding steps, next departures) cannot be computed
//! from the road model. So do `Exclude` (a country exclusion is "strictly excluded", and
//! no road carries the country it is in), any `SpanAdditionalFeatures`, and any
//! `LegAdditionalFeatures` other than `Summary` and `TravelStepInstructions`: those ask for
//! data the road model does not carry, and an empty list in their place would be absence
//! dressed as success.
//!
//! `Avoid` is best-effort by its own documentation — a violated avoidance "produces a
//! notice for the violation" — so a route that uses a controlled-access highway the
//! request asked to avoid carries the model's `ViolatedAvoidControlledAccessHighway`
//! notice. Nothing else `Avoid` names exists on this network to be used or violated (no
//! toll road, ferry, tunnel, dirt road, car shuttle train, seasonal closure or zone), and
//! the computed path never turns back on itself.
//!
//! ## Durations
//!
//! A route's `Summary.Duration` is what the whole journey TAKES: the travel of every leg
//! plus the `StopDuration` of every waypoint it stops at, since a stop is time spent on
//! the route. The same split runs through a leg's own summary — `Overview` covers "before
//! travel, travel and after travel steps", so it includes the stop at the waypoint the leg
//! ends at, while `TravelOnly` covers the travel steps alone — and a leg's `Departure` and
//! `Arrival` times bound its travel, with the stop falling between one leg's arrival and
//! the next leg's departure.
//!
//! ## Travel steps
//!
//! A leg's `TravelSteps` are its manoeuvres, one per RUN of road the leg travels: the road
//! model in [`crate::state`] carries each road's orientation and grid index, so a leg's
//! snapped geometry cuts into runs at the junctions where its road changes, and each
//! junction carries the angle between the two roads' own directions. A step therefore
//! reports the run's share of the leg's distance and duration (the shares sum to exactly
//! the leg's own), its `GeometryOffset`, and a `Type` classified from the road classes it
//! joins and leaves and from that angle — never chosen. `AfterTravelSteps` is the stop the
//! route makes at the leg's end: a vehicle parks and a pedestrian waits for the waypoint's
//! `StopDuration`, which is the one value each of those step-type enums declares; a leg
//! with no stop at its end has nothing to perform after travelling and so an empty list.
//!
//! A step carries only what the model REQUIRES of it (`Duration`, `Type`) and what the
//! request asked for: the narration — `Instruction`, `CurrentRoad` and `NextRoad` — is
//! reported only for a request whose `LegAdditionalFeatures` name
//! `TravelStepInstructions`, the one additional feature that asks for the per-step detail,
//! and within that `Instruction` only when the effective `TravelStepType` is `Default`
//! ("Only available when the TravelStepType is Default"), while `Signpost` is carried only
//! for `TurnByTurn`, as its own documentation says, and names the road the junction leads
//! onto. `ExitNumber` is a list the service computes, so it is always reported — empty
//! where the step takes no numbered exit, which is every step over this network.
//!
//! What the network does NOT hold is left out rather than invented: it has no roundabout
//! and no ramp, so no `RoundaboutEnter`/`RoundaboutExit`/`RoundaboutPass`/`Ramp` step
//! arises, and it names no destination reachable along a road, so `Towards` stays empty.
//! It DOES number its motorways, so a road the route travels is labelled by its
//! `RouteNumber` where the network numbers it and by its `RoadName` otherwise — in
//! `MajorRoadLabels`, in a step's `CurrentRoad`/`NextRoad` and on a signpost label alike.
//!
//! `Languages` and `InstructionsMeasurementSystem` act on the instruction text: the
//! requested tags are resolved in order against the languages this emulator holds
//! narration phrases in ([`NARRATIONS`]), an instruction is rendered in the first that
//! resolves and the leg reports the language it was rendered in. Only a request where
//! NONE of the tags resolves falls back to the regional default, and that is the one case
//! the model's own `MainLanguageNotFound` notice reports, since "the request uses the
//! regional default as the fallback if the requested language can't be provided". A
//! step's distance is stated in metres and kilometres for `Metric` and in feet and miles
//! for `Imperial`.
//!
//! `Allow`, `Tolls`, `Traffic` and `TravelModeOptions` have nothing to act on: the model
//! carries no HOV/HOT lane, toll, traffic variation or vehicle restriction. A
//! `TravelModeOptions` block is still held to the `TravelMode` it belongs to — the member
//! names its mode — so a block for another mode is refused rather than dropped.
//!
//! ## Places
//!
//! A leg's departure and arrival report two positions, which the model keeps apart:
//! `OriginalPosition` is "Position provided in the request", so it is the submitted
//! position echoed verbatim at full double precision, and `Position` is the point in WGS 84
//! the traveller actually departs from or arrives at — the submitted position MATCHED onto
//! the road network by the same
//! [`crate::state::ComAmazonawsGeoroutesRoutesserviceState::snap`] that SnapToRoads reports
//! its snapped trace points with. The two therefore differ by the snap offset, and coincide
//! only where the submitted position already lies on a road the travel mode may use. A
//! pass-through waypoint's place reports the same pair, under the same rule.
//!
//! The leg GEOMETRY still begins and ends at the submitted positions, which is where the
//! traveller is before the route reaches a road: the line runs from the submitted position
//! onto the network and back off it, and its length is the leg's distance.
//!
//! ## Leg summary
//!
//! A leg's `Summary` summarises the leg that WAS computed — its `Overview` is the whole
//! leg's distance and duration and its `TravelOnly` the travelling part — so it is
//! reported for every leg, as the recorded behavior of a request naming no additional
//! feature shows. `Summary` therefore stays an accepted `LegAdditionalFeature` that asks
//! for nothing this answer does not already carry. `TypicalDuration` and `BestCaseDuration`
//! are the computed duration: the road model carries no traffic variation for either to
//! differ by.
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
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CalculateRoutes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! CalculateRoutes computes routes given the following required parameters: Origin and
//! Destination. For more information, see Calculate routes in the Amazon Location Service
//! Developer Guide.
//!
//! ## Input
//!
//! * `Allow` (RouteAllowOptions) — Features that are allowed while calculating a route. Not
//!   supported in ap-southeast-1 and ap-southeast-5 regions for GrabMaps customers.
//! * `ArrivalTime` (TimestampWithTimezoneOffset) — Time of arrival at the destination.
//! * `Avoid` (RouteAvoidanceOptions) — Features that are avoided while calculating a route.
//! * `DepartNow` (SensitiveBoolean) — Uses the current time as the time of departure.
//! * `DepartureTime` (TimestampWithTimezoneOffset) — Time of departure from the origin.
//! * `Destination` (Position, len 2..2) **required** — The final position for the route. In
//!   the World Geodetic System (WGS 84) format: [longitude, latitude].
//! * `DestinationOptions` (RouteDestinationOptions) — Destination related options.
//! * `Driver` (RouteDriverOptions) — Driver related options.
//! * `Exclude` (RouteExclusionOptions) — Features to be strictly excluded while calculating
//!   the route.
//! * `InstructionsMeasurementSystem` (MeasurementSystem, one of: Imperial | Metric) —
//!   Measurement system to be used for instructions within steps in the response.
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization.
//! * `Languages` (LanguageTagList) — List of languages for instructions within steps.
//! * `LegAdditionalFeatures` (RouteLegAdditionalFeatureList, len ..12) — A list of optional
//!   additional parameters such as timezone that can be requested for each result. …
//!   Summary: Returns a summary of the route, including distance and duration. …
//! * `LegGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — Specifies the
//!   format of the geometry returned for each leg of the route. FlexiblePolyline: A compact
//!   and precise encoding format for the leg geometry (see the GitHub repository for
//!   https://github.com/aws-geospatial/polyline). Simple: A less compact encoding.
//! * `MaxAlternatives` (Integer) — Maximum number of alternative routes to be provided in the
//!   response, if available.
//! * `OptimizeRoutingFor` (RoutingObjective, one of: FastestRoute | ShortestRoute) — Controls
//!   the trade-off between achieving the shortest travel time (FastestRoute) and achieving
//!   the shortest physical distance ((ShortestRoute). Default value: FastestRoute
//! * `Origin` (Position, len 2..2) **required** — The start position for the route in World
//!   Geodetic System (WGS 84) format: [longitude, latitude].
//! * `OriginOptions` (RouteOriginOptions) — Specifies how the origin point should be matched
//!   to the road network.
//! * `SpanAdditionalFeatures` (RouteSpanAdditionalFeatureList, len ..24) — A list of optional
//!   features such as SpeedLimit that can be requested for a Span.
//! * `Tolls` (RouteTollOptions) — Toll related options.
//! * `Traffic` (RouteTrafficOptions) — Traffic related options.
//! * `TravelMode` (RouteTravelMode, one of: Car | Intermodal | Pedestrian | Scooter | Transit
//!   | Truck) — Specifies the mode of transport when calculating a route. Used in estimating
//!   the speed of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (RouteTravelModeOptions) — Travel mode related options.
//! * `TravelStepType` (RouteTravelStepType, one of: Default | TurnByTurn) — Type of step
//!   returned by the response.
//! * `Waypoints` (RouteWaypointList) — List of waypoints between the Origin and Destination.
//!   Max length: 23
//!
//! ## Output
//!
//! * `LegGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required** —
//!   Specifies the format of the geometry returned for each leg of the route.
//! * `Notices` (RouteResponseNoticeList) **required** — Notices are additional information
//!   returned that indicate issues that occurred during route calculation.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `Routes` (RouteList) **required** — The path from the origin to the destination.
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
    self, CachedLeg, Instant, Junction, LegType, PathVertex, Position, RoadClass, RoutingObjective, TravelMode,
    TravelledRoad,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CalculateRoutesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.georoutes#CalculateRoutes")]
#[derive(Debug)]
pub enum CalculateRoutesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::calculate_routes::CalculateRoutesError`). The wrapper is kept because
/// `TravelMode` `Transit` and `Intermodal` are answered with its 501.
pub type CalculateRoutesError = crate::scaffolded::Scaffolded<CalculateRoutesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CalculateRoutesErrorModeled> for CalculateRoutesError {
    fn from(modeled: CalculateRoutesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> CalculateRoutesError {
    CalculateRoutesErrorModeled::ValidationException(ValidationException {
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
fn access_denied(message: String) -> CalculateRoutesError {
    CalculateRoutesErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator
/// rather than of the request.
fn internal(message: &str) -> CalculateRoutesError {
    CalculateRoutesErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// The option blocks a route request may set for `mode`.
///
/// A block belongs to the mode its member names, so a mode permits the block that names it.
/// A car also permits the service's electric-vehicle block, which it names `Ev`: the two
/// are the set this refusal states for `Car`, and `RouteTravelModeOptions` in the model this
/// crate is generated from carries no member for the second one yet, so nothing reads it —
/// a refusal must state the set the service permits, not a narrower one.
fn permitted_travel_mode_options(
    mode: TravelMode,
    declared: &state::DeclaredTravelModeOptions<'static>,
) -> Vec<&'static str> {
    let mut permitted: Vec<&'static str> =
        state::block_of_mode(mode.as_model(), declared).into_iter().collect();
    if matches!(mode, TravelMode::Car) {
        permitted.push(EV_OPTIONS_BLOCK);
    }
    permitted
}

/// The electric-vehicle option block a car route may set beside its own.
const EV_OPTIONS_BLOCK: &str = "Ev";

/// Every per-mode option block `RouteTravelModeOptions` declares, in the model's order,
/// paired with whether the request set it — what
/// [`state::check_travel_mode_options`] holds against the selected `TravelMode`.
fn declared_travel_mode_options(
    options: Option<&RouteTravelModeOptions>,
) -> [(&'static str, bool); 6] {
    [
        ("Car", options.is_some_and(|set| set.car.is_some())),
        (
            "Pedestrian",
            options.is_some_and(|set| set.pedestrian.is_some()),
        ),
        ("Scooter", options.is_some_and(|set| set.scooter.is_some())),
        ("Truck", options.is_some_and(|set| set.truck.is_some())),
        (
            "Intermodal",
            options.is_some_and(|set| set.intermodal.is_some()),
        ),
        ("Transit", options.is_some_and(|set| set.transit.is_some())),
    ]
}

/// The documented maximum number of waypoints between the origin and the destination
/// ("Max length: 23" on `CalculateRoutesRequest$Waypoints`).
const MAX_WAYPOINTS: usize = 23;

/// The upper bound of `RouteWaypoint$StopDuration`'s modeled range (`0..49999`).
const MAX_STOP_DURATION_S: i64 = 49_999;

/// The `LegAdditionalFeatures` this slice ACCEPTS: the step instructions the travel steps
/// carry, and `Summary` — which asks for the leg summary this answer reports for every leg
/// anyway, so naming it adds nothing. Every other feature asks for data the road model
/// does not hold (tolls, incidents, elevation, zones, truck road types, transit stops and
/// departures), so a request naming one is declined rather than answered without it.
const COMPUTED_LEG_FEATURES: [&str; 2] = ["Summary", "TravelStepInstructions"];

/// The `RouteLegAdditionalFeature` that asks for the per-step narration: the step
/// `Instruction`, and the `CurrentRoad`/`NextRoad` that instruction names. A step of a
/// request that does not ask for it carries the members the model requires and nothing
/// more.
const TRAVEL_STEP_INSTRUCTIONS_FEATURE: &str = "TravelStepInstructions";

/// The narration of a route in ONE language: the phrase of each manoeuvre a step of this
/// model can perform, of the stop that follows a leg's travel, and of the words a phrase
/// names a direction with.
///
/// Each phrase is a TEMPLATE of this emulator's own, filled from the computed route:
/// `{road}` is the road the network holds there, `{distance}` the step's own distance in
/// the requested measurement system, `{steering}` which way the junction turns and
/// `{duration}` how long the stop lasts. Nothing in a phrase is read from the request.
#[derive(Debug)]
struct Narration {
    /// The BCP 47 primary language subtag this entry holds the phrases in. A requested
    /// tag resolves to this entry when its own primary subtag is this one, so `es-ES` and
    /// `es-419` are both answered in Spanish.
    language: &'static str,
    depart: &'static str,
    continue_on: &'static str,
    continue_highway: &'static str,
    enter_highway: &'static str,
    exit_highway: &'static str,
    keep: &'static str,
    turn: &'static str,
    u_turn: &'static str,
    arrive: &'static str,
    /// The after-travel step of a vehicle leg, and of a pedestrian leg.
    park: &'static str,
    wait: &'static str,
    /// What a phrase calls a road the network left unnamed.
    road_ahead: &'static str,
    /// The `RouteSteeringDirection` values a phrase states in words.
    straight: &'static str,
    left: &'static str,
    right: &'static str,
}

impl Narration {
    /// The word for a `RouteSteeringDirection` of this model.
    fn steering(&self, direction: &str) -> &'static str {
        match direction {
            "Left" => self.left,
            "Right" => self.right,
            _ => self.straight,
        }
    }

    /// `template` with the values of this step filled in.
    fn fill(template: &str, road: &str, distance: &str, steering: &str) -> String {
        template
            .replace("{road}", road)
            .replace("{distance}", distance)
            .replace("{steering}", steering)
    }
}

/// The language tag this emulator reports when it renders the narration in its own
/// regional default rather than in a language the request asked for: US English, the
/// regional default of the region this emulator serves. A request that asked for a
/// language the catalog HOLDS is answered under the tag it asked with instead.
///
/// The region subtag is lower-case because that is how the service's own fallback answers
/// it; a requested tag is reported back as the caller spelled it, so the two forms sitting
/// side by side in one answer is correct and not a normalisation to tidy up.
const DEFAULT_LANGUAGE_TAG: &str = "en-us";

/// The narration of the regional default language, which a request resolving to none of
/// the catalog's languages is answered in.
const DEFAULT_NARRATION: &Narration = &ENGLISH;

/// The languages this emulator can render a route's narration in.
///
/// A request whose `Languages` names a tag of one of these languages is answered in that
/// language and reports it; one naming no tag of any of them is answered in the default —
/// "the request uses the regional default as the fallback if the requested language can't
/// be provided" — and carries the model's `MainLanguageNotFound` notice, which is reserved
/// for exactly that case.
const NARRATIONS: &[Narration] = &[ENGLISH, SPANISH, FRENCH, GERMAN, ITALIAN, PORTUGUESE];

const ENGLISH: Narration = Narration {
    language: "en",
    depart: "Depart on {road} and continue for {distance}.",
    continue_on: "Continue on {road} for {distance}.",
    continue_highway: "Continue on the highway {road} for {distance}.",
    enter_highway: "Enter the highway {road} and continue for {distance}.",
    exit_highway: "Exit the highway onto {road} and continue for {distance}.",
    keep: "Keep {steering} onto {road} and continue for {distance}.",
    turn: "Turn {steering} onto {road} and continue for {distance}.",
    u_turn: "Make a U-turn onto {road} and continue for {distance}.",
    arrive: "Arrive at your destination.",
    park: "Park for {duration}.",
    wait: "Wait for {duration}.",
    road_ahead: "the road ahead",
    straight: "straight",
    left: "left",
    right: "right",
};

const SPANISH: Narration = Narration {
    language: "es",
    depart: "Salga por {road} y continúe durante {distance}.",
    continue_on: "Continúe por {road} durante {distance}.",
    continue_highway: "Continúe por la autopista {road} durante {distance}.",
    enter_highway: "Entre en la autopista {road} y continúe durante {distance}.",
    exit_highway: "Salga de la autopista hacia {road} y continúe durante {distance}.",
    keep: "Manténgase a la {steering} hacia {road} y continúe durante {distance}.",
    turn: "Gire a la {steering} hacia {road} y continúe durante {distance}.",
    u_turn: "Haga un cambio de sentido hacia {road} y continúe durante {distance}.",
    arrive: "Ha llegado a su destino.",
    park: "Aparque durante {duration}.",
    wait: "Espere durante {duration}.",
    road_ahead: "la vía actual",
    straight: "recto",
    left: "izquierda",
    right: "derecha",
};

const FRENCH: Narration = Narration {
    language: "fr",
    depart: "Partez sur {road} et continuez pendant {distance}.",
    continue_on: "Continuez sur {road} pendant {distance}.",
    continue_highway: "Continuez sur l'autoroute {road} pendant {distance}.",
    enter_highway: "Entrez sur l'autoroute {road} et continuez pendant {distance}.",
    exit_highway: "Quittez l'autoroute vers {road} et continuez pendant {distance}.",
    keep: "Serrez à {steering} vers {road} et continuez pendant {distance}.",
    turn: "Tournez à {steering} vers {road} et continuez pendant {distance}.",
    u_turn: "Faites demi-tour vers {road} et continuez pendant {distance}.",
    arrive: "Vous êtes arrivé à destination.",
    park: "Stationnez pendant {duration}.",
    wait: "Attendez pendant {duration}.",
    road_ahead: "la route actuelle",
    straight: "tout droit",
    left: "gauche",
    right: "droite",
};

const GERMAN: Narration = Narration {
    language: "de",
    depart: "Starten Sie auf {road} und fahren Sie {distance} weiter.",
    continue_on: "Folgen Sie {road} für {distance}.",
    continue_highway: "Folgen Sie der Autobahn {road} für {distance}.",
    enter_highway: "Fahren Sie auf die Autobahn {road} und fahren Sie {distance} weiter.",
    exit_highway: "Verlassen Sie die Autobahn auf {road} und fahren Sie {distance} weiter.",
    keep: "Halten Sie sich {steering} auf {road} und fahren Sie {distance} weiter.",
    turn: "Biegen Sie {steering} auf {road} ab und fahren Sie {distance} weiter.",
    u_turn: "Wenden Sie auf {road} und fahren Sie {distance} weiter.",
    arrive: "Sie haben Ihr Ziel erreicht.",
    park: "Parken Sie {duration}.",
    wait: "Warten Sie {duration}.",
    road_ahead: "die Straße voraus",
    straight: "geradeaus",
    left: "links",
    right: "rechts",
};

const ITALIAN: Narration = Narration {
    language: "it",
    depart: "Parti su {road} e continua per {distance}.",
    continue_on: "Continua su {road} per {distance}.",
    continue_highway: "Continua sull'autostrada {road} per {distance}.",
    enter_highway: "Entra sull'autostrada {road} e continua per {distance}.",
    exit_highway: "Esci dall'autostrada su {road} e continua per {distance}.",
    keep: "Mantieni la {steering} su {road} e continua per {distance}.",
    turn: "Svolta a {steering} su {road} e continua per {distance}.",
    u_turn: "Fai un'inversione a U su {road} e continua per {distance}.",
    arrive: "Sei arrivato a destinazione.",
    park: "Parcheggia per {duration}.",
    wait: "Attendi per {duration}.",
    road_ahead: "la strada attuale",
    straight: "dritto",
    left: "sinistra",
    right: "destra",
};

const PORTUGUESE: Narration = Narration {
    language: "pt",
    depart: "Siga por {road} e continue por {distance}.",
    continue_on: "Continue por {road} por {distance}.",
    continue_highway: "Continue pela autoestrada {road} por {distance}.",
    enter_highway: "Entre na autoestrada {road} e continue por {distance}.",
    exit_highway: "Saia da autoestrada para {road} e continue por {distance}.",
    keep: "Mantenha-se à {steering} para {road} e continue por {distance}.",
    turn: "Vire à {steering} para {road} e continue por {distance}.",
    u_turn: "Faça um retorno para {road} e continue por {distance}.",
    arrive: "Chegou ao seu destino.",
    park: "Estacione por {duration}.",
    wait: "Aguarde por {duration}.",
    road_ahead: "a estrada atual",
    straight: "em frente",
    left: "esquerda",
    right: "direita",
};

/// The angle bands, in degrees, a manoeuvre at a junction is classified into: inside
/// `STRAIGHT` the leg continues straight on, up to `KEEP` it keeps to one side of a fork,
/// at `UTURN` or beyond it doubles back, and anything between is a turn. AWS publishes no
/// bands, so these are this emulator's own, applied to every leg and travel mode alike.
const STRAIGHT_ANGLE_DEG: f64 = 10.0;
const KEEP_ANGLE_DEG: f64 = 45.0;
const UTURN_ANGLE_DEG: f64 = 160.0;

/// The bands of `RouteTurnIntensity`, in degrees: a turn gentler than `SLIGHT` is
/// `Slight`, one at `SHARP` or beyond is `Sharp`, and the rest are `Typical`. This
/// emulator's own, as above.
const SLIGHT_TURN_ANGLE_DEG: f64 = 60.0;
const SHARP_TURN_ANGLE_DEG: f64 = 120.0;

/// The exact definitions of the units an instruction states a distance in.
const METRES_PER_KILOMETRE: f64 = 1_000.0;
const METRES_PER_FOOT: f64 = 0.3048;
const METRES_PER_MILE: f64 = 1_609.344;

/// Seconds in a minute, for the duration an after-travel step states.
const SECONDS_PER_MINUTE: i64 = 60;

/// `RouteTravelStepType`: how much detail the steps of the answer carry.
///
/// The member carries no modeled default. `Default` — "basic steps intended for web based
/// applications" — is what a request naming none gets, and it is the only value whose
/// steps carry an `Instruction` ("Only available when the TravelStepType is Default");
/// `TurnByTurn` carries instead the members the model marks as its own, the manoeuvre
/// details and the `Signpost` "applicable only for TurnByTurn steps".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepDetail {
    Default,
    TurnByTurn,
}

impl StepDetail {
    /// The detail named by a model enum value, read against the generated value list so the
    /// accepted set cannot drift from the model.
    fn from_model(value: &str) -> Option<Self> {
        if !ROUTE_TRAVEL_STEP_TYPE_VALUES.contains(&value) {
            return None;
        }
        match value {
            "Default" => Some(Self::Default),
            "TurnByTurn" => Some(Self::TurnByTurn),
            _ => None,
        }
    }
}

/// `MeasurementSystem`: the units a step's instruction states its distance in. A request
/// naming none gets `Metric`, the system every distance this model carries is in
/// (`DistanceMeters`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measurement {
    Metric,
    Imperial,
}

impl Measurement {
    /// The system named by a model enum value, read against the generated value list.
    fn from_model(value: &str) -> Option<Self> {
        if !MEASUREMENT_SYSTEM_VALUES.contains(&value) {
            return None;
        }
        match value {
            "Metric" => Some(Self::Metric),
            "Imperial" => Some(Self::Imperial),
            _ => None,
        }
    }
}

/// What the steps of the answer are rendered with.
#[derive(Debug, Clone, Copy)]
struct StepRendering<'a> {
    /// The effective `TravelStepType`.
    detail: StepDetail,
    /// The language tag the instructions and the localized road names are rendered in,
    /// as the answer reports it.
    language: &'a str,
    /// The phrases that language's narration is built from.
    narration: &'static Narration,
    /// The effective `InstructionsMeasurementSystem`.
    measurement: Measurement,
    /// Whether `LegAdditionalFeatures` asked for the per-step narration
    /// ([`TRAVEL_STEP_INSTRUCTIONS_FEATURE`]).
    narrate: bool,
}

/// Where the route's clock is pinned: at its departure or at its arrival.
#[derive(Debug, Clone, Copy)]
enum Anchor {
    Departure(Instant),
    Arrival(Instant),
}

/// What rendering a computed route into the answer needs beyond the route itself.
#[derive(Debug, Clone, Copy)]
struct Rendering<'a> {
    travel_mode: TravelMode,
    geometry_format: &'a str,
    anchor: Anchor,
    /// The UTC offset the request stated the time it pinned the route at, which is the
    /// offset every time of the answer is reported at.
    clock_offset: state::UtcOffset,
    /// Whether the route violates an avoidance the request asked for.
    violated_avoidance: bool,
    /// What the leg's travel steps are rendered with.
    steps: StepRendering<'a>,
}

/// One point of the requested sequence: origin, a waypoint, or the destination.
#[derive(Debug, Clone)]
struct SequencePoint {
    position: Position,
    /// The point of the road network the submitted position is matched onto, which is
    /// where the traveller departs from or arrives at. `None` only where the network holds
    /// no road the travel mode may use near the position, which leaves nothing to match
    /// it to.
    matched_position: Option<Position>,
    /// Index in the request's `Waypoints` list, for the places that report one.
    waypoint_index: Option<usize>,
    /// Whether the point splits the route into legs. False only for a `PassThrough`
    /// waypoint, which "is passed through and doesn't split the route into different
    /// legs".
    is_stop: bool,
    /// `StopDuration`: how long the traveller stays at the point.
    stop_duration_s: i64,
}

impl SequencePoint {
    /// `Position`: the point in WGS 84 the traveller departs from or arrives at, which is
    /// the submitted position matched onto the road network. A position the network has no
    /// road for is reported as submitted rather than moved to a road that does not exist.
    fn reported_position(&self) -> Position {
        self.matched_position.unwrap_or(self.position)
    }
}

/// One leg of the answer: the computed sub-legs between two stops, merged.
#[derive(Debug, Clone)]
struct MergedLeg {
    distance_m: i64,
    duration_s: i64,
    /// The leg's geometry, each vertex with the road it is on.
    path: Vec<PathVertex>,
    /// The runs of the merged leg: the manoeuvres its travel steps report.
    runs: Vec<state::LegRun>,
    /// The pass-through waypoints inside this leg, with their offset into its geometry.
    pass_through: Vec<(SequencePoint, usize)>,
    start: SequencePoint,
    end: SequencePoint,
}

/// The `CalculateRoutes` operation.
pub struct CalculateRoutes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CalculateRoutes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CalculateRoutes<Eng>
{
    type Input = CalculateRoutesInput;
    type Output = CalculateRoutesOutput;
    type Error = CalculateRoutesError;

    const NAME: &'static str = "CalculateRoutes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CALCULATE_ROUTES_OUTPUT_SCHEMA;
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
            Some(value) => TravelMode::from_model_values(value, ROUTE_TRAVEL_MODE_VALUES)
                .ok_or_else(|| invalid("TravelMode", state::must_be_one_of("TravelMode", ROUTE_TRAVEL_MODE_VALUES)))?,
        };
        if matches!(travel_mode, TravelMode::Transit | TravelMode::Intermodal) {
            // No transit network or timetable is held, so there is nothing to compute a
            // transit leg from. 501 says "not written yet", which is the truth.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with TravelMode Transit or Intermodal",
            ));
        }
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
        // A per-mode option block belongs to the one travel mode its member names, so a
        // request may set only a block this operation permits for the `TravelMode` it
        // selected. Accepting another mode's block would answer with a route computed for a
        // different vehicle than the constraints the caller supplied describe.
        let declared = declared_travel_mode_options(input.travel_mode_options.as_ref());
        state::check_travel_mode_options(
            travel_mode.as_model(),
            &permitted_travel_mode_options(travel_mode, &declared),
            &declared,
        )
        .map_err(|(member, message)| invalid(&member, message))?;
        let geometry_format = state::geometry_format("LegGeometryFormat", input.leg_geometry_format.as_ref())
            .map_err(|message| invalid("LegGeometryFormat", message))?;

        // What this slice does not compute is declined, not answered with an empty list.
        // `Exclude` is "strictly excluded", and nothing in the road model carries the
        // country a road is in, so an exclusion cannot be honoured or reported.
        if input.exclude.is_some() {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with Exclude",
            ));
        }
        // An avoidance area is a routing constraint: the route would have to be computed
        // around it, which this model does not do, so it is declined rather than violated
        // silently.
        if input
            .avoid
            .as_ref()
            .is_some_and(|avoid| avoid.areas.as_deref().is_some_and(|areas| !areas.is_empty()))
        {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with Avoid.Areas",
            ));
        }
        // A span is "a section of a Leg for which the requested features have the same
        // values"; this slice computes none of those features, so a request asking for
        // them is declined rather than answered with an empty `Spans` list.
        if input
            .span_additional_features
            .as_deref()
            .is_some_and(|features| !features.is_empty())
        {
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "CalculateRoutes with SpanAdditionalFeatures",
            ));
        }
        // The steps of a leg are its manoeuvres, which the road model now determines: the
        // requested step detail and measurement system decide what each step says, not
        // whether one can be computed.
        let step_detail = match input.travel_step_type.as_deref() {
            None => StepDetail::Default,
            Some(value) => StepDetail::from_model(value).ok_or_else(|| {
                invalid(
                    "TravelStepType",
                    state::must_be_one_of("TravelStepType", ROUTE_TRAVEL_STEP_TYPE_VALUES),
                )
            })?,
        };
        let measurement = match input.instructions_measurement_system.as_deref() {
            None => Measurement::Metric,
            Some(value) => Measurement::from_model(value).ok_or_else(|| {
                invalid(
                    "InstructionsMeasurementSystem",
                    state::must_be_one_of("InstructionsMeasurementSystem", MEASUREMENT_SYSTEM_VALUES),
                )
            })?,
        };
        let (language, narration, language_fell_back) =
            instruction_language(input.languages.as_deref().unwrap_or_default());
        // `Summary` and `TravelStepInstructions` are the leg features this slice computes;
        // every other one asks for data the road model does not carry (tolls, incidents,
        // elevation, zones, truck road types, transit stops and departures).
        let leg_features = input.leg_additional_features.as_deref().unwrap_or_default();
        if let Some(feature) = leg_features
            .iter()
            .find(|feature| !COMPUTED_LEG_FEATURES.contains(&feature.as_str()))
        {
            return Err(crate::scaffolded::Scaffolded::not_implemented(&format!(
                "CalculateRoutes with the LegAdditionalFeature {feature}"
            )));
        }
        let wants_step_narration = leg_features
            .iter()
            .any(|feature| feature == TRAVEL_STEP_INSTRUCTIONS_FEATURE);

        // The model pins `MaxAlternatives` to `0..5` and the boundary validator refuses
        // anything outside that before this handler runs. A value that nonetheless reaches
        // here out of range is REFUSED rather than quietly adjusted: answering 200 with a
        // different number of routes than was asked for would let a caller read an invalid
        // request as a served one.
        let alternatives = input.max_alternatives.unwrap_or(0);
        state::check_range(
            "MaxAlternatives",
            (0.0, f64::from(state::MAX_ALTERNATIVES)),
            f64::from(alternatives),
        )
        .map_err(|message| invalid("MaxAlternatives", message))?;

        // Documented on `Waypoints`: "Max length: 23". The member carries no modeled
        // length constraint, so the cap is the handler's to apply, and it is what bounds
        // the legs, the geometry and the label aggregation of one request.
        let submitted_waypoints = input.waypoints.as_deref().unwrap_or_default();
        if submitted_waypoints.len() > MAX_WAYPOINTS {
            return Err(invalid(
                "Waypoints",
                format!(
                    "at most {MAX_WAYPOINTS} waypoints are allowed between the origin and the destination; got {}",
                    submitted_waypoints.len()
                ),
            ));
        }

        let (anchor, clock_offset) = self.anchor(&input)?;

        // The requested sequence: origin, every waypoint, destination.
        let mut sequence = vec![SequencePoint {
            position: state::validate_position(&input.origin)
                .map_err(|message| invalid("Origin", message))?,
            matched_position: None,
            waypoint_index: None,
            is_stop: true,
            stop_duration_s: 0,
        }];
        for (index, waypoint) in submitted_waypoints.iter().enumerate() {
            // `StopDuration`'s modeled range is `0..49999`, which the boundary validator
            // does not enforce on a defaulted member, so the handler does: the value is
            // added to the route's clock, which must not be driven anywhere by it.
            let stop_duration_member = format!("Waypoints[{index}].StopDuration");
            state::check_range(
                &stop_duration_member,
                (0.0, MAX_STOP_DURATION_S as f64),
                waypoint.stop_duration as f64,
            )
            .map_err(|message| invalid(&stop_duration_member, message))?;
            let member = format!("Waypoints[{index}].Position");
            sequence.push(SequencePoint {
                position: state::validate_position(&waypoint.position)
                    .map_err(|message| invalid(&member, message))?,
                matched_position: None,
                waypoint_index: Some(index),
                is_stop: waypoint.pass_through != Some(true),
                stop_duration_s: waypoint.stop_duration,
            });
        }
        sequence.push(SequencePoint {
            position: state::validate_position(&input.destination)
                .map_err(|message| invalid("Destination", message))?,
            matched_position: None,
            waypoint_index: None,
            is_stop: true,
            stop_duration_s: 0,
        });

        let positions: Vec<Position> = sequence.iter().map(|point| point.position).collect();

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Every route of the answer is computed under ONE lock on the shared state: at
        // most `MAX_ALTERNATIVES + 1` routes, and the lock is taken once rather than per
        // route. The submitted positions are matched onto the road network under the same
        // lock, since that is what each leg's departure and arrival `Position` reports.
        let (computed_routes, pricing_bucket) = {
            let mut state = state_handle.lock();
            for point in &mut sequence {
                point.matched_position = state
                    .snap(point.position, travel_mode)
                    .map(|snapped| snapped.position);
            }
            let computed: Vec<state::CachedRoute> = (0..=u32::try_from(alternatives).unwrap_or(0))
                .map(|variant| state.compute_route(&positions, travel_mode, objective, variant))
                .collect();
            (computed, state.pricing_bucket(Self::NAME))
        };

        let mut routes = Vec::new();
        for computed in &computed_routes {
            let legs = merge_legs(&sequence, &computed.legs);
            // "Avoidance is on a best-case basis. If an avoidance can't be satisfied for a
            // particular case, it violates the avoidance and the returned response
            // produces a notice for the violation." The only avoidance this road model can
            // violate is of controlled-access highways: it carries no toll road, ferry,
            // tunnel, dirt road, car shuttle train, seasonal closure or zone to use, and
            // the computed path never turns back on itself.
            let violated_avoidance = input
                .avoid
                .as_ref()
                .is_some_and(|avoid| avoid.controlled_access_highways == Some(true))
                && computed.uses_controlled_access_highway;
            routes.push(self.route_output(
                &legs,
                &computed.major_roads,
                Rendering {
                    travel_mode,
                    geometry_format: &geometry_format,
                    anchor,
                    clock_offset,
                    violated_avoidance,
                    steps: StepRendering {
                        detail: step_detail,
                        language: &language,
                        narration,
                        measurement,
                        narrate: wants_step_narration,
                    },
                },
            )?);
        }

        Ok(CalculateRoutesOutput {
            leg_geometry_format: geometry_format,
            // The one thing worth a notice about a route over this road model: the
            // instructions could not be rendered in the language the request asked for, so
            // they fell back to the regional default.
            notices: if language_fell_back {
                vec![RouteResponseNotice {
                    code: "MainLanguageNotFound".to_string(),
                    impact: Some("Low".to_string()),
                }]
            } else {
                Vec::new()
            },
            pricing_bucket,
            routes,
        })
    }
}

impl<Eng> CalculateRoutes<Eng> {
    /// Where the route's clock is pinned, and the UTC offset the request stated it at.
    /// `DepartureTime`, `ArrivalTime` and `DepartNow`
    /// each pin it, so naming more than one is contradictory and refused; naming none
    /// departs now, which is what the service does for a request that gives no time.
    fn anchor(&self, input: &CalculateRoutesInput) -> Result<(Anchor, state::UtcOffset), CalculateRoutesError> {
        let depart_now = input.depart_now == Some(true);
        let named = usize::from(input.departure_time.is_some())
            + usize::from(input.arrival_time.is_some())
            + usize::from(depart_now);
        if named > 1 {
            return Err(invalid(
                "DepartureTime",
                "DepartureTime, ArrivalTime and DepartNow each set the time of the route, so at most one may be supplied"
                    .to_string(),
            ));
        }
        if let Some(departure) = &input.departure_time {
            return Ok((
                Anchor::Departure(
                    state::parse_timestamp("DepartureTime", departure)
                        .map_err(|message| invalid("DepartureTime", message))?,
                ),
                state::UtcOffset::stated_in(Some(departure)),
            ));
        }
        if let Some(arrival) = &input.arrival_time {
            return Ok((
                Anchor::Arrival(
                    state::parse_timestamp("ArrivalTime", arrival).map_err(|message| invalid("ArrivalTime", message))?,
                ),
                state::UtcOffset::stated_in(Some(arrival)),
            ));
        }
        // The instant comes from this service's clock rather than from the request, so
        // there is no offset it was stated at and it is reported in UTC.
        Ok((Anchor::Departure(state::now()), state::UtcOffset::UTC))
    }

    /// One `Route` of the answer, with its legs, labels and summary.
    fn route_output(
        &self,
        legs: &[MergedLeg],
        major_roads: &[TravelledRoad],
        rendering: Rendering<'_>,
    ) -> Result<Route, CalculateRoutesError> {
        let distance: i64 = state::clamp_cost(legs.iter().map(|leg| leg.distance_m as f64).sum());
        // What the route SPENDS: travelling, plus the stop at every waypoint it stops at.
        // A stop is time the traveller is on the route and not moving — the after-travel
        // step of the leg it ends — so it belongs to the route's duration, which is what a
        // client schedules the journey against. The per-leg travel-only summary is where
        // the travelling alone is reported.
        let travel_duration: i64 = state::clamp_cost(legs.iter().map(|leg| leg.duration_s as f64).sum());
        let stops: i64 = legs
            .iter()
            .map(|leg| leg.end.stop_duration_s)
            .fold(0_i64, i64::saturating_add);
        let duration: i64 = state::clamp_cost(travel_duration.saturating_add(stops) as f64);

        // The clock walks the legs forward from the departure. An `ArrivalTime` request
        // pins the far end instead, so the departure is the arrival less everything the
        // route spends: travel plus the stop at each intermediate waypoint. Every step is
        // saturating, so no request's numbers can drive the clock past what an `i64` of
        // seconds holds (the model caps each `StopDuration` at 49,999 and each duration at
        // 4,294,967,295, and the waypoints at 23, but the arithmetic does not rely on it).
        let mut clock = match rendering.anchor {
            Anchor::Departure(departure) => departure,
            Anchor::Arrival(arrival) => state::shift_seconds(arrival, duration.saturating_neg()),
        };

        let mut out = Vec::with_capacity(legs.len());
        for leg in legs {
            let departure = clock;
            let arrival = state::shift_seconds(departure, leg.duration_s);
            clock = state::shift_seconds(arrival, leg.end.stop_duration_s);
            out.push(self.leg_output(leg, rendering, departure, arrival)?);
        }

        Ok(Route {
            legs: out,
            major_road_labels: major_roads
                .iter()
                .map(|road| major_road_label(road, rendering.steps.language))
                .collect(),
            summary: Some(RouteSummary {
                distance,
                duration,
                tolls: None,
            }),
        })
    }

    /// One `RouteLeg`: its geometry in the requested format, and the details of the type
    /// the travel mode produces.
    fn leg_output(
        &self,
        leg: &MergedLeg,
        rendering: Rendering<'_>,
        departure: Instant,
        arrival: Instant,
    ) -> Result<RouteLeg, CalculateRoutesError> {
        let travel_mode = rendering.travel_mode;
        let violated_avoidance = rendering.violated_avoidance;
        let geometry = leg_geometry(&state::path_positions(&leg.path), rendering.geometry_format)?;
        let leg_type = LegType::for_mode(travel_mode);
        let departure_time = rendering
            .clock_offset
            .timestamp(departure)
            .ok_or_else(|| internal("the leg departure time is not representable"))?;
        let arrival_time = rendering
            .clock_offset
            .timestamp(arrival)
            .ok_or_else(|| internal("the leg arrival time is not representable"))?;

        let mut route_leg = RouteLeg {
            geometry,
            // The language the leg's instructions and localized names are rendered in,
            // which is the language actually used — not the tag that was asked for when
            // this emulator holds no text in it.
            language: Some(rendering.steps.language.to_string()),
            travel_mode: travel_mode.as_model().to_string(),
            r#type: leg_type.as_model().to_string(),
            ..Default::default()
        };

        // The leg's OVERVIEW covers "before travel, travel and after travel steps", so it
        // includes the stop performed at the waypoint the leg ends at; TRAVEL ONLY covers
        // the travel steps, so it does not.
        let overview_duration = state::clamp_cost(
            leg.duration_s
                .saturating_add(after_travel_stop(leg).unwrap_or(0)) as f64,
        );

        match leg_type {
            LegType::Pedestrian => {
                let steps = manoeuvres(leg, leg_type);
                route_leg.pedestrian_leg_details = Some(RoutePedestrianLegDetails {
                    after_travel_steps: after_travel_stop(leg)
                        .map(|duration_s| RoutePedestrianAfterTravelStep {
                            duration: duration_s,
                            instruction: (rendering.steps.detail == StepDetail::Default)
                                .then(|| stop_instruction(rendering.steps.narration.wait, duration_s)),
                            // The one value `RoutePedestrianAfterTravelStepType` declares.
                            r#type: "Wait".to_string(),
                        })
                        .into_iter()
                        .collect(),
                    arrival: RoutePedestrianArrival {
                        place: pedestrian_place(&leg.end),
                        time: Some(arrival_time),
                    },
                    departure: RoutePedestrianDeparture {
                        place: pedestrian_place(&leg.start),
                        time: Some(departure_time),
                    },
                    notices: Vec::new(),
                    pass_through_waypoints: pass_through_waypoints(leg),
                    // Spans are only computed for the features `SpanAdditionalFeatures`
                    // asks for, and this slice computes none of them.
                    spans: Vec::new(),
                    summary: Some(RoutePedestrianSummary {
                        overview: Some(RoutePedestrianOverviewSummary {
                            distance: leg.distance_m,
                            duration: overview_duration,
                        }),
                        travel_only: Some(RoutePedestrianTravelOnlySummary {
                            duration: leg.duration_s,
                        }),
                    }),
                    travel_steps: steps
                        .iter()
                        .map(|manoeuvre| pedestrian_step(manoeuvre, rendering.steps))
                        .collect(),
                });
            }
            LegType::Vehicle => {
                let steps = manoeuvres(leg, leg_type);
                route_leg.vehicle_leg_details = Some(RouteVehicleLegDetails {
                    after_travel_steps: after_travel_stop(leg)
                        .map(|duration_s| RouteVehicleAfterTravelStep {
                            // The model's charge details are for an electric vehicle's
                            // charging stop, which this model has no charger to compute.
                            charge_step_details: None,
                            duration: duration_s,
                            instruction: (rendering.steps.detail == StepDetail::Default)
                                .then(|| stop_instruction(rendering.steps.narration.park, duration_s)),
                            // The one value `RouteVehicleAfterTravelStepType` declares.
                            r#type: "Park".to_string(),
                        })
                        .into_iter()
                        .collect(),
                    arrival: RouteVehicleArrival {
                        place: vehicle_place(&leg.end),
                        time: Some(arrival_time),
                    },
                    departure: RouteVehicleDeparture {
                        place: vehicle_place(&leg.start),
                        time: Some(departure_time),
                    },
                    incidents: Vec::new(),
                    notices: if violated_avoidance {
                        // The route uses a controlled-access highway the request asked to
                        // avoid, which the model's own notice code names.
                        vec![RouteVehicleNotice {
                            code: "ViolatedAvoidControlledAccessHighway".to_string(),
                            details: Vec::new(),
                            impact: Some("High".to_string()),
                        }]
                    } else {
                        Vec::new()
                    },
                    pass_through_waypoints: pass_through_waypoints(leg),
                    spans: Vec::new(),
                    summary: Some(RouteVehicleSummary {
                        overview: Some(RouteVehicleOverviewSummary {
                            // This road model carries no traffic variation, so the best
                            // case and the typical case are the computed duration.
                            best_case_duration: overview_duration,
                            distance: leg.distance_m,
                            duration: overview_duration,
                            typical_duration: overview_duration,
                        }),
                        travel_only: Some(RouteVehicleTravelOnlySummary {
                            best_case_duration: leg.duration_s,
                            duration: leg.duration_s,
                            typical_duration: leg.duration_s,
                        }),
                    }),
                    tolls: Vec::new(),
                    toll_systems: Vec::new(),
                    travel_steps: steps
                        .iter()
                        .map(|manoeuvre| vehicle_step(manoeuvre, rendering.steps))
                        .collect(),
                    truck_road_types: Vec::new(),
                    zones: Vec::new(),
                });
            }
        }
        Ok(route_leg)
    }
}

/// The computed sub-legs grouped into the legs of the answer: a new leg starts at every
/// stop, so `N` waypoints that are stops give `N + 1` legs and a `PassThrough` waypoint
/// gives none of its own.
///
/// `compute_route` returns one sub-leg per consecutive pair of the sequence it was given,
/// so there is always a pair of sequence points for each one; a sub-leg without them is
/// skipped rather than indexed for.
fn merge_legs(sequence: &[SequencePoint], computed: &[CachedLeg]) -> Vec<MergedLeg> {
    let mut legs: Vec<MergedLeg> = Vec::new();
    let mut current: Option<MergedLeg> = None;
    for (index, leg) in computed.iter().enumerate() {
        let (Some(start), Some(end)) = (sequence.get(index).cloned(), sequence.get(index + 1).cloned()) else {
            continue;
        };
        match current.as_mut() {
            None => {
                current = Some(MergedLeg {
                    distance_m: leg.distance_m,
                    duration_s: leg.duration_s,
                    path: leg.path.clone(),
                    runs: Vec::new(),
                    pass_through: Vec::new(),
                    start,
                    end,
                });
            }
            Some(open) => {
                // `start` is the pass-through waypoint this sub-leg continues from: it
                // sits at the end of the geometry accumulated so far.
                let offset = open.path.len().saturating_sub(1);
                open.pass_through.push((start, offset));
                open.distance_m = state::clamp_cost(open.distance_m.saturating_add(leg.distance_m) as f64);
                open.duration_s = state::clamp_cost(open.duration_s.saturating_add(leg.duration_s) as f64);
                open.path.extend(leg.path.iter().skip(1).cloned());
                open.end = end;
            }
        }
        if current.as_ref().is_some_and(|open| open.end.is_stop) {
            legs.extend(current.take());
        }
    }
    legs.extend(current);
    // The runs of a leg are cut from the WHOLE of its geometry, so they are computed once
    // the sub-legs of a pass-through waypoint have been merged into it: a waypoint passed
    // through without stopping does not end a manoeuvre, and the runs of the merged leg
    // carry its merged distance and duration.
    for leg in &mut legs {
        leg.runs = state::leg_runs(&leg.path, leg.distance_m, leg.duration_s);
    }
    legs
}

/// A leg's geometry in the requested format: a Flexible Polyline string, or the plain
/// `LineString` the `Simple` format asks for.
fn leg_geometry(positions: &[Position], format: &str) -> Result<RouteLegGeometry, CalculateRoutesError> {
    if format == "Simple" {
        return Ok(RouteLegGeometry {
            line_string: Some(state::line_string(positions)),
            polyline: None,
        });
    }
    Ok(RouteLegGeometry {
        line_string: None,
        polyline: Some(
            state::encode_flexible_polyline(positions)
                .ok_or_else(|| internal("the leg geometry could not be encoded as a Flexible Polyline"))?,
        ),
    })
}

/// The place of a vehicle leg's departure or arrival.
fn vehicle_place(point: &SequencePoint) -> RouteVehiclePlace {
    RouteVehiclePlace {
        position: state::position_to_vec(point.reported_position()),
        original_position: Some(state::position_to_vec(point.position)),
        waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
        ..Default::default()
    }
}

/// The place of a pedestrian leg's departure or arrival.
fn pedestrian_place(point: &SequencePoint) -> RoutePedestrianPlace {
    RoutePedestrianPlace {
        position: state::position_to_vec(point.reported_position()),
        original_position: Some(state::position_to_vec(point.position)),
        waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
        ..Default::default()
    }
}

/// The language a step's instruction and localized road names are rendered in, and whether
/// the request asked for one this emulator has no text in.
///
/// The requested tags are resolved IN ORDER against the languages this emulator holds
/// narration in: the first whose primary subtag names one of them is the language used,
/// and the tag is reported as asked, so `en-GB` is answered in English and `es-ES` in
/// Spanish. Only a request where NONE of the tags resolves falls back to the regional
/// default, which the caller is told about through the model's `MainLanguageNotFound`
/// notice rather than by silently reading as a translation.
fn instruction_language(requested: &[String]) -> (String, &'static Narration, bool) {
    for tag in requested {
        let primary = primary_subtag(tag);
        if let Some(narration) = NARRATIONS
            .iter()
            .find(|narration| primary.eq_ignore_ascii_case(narration.language))
        {
            return (tag.clone(), narration, false);
        }
    }
    (
        DEFAULT_LANGUAGE_TAG.to_string(),
        DEFAULT_NARRATION,
        // A request that named no language asked for nothing that could not be provided.
        !requested.is_empty(),
    )
}

/// The primary language subtag of a BCP 47 tag: everything before the first subtag
/// separator, so `en-US` and `en` name the same language.
fn primary_subtag(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// The manoeuvre a travel step performs, as the model's travel-step-type enums name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Depart,
    Continue,
    ContinueHighway,
    EnterHighway,
    Exit,
    Keep,
    Turn,
    UTurn,
    Arrive,
}

impl StepKind {
    /// This manoeuvre as the model's enum value. `RouteVehicleTravelStepType` declares
    /// every one of them; `RoutePedestrianTravelStepType` declares only `Arrive`,
    /// `Continue`, `Depart`, `Keep` and `Turn`, which is exactly what [`StepKind::classify`]
    /// yields for a pedestrian leg.
    fn as_model(self) -> &'static str {
        match self {
            Self::Depart => "Depart",
            Self::Continue => "Continue",
            Self::ContinueHighway => "ContinueHighway",
            Self::EnterHighway => "EnterHighway",
            Self::Exit => "Exit",
            Self::Keep => "Keep",
            Self::Turn => "Turn",
            Self::UTurn => "UTurn",
            Self::Arrive => "Arrive",
        }
    }

    /// The manoeuvre at a junction: the angle the leg turns through there, the road it
    /// leaves and the road it joins.
    ///
    /// The road CLASS decides it first — joining, leaving or staying on a controlled-access
    /// highway is that manoeuvre whatever the angle — and otherwise the angle does: inside
    /// the straight band the leg continues, a shallow angle keeps to one side of a fork, a
    /// reversal is a U-turn and anything between is a turn. A pedestrian never travels a
    /// motorway of this network, and its step-type enum declares neither the highway
    /// manoeuvres nor a U-turn, so a pedestrian reversal is a turn.
    fn classify(
        leg_type: LegType,
        angle_deg: f64,
        from: Option<&TravelledRoad>,
        onto: Option<&TravelledRoad>,
    ) -> Self {
        let highway = |road: Option<&TravelledRoad>| road.is_some_and(|road| road.class.is_controlled_access());
        if leg_type == LegType::Vehicle {
            match (highway(from), highway(onto)) {
                (false, true) => return Self::EnterHighway,
                (true, false) => return Self::Exit,
                (true, true) => return Self::ContinueHighway,
                (false, false) => {}
            }
        }
        let angle = angle_deg.abs();
        if angle <= STRAIGHT_ANGLE_DEG {
            Self::Continue
        } else if angle <= KEEP_ANGLE_DEG {
            Self::Keep
        } else if angle >= UTURN_ANGLE_DEG && leg_type == LegType::Vehicle {
            Self::UTurn
        } else {
            Self::Turn
        }
    }
}

/// One manoeuvre of a leg, before it is rendered into the step shape of its leg type.
#[derive(Debug, Clone)]
struct Manoeuvre {
    kind: StepKind,
    /// Distance travelled from this manoeuvre to the next, in metres.
    distance_m: i64,
    /// Duration of that travel, in seconds.
    duration_s: i64,
    /// Offset into the leg's geometry of the vertex the manoeuvre is performed at.
    geometry_offset: usize,
    /// Signed angle the manoeuvre turns through, in degrees; `None` where the model
    /// determines none — a departure and an arrival turn through nothing.
    turn_angle_deg: Option<f64>,
    /// Names of the roads that meet in the junction the manoeuvre is performed in; empty
    /// where the model has no junction there.
    intersection: Vec<String>,
    /// The road travelled after the manoeuvre, which is the road it joins.
    current_road: Option<TravelledRoad>,
    /// The road the NEXT manoeuvre joins.
    next_road: Option<TravelledRoad>,
    /// The junction the manoeuvre is performed in, when the two roads cross.
    junction: Option<Junction>,
}

/// The manoeuvres of a leg: one per run of road it travels, then its arrival.
///
/// A run is a stretch of ONE road, so a run boundary is a junction of two and the manoeuvre
/// performed there is classified from the angle between them. The first run departs and the
/// last entry arrives at the leg's end, travelling nothing further.
fn manoeuvres(leg: &MergedLeg, leg_type: LegType) -> Vec<Manoeuvre> {
    let mut out = Vec::with_capacity(leg.runs.len() + 1);
    for (ordinal, run) in leg.runs.iter().enumerate() {
        let previous = ordinal.checked_sub(1).and_then(|index| leg.runs.get(index));
        let from = previous.and_then(|previous| previous.road.as_ref());
        let kind = match (ordinal, run.turn_angle_deg) {
            (0, _) => StepKind::Depart,
            (_, Some(angle)) => StepKind::classify(leg_type, angle, from, run.road.as_ref()),
            // Neither run gives a direction for this boundary, so the model determines no
            // angle there and the leg travels on rather than manoeuvring.
            (_, None) => StepKind::Continue,
        };
        out.push(Manoeuvre {
            kind,
            distance_m: run.distance_m,
            duration_s: run.duration_s,
            geometry_offset: run.geometry_offset,
            turn_angle_deg: run.turn_angle_deg,
            // The junction is where the road the leg leaves and the road it joins cross,
            // so the intersection is named by those two roads.
            intersection: run
                .junction
                .map(|_| {
                    [from, run.road.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(|road| road.label.clone())
                        .collect()
                })
                .unwrap_or_default(),
            current_road: run.road.clone(),
            next_road: leg.runs.get(ordinal + 1).and_then(|next| next.road.clone()),
            junction: run.junction,
        });
    }
    out.push(Manoeuvre {
        kind: StepKind::Arrive,
        distance_m: 0,
        duration_s: 0,
        geometry_offset: leg.path.len().saturating_sub(1),
        turn_angle_deg: None,
        intersection: Vec::new(),
        current_road: leg.runs.last().and_then(|run| run.road.clone()),
        next_road: None,
        junction: None,
    });
    out
}

/// One `RouteVehicleTravelStep`: the manoeuvre, its own details and the roads it joins.
fn vehicle_step(manoeuvre: &Manoeuvre, rendering: StepRendering<'_>) -> RouteVehicleTravelStep {
    let details = turn_details(manoeuvre, rendering.language);
    let kind = manoeuvre.kind;
    RouteVehicleTravelStep {
        continue_highway_step_details: (kind == StepKind::ContinueHighway).then(|| {
            RouteContinueHighwayStepDetails {
                intersection: details.intersection.clone(),
                steering_direction: details.steering_direction.clone(),
                turn_angle: details.turn_angle,
                turn_intensity: details.turn_intensity.clone(),
            }
        }),
        continue_step_details: (kind == StepKind::Continue).then(|| RouteContinueStepDetails {
            intersection: details.intersection.clone(),
        }),
        current_road: rendering
            .narrate
            .then_some(manoeuvre.current_road.as_ref())
            .flatten()
            .map(|road| route_road(road, rendering.language)),
        distance: manoeuvre.distance_m,
        duration: manoeuvre.duration_s,
        enter_highway_step_details: (kind == StepKind::EnterHighway).then(|| RouteEnterHighwayStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        // `ExitNumber` is a list this service computes, so it is reported rather than
        // omitted: this network numbers no road exit, so no step takes one and the list is
        // empty.
        exit_number: Some(Vec::new()),
        exit_step_details: (kind == StepKind::Exit).then(|| RouteExitStepDetails {
            intersection: details.intersection.clone(),
            // `RelativeExit` is which of several exits to take, which a junction of two
            // runs does not have.
            relative_exit: None,
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        geometry_offset: i32::try_from(manoeuvre.geometry_offset).ok(),
        instruction: (rendering.narrate && rendering.detail == StepDetail::Default)
            .then(|| instruction(manoeuvre, rendering)),
        keep_step_details: (kind == StepKind::Keep).then(|| RouteKeepStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        next_road: rendering
            .narrate
            .then_some(manoeuvre.next_road.as_ref())
            .flatten()
            .map(|road| route_road(road, rendering.language)),
        // The network holds no ramp and no roundabout, so no step of a route over it is
        // one: an empty detail structure in their place would be a manoeuvre invented.
        ramp_step_details: None,
        roundabout_enter_step_details: None,
        roundabout_exit_step_details: None,
        roundabout_pass_step_details: None,
        signpost: (rendering.detail == StepDetail::TurnByTurn)
            .then(|| signpost(manoeuvre, rendering.language))
            .flatten(),
        turn_step_details: (kind == StepKind::Turn).then(|| RouteTurnStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        r#type: kind.as_model().to_string(),
        u_turn_step_details: (kind == StepKind::UTurn).then(|| RouteUTurnStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
    }
}

/// One `RoutePedestrianTravelStep`. A pedestrian leg travels no motorway of this network,
/// so none of the highway manoeuvres its step-type enum leaves out can arise on one.
fn pedestrian_step(manoeuvre: &Manoeuvre, rendering: StepRendering<'_>) -> RoutePedestrianTravelStep {
    let details = turn_details(manoeuvre, rendering.language);
    let kind = manoeuvre.kind;
    RoutePedestrianTravelStep {
        continue_step_details: (kind == StepKind::Continue).then(|| RouteContinueStepDetails {
            intersection: details.intersection.clone(),
        }),
        current_road: rendering
            .narrate
            .then_some(manoeuvre.current_road.as_ref())
            .flatten()
            .map(|road| route_road(road, rendering.language)),
        distance: manoeuvre.distance_m,
        duration: manoeuvre.duration_s,
        // See `vehicle_step`: the list is computed and empty, not unknown.
        exit_number: Some(Vec::new()),
        geometry_offset: i32::try_from(manoeuvre.geometry_offset).ok(),
        instruction: (rendering.narrate && rendering.detail == StepDetail::Default)
            .then(|| instruction(manoeuvre, rendering)),
        keep_step_details: (kind == StepKind::Keep).then(|| RouteKeepStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        next_road: rendering
            .narrate
            .then_some(manoeuvre.next_road.as_ref())
            .flatten()
            .map(|road| route_road(road, rendering.language)),
        roundabout_enter_step_details: None,
        roundabout_exit_step_details: None,
        roundabout_pass_step_details: None,
        signpost: (rendering.detail == StepDetail::TurnByTurn)
            .then(|| signpost(manoeuvre, rendering.language))
            .flatten(),
        turn_step_details: (kind == StepKind::Turn).then(|| RouteTurnStepDetails {
            intersection: details.intersection.clone(),
            steering_direction: details.steering_direction.clone(),
            turn_angle: details.turn_angle,
            turn_intensity: details.turn_intensity.clone(),
        }),
        r#type: kind.as_model().to_string(),
    }
}

/// The members every manoeuvre detail structure of the model carries: where the manoeuvre
/// is performed, which way it steers and how far through it turns.
#[derive(Debug, Clone)]
struct TurnDetails {
    intersection: Vec<LocalizedString>,
    steering_direction: Option<String>,
    turn_angle: f64,
    turn_intensity: Option<String>,
}

/// The manoeuvre details of a step, read off the angle the junction determined.
fn turn_details(manoeuvre: &Manoeuvre, language: &str) -> TurnDetails {
    let angle = manoeuvre.turn_angle_deg.unwrap_or_default();
    TurnDetails {
        intersection: manoeuvre
            .intersection
            .iter()
            .map(|name| localized(name, language))
            .collect(),
        steering_direction: Some(steering_direction(angle).to_string()),
        turn_angle: angle,
        turn_intensity: Some(turn_intensity(angle).to_string()),
    }
}

/// `RouteSteeringDirection` of a manoeuvre turning `angle_deg`: a turn to the right is a
/// positive angle (clockwise from the bearing travelled in), one to the left negative, and
/// an angle inside the straight band steers straight on.
fn steering_direction(angle_deg: f64) -> &'static str {
    if angle_deg.abs() <= STRAIGHT_ANGLE_DEG {
        "Straight"
    } else if angle_deg > 0.0 {
        "Right"
    } else {
        "Left"
    }
}

/// `RouteTurnIntensity` of a manoeuvre turning `angle_deg`.
fn turn_intensity(angle_deg: f64) -> &'static str {
    let angle = angle_deg.abs();
    if angle < SLIGHT_TURN_ANGLE_DEG {
        "Slight"
    } else if angle >= SHARP_TURN_ANGLE_DEG {
        "Sharp"
    } else {
        "Typical"
    }
}

/// A `RouteRoad` for a road the leg travels: how the network identifies it, and the road
/// type its class is.
///
/// A numbered road carries its `RouteNumber`; `Towards` stays the empty required list,
/// because the network names no destination reachable along a road.
fn route_road(road: &TravelledRoad, language: &str) -> RouteRoad {
    RouteRoad {
        road_name: vec![localized(&road.label, language)],
        route_number: road
            .route_number()
            .map(|number| route_number(&number, language))
            .into_iter()
            .collect(),
        towards: Vec::new(),
        r#type: Some(road_type(road.class).to_string()),
    }
}

/// A `RouteMajorRoadLabel` for one of the roads that "differentiate the current route from
/// the alternatives presented".
///
/// The two members of the shape are the two ways a road is identified, so the label carries
/// the one the road HAS: a numbered road is labelled by its `RouteNumber` — which is what a
/// client renders a highway shield from — and an unnumbered one by its `RoadName`. Either
/// way the string declares the language it is rendered in, which is what a localized
/// rendering keys on.
fn major_road_label(road: &TravelledRoad, language: &str) -> RouteMajorRoadLabel {
    match road.route_number() {
        Some(number) => RouteMajorRoadLabel {
            road_name: None,
            route_number: Some(route_number(&number, language)),
        },
        None => RouteMajorRoadLabel {
            road_name: Some(localized(&road.label, language)),
            route_number: None,
        },
    }
}

/// A `RouteNumber` in the language the road text is rendered in. `Direction` is the
/// directional identifier of a carriageway, which the roads of this network do not carry.
fn route_number(value: &str, language: &str) -> RouteNumber {
    RouteNumber {
        direction: None,
        language: Some(language.to_string()),
        value: value.to_string(),
    }
}

/// `RouteRoadType` of a road class: a controlled-access road of the network — a motorway
/// or a trunk road — is a `Highway`, and a street or footpath of the grid is `Urban` — at
/// its ~55 m spacing the network is a city grid, and nothing in it is `Rural`.
fn road_type(class: RoadClass) -> &'static str {
    match class {
        RoadClass::Motorway | RoadClass::Trunk => "Highway",
        RoadClass::Street | RoadClass::Footpath => "Urban",
    }
}

/// The sign at the junction a step is performed in. The network holds no sign text, but a
/// junction of two named roads is signed by the road it leads onto, so the label carries
/// that road's name. `None` where the model has no junction for the step — a departure, an
/// arrival, or a boundary whose two runs do not cross — since then there is no sign.
fn signpost(manoeuvre: &Manoeuvre, language: &str) -> Option<RouteSignpost> {
    let road = manoeuvre.junction.and(manoeuvre.current_road.as_ref())?;
    Some(RouteSignpost {
        labels: vec![RouteSignpostLabel {
            // A sign identifies a numbered road by its number, as `route_road` does.
            route_number: road.route_number().map(|number| route_number(&number, language)),
            text: Some(localized(&road.label, language)),
        }],
    })
}

/// A `LocalizedString` in the language the step text is rendered in.
fn localized(value: &str, language: &str) -> LocalizedString {
    LocalizedString {
        language: Some(language.to_string()),
        value: value.to_string(),
    }
}

/// The instruction text of a step: the manoeuvre performed, the road it joins and how far
/// it then runs.
///
/// The network carries the text of no instruction, so the sentence is BUILT from the
/// manoeuvre the junction determined, the road name the network holds and the step's own
/// distance in the requested measurement system. Nothing in it is read from the request.
fn instruction(manoeuvre: &Manoeuvre, rendering: StepRendering<'_>) -> String {
    let narration = rendering.narration;
    let road = manoeuvre
        .current_road
        .as_ref()
        .map_or_else(|| narration.road_ahead.to_string(), |road| road.label.clone());
    let distance = distance_text(manoeuvre.distance_m, rendering.measurement);
    let steering = narration.steering(steering_direction(manoeuvre.turn_angle_deg.unwrap_or_default()));
    let template = match manoeuvre.kind {
        StepKind::Depart => narration.depart,
        StepKind::Continue => narration.continue_on,
        StepKind::ContinueHighway => narration.continue_highway,
        StepKind::EnterHighway => narration.enter_highway,
        StepKind::Exit => narration.exit_highway,
        StepKind::Keep => narration.keep,
        StepKind::Turn => narration.turn,
        StepKind::UTurn => narration.u_turn,
        StepKind::Arrive => narration.arrive,
    };
    Narration::fill(template, &road, &distance, steering)
}

/// `distance_m` as an instruction states it: metres below a kilometre and kilometres above
/// it for `Metric`, feet below a mile and miles above it for `Imperial`. The conversions
/// are the exact definitions of the units — a foot is 0.3048 m and a mile 1,609.344 m.
fn distance_text(distance_m: i64, measurement: Measurement) -> String {
    let metres = distance_m.max(0) as f64;
    match measurement {
        Measurement::Metric if metres < METRES_PER_KILOMETRE => format!("{} m", distance_m.max(0)),
        Measurement::Metric => format!("{:.1} km", metres / METRES_PER_KILOMETRE),
        Measurement::Imperial if metres < METRES_PER_MILE => format!("{:.0} ft", metres / METRES_PER_FOOT),
        Measurement::Imperial => format!("{:.1} mi", metres / METRES_PER_MILE),
    }
}

/// The instruction of an after-travel step: the phrase for the stop, stating how long the
/// traveller stays there.
fn stop_instruction(template: &str, duration_s: i64) -> String {
    template.replace("{duration}", &duration_text(duration_s))
}

/// A duration as an instruction states it: seconds below a minute and whole minutes above.
fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < SECONDS_PER_MINUTE {
        format!("{seconds} s")
    } else {
        format!("{} min", seconds / SECONDS_PER_MINUTE)
    }
}

/// How long the route stops at the end of a leg, when it stops there at all: the
/// `StopDuration` of the waypoint the leg ends at. A leg that ends at a waypoint with no
/// stop duration — the route's destination included — has nothing to perform after
/// travelling, so it carries no after-travel step.
fn after_travel_stop(leg: &MergedLeg) -> Option<i64> {
    (leg.end.stop_duration_s > 0).then_some(leg.end.stop_duration_s)
}

/// The pass-through waypoints of a leg, each with its offset into the leg's geometry.
fn pass_through_waypoints(leg: &MergedLeg) -> Vec<RoutePassThroughWaypoint> {
    leg.pass_through
        .iter()
        .map(|(point, offset)| RoutePassThroughWaypoint {
            geometry_offset: i32::try_from(*offset).ok(),
            place: RoutePassThroughPlace {
                position: state::position_to_vec(point.reported_position()),
                original_position: Some(state::position_to_vec(point.position)),
                waypoint_index: point.waypoint_index.and_then(|index| i32::try_from(index).ok()),
            },
        })
        .collect()
}
