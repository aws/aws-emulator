// SPDX-License-Identifier: CC0-1.0

//! The Spot price of an instance type in one Availability Zone: the market price a Spot
//! Instance request's maximum price is weighed against, and the one `DescribeSpotPriceHistory`
//! reports a history of.
//!
//! ONE derivation, in one place, because the two readers have to agree: a client that read the
//! price history to pick a bid and the request it then places are talking about the same
//! market. "If you specify a maximum price, your instances will be interrupted more frequently
//! than if you do not specify this parameter" (`RequestSpotInstancesRequest$SpotPrice`) and a
//! bid below the price in force buys nothing at all, so the request stays open rather than
//! being fulfilled — which it can only do if the price it is judged against is the published
//! one.
//!
//! DERIVED, not drawn: the same inputs always answer the same price, so repeated reads report a
//! stable price and nothing has to be stored for one. A price is a share of this crate's ONE
//! On-Demand hour ([`super::on_demand_hour_micros`]), so a Spot price and a Reserved Instance
//! offering are shares of the same rate rather than of two, and a price stays ordered by the
//! rate it is a share of.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{AvailabilityZoneRecord, availability_zones_in_region, instance_type_record};

/// How often a Spot price changes: the span between two consecutive change points, which is
/// what makes a history a list of CHANGES rather than one entry per instant.
///
/// CHOSEN: AWS publishes no schedule on which a Spot price moves, and neither the model nor a
/// request carries one. A caller can rely on the RULE — a price changes at a point on this
/// grid, is in force until the next one, and is reported with the instant it took effect — not
/// on the span.
pub const SECONDS_PER_SPOT_PRICE_PERIOD: i64 = 24 * 60 * 60;

/// The lowest percentage of the On-Demand rate a Spot price falls to, and the highest. A
/// Spot price is the On-Demand rate discounted into that band, which is what keeps every
/// price below the On-Demand rate and above zero.
///
/// CHOSEN, like the rates themselves: AWS publishes the discount as varying with supply
/// and demand, which this emulator holds no data for.
pub const SPOT_FLOOR_PERCENT: u64 = 30;

/// The top of that band. See [`SPOT_FLOOR_PERCENT`].
pub const SPOT_CEILING_PERCENT: u64 = 70;

/// How far along the band the share moves from one change point to the next, and how far it
/// is offset for each further Availability Zone of the Region.
///
/// CHOSEN, with the band itself: the two only have to be shares of the rate that MOVE — a
/// price history whose price never changed would be a history of nothing, and two zones of a
/// Region do not sell their spare capacity at one price. Both steps are coprime with the
/// band's width, so the share walks the whole band before it repeats.
pub const SHARE_STEP_PERCENT: u64 = 7;

/// The per-zone offset. See [`SHARE_STEP_PERCENT`].
pub const ZONE_SHARE_OFFSET_PERCENT: u64 = 13;

/// The micro-dollars a reported price is a whole multiple of: AWS publishes a Spot price to
/// four decimal places of the currency, which is this many micro-dollars.
pub const PRICE_QUANTUM_MICROS: u64 = 100;

/// The qualifier a `RIProductDescription` carries for the VPC form of a platform.
const AMAZON_VPC_QUALIFIER: &str = " (Amazon VPC)";

/// The platform whose price carries a licence premium: the `Windows` member of the modelled
/// `RIProductDescription` enum, as [`platform_of`] reads it.
pub const WINDOWS_PLATFORM: &str = "Windows";

/// The product description of a Spot price with the Amazon VPC qualifier stripped: the
/// PLATFORM the price is for.
///
/// `Linux/UNIX` and `Linux/UNIX (Amazon VPC)` are the same platform reported for the two
/// network kinds a Spot Instance can run in, and AWS prices them alike, so the qualifier is
/// removed before anything is priced rather than being priced as a second platform.
#[must_use]
pub fn platform_of(product_description: &str) -> &str {
    product_description
        .strip_suffix(AMAZON_VPC_QUALIFIER)
        .unwrap_or(product_description)
}

/// What one hour of `record`'s instance type costs On-Demand on the platform `platform`
/// prices, in micro-dollars: the crate's one On-Demand hour
/// ([`super::on_demand_hour_micros`]), told whether that platform carries a licence premium.
#[must_use]
pub fn on_demand_micros(record: &super::InstanceTypeRecord, platform: &str) -> u64 {
    super::on_demand_hour_micros(record, platform_of(platform) == WINDOWS_PLATFORM)
}

/// Where `zone` sits in `zones`, the Availability Zones of its Region: the zone's own
/// position, which is what [`spot_share_percent`] reads a per-zone share from. A zone the
/// list does not hold is read as the Region's first.
#[must_use]
pub fn zone_position(zones: &[AvailabilityZoneRecord], zone: &AvailabilityZoneRecord) -> usize {
    zones
        .iter()
        .position(|candidate| candidate.zone_id == zone.zone_id)
        .unwrap_or(0)
}

/// The share of the On-Demand rate the Spot price of the Region's `zone_index`-th
/// Availability Zone is at the change point `period`, as a percentage inside the band
/// [`SPOT_FLOOR_PERCENT`]..=[`SPOT_CEILING_PERCENT`].
///
/// The share walks the band in [`SHARE_STEP_PERCENT`] steps, one step per change point,
/// offset by [`ZONE_SHARE_OFFSET_PERCENT`] per zone: so it moves from each change point to
/// the next, two zones of one Region differ at the same change point, and the same zone and
/// change point always answer the same share.
#[must_use]
pub fn spot_share_percent(zone_index: usize, period: i64) -> u64 {
    let band = SPOT_CEILING_PERCENT - SPOT_FLOOR_PERCENT + 1;
    let zone = u64::try_from(zone_index).unwrap_or(0);
    // The change point is a count of periods since the epoch, which is negative before it;
    // `rem_euclid` is what keeps the walk inside the band on either side of it.
    let steps = period
        .rem_euclid(i64::try_from(band).unwrap_or(1))
        .unsigned_abs();
    let walked = steps
        .saturating_mul(SHARE_STEP_PERCENT)
        .saturating_add(zone.saturating_mul(ZONE_SHARE_OFFSET_PERCENT));
    SPOT_FLOOR_PERCENT + walked % band
}

/// The Spot price in force from change point `period` in the Region's `zone_index`-th
/// Availability Zone, for an instance type whose On-Demand rate on the priced platform is
/// `on_demand_micros`, in micro-dollars: that rate at the share [`spot_share_percent`] gives.
///
/// The SHARE is a property of the zone at a change point rather than of one instance type —
/// spare capacity is what a zone has, not what a type has — so at any one change point every
/// type of a zone is discounted alike.
#[must_use]
pub fn spot_price_micros(on_demand_micros: u64, zone_index: usize, period: i64) -> u64 {
    let micros = on_demand_micros.saturating_mul(spot_share_percent(zone_index, period)) / 100;
    // Rounded to the places a price is published to, and never to nothing: a priced instance
    // type costs something, however small its share of the rate is.
    let rounded = micros
        .saturating_add(PRICE_QUANTUM_MICROS / 2)
        .saturating_div(PRICE_QUANTUM_MICROS)
        .saturating_mul(PRICE_QUANTUM_MICROS);
    rounded.max(PRICE_QUANTUM_MICROS)
}

/// `micros` micro-dollars as `SpotPrice$SpotPrice` carries a price: a decimal amount with
/// the six fractional digits AWS publishes a Spot price with.
#[must_use]
pub fn format_price_micros(micros: u64) -> String {
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// The change point in force NOW: the period the current instant falls in.
///
/// The CLOCK's, not a caller's: the price a request is judged against is the one in force when
/// the judgement falls due, which is the same price a history read at that instant reports as
/// its most recent change.
#[must_use]
pub fn current_price_period(at: SystemTime) -> i64 {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    seconds.div_euclid(SECONDS_PER_SPOT_PRICE_PERIOD)
}

/// The Spot price in force at `at` for `instance_type` in the zone `zone_name` of `region` on
/// the product description `product_description`, in micro-dollars — or `None` for an instance
/// type this emulator publishes no facts for, which has no On-Demand rate to be a share of.
///
/// The price a `DescribeSpotPriceHistory` read at the same instant reports as its most recent
/// change for that zone, type and product: one derivation, so a bid a client chose off the
/// history is judged against the price the history showed it.
#[must_use]
pub fn spot_market_price_micros(
    region: &str,
    zone_name: &str,
    instance_type: &str,
    product_description: &str,
    at: SystemTime,
) -> Option<u64> {
    let record = instance_type_record(instance_type)?;
    let zones = availability_zones_in_region(region);
    let zone_index = zones
        .iter()
        .position(|zone| zone.zone_name == zone_name)
        .unwrap_or(0);
    Some(spot_price_micros(
        on_demand_micros(&record, product_description),
        zone_index,
        current_price_period(at),
    ))
}

/// The hourly price `price` names, in micro-dollars — or `None` for a value that is not a
/// price at all.
///
/// The reverse of [`format_price_micros`], for the one place a price a CALLER wrote has to be
/// compared with a price this crate derived (a Spot bid against the market price). The member
/// is a plain `String` in the model and is held to being a non-negative number where it is
/// read ([`super::validate_hourly_price`]), so a value that is not one has no amount here.
#[must_use]
pub fn hourly_price_micros(price: &str) -> Option<u64> {
    let parsed = price.trim().parse::<f64>().ok()?;
    if !parsed.is_finite() || parsed < 0.0 {
        return None;
    }
    let micros = (parsed * 1_000_000.0).round();
    if micros < 0.0 || micros > u64::MAX as f64 {
        return None;
    }
    Some(micros as u64)
}
