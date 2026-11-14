// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services locations this service can host game servers in — the
//! managed-location catalog `ListLocations` reports under the `AWS` filter, and the UDP
//! ping beacon each of them carries.
//!
//! A managed location is not a resource a caller creates (those are the custom locations
//! `CreateLocation` persists, which `super::CustomLocationRecord` holds): it is reference
//! data, the set of Amazon Web Services Regions and Local Zones the service offers. So it
//! lives here as a table rather than in the store, and `super` re-exports every item, so a
//! caller names `crate::state::MANAGED_LOCATIONS` as it does the rest of the state API.
//!
//! The beacon a location reports is reference data too: the host name and port the service
//! publishes FOR that hosting location, which it reports so a game client can measure its
//! players' latency to it. Nothing here resolves, dials or listens on it — the value is
//! formatted into the `ListLocations` response body and read by nothing else in this crate,
//! and the emulator opens no socket and makes no outbound connection for it.

use crate::ops::types::{LocationModel, PingBeacon, UDPEndpoint};

/// The port a managed location's ping beacon answers on: `UDPEndpoint$Port`'s own doc —
/// "For Amazon GameLift Servers ping beacons, this is typically port 7770"
/// (`smithy_query(query="doc",shape="UDPEndpoint",member="Port")`).
pub const PING_BEACON_PORT: i32 = 7770;

/// The host-name prefix every managed location's ping beacon carries, in the service's own
/// `api.aws` zone: `gamelift-ping.<region>.api.aws` for a Region and
/// `gamelift-ping-<zone code>.<parent region>.api.aws` for a Local Zone, where the zone
/// code is the part of the location's name that follows its parent Region.
const PING_BEACON_PREFIX: &str = "gamelift-ping";

/// The DNS zone the beacons are published in.
const PING_BEACON_SUFFIX: &str = "api.aws";

/// Every Amazon Web Services location this service hosts game servers in, as
/// `(location name, the Region the location sits in)` — equal for a Region itself, the
/// parent Region for a Local Zone.
///
/// `LocationStringModel` types a location name as a plain string (len 1..64, pattern
/// `^[A-Za-z0-9\-]+$`), so the value set is not in the model: it is the published Amazon
/// GameLift Servers service-location list (the "Service locations" reference
/// `ListLocations`' own doc links to, which names the Regions and the Local Zones the
/// service offers). Listed Region by Region, each Region followed by its Local Zones and
/// the Regions in name order, so one reader can see at a glance which locations belong
/// together — and so the list a paginated walk pages over is stable.
pub const MANAGED_LOCATIONS: &[(&str, &str)] = &[
    ("af-south-1", "af-south-1"),
    ("ap-east-1", "ap-east-1"),
    ("ap-northeast-1", "ap-northeast-1"),
    ("ap-northeast-2", "ap-northeast-2"),
    ("ap-northeast-3", "ap-northeast-3"),
    ("ap-south-1", "ap-south-1"),
    ("ap-southeast-1", "ap-southeast-1"),
    ("ap-southeast-2", "ap-southeast-2"),
    ("ap-southeast-3", "ap-southeast-3"),
    ("ap-southeast-5", "ap-southeast-5"),
    ("ca-central-1", "ca-central-1"),
    ("eu-central-1", "eu-central-1"),
    ("eu-central-2", "eu-central-2"),
    ("eu-north-1", "eu-north-1"),
    ("eu-west-1", "eu-west-1"),
    ("eu-west-2", "eu-west-2"),
    ("eu-west-3", "eu-west-3"),
    ("il-central-1", "il-central-1"),
    ("me-central-1", "me-central-1"),
    ("me-south-1", "me-south-1"),
    ("sa-east-1", "sa-east-1"),
    ("us-east-1", "us-east-1"),
    ("us-east-1-atl-1", "us-east-1"),
    ("us-east-1-bos-1", "us-east-1"),
    ("us-east-1-bue-1", "us-east-1"),
    ("us-east-1-chi-1", "us-east-1"),
    ("us-east-1-chi-2", "us-east-1"),
    ("us-east-1-dfw-1", "us-east-1"),
    ("us-east-1-dfw-2", "us-east-1"),
    ("us-east-1-iah-1", "us-east-1"),
    ("us-east-1-lim-1", "us-east-1"),
    ("us-east-1-mci-1", "us-east-1"),
    ("us-east-1-mia-1", "us-east-1"),
    ("us-east-1-mia-2", "us-east-1"),
    ("us-east-1-msp-1", "us-east-1"),
    ("us-east-1-nyc-1", "us-east-1"),
    ("us-east-1-phl-1", "us-east-1"),
    ("us-east-1-qro-1", "us-east-1"),
    ("us-east-1-scl-1", "us-east-1"),
    ("us-east-2", "us-east-2"),
    ("us-west-1", "us-west-1"),
    ("us-west-2", "us-west-2"),
    ("us-west-2-den-1", "us-west-2"),
    ("us-west-2-las-1", "us-west-2"),
    ("us-west-2-lax-1", "us-west-2"),
    ("us-west-2-phx-1", "us-west-2"),
    ("us-west-2-phx-2", "us-west-2"),
];

/// Whether `location` is one of the locations this service publishes — a Region it offers or
/// a Local Zone of such a Region — rather than a location this store merely knows about
/// because a request was signed for it or a stored fleet deploys to it.
pub fn is_managed_location(location: &str) -> bool {
    MANAGED_LOCATIONS.iter().any(|(name, _)| *name == location)
}

/// The Region `location` sits in: the location itself when it names a Region, its parent
/// Region when it names a Local Zone of one, and — for a location this table does not
/// carry, such as a Region a request was signed for or one a stored fleet deploys to — the
/// location itself, since nothing names it as a zone of anywhere else.
pub fn parent_region(location: &str) -> &str {
    MANAGED_LOCATIONS
        .iter()
        .find(|(name, _)| *name == location)
        .map_or_else(
            || {
                MANAGED_LOCATIONS
                    .iter()
                    .filter(|(name, region)| name == region)
                    .map(|(_, region)| *region)
                    .filter(|region| {
                        location
                            .strip_prefix(region)
                            .is_some_and(|rest| rest.starts_with('-'))
                    })
                    // The longest matching Region wins, so a zone of a Region whose name
                    // another Region's name is a prefix of is attributed to the right one.
                    .max_by_key(|region| region.len())
                    .unwrap_or(location)
            },
            |(_, region)| *region,
        )
}

/// The ping-beacon host name `location` answers UDP latency probes on.
///
/// `PingBeacon` is "information about the UDP ping beacon for this location. Ping beacons
/// are fixed endpoints that you can use to measure network latency between a player device
/// and an Amazon GameLift Servers hosting location" — a fixed address of the hosting
/// location itself, published per Region and per Local Zone, so it is derived from the
/// location's name rather than stored.
pub fn ping_beacon_domain(location: &str) -> String {
    let region = parent_region(location);
    match location
        .strip_prefix(region)
        .and_then(|rest| rest.strip_prefix('-'))
    {
        Some(zone) if !zone.is_empty() => {
            format!("{PING_BEACON_PREFIX}-{zone}.{region}.{PING_BEACON_SUFFIX}")
        }
        _ => format!("{PING_BEACON_PREFIX}.{region}.{PING_BEACON_SUFFIX}"),
    }
}

/// The `PingBeacon` a managed location carries.
pub fn ping_beacon(location: &str) -> PingBeacon {
    PingBeacon {
        udp_endpoint: Some(UDPEndpoint {
            domain: Some(ping_beacon_domain(location)),
            port: Some(PING_BEACON_PORT),
        }),
    }
}

/// An Amazon Web Services location as `ListLocations` reports it: its name and, for a
/// location this service publishes, the ping beacon published for it — and no ARN, since an
/// ARN "is assigned to a custom location resource", which this is not.
///
/// A location the published table does not carry is reported without a beacon: the service
/// publishes one only for the locations it operates, so naming an address for a location it
/// does not would be an endpoint nothing answers on.
pub fn managed_location_model(location: &str) -> LocationModel {
    LocationModel {
        location_name: Some(location.to_string()),
        location_arn: None,
        ping_beacon: is_managed_location(location).then(|| ping_beacon(location)),
    }
}
