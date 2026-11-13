// SPDX-License-Identifier: CC0-1.0

//! The static Region catalog — the Regions and Availability Zones this service
//! serves and every location a resource is validated against — and the addresses
//! a resource is given in one.

use super::*;

/// The Region names a stored `ResourceLocation` may carry — the modelled
/// `RegionName` enum's values (`smithy_query(query="shape",shape="RegionName")`,
/// generated here as `REGION_NAME_VALUES`) — for a refusal that has to tell the
/// caller what to send instead.
///
/// The enum is WIDER than the Regions `GetRegions` lists as served, so a refusal
/// about where a resource can be CREATED names
/// [`served_region_names`] instead.
pub fn region_names() -> String {
    REGION_NAME_VALUES.join(", ")
}

// ---------------------------------------------------------------------------
// Region catalog
// ---------------------------------------------------------------------------

/// One Region Lightsail serves, as `GetRegions` reports it.
///
/// The catalog is the Regions the service SERVES, which is a subset of the
/// modelled `RegionName` enum (`smithy_query(query="shape",shape="RegionName")`,
/// generated here as `REGION_NAME_VALUES`): the enum is the set of Region names
/// a `ResourceLocation` may carry, and it still admits `us-west-1`, which
/// Lightsail does not operate in and does not list. Each entry adds the values
/// the enum cannot carry: the place AWS names the Region after
/// (`Region$displayName` — "The display name (`Ohio`)"), the area AWS
/// recommends it for (`Region$description` — "This region is recommended to
/// serve users in the eastern United States and eastern Canada") and the
/// Availability Zones the Region publishes. The continent code is not stored,
/// because it follows from the name: see [`continent_code_of_region`].
#[derive(Debug, Clone, Copy)]
pub struct RegionSpec {
    /// The `RegionName` value, as `Region$name` reports it.
    pub name: &'static str,
    /// The place the Region is named after, as `Region$displayName` reports it.
    pub display_name: &'static str,
    /// The area the Region is recommended for: the clause `Region$description`
    /// puts after "is recommended to serve users in".
    ///
    /// It is the WHOLE geography the Region is recommended for, as the catalog
    /// words it — the place the Region sits in together with the wider area it
    /// serves (`France and central Europe`) — not the Region's own territory
    /// alone: the sentence is the only human-readable guidance in this response,
    /// so a Region picker renders this clause to a caller choosing where to put a
    /// resource.
    pub serves: &'static str,
    /// Whether this Region is an OPT-IN Region — one AWS leaves disabled until an
    /// account enables it, which every Region introduced after the opt-in
    /// mechanism was published is.
    ///
    /// It is the description that carries this: the sentence of an opt-in Region
    /// opens by saying so ([`RegionSpec::description`]), and this response has no
    /// other member that tells a caller the Region is disabled by default.
    pub is_opt_in: bool,
    /// The zone letters of this Region's Availability Zones, in order: a zone
    /// name is the Region's name followed by one of these letters, the format
    /// `AvailabilityZone$zoneName` documents ("The format is `us-east-2a`
    /// (case-sensitive)").
    ///
    /// Per Region, because the count and the letters differ per Region and are
    /// not a run of `a`, `b`, `c`: `us-west-2` publishes four zones and Tokyo
    /// publishes `a`, `c` and `d` with no `b`.
    pub availability_zone_letters: &'static str,
}

impl RegionSpec {
    /// The sentence `Region$description` carries, in the form that member's own
    /// documentation shows: what the Region is recommended for, opened by the
    /// opt-in clause where the Region is one an account must enable
    /// ([`Self::is_opt_in`]).
    ///
    /// An opt-in Region's sentence is PUNCTUATED — it closes with a period, which
    /// the sentence of a Region that is enabled by default does not carry — so
    /// the opt-in clause and the closing period travel together, as the recorded
    /// opt-in entries report them.
    pub fn description(&self) -> String {
        let (opening, closing) = if self.is_opt_in {
            (
                "This opt-in Region (Region that is disabled by default)",
                ".",
            )
        } else {
            ("This region", "")
        };
        format!(
            "{opening} is recommended to serve users in {}{closing}",
            self.serves
        )
    }

    /// The Availability Zones this Region publishes, as `GetRegions` reports
    /// them to a caller that asked for them: one per letter in
    /// [`Self::availability_zone_letters`], each named `<region><letter>` and
    /// serving.
    pub fn availability_zones(&self) -> Vec<AvailabilityZone> {
        self.zone_names()
            .into_iter()
            .map(|zone_name| AvailabilityZone {
                zone_name: Some(zone_name),
                state: Some(AVAILABILITY_ZONE_STATE_AVAILABLE.to_string()),
            })
            .collect()
    }

    /// The names of the Availability Zones this Region publishes, in the
    /// catalog's order — the `<region><letter>` form
    /// [`Self::availability_zone_letters`] documents.
    pub fn zone_names(&self) -> Vec<String> {
        self.availability_zone_letters
            .chars()
            .map(|letter| format!("{}{letter}", self.name))
            .collect()
    }

    /// This Region as the modelled `Region`, for a request served in
    /// `request_region`.
    ///
    /// BOTH zone lists are ALWAYS reported, empty when they carry nothing: they
    /// are list members, and an absent member is not an empty list to a typed
    /// caller — it hands an SDK a null where the caller iterates a collection,
    /// instead of saying "no zones to report here".
    ///
    /// A list carries zones only when the request asked for that list AND this
    /// Region is the one the request was served in: a Lightsail endpoint is
    /// regional and reports the Availability Zones of its OWN Region, so every
    /// other Region in the catalog answers an empty zone list however the
    /// include flags are set. That is what the recorded answer shows — the zones
    /// of one Region, the request's own, and empty lists for all the others.
    pub fn view(
        &self,
        include_zones: bool,
        include_relational_database_zones: bool,
        request_region: &str,
    ) -> Region {
        let is_request_region = self.name == request_region;
        let zones = |include: bool| {
            if include && is_request_region {
                self.availability_zones()
            } else {
                Vec::new()
            }
        };
        Region {
            continent_code: continent_code_of_region(self.name).map(str::to_string),
            description: Some(self.description()),
            display_name: Some(self.display_name.to_string()),
            name: Some(self.name.to_string()),
            availability_zones: Some(zones(include_zones)),
            // Lightsail publishes no zone list for databases separate from a
            // Region's own, so both lists draw on the same zones.
            relational_database_availability_zones: Some(zones(
                include_relational_database_zones,
            )),
        }
    }
}

/// Every Region this service serves, in the order `GetRegions` reports them:
/// the US Regions first — the recorded answer opens with `us-east-1` — then the
/// rest in the order AWS lists its Regions in, with the Regions opened later
/// appended. `GetRegions` walks this slice, so two calls answer one stable
/// order.
///
/// MEMBERSHIP: the Regions Lightsail operates in. That is a subset of the
/// modelled `RegionName` enum, which also admits `us-west-1`: Lightsail serves
/// no resources there and the recorded answer carries no such entry, so the
/// catalog has no row for it.
///
/// `display_name` is the city, state or country AWS names each Region after
/// (ASCII, as AWS reports it: `Sao Paulo`). `serves` is the geography that
/// Region is recommended for — the clause after the sentence stem
/// `Region$description` documents, as AWS's own answer gives it for that Region
/// (`eu-west-1` and `eu-west-2` share one clause, and `eu-central-1`'s names a
/// whole set of continents). The clause is the WHOLE geography — the Region's
/// own territory together with the wider area it is recommended for (`France and
/// central Europe`, `Austalia, New Zealand, and the South Pacific`, in the
/// catalog's own spelling) — and is never shortened to the territory the display
/// name already names: the wider area is what tells a caller which users the
/// Region is for. `is_opt_in` marks the Regions AWS leaves disabled
/// until an account enables them — the Regions introduced after the opt-in
/// mechanism was published (`ap-east-1`, `ap-southeast-3`, `ap-southeast-5` and
/// `eu-south-2` of this catalog) — whose description opens with the opt-in
/// clause instead.
///
/// `availability_zone_letters` is the Availability Zones the Lightsail user
/// guide's "Regions and Availability Zones" table lists for each Region: six in
/// `us-east-1`, four in `us-west-2` (`a` through `d`, the recorded set), two in
/// `ca-central-1`, and `a`, `c`, `d` in `ap-northeast-1`, which publishes no
/// `b`.
pub const REGION_SPECS: &[RegionSpec] = &[
    RegionSpec {
        name: "us-east-1",
        display_name: "Virginia",
        serves: "the eastern United States",
        is_opt_in: false,
        availability_zone_letters: "abcdef",
    },
    RegionSpec {
        name: "us-east-2",
        display_name: "Ohio",
        serves: "the eastern United States",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "us-west-2",
        display_name: "Oregon",
        serves: "the northwestern United States, Alaska, and western Canada",
        is_opt_in: false,
        availability_zone_letters: "abcd",
    },
    RegionSpec {
        name: "ap-south-1",
        display_name: "Mumbai",
        serves: "India and southern Asia",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-2",
        display_name: "Seoul",
        serves: "South Korea",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-1",
        display_name: "Singapore",
        serves: "India and Southeast Asia",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-2",
        display_name: "Sydney",
        serves: "Austalia, New Zealand, and the South Pacific",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-northeast-1",
        display_name: "Tokyo",
        serves: "Japan",
        is_opt_in: false,
        availability_zone_letters: "acd",
    },
    RegionSpec {
        name: "ca-central-1",
        display_name: "Montreal",
        serves: "Canada and the northeastern United States",
        is_opt_in: false,
        availability_zone_letters: "ab",
    },
    RegionSpec {
        name: "eu-central-1",
        display_name: "Frankfurt",
        serves: "Europe, the Middle East, and Africa",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-1",
        display_name: "Ireland",
        serves: "Ireland, the United Kingdom, and Iceland",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-2",
        display_name: "London",
        serves: "Ireland, the United Kingdom, and Iceland",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-west-3",
        display_name: "Paris",
        serves: "France and central Europe",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-north-1",
        display_name: "Stockholm",
        serves: "Stockholm and north Europe",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "sa-east-1",
        display_name: "Sao Paulo",
        serves: "Brazil and South America",
        is_opt_in: false,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-east-1",
        display_name: "Hong Kong",
        serves: "Hong Kong and Asia",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "eu-south-2",
        display_name: "Spain",
        serves: "Spain and South Europe",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-3",
        display_name: "Jakarta",
        serves: "Indonesia and Southeast Asia",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
    RegionSpec {
        name: "ap-southeast-5",
        display_name: "Malaysia",
        serves: "Malaysia and Southeast Asia",
        is_opt_in: true,
        availability_zone_letters: "abc",
    },
];

/// The Regions `GetRegions` reports, as the modelled `Region` list, for a
/// request served in `request_region` — [`REGION_SPECS`] viewed in its own
/// order.
///
/// One function, so the catalog is walked the same way wherever it is reported.
pub fn regions_view(
    include_zones: bool,
    include_relational_database_zones: bool,
    request_region: &str,
) -> Vec<Region> {
    REGION_SPECS
        .iter()
        .map(|spec| {
            spec.view(
                include_zones,
                include_relational_database_zones,
                request_region,
            )
        })
        .collect()
}

/// The catalog entry for the Region `name`, or `None` when this service serves
/// no such Region.
pub fn region_spec(name: &str) -> Option<&'static RegionSpec> {
    REGION_SPECS.iter().find(|spec| spec.name == name)
}

/// The catalog entry for the Region that publishes the Availability Zone `zone`,
/// or `None` when no Region this service serves publishes it.
///
/// A zone name is a served Region's name followed by one of the zone letters
/// THAT Region publishes, so a well-formed `<region><letter>` whose letter the
/// Region does not publish — and a zone of a Region the catalog does not hold —
/// resolves to nothing: it names no place a resource can be created.
pub fn region_spec_of_availability_zone(zone: &str) -> Option<&'static RegionSpec> {
    REGION_SPECS
        .iter()
        .find(|spec| spec.zone_names().iter().any(|name| name == zone))
}

/// The Regions `GetRegions` reports as served, for a refusal that has to tell
/// the caller where it can create a resource.
pub fn served_region_names() -> String {
    REGION_SPECS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Region a resource the caller asks to create in the Availability Zone
/// `zone` belongs to, or the sentence that request is refused with.
///
/// The ONE resolution every operation that creates a resource in a
/// caller-named zone goes through, so one zone string cannot be accepted by one
/// of them and refused by another. `availabilityZone` is documented as a value
/// the caller takes from `GetRegions` ("Use the `get regions` operation to list
/// the Availability Zones where Lightsail is currently available"), so the zone
/// must be one a served Region PUBLISHES — see
/// [`region_spec_of_availability_zone`] — rather than merely one that reads like
/// a zone name, and a zone this service publishes nowhere is refused instead of
/// stored in a `ResourceLocation` no `GetRegions` answer accounts for.
///
/// `member` is the path of the member the zone arrived in, so the refusal names
/// what the CALLER sent rather than the member of whichever operation the
/// resolution is shared with.
pub fn created_in_availability_zone(
    member: &str,
    zone: &str,
) -> Result<&'static RegionSpec, String> {
    region_spec_of_availability_zone(zone).ok_or_else(|| {
        format!(
            "{member} '{zone}' is not an Availability Zone this service publishes: it must be \
             one of the zones GetRegions reports for a region of [{}]",
            served_region_names()
        )
    })
}

/// The continent code of the Region `name`, or `None` when its geography prefix
/// is not one this service serves.
///
/// An AWS Region name begins with the geography it sits in, so the continent
/// code follows from the name rather than needing its own table: `us-`/`ca-` are
/// North America, `sa-` South America, `eu-` Europe and `ap-` Asia Pacific —
/// the pairing the modelled `Region$continentCode` documentation shows ("The
/// continent code (`NA`, meaning North America)"). An unlisted prefix reports
/// NO continent code: the member is optional, and guessing one would be worse
/// than saying nothing.
pub fn continent_code_of_region(name: &str) -> Option<&'static str> {
    match name.split('-').next()? {
        "us" | "ca" => Some("NA"),
        "sa" => Some("SA"),
        "eu" => Some("EU"),
        "ap" => Some("AP"),
        _ => None,
    }
}

/// The `state` a serving Availability Zone reports.
///
/// A Lightsail Availability Zone is an EC2 one, and `available` is EC2's state
/// for a zone that is operating normally.
pub const AVAILABILITY_ZONE_STATE_AVAILABLE: &str = "available";

/// Where a stored record lives.
///
/// `availability_zone` is absent for a resource that is not zonal: the member
/// documents the `us-east-2a` form, which addresses one zone of one region, and
/// a DNS zone is held by the region as a whole — as is a container service — so
/// there is no zone to name.
#[derive(Debug, Clone)]
pub struct StoredLocation {
    pub availability_zone: Option<String>,
    pub region_name: String,
}

impl StoredLocation {
    /// The location of a resource created in one availability zone.
    pub fn zonal(availability_zone: &str, region_name: &str) -> Self {
        Self {
            availability_zone: Some(availability_zone.to_string()),
            region_name: region_name.to_string(),
        }
    }

    /// The location of a resource that lives in `region` as a whole.
    pub fn regional(region: &str) -> Self {
        Self {
            availability_zone: None,
            region_name: region.to_string(),
        }
    }

    /// The location of a domain recordset: [`DOMAIN_REGION_NAME`], and no
    /// availability zone at all.
    pub fn domain() -> Self {
        Self {
            availability_zone: None,
            region_name: DOMAIN_REGION_NAME.to_string(),
        }
    }

    /// This location as the modelled `ResourceLocation`.
    pub fn view(&self) -> ResourceLocation {
        ResourceLocation {
            availability_zone: self.availability_zone.clone(),
            region_name: Some(self.region_name.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Addresses
// ---------------------------------------------------------------------------

/// Base of the range public addresses are drawn from: `198.18.0.0/15`, the RFC
/// 2544 benchmarking range. It is reserved and routes nowhere, so an emulated
/// instance never advertises an address that belongs to somebody else.
pub(super) const PUBLIC_IPV4_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// Base of the range private addresses are drawn from: `172.26.0.0/16`, the CIDR
/// a Lightsail region's default VPC uses.
pub(super) const PRIVATE_IPV4_BASE: u32 = u32::from_be_bytes([172, 26, 0, 0]);

/// `base` offset by `offset`, as a dotted quad — the form the modelled
/// `IpAddress` pattern `^([0-9]{1,3}\.){3}[0-9]{1,3}$` requires.
pub(super) fn ipv4(base: u32, offset: u32) -> String {
    let [a, b, c, d] = base.wrapping_add(offset).to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

/// One address from `2001:DB8::/32`, the RFC 3849 documentation prefix, rendered
/// as the eight uppercase hex groups the modelled `Ipv6Address` pattern
/// `^([A-F0-9]{1,4}:){7}[A-F0-9]{1,4}$` requires.
pub(super) fn ipv6(offset: u32) -> String {
    let high = (offset >> 16) & 0xFFFF;
    let low = offset & 0xFFFF;
    format!("2001:DB8:0:0:0:0:{high:X}:{low:X}")
}

/// Whether `ip_address_type` gives the instance an IPv4 address, from the
/// modelled `IpAddressType` enum's own documentation: `ipv4` is IPv4 only,
/// `ipv6` is IPv6 only and `dualstack` is both.
pub fn ip_address_type_has_ipv4(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv4" | "dualstack")
}

/// Whether `ip_address_type` gives the instance an IPv6 address. See
/// [`ip_address_type_has_ipv4`].
pub fn ip_address_type_has_ipv6(ip_address_type: &str) -> bool {
    matches!(ip_address_type, "ipv6" | "dualstack")
}

/// The `ipAddressType` values a CDN distribution may carry.
///
/// The modelled `LightsailDistribution$ipAddressType` and
/// `CreateDistributionRequest$ipAddressType` each document the domain their
/// resource has: "The possible values are `ipv4` for IPv4 only, and `dualstack`
/// for IPv4 and IPv6" — two of the three members the shared `IpAddressType`
/// enum carries. In documented order, which is the order a refusal names them
/// in.
pub const DISTRIBUTION_IP_ADDRESS_TYPES: &[&str] = &["ipv4", "dualstack"];

/// How the model words each member of the `IpAddressType` enum, from the
/// documentation every member of it shares: "`ipv4` for IPv4 only, `ipv6` for
/// IPv6 only, and `dualstack` for IPv4 and IPv6". In that documented order, so a
/// refusal built from it reads the way the documentation does.
const IP_ADDRESS_TYPE_PHRASES: [(&str, &str); 3] = [
    ("ipv4", "ipv4 for IPv4 only"),
    ("ipv6", "ipv6 for IPv6 only"),
    ("dualstack", "dualstack for IPv4 and IPv6"),
];

/// The `ipAddressType` values a resource of `resource_type` may carry, from THAT
/// resource's own modelled member rather than from the shared `IpAddressType`
/// enum the input members target.
///
/// A distribution's member documents two values
/// ([`DISTRIBUTION_IP_ADDRESS_TYPES`]), while `Instance$ipAddressType` and
/// `LoadBalancer$ipAddressType` each document all three ("The possible values
/// are `ipv4` for IPv4 only, `ipv6` for IPv6 only, and `dualstack` for IPv4 and
/// IPv6"), which is the whole enum.
pub fn ip_address_types_of(resource_type: &str) -> &'static [&'static str] {
    if resource_type == RESOURCE_TYPE_DISTRIBUTION {
        DISTRIBUTION_IP_ADDRESS_TYPES
    } else {
        crate::ops::types::IP_ADDRESS_TYPE_VALUES
    }
}

/// The message refusing `ip_address_type` for a resource of `resource_type`, and
/// `None` when that resource may carry it.
///
/// The one owner of the per-resource value domain: every door that writes a
/// resource's `ipAddressType` — the creates and `SetIpAddressType` — asks this,
/// so a value the resource's own member does not document is refused wherever it
/// arrives rather than at one door only. For an instance and a load balancer the
/// domain is the whole enum the member targets, which the centralized
/// constraint walk already holds a request to, so those doors ask it to keep ONE
/// owner of the rule rather than because a request could get past the walk.
pub fn ip_address_type_refusal(resource_type: &str, ip_address_type: &str) -> Option<String> {
    let admitted = ip_address_types_of(resource_type);
    if admitted.contains(&ip_address_type) {
        return None;
    }
    Some(format!(
        "ipAddressType '{ip_address_type}' is not an IP address type a {resource_type} can \
         have: the possible values are {}",
        possible_ip_address_types_phrase(admitted)
    ))
}

/// `values` as the documentation words a value domain: each value with what it
/// gives the resource, the last one introduced by "and".
///
/// A value [`IP_ADDRESS_TYPE_PHRASES`] has no wording for is still named, as
/// itself, so a member the model may add later is listed rather than dropped
/// from the refusal.
fn possible_ip_address_types_phrase(values: &[&str]) -> String {
    let phrases: Vec<&str> = IP_ADDRESS_TYPE_PHRASES
        .iter()
        .filter(|(value, _)| values.contains(value))
        .map(|(_, phrase)| *phrase)
        .chain(
            values
                .iter()
                .filter(|value| {
                    !IP_ADDRESS_TYPE_PHRASES
                        .iter()
                        .any(|(known, _)| known == *value)
                })
                .copied(),
        )
        .collect();
    match phrases.split_last() {
        None => String::new(),
        Some((last, &[])) => (*last).to_string(),
        Some((last, leading)) => format!("{}, and {last}", leading.join(", ")),
    }
}
