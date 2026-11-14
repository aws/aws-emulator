// SPDX-License-Identifier: CC0-1.0

//! The ROAD GRAPH of this service's network, and the traversal every route metric is read
//! off it.
//!
//! A route is travel ALONG ROADS. The grid carries no diagonal, so a leg leaves the road
//! its origin matched onto, crosses onto the roads that lead towards its destination, and
//! arrives on the road the destination matched onto. Two numbers follow from that path and
//! from nothing else:
//!
//! * its DISTANCE is the sum of the stretches it travels — never the straight line between
//!   its endpoints, which no road of the network follows;
//! * its DURATION is each stretch's length at the posted speed of the road THAT stretch
//!   runs on, held down to the travel mode's own top speed wherever the mode cannot keep up
//!   with the road.
//!
//! That is why the two hang together the way a road answer does: a leg long enough to be
//! worth reaching the fast classes for travels most of its length near their posted speed,
//! while a short leg spends a larger share of its length on the local roads it needs to
//! reach them and so comes out slower. No single average speed is applied to anything.
//!
//! WHICH roads a leg takes is chosen here, by the request's own objective. A bounded set of
//! candidate through-roads per orientation ([`MAX_CANDIDATES`]) — the road each end sits on,
//! the roads of each numbered class around the corridor, and roads further out again — is
//! costed by [`cost_of`], and the cheapest in the objective's own currency (time for
//! `FastestRoute`, length for `ShortestRoute`) is the route. The next-best ways with a
//! distinct length are this leg's alternatives, so an alternative is another path of the
//! same graph rather than the same path pushed sideways.
//!
//! The work per leg is bounded by the network's own rule and by nothing a request can grow:
//! at most [`MAX_CANDIDATES`] roads per orientation are weighed, a way is at most
//! [`MAX_ROADS`] roads long, and the graph is read off the grid's arithmetic rather than
//! searched, so no request chooses how far the search runs.

use super::*;

/// One stretch of a traversal: travel along ONE road of the network, from `from` to `to`.
#[derive(Debug, Clone)]
pub struct Stretch {
    /// The road travelled, as a leg's travel step reports it.
    pub road: TravelledRoad,
    /// Where the stretch starts: the leg's own start, or the junction it joined this road
    /// in.
    pub from: Position,
    /// Where it ends: the junction it leaves this road in, or the leg's own end.
    pub to: Position,
    /// Its length in metres, by the same geodesic the reported geometry's length is
    /// measured with.
    pub length_m: f64,
    /// What travelling it takes, in seconds: its length at [`GridRoad::pace`].
    pub seconds: f64,
}

/// A road of the grid, as the traversal names one: which way it runs and which multiple of
/// [`ROAD_SPACING_DEG`] it sits at.
///
/// It is the ROAD and not one of the per-cell [`RoadSegment`]s the network materialises
/// pieces of it as, so a traversal reads the graph off the grid's own rule and materialises
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridRoad {
    orientation: RoadOrientation,
    index: i64,
}

impl GridRoad {
    /// The road a matched position sits on.
    fn of(road: &TravelledRoad) -> Self {
        Self {
            orientation: road.orientation,
            index: road.grid_index,
        }
    }

    /// This road's class, from the grid's own rule.
    fn class(self) -> RoadClass {
        class_of(self.orientation, self.index)
    }

    /// Whether `mode` may travel this road.
    fn allows(self, mode: TravelMode) -> bool {
        allows_of(self.class()).contains(&mode)
    }

    /// The pace travel makes along this road, in metres per second: the road's own posted
    /// speed, held down to the travel mode's top speed wherever the mode is the slower of
    /// the two. Neither alone is the speed of travel — a vehicle does not exceed the road's
    /// limit, and no road lets a pedestrian walk at a motorist's pace.
    fn pace(self, top_speed_mps: f64) -> f64 {
        (speed_of(self.class()) * KPH_TO_MPS)
            .min(top_speed_mps)
            .max(f64::MIN_POSITIVE)
    }

    /// The coordinate this road sits at, in degrees: its longitude where it runs
    /// north-south, its latitude where it runs east-west.
    fn coordinate(self) -> f64 {
        self.index as f64 * ROAD_SPACING_DEG
    }

    /// Where this road and `other` cross. The roads of a way alternate orientation, so the
    /// two passed here always do cross: a meridian road's longitude and a parallel road's
    /// latitude.
    fn crossing(self, other: Self) -> Position {
        let (meridian, parallel) = match self.orientation {
            RoadOrientation::Meridian => (self, other),
            RoadOrientation::Parallel => (other, self),
        };
        round_to_network_precision([meridian.coordinate(), parallel.coordinate()])
    }

    /// This road as a leg reports the road it travels.
    fn travelled(self) -> TravelledRoad {
        road_at(self.orientation, self.index)
    }
}

/// Most roads a way is built of: each end's own road, and the through-roads between them —
/// one where the two ends' roads cross each other, three where they run parallel.
const MAX_ROADS: usize = 5;

/// Most candidate through-roads weighed per orientation.
const MAX_CANDIDATES: usize = 8;

/// How far outside the corridor a way may reach for a through-road, in multiples of the
/// spacing between numbered roads. These are the ways that make a leg's alternatives
/// distinct from its route.
const DETOUR_STEPS: i64 = 2;

/// One way through the graph: the roads it travels in order, and what it costs.
#[derive(Debug, Clone, Copy)]
struct Way {
    roads: [GridRoad; MAX_ROADS],
    roads_len: usize,
    length_m: f64,
    seconds: f64,
}

impl Way {
    /// The roads of this way, in travel order.
    fn roads(&self) -> &[GridRoad] {
        &self.roads[..self.roads_len]
    }
}

/// The way through the graph a leg takes between two matched positions, as the stretches it
/// travels in order.
///
/// `variant` 0 is the route the objective chooses; 1, 2, … are the next-best ways with a
/// distinct length, which is what this service answers an alternative with. A leg whose
/// graph offers fewer distinct ways than that answers with its best one.
pub fn traverse(
    start: &SnappedPosition,
    end: &SnappedPosition,
    mode: TravelMode,
    objective: RoutingObjective,
    top_speed_mps: f64,
    variant: u32,
) -> Vec<Stretch> {
    let (first, last) = (GridRoad::of(&start.road), GridRoad::of(&end.road));
    let (from, to) = (start.position, end.position);

    let meridians = through_roads(RoadOrientation::Meridian, from[0], to[0], mode);
    let parallels = through_roads(RoadOrientation::Parallel, from[1], to[1], mode);
    // The roads of a way alternate orientation, and both ends' own roads are fixed, so the
    // orientation of every through-road between them is fixed too.
    let (crossing, along) = match first.orientation {
        RoadOrientation::Meridian => (&parallels, &meridians),
        RoadOrientation::Parallel => (&meridians, &parallels),
    };
    let crossing_roads = |index: i64| GridRoad {
        orientation: crossing_orientation(first.orientation),
        index,
    };
    let along_roads = |index: i64| GridRoad {
        orientation: first.orientation,
        index,
    };

    let mut ways: Vec<Way> = Vec::with_capacity(crossing.len() * along.len());
    let mut weigh = |roads: [GridRoad; MAX_ROADS], roads_len: usize| {
        let (length_m, seconds) = cost_of(&roads[..roads_len], from, to, top_speed_mps);
        ways.push(Way {
            roads,
            roads_len,
            length_m,
            seconds,
        });
    };
    if first.orientation == last.orientation {
        // The two ends run parallel and never meet: a way crosses onto a through-road near
        // each end and runs between them on a road of the ends' own orientation.
        for &near in crossing {
            for &between in along {
                for &far in crossing {
                    weigh(
                        [
                            first,
                            crossing_roads(near),
                            along_roads(between),
                            crossing_roads(far),
                            last,
                        ],
                        5,
                    );
                }
            }
        }
    } else {
        // The two ends' roads cross each other, so one through-road of each orientation
        // joins them.
        for &near in crossing {
            for &between in along {
                weigh(
                    [first, crossing_roads(near), along_roads(between), last, last],
                    4,
                );
            }
        }
    }

    // Cheapest first in the objective's own currency, the other cost breaking a tie: the
    // objective is what "optimize routing for" asks for, and it reaches the path itself
    // rather than a multiplier on it.
    ways.sort_by(|a, b| {
        objective_cost(a, objective)
            .total_cmp(&objective_cost(b, objective))
            .then_with(|| other_cost(a, objective).total_cmp(&other_cost(b, objective)))
    });

    // One way per distinct length: an alternative that reported the route's own distance
    // would not be another route. The lengths kept are the alternatives already answered,
    // which the model's own `MaxAlternatives` range bounds.
    let mut lengths: Vec<i64> = Vec::new();
    let mut chosen = ways.first().copied();
    for way in &ways {
        let length = clamp_cost(way.length_m);
        if lengths.contains(&length) {
            continue;
        }
        lengths.push(length);
        if lengths.len() > variant as usize {
            chosen = Some(*way);
            break;
        }
    }
    chosen
        .map(|way| stretches_of(&way, from, to, top_speed_mps))
        .unwrap_or_default()
}

/// The orientation a road of `orientation` crosses.
fn crossing_orientation(orientation: RoadOrientation) -> RoadOrientation {
    match orientation {
        RoadOrientation::Meridian => RoadOrientation::Parallel,
        RoadOrientation::Parallel => RoadOrientation::Meridian,
    }
}

/// What the objective a request asked for minimizes, out of a way's two costs.
fn objective_cost(way: &Way, objective: RoutingObjective) -> f64 {
    match objective {
        RoutingObjective::FastestRoute => way.seconds,
        RoutingObjective::ShortestRoute => way.length_m,
    }
}

/// The cost the objective does not minimize, which breaks a tie between two ways that cost
/// the same in the one it does.
fn other_cost(way: &Way, objective: RoutingObjective) -> f64 {
    match objective {
        RoutingObjective::FastestRoute => way.length_m,
        RoutingObjective::ShortestRoute => way.seconds,
    }
}

/// The length and the duration of the way `roads` travels from `from` to `to`.
///
/// Each stretch runs ALONG one road, so it is measured by the same geodesic the reported
/// geometry's length is measured with and costed at that road's own pace. The sum is
/// therefore never shorter than the staircase the grid forces on a diagonal journey, and
/// never quicker than the roads travelled allow — and because it is the measure the
/// response reports, the way this picks as the shortest is the one that reports the
/// shortest distance.
fn cost_of(roads: &[GridRoad], from: Position, to: Position, top_speed_mps: f64) -> (f64, f64) {
    let (mut length_m, mut seconds) = (0.0, 0.0);
    let mut at = from;
    for (index, road) in roads.iter().enumerate() {
        let next = match roads.get(index + 1) {
            Some(next) => road.crossing(*next),
            None => to,
        };
        let metres = haversine_m(at, next);
        length_m += metres;
        seconds += metres / road.pace(top_speed_mps);
        at = next;
    }
    (length_m, seconds)
}

/// The stretches of `way`, each measured by the geodesic the geometry carries and costed at
/// its own road's pace.
///
/// A stretch of no length at all is where two roads of the way cross in one point rather
/// than travel the traveller makes, so it is not reported: the stretches that remain still
/// run end to end, because each starts where the one before it ended.
fn stretches_of(way: &Way, from: Position, to: Position, top_speed_mps: f64) -> Vec<Stretch> {
    let roads = way.roads();
    let mut stretches = Vec::with_capacity(roads.len());
    let mut at = from;
    for (index, road) in roads.iter().enumerate() {
        let next = match roads.get(index + 1) {
            Some(next) => road.crossing(*next),
            None => to,
        };
        let length_m = haversine_m(at, next);
        if length_m > 0.0 {
            stretches.push(Stretch {
                road: road.travelled(),
                from: at,
                to: next,
                length_m,
                seconds: length_m / road.pace(top_speed_mps),
            });
        }
        at = next;
    }
    stretches
}

/// The roads of `orientation` weighed as a through-road between the two ends of a leg.
///
/// `from` and `to` are the ends' coordinates on the axis roads of this orientation sit at:
/// the LONGITUDE of a road running north-south, the LATITUDE of one running east-west. A
/// road between the two carries travel at no detour at all; one outside them is a detour,
/// which pays only where the road it reaches is enough faster to earn it back — so the set
/// carries both kinds and [`cost_of`] decides. Every road in it is one `mode` may travel.
fn through_roads(orientation: RoadOrientation, from: f64, to: f64, mode: TravelMode) -> Vec<i64> {
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    let lo_index = (lo / ROAD_SPACING_DEG).round() as i64;
    let hi_index = (hi / ROAD_SPACING_DEG).round() as i64;
    let mut candidates: Vec<i64> = Vec::with_capacity(MAX_CANDIDATES);
    {
        let mut push = |index: i64| {
            if candidates.len() < MAX_CANDIDATES
                && !candidates.contains(&index)
                && (GridRoad { orientation, index }).allows(mode)
            {
                candidates.push(index);
            }
        };
        // The roads the two ends sit beside: travel along them is travel with no detour.
        push(nearest_usable(orientation, lo_index, mode));
        push(nearest_usable(orientation, hi_index, mode));
        // The numbered classes, which is what a leg long enough to pay for reaching one
        // travels: the road of each class nearest each end, on either side of it.
        for class in [RoadClass::Motorway, RoadClass::Trunk] {
            if !allows_of(class).contains(&mode) {
                continue;
            }
            let every = spacing_of(class);
            for anchor in [lo_index, hi_index] {
                let base = anchor.div_euclid(every);
                push(base * every);
                push((base + 1) * every);
            }
        }
        // Further out again: a longer way through the graph, which is what makes a leg's
        // alternatives distinct from its route.
        for step in 1..=DETOUR_STEPS {
            push(nearest_usable(orientation, lo_index - step * TRUNK_EVERY, mode));
            push(nearest_usable(orientation, hi_index + step * TRUNK_EVERY, mode));
        }
    }
    candidates
}

/// The road of `orientation` nearest grid index `index` that `mode` may travel.
///
/// A mode barred from the numbered roads still has the local streets, which sit at all but
/// a few of the grid's indices, so this is over in a step or two; an index whose road no
/// mode of this network may use is answered with itself rather than searched for forever.
fn nearest_usable(orientation: RoadOrientation, index: i64, mode: TravelMode) -> i64 {
    for step in 0..=TRUNK_EVERY {
        for candidate in [index + step, index - step] {
            if (GridRoad { orientation, index: candidate }).allows(mode) {
                return candidate;
            }
        }
    }
    index
}
