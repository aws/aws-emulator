// SPDX-License-Identifier: CC0-1.0

//! The local-gateway stores (route tables, virtual interfaces and their groups) and the
//! filter-value pattern matcher every describe operation's filters are read through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// Every AWS partition that is not the commercial one: the prefix every Region code of
/// that partition starts with, and the DNS suffix its service endpoints carry.
///
/// The prefixes are AWS's own partition prefixes — AWS China, GovCloud and the isolated
/// partitions — the same ones
/// `service-com-amazonaws-observabilityadmin-observabilityadmin` partitions
/// `KNOWN_REGIONS` by. The suffix of each is the `dnsSuffix` that partition publishes in
/// the AWS SDK partition metadata (`endpoints.json`), which is what every regional
/// endpoint of the partition is built from: `aws-cn` `amazonaws.com.cn`, `aws-us-gov`
/// `amazonaws.com`, `aws-iso` `c2s.ic.gov`, `aws-iso-b` `sc2s.sgov.gov`, `aws-iso-e`
/// `cloud.adc-e.uk`, `aws-iso-f` `csp.hci.ic.gov`.
///
/// Every prefix ends in a hyphen, so `us-iso-` cannot swallow `us-isob-` and the order of
/// the entries carries no meaning.
pub(super) const NON_COMMERCIAL_PARTITIONS: [(&str, &str); 6] = [
    ("cn-", "amazonaws.com.cn"),
    ("us-gov-", "amazonaws.com"),
    ("us-iso-", "c2s.ic.gov"),
    ("us-isob-", "sc2s.sgov.gov"),
    ("us-isof-", "csp.hci.ic.gov"),
    ("eu-isoe-", "cloud.adc-e.uk"),
];

/// The DNS suffix the commercial (`aws`) partition's endpoints carry — the one
/// `DescribeRegionsRequest$Filters` spells out for the `endpoint` filter
/// (`ec2.us-east-1.amazonaws.com`).
pub const COMMERCIAL_DNS_SUFFIX: &str = "amazonaws.com";

/// This service's endpoint prefix: the `endpointPrefix` of its `aws.api#service` trait
/// (`smithy_query(query="service_info")`), which is the first label of every regional
/// endpoint the service is reached through.
pub const EC2_ENDPOINT_PREFIX: &str = "ec2";

/// The Regions of the commercial partition an account can use WITHOUT opting in: the
/// "Regions that are enabled by default" table of the Amazon Web Services Account
/// Management Reference Guide, "Specify which Amazon Web Services Regions your account can
/// use" — the guide `DescribeRegions`'s own documentation links to.
///
/// The ENABLED set is the written list and the opt-in set is its complement, rather than
/// the other way round, because that is the direction AWS's rule runs: a Region
/// introduced after the guide's cut-off is disabled until the account opts in, so a Region
/// this emulator's catalog gains later is an opt-in Region without this list being
/// touched. Regions outside the commercial partition are not in it at all — an account of
/// the China, GovCloud or an isolated partition reaches its own partition's Regions
/// directly — which is why [`region_opt_in_status`] asks about the partition first.
pub const REGIONS_ENABLED_BY_DEFAULT: [&str; 17] = [
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// The geography of each Region of this emulator's Region catalog: the country or area the
/// Region's infrastructure sits in, as AWS publishes it per Region in the "Regions and
/// Availability Zones" table of the AWS global infrastructure documentation, in the form
/// `Region$Geography` gives ("The name of the geography, for example, `United States of
/// America`" — `RegionGeography$Name`).
///
/// A geography is a FACT of a Region, like its endpoint and its opt-in status: no EC2
/// operation sets one, and two calls asking about one Region answer the same geography. The
/// table is keyed by Region code and covers every entry of the catalog
/// [`regions_in_partition_of`] narrows (`emulator_telemetry::policy::KNOWN_REGIONS`), so a
/// Region this emulator reports is never reported without its geography; a Region code the
/// catalog does not hold — the one a request may be signed for — carries no published
/// geography, and [`region_geography`] answers `None` for it rather than inventing one.
pub(super) const REGION_GEOGRAPHIES: [(&str, &str); 44] = [
    ("af-south-1", "South Africa"),
    ("ap-east-1", "Hong Kong"),
    ("ap-east-2", "Taiwan"),
    ("ap-northeast-1", "Japan"),
    ("ap-northeast-2", "South Korea"),
    ("ap-northeast-3", "Japan"),
    ("ap-south-1", "India"),
    ("ap-south-2", "India"),
    ("ap-southeast-1", "Singapore"),
    ("ap-southeast-2", "Australia"),
    ("ap-southeast-3", "Indonesia"),
    ("ap-southeast-4", "Australia"),
    ("ap-southeast-5", "Malaysia"),
    ("ap-southeast-6", "New Zealand"),
    ("ap-southeast-7", "Thailand"),
    ("ca-central-1", "Canada"),
    ("ca-west-1", "Canada"),
    ("cn-north-1", "China"),
    ("cn-northwest-1", "China"),
    ("eu-central-1", "Germany"),
    ("eu-central-2", "Switzerland"),
    ("eu-isoe-west-1", "United Kingdom"),
    ("eu-north-1", "Sweden"),
    ("eu-south-1", "Italy"),
    ("eu-south-2", "Spain"),
    ("eu-west-1", "Ireland"),
    ("eu-west-2", "United Kingdom"),
    ("eu-west-3", "France"),
    ("il-central-1", "Israel"),
    ("me-central-1", "United Arab Emirates"),
    ("me-south-1", "Bahrain"),
    ("mx-central-1", "Mexico"),
    ("sa-east-1", "Brazil"),
    ("us-east-1", "United States of America"),
    ("us-east-2", "United States of America"),
    ("us-gov-east-1", "United States of America"),
    ("us-gov-west-1", "United States of America"),
    ("us-iso-east-1", "United States of America"),
    ("us-iso-west-1", "United States of America"),
    ("us-isob-east-1", "United States of America"),
    ("us-isof-east-1", "United States of America"),
    ("us-isof-south-1", "United States of America"),
    ("us-west-1", "United States of America"),
    ("us-west-2", "United States of America"),
];

/// The geography AWS publishes for `region` ([`REGION_GEOGRAPHIES`]), or `None` for a Region
/// code the catalog does not hold.
pub fn region_geography(region: &str) -> Option<&'static str> {
    REGION_GEOGRAPHIES
        .iter()
        .find(|(code, _)| *code == region)
        .map(|(_, geography)| *geography)
}

/// `Region$OptInStatus` for a Region the account has opted in to — one of the three
/// values the member's own doc lists ("The possible values are `opt-in-not-required`,
/// `opted-in`, and `not-opted-in`").
pub const OPT_IN_STATUS_OPTED_IN: &str = "opted-in";

/// `Region$OptInStatus` for a Region that requires an opt-in the account has not made.
pub const OPT_IN_STATUS_NOT_OPTED_IN: &str = "not-opted-in";

/// Every filter `DescribeRegions` documents, as its `Filters` member lists them
/// (`smithy_query(query="doc",shape="DescribeRegionsRequest",member="Filters")`). A
/// filter outside this set is refused; a filter inside it is answered by
/// [`RegionRecord::filter_values`], which must cover every entry.
pub const REGION_FILTERS: [&str; 3] = ["endpoint", "opt-in-status", "region-name"];

/// The [`NON_COMMERCIAL_PARTITIONS`] entry `region` belongs to, or `None` when `region`
/// belongs to the commercial partition, whose Region codes carry no partition prefix at
/// all.
///
/// One lookup answers both the prefix and the DNS suffix, so [`partition_prefix_of`] and
/// [`partition_dns_suffix`] can never disagree about which prefix wins.
pub(super) fn partition_of(region: &str) -> Option<&'static (&'static str, &'static str)> {
    NON_COMMERCIAL_PARTITIONS
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
}

/// The Region-code prefix of `region`'s partition, or `None` when `region` belongs to the
/// commercial partition.
pub(super) fn partition_prefix_of(region: &str) -> Option<&'static str> {
    partition_of(region).map(|(prefix, _)| *prefix)
}

/// The DNS suffix the endpoints of `region`'s partition carry.
pub fn partition_dns_suffix(region: &str) -> &'static str {
    partition_of(region).map_or(COMMERCIAL_DNS_SUFFIX, |(_, suffix)| *suffix)
}

/// `region`'s EC2 service endpoint: this service's endpoint prefix, the Region code, and
/// the DNS suffix of the Region's partition — `ec2.us-east-1.amazonaws.com`, the form
/// `DescribeRegions`'s `endpoint` filter doc gives.
pub fn region_endpoint(region: &str) -> String {
    format!(
        "{EC2_ENDPOINT_PREFIX}.{region}.{}",
        partition_dns_suffix(region)
    )
}

/// Every Region of the partition `region` belongs to, in Region-code order.
///
/// Neither the Smithy model nor any operation of this emulator carries a Region catalog —
/// the model's endpoint rule set resolves ONE Region name rather than enumerating them,
/// and no operation creates a Region — so the catalog is
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry",
/// already sorted for binary search), the same source two other services derive their
/// Region catalogs from, rather than a second hand-written list that would drift from it.
///
/// It is narrowed to ONE partition because a partition is what bounds the Regions an
/// account can ever see: an account lives in exactly one, and its Region codes are what
/// name it. The order the catalog inherits is Region-code order, which is what makes
/// [`MAX_SCORED_REGIONS`] a prefix of [`commercial_regions`].
pub fn regions_in_partition_of(region: &str) -> Vec<&'static str> {
    let partition = partition_prefix_of(region);
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .iter()
        .copied()
        .filter(|known| partition_prefix_of(known) == partition)
        .collect()
}

/// Whether `region` is a Region of the catalog [`regions_in_partition_of`] narrows — that
/// is, a Region that EXISTS, as opposed to text merely shaped like a Region code
/// ([`region_name_is_well_formed`]).
///
/// The catalog is the same `emulator_telemetry::policy::KNOWN_REGIONS` that function reads,
/// already sorted, so one lookup answers it without building a Vec. Asked where a
/// caller-controlled Region would otherwise cause a RESOURCE to be stored under it — the
/// ambient default network of [`ComAmazonawsEc2Amazonec2State::ensure_default_network`] —
/// rather than where the Region is only rendered back into an answer.
pub fn is_catalog_region(region: &str) -> bool {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&region)
        .is_ok()
}

/// The Regions of the commercial (`aws`) partition, in Region-code order: the ones a
/// `GetSpotPlacementScores` request that narrows nothing is scored over.
///
/// `RegionNames` holds "the Regions used to narrow down the list of Regions to be
/// scored" (`GetSpotPlacementScoresRequest$RegionNames`), so the list it narrows has to
/// exist; it is [`regions_in_partition_of`] for the partition [`DEFAULT_REGION`] belongs
/// to.
pub fn commercial_regions() -> Vec<&'static str> {
    regions_in_partition_of(DEFAULT_REGION)
}

/// `region`'s opt-in status for the account this emulator serves.
///
/// A Region outside the commercial partition, and a commercial Region of
/// [`REGIONS_ENABLED_BY_DEFAULT`], needs no opt-in at all. Every OTHER Region requires one
/// the account has to make, and this emulator holds no account-level opt-in store — EC2 has
/// no operation that opts a Region in or out (that is the Account API's `EnableRegion`) — so
/// this account has made NONE of them and every opt-in Region is reported `not-opted-in`.
///
/// That is the same fact [`region_is_enabled_for_account`] gates a request on, so
/// `DescribeRegions` cannot report a Region as available that a request signed for it would
/// be refused in: a disabled Region answers nothing through its own endpoint, so calling in
/// one is not evidence of an opt-in.
pub fn region_opt_in_status(region: &str) -> &'static str {
    if partition_prefix_of(region).is_some() || REGIONS_ENABLED_BY_DEFAULT.contains(&region) {
        OPT_IN_NOT_REQUIRED
    } else {
        OPT_IN_STATUS_NOT_OPTED_IN
    }
}

/// Whether a request signed for `region` reaches this account's EC2 at all.
///
/// A Region an account has not enabled serves it nothing: its regional endpoint refuses
/// every request with `AuthFailure` rather than answering an empty result, which is what
/// lets a client tell "this Region is not available to me" from "this Region is empty". So
/// a Region of the catalog whose [`region_opt_in_status`] is `not-opted-in` is closed to
/// this account.
///
/// Opt-in is a fact of a REGION, and only a Region the catalog holds ([`is_catalog_region`])
/// has one: a well-formed code naming no Region at all carries no opt-in status to refuse
/// on, so it is not closed here — the credential scope is caller-controlled text, and this
/// gate answers what AWS publishes about Regions rather than reading authorization out of
/// that text.
pub fn region_is_enabled_for_account(region: &str) -> bool {
    !is_catalog_region(region) || region_opt_in_status(region) != OPT_IN_STATUS_NOT_OPTED_IN
}

/// The sentence EC2 answers `AuthFailure` with for a request signed for a Region the
/// account has not enabled.
///
/// FIXED text: it names nothing of the request — not the Region, not the operation — so it
/// is the whole message rather than one run's sample of a varying one, and it is the
/// sentence EC2 answers whenever it will not validate the credentials a request was signed
/// with. Named here so the gate and the suites that assert it cannot drift apart.
pub const REGION_NOT_ENABLED_MESSAGE: &str =
    "AWS was not able to validate the provided access credentials";

/// EC2's error code for a request its credentials do not authorize — the caller's standing
/// with what the request reached, as opposed to [`UNAUTHORIZED_OPERATION`], which is about
/// the caller's permission to call the operation at all.
///
/// Named here so the one place a handler answers it
/// ([`crate::scaffolded::Scaffolded::auth_failure`]) and the request-dispatch Region gate
/// cannot spell it two ways.
pub const AUTH_FAILURE: &str = "AuthFailure";

/// One Region of the account's partition, as the describe reads it.
///
/// `opt_in_status` is a `&'static str` because it is one of the three words
/// `Region$OptInStatus` documents rather than text a request can carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionRecord {
    /// `us-east-1` — the Region code.
    pub region_name: String,
    /// The Region's EC2 service endpoint, as [`region_endpoint`] builds it.
    pub endpoint: String,
    /// `opt-in-not-required`, `opted-in` or `not-opted-in`.
    pub opt_in_status: &'static str,
}

impl RegionRecord {
    /// This Region as the modelled `Region` shape.
    ///
    /// `Geography` is "the geography information for the Region. The geography is returned as
    /// a list" (`Region$Geography`), which is the country or area AWS publishes for the Region
    /// ([`region_geography`]) — a fact of the Region, so it is reported as the one-item list
    /// the member is, and a Region code the catalog publishes no geography for reports the
    /// member absent rather than an invented name.
    pub fn to_region(&self) -> Region {
        Region {
            region_name: Some(self.region_name.clone()),
            endpoint: Some(self.endpoint.clone()),
            opt_in_status: Some(self.opt_in_status.to_string()),
            geography: region_geography(&self.region_name).map(|geography| {
                vec![RegionGeography {
                    name: Some(geography.to_string()),
                }]
            }),
        }
    }

    /// Whether this Region is enabled for the account: every Region but one carrying an
    /// opt-in the account has not made.
    pub fn is_enabled(&self) -> bool {
        self.opt_in_status != OPT_IN_STATUS_NOT_OPTED_IN
    }

    /// The values the EC2 filter `name` compares against this Region, or `None` when
    /// `name` is not one of [`REGION_FILTERS`].
    pub fn filter_values(&self, name: &str) -> Option<Vec<&str>> {
        let values = match name {
            "endpoint" => vec![self.endpoint.as_str()],
            "opt-in-status" => vec![self.opt_in_status],
            "region-name" => vec![self.region_name.as_str()],
            _ => return None,
        };
        Some(values)
    }
}

/// The Regions a request signed for `signed_region` can see, in Region-code order:
/// every Region of that Region's partition ([`regions_in_partition_of`]), each with its
/// endpoint and its opt-in status.
///
/// A well-formed `signed_region` the catalog does not hold is reported beside them, so a
/// caller is never told the Region it is calling in does not exist — the same allowance
/// `GetSpotPlacementScores` makes for the Region a call was signed for. A credential
/// scope is caller-controlled text, so one that is not shaped like a Region code
/// ([`region_name_is_well_formed`]) adds nothing.
pub fn regions_for_request(signed_region: &str) -> Vec<RegionRecord> {
    let mut names: Vec<&str> = regions_in_partition_of(signed_region);
    if region_name_is_well_formed(signed_region) && !names.contains(&signed_region) {
        names.push(signed_region);
        names.sort_unstable();
    }
    names
        .into_iter()
        .map(|name| RegionRecord {
            region_name: name.to_string(),
            endpoint: region_endpoint(name),
            opt_in_status: region_opt_in_status(name),
        })
        .collect()
}

// =============================================================================
// Spot placement scores
// =============================================================================

/// The most Regions one `GetSpotPlacementScores` response scores: "The Spot placement
/// score for the top 10 Regions or Availability Zones"
/// (`GetSpotPlacementScoresResult$SpotPlacementScores`).
///
/// The ceiling is on REGIONS rather than on response items, so a request that scores
/// single Availability Zones scores the zones OF these Regions and can report more items
/// than this, paged by `MaxResults` and `NextToken`. That reading is the one the rest of
/// the model supports: the same operation publishes `MaxResults` with a range of 10..1000
/// and a `NextToken` on both its input and its output, and neither a page size above ten
/// nor a continuation token could ever do anything if one response held at most ten items.
pub const MAX_SCORED_REGIONS: usize = 10;

/// The bottom of the published score scale: "The placement score, on a scale from 1 to
/// 10. ... A score of 1 indicates that your Spot request is not likely to succeed"
/// (`SpotPlacementScore$Score`).
pub const MIN_PLACEMENT_SCORE: i32 = 1;

/// The top of the published score scale: "A score of 10 indicates that your Spot request
/// is highly likely to succeed in this Region or Availability Zone"
/// (`SpotPlacementScore$Score`).
pub const MAX_PLACEMENT_SCORE: i32 = 10;

/// The instance type count a request is diversified from: "We recommend that you specify
/// at least three instance types" (`GetSpotPlacementScoresRequest$InstanceTypes`).
pub const RECOMMENDED_INSTANCE_TYPE_COUNT: usize = 3;

/// The best score a request that names fewer than [`RECOMMENDED_INSTANCE_TYPE_COUNT`]
/// instance types gets: "If you specify one or two instance types, or specify variations
/// of a single instance type ... the returned placement score will always be low"
/// (`GetSpotPlacementScoresRequest$InstanceTypes`) — low on the 1..10 scale.
pub const UNDIVERSIFIED_PLACEMENT_SCORE: i32 = 3;

/// The placement score for `capacity` of demand placed on ONE location, given how many
/// DISTINCT instance types the request leaves Amazon EC2 to choose from (0 when it names
/// none, and so leaves EC2 to choose).
///
/// A score "reflects how likely it is that each Region or Availability Zone will succeed
/// at fulfilling the specified target capacity" (`GetSpotPlacementScoresResult$
/// SpotPlacementScores`), so it falls as the demand on a location grows and as the set of
/// instance types that could fulfil it narrows: the score starts at
/// [`MAX_PLACEMENT_SCORE`] and drops by one for each decimal order of magnitude of the
/// demand, down to [`MIN_PLACEMENT_SCORE`], and a request naming one or two instance types
/// is never scored above [`UNDIVERSIFIED_PLACEMENT_SCORE`].
///
/// The unit the capacity is counted in does not enter the score, which is a DELIBERATE
/// declining of the conversion rather than a missing catalog: [`instance_type_info`] now
/// derives each type's vCPUs and memory, so `vcpu` or `memory-mib` demand could be divided
/// by a per-type figure — but WHICH type a location would have placed is exactly what AWS
/// decides and this emulator does not, and a request may name none at all
/// (`GetSpotPlacementScoresRequest$InstanceTypes` is optional) or a whole set of types with
/// different figures. Dividing by one chosen member of that set would invent a placement
/// decision, so the capacity is scored in whatever unit the request states it in.
pub fn placement_score(capacity: u64, instance_type_count: usize) -> i32 {
    // `ilog10` of a `u64` is at most 19, so this conversion cannot actually saturate; the
    // fallback is there so the arithmetic below needs no unchecked cast.
    let orders_of_magnitude = i32::try_from(capacity.max(1).ilog10()).unwrap_or(i32::MAX);
    let score = MAX_PLACEMENT_SCORE
        .saturating_sub(orders_of_magnitude)
        .clamp(MIN_PLACEMENT_SCORE, MAX_PLACEMENT_SCORE);
    if (1..RECOMMENDED_INSTANCE_TYPE_COUNT).contains(&instance_type_count) {
        score.min(UNDIVERSIFIED_PLACEMENT_SCORE)
    } else {
        score
    }
}

/// One scored location of a `GetSpotPlacementScores` response: a whole Region, or one
/// Availability Zone of a Region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotPlacementScoreRecord {
    /// The Region scored, or the Region of the scored Availability Zone.
    pub region: String,
    /// The zone id of the scored Availability Zone — `None` when the whole Region is
    /// scored, because then the response names no zone.
    pub availability_zone_id: Option<String>,
    /// The score, on the 1..10 scale.
    pub score: i32,
}

impl SpotPlacementScoreRecord {
    /// This record as the modelled `SpotPlacementScore` shape.
    pub fn to_spot_placement_score(&self) -> SpotPlacementScore {
        SpotPlacementScore {
            region: Some(self.region.clone()),
            availability_zone_id: self.availability_zone_id.clone(),
            score: Some(self.score),
        }
    }
}

/// Every location of `regions` scored for `capacity` and `instance_type_count`, in report
/// order: highest score first, then by location.
///
/// A request scores either whole Regions or single Availability Zones —
/// "`SingleAvailabilityZone`: specify true so that the response returns a list of scored
/// Availability Zones. Otherwise, the response returns a list of scored Regions." A
/// Region's score "assumes that your fleet request will be configured to use all
/// Availability Zones", whereas a caller scoring single zones "want[s] to launch all of
/// [its] Spot capacity into a single Availability Zone": one zone carries the whole target
/// capacity that a Region's zones would otherwise share, so a zone is scored for that
/// larger per-location demand and therefore never scores above its own Region.
pub fn spot_placement_scores(
    regions: &[String],
    capacity: u64,
    instance_type_count: usize,
    single_availability_zone: bool,
) -> Vec<SpotPlacementScoreRecord> {
    let per_zone_capacity =
        capacity.saturating_mul(u64::try_from(AVAILABILITY_ZONES_PER_REGION).unwrap_or(1));
    let mut scored: Vec<SpotPlacementScoreRecord> = regions
        .iter()
        .flat_map(|region| {
            if single_availability_zone {
                availability_zones_in_region(region)
                    .into_iter()
                    .map(|zone| SpotPlacementScoreRecord {
                        region: region.clone(),
                        availability_zone_id: Some(zone.zone_id),
                        score: placement_score(per_zone_capacity, instance_type_count),
                    })
                    .collect::<Vec<SpotPlacementScoreRecord>>()
            } else {
                vec![SpotPlacementScoreRecord {
                    region: region.clone(),
                    availability_zone_id: None,
                    score: placement_score(capacity, instance_type_count),
                }]
            }
        })
        .collect();
    scored.sort_by_cached_key(placement_score_page_cursor);
    scored
}

/// The pagination cursor naming `record`'s place in report order: its distance below the
/// top of the scale, then its Region, then its zone id, joined by spaces.
///
/// Paging is by cursor rather than by index, the way [`encode_page_token`] and
/// [`offering_page_cursor`] do it, so a token names the location the next page starts at.
/// The distance below the top of the scale comes first and fixed-width, so ascending
/// cursor order IS report order — highest score first, then location — and the separator
/// sorts ahead of every character a Region name or a zone id can carry, so one Region
/// being a prefix of another cannot reorder the two.
pub fn placement_score_page_cursor(record: &SpotPlacementScoreRecord) -> String {
    format!(
        "{:02} {} {}",
        MAX_PLACEMENT_SCORE.saturating_sub(record.score),
        record.region,
        record.availability_zone_id.as_deref().unwrap_or_default()
    )
}

/// Whether `name` is shaped like a Region code — "enter the Region code, for example,
/// us-east-1" (`GetSpotPlacementScoresRequest$RegionNames`).
///
/// A Region code is hyphen-joined lowercase groups ending in a number: `us-east-1`,
/// `ap-northeast-3`, `us-gov-west-1`. This is the grammar alone, with no allowlist: it is
/// what the Region taken from a SigV4 credential scope is held to, which is
/// caller-controlled text rendered into the zone names and zone ids a scored zone reports,
/// and that Region belongs to whichever partition the caller is calling. A Region a
/// REQUEST names is held to the catalog itself ([`commercial_regions`]) by the handler,
/// the way an instance type is held to [`offered_instance_types`].
pub fn region_name_is_well_formed(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let parts: Vec<&str> = name.split('-').collect();
    let Some((number, words)) = parts.split_last() else {
        return false;
    };
    // `us-east-1` is the shortest shape: a partition code, at least one word, then the
    // Region's number — so at least two hyphen-separated words ahead of that number.
    words.len() >= 2
        && (1..=2).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// One token of a compiled filter value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FilterToken {
    /// A literal character, whether or not it was escaped.
    Literal(char),
    /// `*`: zero or more characters.
    Many,
    /// `?`: exactly one character.
    One,
}

/// One EC2 filter value, compiled once so it can be matched against many candidates.
///
/// Filter values are case-sensitive and carry two wildcards, documented in "List and
/// filter using the CLI and API" — the guide the modelled `Filter` shape links to:
/// `*` matches zero or more characters, `?` matches exactly one, and a backslash
/// escapes either so a value can hold a literal `*` or `?`. That escaping is EC2's
/// own, which is why the matcher is written here rather than taken from a glob crate:
/// the maintained ones implement `*`/`?` without the backslash escape.
///
/// Compiling is separate from matching so a describe that compares one filter value
/// against a whole candidate set parses the value once instead of once per candidate.
///
/// A value longer than [`MAX_FILTER_PATTERN_TOKENS`] is not compiled into a pattern at
/// all: it is recorded as oversize and matches NOTHING, so neither the compiled vector
/// nor the scan over it can be grown by a request beyond that bound. A filter value no
/// candidate of this service can be that long cannot select anything anyway, and
/// matching nothing can never widen a result.
#[derive(Clone, Debug)]
pub struct FilterPattern {
    pub(super) tokens: Vec<FilterToken>,
    /// Whether the value was longer than [`MAX_FILTER_PATTERN_TOKENS`], in which case it
    /// was not compiled at all and matches nothing — see [`Self::compile`].
    pub(super) oversize: bool,
}

/// How many tokens one compiled filter value may hold.
///
/// A filter value is request-controlled text compiled one token per character, and the
/// matcher's cost is the product of the pattern's length and the candidate's, so the token
/// count is the one factor of that product a caller could otherwise make unbounded. The
/// ceiling is [`MAX_FILTER_VALUE_BYTES`], the size every describe of this service already
/// refuses a filter value above, so a value within the documented bound always compiles
/// whole and only one that no handler would have accepted is capped here.
pub const MAX_FILTER_PATTERN_TOKENS: usize = MAX_FILTER_VALUE_BYTES;

impl FilterPattern {
    /// Compile one filter value.
    ///
    /// A value of more than [`MAX_FILTER_PATTERN_TOKENS`] tokens is NOT compiled: the
    /// pattern it would build is what makes one comparison arbitrarily expensive, so such a
    /// value matches nothing instead of being truncated to a pattern that would match MORE
    /// than the caller asked for.
    pub fn compile(pattern: &str) -> Self {
        let mut tokens: Vec<FilterToken> = Vec::new();
        let mut chars = pattern.chars();
        while let Some(c) = chars.next() {
            if tokens.len() >= MAX_FILTER_PATTERN_TOKENS {
                return Self {
                    tokens: Vec::new(),
                    oversize: true,
                };
            }
            match c {
                '\\' => match chars.next() {
                    // A trailing backslash escapes nothing, so it is itself a literal.
                    None => tokens.push(FilterToken::Literal('\\')),
                    Some(escaped) => tokens.push(FilterToken::Literal(escaped)),
                },
                '*' => tokens.push(FilterToken::Many),
                '?' => tokens.push(FilterToken::One),
                other => tokens.push(FilterToken::Literal(other)),
            }
        }
        Self {
            tokens,
            oversize: false,
        }
    }

    /// Does this filter value match `value`?
    ///
    /// Unbudgeted, for a comparison made once; a describe that scans many candidates
    /// uses [`Self::matches_within`] so one request's total work is bounded.
    pub fn matches(&self, value: &str) -> bool {
        self.matches_within(value, &mut MatchBudget::unlimited())
    }

    /// [`Self::matches`], charging every step it takes to `budget`.
    ///
    /// Answers `false` as soon as the budget is spent; the caller tells that apart
    /// from a genuine non-match with [`MatchBudget::exhausted`].
    ///
    /// EVERY path through this charges at least one step: the comparison itself on entry,
    /// one per character of the candidate the loop walks, and one per token of the pattern
    /// the trailing check walks. So no shape of filter value — an empty candidate value, a
    /// pattern that is all `*` — can do request-scaled work for free, which is what makes
    /// the budget a bound on the PRODUCT of candidates, filters, values and pattern length
    /// rather than on the candidates alone.
    pub fn matches_within(&self, value: &str, budget: &mut MatchBudget) -> bool {
        if !budget.spend() {
            return false;
        }
        // A value no handler would have accepted was never compiled, so it matches nothing.
        if self.oversize {
            return false;
        }
        let tokens = &self.tokens;
        let value: Vec<char> = value.chars().collect();
        // Greedy match with one backtrack point per `*`, which is all a pattern with no
        // alternation needs: on a mismatch, the most recent `*` consumes one more
        // character and matching resumes after it.
        let (mut token, mut position) = (0usize, 0usize);
        let mut backtrack: Option<(usize, usize)> = None;
        while position < value.len() {
            if !budget.spend() {
                return false;
            }
            let matched = match tokens.get(token) {
                Some(FilterToken::Many) => {
                    backtrack = Some((token, position));
                    token += 1;
                    continue;
                }
                Some(FilterToken::One) => true,
                Some(FilterToken::Literal(c)) => *c == value[position],
                None => false,
            };
            if matched {
                token += 1;
                position += 1;
            } else if let Some((star, consumed)) = backtrack {
                token = star + 1;
                position = consumed + 1;
                backtrack = Some((star, consumed + 1));
            } else {
                return false;
            }
        }
        // The value is exhausted: what is left of the pattern must match nothing. Each
        // remaining token is charged, so a long all-`*` tail costs what it walks even when
        // the candidate's value was empty and the loop above never ran.
        for remaining in &tokens[token.min(tokens.len())..] {
            if !budget.spend() {
                return false;
            }
            if *remaining != FilterToken::Many {
                return false;
            }
        }
        true
    }
}

/// Does the EC2 filter value `pattern` match `value`?
///
/// The one-shot form of [`FilterPattern`], for a comparison made once; a describe that
/// compares one value against many candidates should compile the pattern instead.
pub fn filter_value_matches(pattern: &str, value: &str) -> bool {
    FilterPattern::compile(pattern).matches(value)
}

/// Does a candidate resource satisfy EVERY compiled filter, charging each comparison to
/// `budget`?
///
/// The AND/OR evaluation EC2 defines for every `Filters` member, in one place: "If you
/// specify multiple filters, the filters are joined with an AND" (`Filter`), and within one
/// filter "if you specify multiple values for a filter, the values are joined with an OR"
/// (`Filter$Values`), each value matched case-sensitively with EC2's `*`/`?` wildcards. Only
/// the per-operation lookup differs, so a describe passes it as `values_of`: the values its
/// candidate holds for one filter name, or none at all when the candidate carries nothing
/// for that attribute — which then matches nothing rather than everything.
///
/// One step is charged per candidate as well, so even an unfiltered scan is counted and
/// cannot walk a whole store for free; the budget is what bounds the PRODUCT of candidates,
/// filters, values and value length, which no per-factor cap can do on its own.
pub fn matches_every_filter<'a>(
    filters: &[CompiledFilter],
    budget: &mut MatchBudget,
    values_of: &dyn Fn(&str) -> Vec<Cow<'a, str>>,
) -> bool {
    if !budget.spend() {
        return false;
    }
    filters.iter().all(|filter| {
        let held = values_of(&filter.name);
        filter.patterns.iter().any(|pattern| {
            held.iter()
                .any(|value| pattern.matches_within(value, budget))
        })
    })
}

/// A modelled timestamp as a filter value compares against it: the ISO 8601 UTC rendering
/// the wire carries it as. A member that is not set carries no value for the filter to
/// match, like any other absent attribute.
///
/// Beside [`matches_every_filter`] because every describe that filters on a timestamp needs
/// exactly this rendering, and two copies of it could drift.
pub fn filter_timestamp_values(
    at: Option<::aws_smithy_types::DateTime>,
) -> Vec<Cow<'static, str>> {
    at.and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
        .map(Cow::Owned)
        .into_iter()
        .collect()
}

/// A PRICE as a filter value compares against it: both spellings of the amount the wire
/// carries.
///
/// The wire carries a `Float` as Rust renders it, so a whole number of dollars reaches the
/// client as `9800` — while the values AWS's own documentation writes for the `fixed-price` and
/// `usage-price` filters are `9800.0` and `0.84`, the same amount with its fractional part
/// spelled out. Both are the same price, and a caller may write either, so a candidate holds
/// both as values of the filter; anything else would refuse to match a price the way it is
/// documented or the way it is reported. A member that is not set carries no value for the
/// filter to match, like any other absent attribute.
///
/// Beside [`matches_every_filter`] and [`filter_timestamp_values`] because every describe that
/// filters on a price needs exactly this rendering — the report of the Reserved Instances an
/// account holds and the report of the offerings it may buy both do — and two copies of it
/// could drift.
pub fn price_filter_values(price: Option<f32>) -> Vec<Cow<'static, str>> {
    let Some(price) = price else {
        return Vec::new();
    };
    let reported = price.to_string();
    let with_fraction = if reported.contains('.') {
        None
    } else {
        Some(format!("{reported}.0"))
    };
    ::std::iter::once(Cow::Owned(reported))
        .chain(with_fraction.map(Cow::Owned))
        .collect()
}

/// The tag filter whose values are the KEYS a resource carries, rather than one key's
/// value: "`tag-key` - The key of a tag assigned to the … Use this filter to find all …
/// that have a tag with a specific key, regardless of the tag value"
/// (`smithy_query(query="doc",shape="DescribeInstanceEventWindowsRequest",member="Filters")`,
/// which spells the whole family out).
pub const TAG_KEY_FILTER: &str = "tag-key";

/// The tag filter whose values are the VALUES a resource carries, whatever key they are
/// under: "`tag-value` - The value of a tag assigned to the … regardless of the tag key"
/// (same member).
pub const TAG_VALUE_FILTER: &str = "tag-value";

/// Is `name` one of the tag filter family — `tag:<key>`, [`TAG_KEY_FILTER`] or
/// [`TAG_VALUE_FILTER`]?
///
/// The family every EC2 describe over a resource that carries tags answers, whether or not
/// that operation's own `Filters` member enumerates it, so it is resolved in ONE place and
/// not re-spelled per describe. "Filter names are case-sensitive" (`Filter$Name`), so the
/// comparison is exact — and a `tag:` prefix with no key names no tag, so it is as
/// undocumented as any other misspelling.
pub fn is_tag_filter_name(name: &str) -> bool {
    name.strip_prefix(TAG_FILTER_PREFIX)
        .is_some_and(|key| !key.is_empty())
        || name == TAG_KEY_FILTER
        || name == TAG_VALUE_FILTER
}

/// The values a tag filter named `name` compares against `tags`, or `None` when `name` is
/// not a tag filter at all.
///
/// The counterpart of [`is_tag_filter_name`] for [`matches_every_filter`]'s `values_of`: a
/// describe asks this first and falls through to its own members when the answer is `None`,
/// so the family is read off a resource's tag list the same way wherever it is answered. A
/// resource carrying no tag of the key the name names compares against NO values, so it
/// selects nothing — the same answer EC2 gives for any attribute a resource has not got.
pub fn tag_filter_values<'a>(name: &str, tags: &'a [Tag]) -> Option<Vec<Cow<'a, str>>> {
    if let Some(key) = name
        .strip_prefix(TAG_FILTER_PREFIX)
        .filter(|key| !key.is_empty())
    {
        return Some(
            tags.iter()
                .filter(|tag| tag.key.as_deref() == Some(key))
                .filter_map(|tag| tag.value.as_deref().map(Cow::Borrowed))
                .collect(),
        );
    }
    match name {
        TAG_KEY_FILTER => Some(
            tags.iter()
                .filter_map(|tag| tag.key.as_deref().map(Cow::Borrowed))
                .collect(),
        ),
        TAG_VALUE_FILTER => Some(
            tags.iter()
                .filter_map(|tag| tag.value.as_deref().map(Cow::Borrowed))
                .collect(),
        ),
        _ => None,
    }
}

/// Refuse a request member's value the way EC2 does: HTTP 400 `InvalidParameterValue`,
/// with a message naming the value at fault.
pub fn invalid_parameter_value<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(INVALID_PARAMETER_VALUE, 400, message)
}

/// Refuse an ARN member that does not name a target the action can act on, the way EC2
/// does: HTTP 400 [`INVALID_TARGET_ARN_UNKNOWN`], with the message EC2 words for it —
/// [`INVALID_TARGET_ARN_MESSAGE_PREFIX`] followed by the ARN the request named.
///
/// This is the code for a member that takes an ARN — the ID format surface's
/// `PrincipalArn` — rather than [`invalid_parameter_value`], which is for a value of any
/// other kind, so every operation that takes one refuses it the same way.
pub fn invalid_target_arn_unknown<E>(arn: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_TARGET_ARN_UNKNOWN,
        400,
        format!("{INVALID_TARGET_ARN_MESSAGE_PREFIX}{arn}"),
    )
}

/// Refuse a resource TYPE the action does not support, the way EC2 does: HTTP 400
/// [`INVALID_RESOURCE_TYPE_UNKNOWN`] with its fixed message
/// [`INVALID_RESOURCE_TYPE_MESSAGE`].
///
/// This is the code for a member whose value must be one of a published list of resource
/// types — the ID format surface's `Resource`/`Resources` — rather than
/// [`invalid_parameter_value`], which is for a value of any other kind. It takes no
/// message because EC2's own is a constant: the refusal does not name the type at fault.
pub fn invalid_resource_type_unknown<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_RESOURCE_TYPE_UNKNOWN,
        400,
        INVALID_RESOURCE_TYPE_MESSAGE,
    )
}

/// Refuse a `Filters` entry naming a filter the operation does not support, the way EC2
/// does: HTTP 400 [`INVALID_FILTER`], with a message enumerating the names it DOES support.
///
/// A filter name is not an ordinary member value: EC2 has a code of its own for one it does
/// not recognise, and the message it answers names the whole supported catalog rather than
/// the entry at fault, so `supported` is given in the order the service enumerates it.
pub fn invalid_filter<E>(supported: &[&str]) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_FILTER,
        400,
        format!("Filter name must be one of [{}]", supported.join(", ")),
    )
}

/// Refuse a `Filters` entry whose VALUE is not an id of the kind that filter selects on, the
/// way EC2 does: HTTP 400 `InvalidParameterValue`, with the sentence it answers for one —
/// the fixed `A filter with an invalid `, the filter's own name, and the value the REQUEST
/// sent in single quotes.
///
/// A filter that selects on a resource id has its values PARSED as ids before anything is
/// narrowed, so a value outside that id's form is refused rather than compared: a client that
/// mistypes one is told its request was wrong instead of being handed the empty result set an
/// id nothing holds would produce, which it could not tell from "the resource is not there".
/// This is about the VALUE, where [`invalid_filter`] and the unknown-filter refusals are about
/// the NAME. The echo is bounded the way every refusal that quotes a caller-chosen id bounds
/// it ([`echoed_resource_id`]).
pub fn invalid_filter_id_value<E>(
    filter_name: &str,
    value: &str,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "A filter with an invalid {filter_name} of '{}' was given",
        echoed_resource_id(value),
    ))
}

/// Refuse a request that reuses an idempotency token for a request that is not the one that
/// token already named, the way EC2 does: HTTP 400 [`IDEMPOTENT_PARAMETER_MISMATCH`].
///
/// The message is EC2's own ([`IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE`]), which states what
/// is wrong rather than echoing what the request sent.
pub fn idempotent_parameter_mismatch<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        IDEMPOTENT_PARAMETER_MISMATCH,
        400,
        IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
    )
}

/// Refuse a request that named a VPC id no VPC of the account has, the way EC2 does:
/// HTTP 400 `InvalidVpcID.NotFound`.
pub fn invalid_vpc_id_not_found<E>(vpc_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VPC_ID_NOT_FOUND,
        400,
        format!("The vpc ID '{vpc_id}' does not exist"),
    )
}

/// Refuse a request that named a subnet id no subnet of the account has, the way EC2
/// does: HTTP 400 `InvalidSubnetID.NotFound`.
pub fn invalid_subnet_id_not_found<E>(subnet_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SUBNET_ID_NOT_FOUND,
        400,
        format!("The subnet ID '{subnet_id}' does not exist"),
    )
}

/// Refuse a request that named a subnet CIDR reservation id no reservation of the account
/// has: HTTP 400 [`INVALID_SUBNET_CIDR_RESERVATION_ID_NOT_FOUND`], worded by the one place
/// this module words a missing resource id ([`id_not_found`]) and naming the id the REQUEST
/// named.
///
/// The resource the sentence names is spelled as EC2's own `ResourceType` spells it
/// ([`RESOURCE_TYPE_SUBNET_CIDR_RESERVATION`]), the way this module words the newer
/// resources that are named by one id — a reservation's own type name rather than a second
/// spelling of it.
pub fn invalid_subnet_cidr_reservation_id_not_found<E>(
    reservation_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    id_not_found(
        INVALID_SUBNET_CIDR_RESERVATION_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_SUBNET_CIDR_RESERVATION} ID"),
        reservation_id,
    )
}

/// Refuse a request that named a subnet CIDR block association id no association of the
/// account has: HTTP 400 [`INVALID_SUBNET_CIDR_BLOCK_ASSOCIATION_ID_NOT_FOUND`], naming the
/// id the REQUEST named.
///
/// The sentence is this resource's own rather than the one [`id_not_found`] builds: EC2
/// words a missing association of this kind as the subnet CIDR BLOCK that the association
/// id names, not as a missing id.
pub fn invalid_subnet_cidr_block_association_id_not_found<E>(
    association_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SUBNET_CIDR_BLOCK_ASSOCIATION_ID_NOT_FOUND,
        400,
        format!("The subnet CIDR block with association ID '{association_id}' does not exist"),
    )
}

/// Refuse a request that named a DHCP options id no option set of the account has, the way
/// EC2 does: HTTP 400 [`INVALID_DHCP_OPTION_ID_NOT_FOUND`], with the id at fault — the
/// same shape of refusal as the VPC and subnet ones above.
pub fn invalid_dhcp_option_id_not_found<E>(
    dhcp_options_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_DHCP_OPTION_ID_NOT_FOUND,
        400,
        format!("The dhcpOptions ID '{dhcp_options_id}' does not exist"),
    )
}

/// Refuse a request that named a security group no group of the account has, the way EC2
/// does: HTTP 400 `InvalidGroup.NotFound`. `group` is the id or the name the request
/// named, whichever it could not resolve.
pub fn invalid_group_not_found<E>(group: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_GROUP_NOT_FOUND,
        400,
        format!("The security group '{group}' does not exist"),
    )
}

/// Refuse a value that is not a security group id at all, the way EC2 does: HTTP 400
/// `InvalidGroupId.Malformed`.
///
/// What a handler answers for a value [`security_group_id_is_well_formed`] rejects, whether
/// it arrived as the operation's own group id or inside a rule's `UserIdGroupPair`, so one
/// malformed id has one refusal wherever it is named.
pub fn invalid_group_id_malformed<E>(group_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_GROUP_ID_MALFORMED,
        400,
        invalid_id_message(group_id),
    )
}

/// Refuse a request that named a security group rule id no rule of the account has, the
/// way EC2 does: HTTP 400 `InvalidSecurityGroupRuleId.NotFound`.
pub fn invalid_security_group_rule_id_not_found<E>(
    rule_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SECURITY_GROUP_RULE_ID_NOT_FOUND,
        400,
        format!("The security group rule ID '{rule_id}' does not exist"),
    )
}

/// The most characters of a refused `CoipPoolId` its message repeats back.
///
/// The message names the pool the request could not resolve, and it is the request's own
/// bytes — `Ipv4PoolCoipId` is a bare string the model puts no `@length` on — so the echo is
/// bounded here the way [`MAX_ECHOED_PAGE_TOKEN_CHARS`] bounds a refused token's. Far above
/// the longest pool id this service mints ([`COIP_POOL_ID_PREFIX`] plus 17 digits), so a
/// real id is always named in full.
pub const MAX_ECHOED_COIP_POOL_ID_CHARS: usize = 128;

/// A `CoipPoolId` from the REQUEST as a refusal may name it: its first
/// [`MAX_ECHOED_COIP_POOL_ID_CHARS`] characters.
///
/// The ONE place this surface's id is cut down to length, so both sentences it is quoted in
/// bound the echo the same way.
pub(super) fn echoed_pool_id(pool_id: &str) -> String {
    pool_id
        .chars()
        .take(MAX_ECHOED_COIP_POOL_ID_CHARS)
        .collect()
}

/// The sentence EC2's customer-owned-IP surface answers for a `CoipPoolId` it could not
/// resolve to a pool of the account.
///
/// ONE wording for the whole surface, whichever code carries it: the recorded refusals of
/// `CreateCoipCidr`, `GetCoipPoolUsage` and `DeleteCoipPool` all read
/// `The pool ID '<id>' does not exist.`, naming the id the request sent — so the message is
/// built from the request, and only the first [`MAX_ECHOED_COIP_POOL_ID_CHARS`] characters
/// of it are named.
pub fn coip_pool_does_not_exist_message(pool_id: &str) -> String {
    format!("The pool ID '{}' does not exist.", echoed_pool_id(pool_id))
}

/// The sentence the same surface answers for a value that is no `CoipPoolId` at all: this
/// class's verdict about the id the request sent, in the shape
/// [`coip_pool_does_not_exist_message`] words its own.
///
/// The id and nothing else: the form a pool id has is not part of what the service sends
/// back for a value outside it.
pub fn coip_pool_id_is_invalid_message(pool_id: &str) -> String {
    format!("The pool ID '{}' is invalid.", echoed_pool_id(pool_id))
}

/// Refuse a request that named an address-pool id no pool of the account has, the way
/// EC2's customer-owned-IP surface does: HTTP 400 `InvalidPoolID.NotFound`, with
/// [`coip_pool_does_not_exist_message`].
pub fn invalid_pool_id_not_found<E>(pool_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_POOL_ID_NOT_FOUND,
        400,
        coip_pool_does_not_exist_message(pool_id),
    )
}

/// Refuse a `CoipPoolId` that resolves to no customer-owned address pool of the account,
/// the way the operations that act on the ONE pool their `CoipPoolId` names do: HTTP 400,
/// with a CODE that says HOW the value failed — which is what a client branches on.
///
/// Three outcomes, in the order EC2 decides them, each taken from a recorded refusal of
/// this surface:
///
/// * the value IS a pool id ([`coip_pool_id_is_well_formed`]) and names no pool of the
///   account: `InvalidPoolID.NotFound` with [`coip_pool_does_not_exist_message`] — the code
///   recorded for `CreateCoipCidr` and `GetCoipPoolUsage` given a well-formed 17-digit id
///   nothing was created for;
/// * the value READS as a pool id ([`coip_pool_id_reads_as_one`]) but carries a digit count
///   EC2 never mints, so the pool it names can never exist: the surface refuses the request
///   PARAMETER with `InvalidParameterValue`, carrying that same sentence — the code recorded
///   for `DeleteCoipPool` given an id whose hexadecimal suffix is neither of EC2's published
///   lengths;
/// * the value is no address-pool id at all — another prefix, none, or a suffix that is not
///   hexadecimal: there is nothing to look up, which is [`invalid_pool_id_malformed`]'s
///   `InvalidPoolID.Malformed`, the code recorded for the same surface's `DescribeCoipPools`
///   given a value carrying another prefix.
///
/// `DescribeCoipPools` resolves `PoolIds` as a SELECTOR list and answers that member's own
/// [`invalid_pool_id_malformed`] for every value that is not a well-formed pool id, the
/// per-operation distinction [`invalid_id_parameter_value`] records.
pub fn coip_pool_id_does_not_exist<E>(pool_id: &str) -> crate::scaffolded::Scaffolded<E> {
    if coip_pool_id_is_well_formed(pool_id) {
        invalid_pool_id_not_found(pool_id)
    } else if coip_pool_id_reads_as_one(pool_id) {
        invalid_parameter_value(coip_pool_does_not_exist_message(pool_id))
    } else {
        invalid_pool_id_malformed(pool_id)
    }
}

/// Refuse a request that named something that is not a customer-owned address pool id at
/// all: HTTP 400 `InvalidPoolID.Malformed`, which EC2 answers BEFORE it looks the id up, so
/// "not a pool id" never reads as "no such pool".
///
/// The form itself is [`coip_pool_id_is_well_formed`], so the one reader of the id decides
/// what a pool id is and this function only words the refusal.
///
/// An address pool is one of the kinds this service words with a sentence of its OWN rather
/// than out of the shared [`invalid_id_message`] template — the surface's
/// [`coip_pool_id_is_invalid_message`], which names the pool id from the request and nothing
/// about the form that was expected.
pub fn invalid_pool_id_malformed<E>(pool_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_POOL_ID_MALFORMED,
        400,
        coip_pool_id_is_invalid_message(pool_id),
    )
}

/// Refuse one entry of a resource-id LIST parameter that is not an id of the kind the
/// parameter names: HTTP 400 `InvalidParameterValue`, naming the parameter and the form it
/// expects ("Value (…) for parameter volumes is invalid. Expected: 'vol-...'.").
///
/// EC2 answers a bad id two ways, and WHICH one is a fact of the operation, not of the id:
/// an operation that resolves the ONE resource it acts on answers that resource's
/// `Invalid<Kind>.Malformed` ([`require_known_volume`], [`require_well_formed_id`]), while
/// an operation whose id list is validated as a request PARAMETER answers
/// `InvalidParameterValue` and names the parameter — the wording AWS publishes for
/// `DescribeVolumes`' `volumes` parameter. This is the ONE place that second wording lives.
pub fn invalid_id_parameter_value<E>(
    parameter: &str,
    value: &str,
    prefix: &str,
) -> crate::scaffolded::Scaffolded<E> {
    invalid_parameter_value(format!(
        "Value ({value}) for parameter {parameter} is invalid. Expected: '{prefix}-...'."
    ))
}

/// The request member an EC2 operation whose `Filters` are checked at the PARAMETER layer
/// names in that refusal: the member of the request, singular, as that layer spells it.
pub(super) const FILTER_PARAMETER: &str = "Filter";

/// Refuse a `Filters` entry naming a filter the operation does not document, the way the
/// operations that check their request members at the PARAMETER layer do: HTTP 400
/// `InvalidParameterValue`, with that layer's one wording for every value it rejects —
/// "Value (<value>) for parameter <member> is invalid. <why>" — carrying the name the
/// request sent.
///
/// Like a malformed id ([`invalid_id_parameter_value`]), an unrecognised filter name is
/// answered two ways by EC2 and WHICH one is a fact of the operation rather than of the
/// name: the describes that resolve their own filters answer `The filter '<name>' is
/// invalid`, while an operation whose `Filters` member is validated as a request parameter
/// answers this one. The parameter layer appends a reason after the period, and a name that
/// is simply not one of the operation's has none to add, so the message ends with that
/// separating space and nothing after it. The name is a caller-chosen string the model puts
/// no `@length` on, so the echo is held to [`MAX_ECHOED_PAGE_TOKEN_CHARS`] — the crate's one
/// ceiling for repeating such a value back.
pub fn invalid_filter_parameter_value<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    let echoed: String = name.chars().take(MAX_ECHOED_PAGE_TOKEN_CHARS).collect();
    invalid_parameter_value(format!(
        "Value ({echoed}) for parameter {FILTER_PARAMETER} is invalid. "
    ))
}

/// The most characters of a caller-chosen value a refusal of this crate repeats back.
///
/// The crate's ceiling for echoing a value the caller chose the length of — a filter name, or
/// any other string the model puts no `@length` on — so a request's own bytes are never
/// reflected in full. Far above any token this service mints ([`encode_page_token`]), which
/// is the value it was first set for.
pub const MAX_ECHOED_PAGE_TOKEN_CHARS: usize = 256;

/// EC2's message for [`INVALID_PAGINATION_TOKEN`]: one fixed sentence, naming no token.
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "The passed pagination token is invalid";

/// Refuse a `NextToken` this service did not issue: HTTP 400 `InvalidPaginationToken`, with
/// EC2's own sentence for that code — [`INVALID_PAGINATION_TOKEN_MESSAGE`].
///
/// The sentence has NO variable part: EC2 answers a token it cannot read by saying that the
/// token it was passed is invalid, and repeats no part of it back. This is the writer every
/// paged read of this crate answers that refusal through.
pub fn invalid_pagination_token<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_PAGINATION_TOKEN,
        400,
        INVALID_PAGINATION_TOKEN_MESSAGE,
    )
}

/// A SHIM for [`invalid_pagination_token`], kept only so the call sites that still pass the
/// refused token keep compiling — it ignores `_refused_token`, because the sentence EC2
/// answers this code with carries no part of it.
///
/// Nothing new should call this: a handler refusing a token calls
/// [`invalid_pagination_token`]. It is not `#[deprecated]` only because this crate's lint
/// settings deny warnings, which would make the attribute break the build at every call site
/// that has not moved yet.
pub fn invalid_pagination_token_for<E>(_refused_token: &str) -> crate::scaffolded::Scaffolded<E> {
    invalid_pagination_token()
}

/// Report that the emulator itself could not serve the request: HTTP 500
/// `InternalError`. Never for anything a request carries.
pub fn internal_error<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(INTERNAL_ERROR, 500, message)
}

/// Answer a `DryRun` request the way EC2 does for a caller that HOLDS the action's
/// permissions: HTTP 412 `DryRunOperation`, and the action itself is not performed.
///
/// A handler reaches this for a caller the service-wide authorization gate has already
/// admitted ([`crate::service::authorization`]), which is what makes the answer the
/// permitted side of the probe rather than a claim that every caller is permitted: a
/// request the policies in force do not allow is refused `UnauthorizedOperation` before any
/// handler runs, for the actions that gate decides.
pub fn dry_run_operation<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        DRY_RUN_OPERATION,
        DRY_RUN_OPERATION_STATUS,
        DRY_RUN_OPERATION_MESSAGE,
    )
}

/// Refuse a caller that may not make the request at all, the way EC2 does:
/// [`UNAUTHORIZED_OPERATION_STATUS`] [`UNAUTHORIZED_OPERATION`].
///
/// The counterpart of [`dry_run_operation`] — the two are the two sides of one permission
/// decision — and of `AuthFailure`, which is about the caller's standing with a resource
/// the request names rather than with the operation itself.
pub fn unauthorized_operation<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        UNAUTHORIZED_OPERATION,
        UNAUTHORIZED_OPERATION_STATUS,
        message,
    )
}

/// Refuse the caller of an operation EC2 permits only to authorized Amazon Web Services
/// services: [`unauthorized_operation`] carrying [`CALLER_NOT_ALLOWED_MESSAGE`].
///
/// Such an operation is not one a principal can be granted; every caller of this emulator
/// signs as a principal of the account, so none of them is one of those services and the
/// answer does not depend on the resources the request names. It is NOT what a `DryRun`
/// request is answered with: that flag asks only whether the caller holds the ACTION, which
/// is [`dry_run_operation`].
pub fn caller_not_allowed_to_call_this_api<E>() -> crate::scaffolded::Scaffolded<E> {
    unauthorized_operation(CALLER_NOT_ALLOWED_MESSAGE)
}

// =============================================================================
// Local gateway route table / virtual interface group associations
// =============================================================================

/// The id prefix EC2 mints an association between a local gateway route table and a
/// virtual interface group with: `lgw-vif-grp-assoc-`, as the AWS Outposts and EC2
/// documentation's own ids read (for example `lgw-vif-grp-assoc-074f1e8e9bEXAMPLE`). The
/// model puts no pattern on
/// `LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`, so the prefix is the one
/// thing that says which resource an id names.
pub const LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX: &str = "lgw-vif-grp-assoc";

/// How many local gateway route table / virtual interface group associations this emulator
/// stores before a write must refuse.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored association, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is no bound on the caller-chosen bytes each record holds, so
/// those are charged against [`MAX_STORED_BYTES`] too
/// ([`local_gateway_vif_group_association_payload_bytes`]).
pub const MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS: usize = 4096;

/// The code EC2 answers when a request would take the account past the associations it may
/// hold, in the `<Resource>LimitExceeded` family this service's other quotas are refused
/// with ([`SECONDARY_NETWORK_LIMIT_EXCEEDED`]).
///
/// The refusal
/// `CreateLocalGatewayRouteTableVirtualInterfaceGroupAssociation` answers when
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// hands it `None`.
pub const LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED: &str =
    "LocalGatewayRouteTableVirtualInterfaceGroupAssociationLimitExceeded";

/// The one resource type a `TagSpecifications` entry on a create of a local gateway route
/// table / virtual interface group association may name: `ResourceType`'s
/// `local-gateway-route-table-virtual-interface-group-association` member
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const RESOURCE_TYPE_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION: &str =
    "local-gateway-route-table-virtual-interface-group-association";

/// The ARN resource path of a local gateway route table:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`, the form
/// `LocalGatewayRouteTableVirtualInterfaceGroupAssociation$LocalGatewayRouteTableArn`
/// carries ("The Amazon Resource Name (ARN) of the local gateway route table for the
/// virtual interface group").
pub fn local_gateway_route_table_arn(region: &str, route_table_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("local-gateway-route-table/{route_table_id}"),
    )
}

/// One association between a local gateway route table and a virtual interface group: the
/// modelled shape every operation that reports one answers with, plus the move the clock
/// owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason
/// [`LocalGatewayRouteTableVpcAssociationRecord`] holds one: a member a writer sets
/// round-trips through
/// `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations` without a
/// field-by-field copy that can silently drop one. The pending move lives beside it because
/// the shape carries no member for when the association's next state falls due.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableVirtualInterfaceGroupAssociationRecord {
    /// The association as the model shapes it, carrying the state reached so far.
    pub view: crate::ops::types::LocalGatewayRouteTableVirtualInterfaceGroupAssociation,
    /// The move an association still `associating` owes; `None` for one in a state the clock
    /// owes nothing on.
    pub pending: Option<PendingStateMove>,
}

/// What one association between a local gateway route table and a virtual interface group
/// is written with: the two resources it joins, the local gateway they belong to, the ARN
/// the route table itself reports, the state it starts in, the state the clock owes it, and
/// its tags.
///
/// Every member the service itself owns — the association id and the owning account — is
/// derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// rather than taken from the caller, so two writers cannot report one association
/// differently.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec {
    /// "The ID of the local gateway route table" the association is made in.
    pub local_gateway_route_table_id: String,
    /// "The Amazon Resource Name (ARN) of the local gateway route table for the virtual
    /// interface group", as the route table itself reports it
    /// ([`local_gateway_route_table_arn`]).
    pub local_gateway_route_table_arn: String,
    /// "The ID of the virtual interface group" the route table is associated with.
    pub local_gateway_virtual_interface_group_id: String,
    /// "The ID of the local gateway" both belong to.
    pub local_gateway_id: String,
    /// "The state of the association"
    /// (`LocalGatewayRouteTableVirtualInterfaceGroupAssociation$State`), which the model
    /// leaves an unconstrained string.
    pub state: String,
    /// The state the clock owes this association, [`TRANSITION_DELAY`] from the write;
    /// `None` for one written in a state nothing is owed on.
    pub becomes: Option<String>,
    /// "The tags assigned to the association".
    pub tags: Vec<Tag>,
}

/// The bytes one stored association holds beyond its fixed-size members: its tags, the state
/// a writer pinned on it (`$State`, which the model leaves an unconstrained string), the ids
/// of the resources it joins and the route table ARN it carries (derived from the route
/// table, but stored per association, so charged per association). Everything else on the
/// record — the association id, the owning account — is derived by this module, so it is
/// fixed-size.
///
/// The ONE definition of that charge, the way [`capacity_block_payload_bytes`] and
/// [`host_payload_bytes`] are for their own stores:
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// charges exactly it, and a removal path must give back the same number.
pub fn local_gateway_vif_group_association_payload_bytes(
    spec: &LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec,
) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.state.len())
        .saturating_add(spec.local_gateway_route_table_id.len())
        .saturating_add(spec.local_gateway_route_table_arn.len())
        .saturating_add(spec.local_gateway_virtual_interface_group_id.len())
        .saturating_add(spec.local_gateway_id.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` local gateway route table / virtual interface group
    /// associations fit inside the store's ceiling
    /// ([`MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS`]).
    ///
    /// A writer may ask this before inserting, the way [`Self::has_vpc_capacity`] and
    /// [`Self::has_dedicated_host_capacity`] gate their own stores, so it can refuse with
    /// [`LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED`] rather than discover the
    /// refusal from an absent id. The insert itself asks it too, so no path can get past it.
    pub fn has_local_gateway_vif_group_association_capacity(&self, count: usize) -> bool {
        self.local_gateway_route_table_virtual_interface_group_associations
            .len()
            .saturating_add(count)
            <= MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS
    }

    /// Store one association between a local gateway route table and a virtual interface
    /// group, and answer the id it was given — or `None` when the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling
    /// ([`Self::has_local_gateway_vif_group_association_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_vif_group_association_payload_bytes`]). Nothing in this emulator
    /// reclaims a stored association, so a write that is not bounded at the seam it is
    /// written through would grow the process's heap for as long as a caller keeps asking.
    /// A caller that gets `None` must refuse the request with
    /// [`LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED`].
    ///
    /// The state the clock owes the association is RECORDED, never slept for:
    /// [`Self::materialize_due_transitions`] applies it once [`TRANSITION_DELAY`] has passed,
    /// so a caller asking straight after the write still sees the transient state the
    /// association starts in.
    pub fn insert_local_gateway_route_table_virtual_interface_group_association(
        &mut self,
        spec: LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_vif_group_association_payload_bytes(&spec);
        if !self.has_local_gateway_vif_group_association_capacity(1)
            || self.would_exceed_stored_bytes(payload_bytes)
        {
            return None;
        }
        let association_id = self.next_id(LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX);
        let record = LocalGatewayRouteTableVirtualInterfaceGroupAssociationRecord {
            view: crate::ops::types::LocalGatewayRouteTableVirtualInterfaceGroupAssociation {
                local_gateway_route_table_virtual_interface_group_association_id: Some(
                    association_id.clone(),
                ),
                local_gateway_virtual_interface_group_id: Some(
                    spec.local_gateway_virtual_interface_group_id,
                ),
                local_gateway_id: Some(spec.local_gateway_id),
                local_gateway_route_table_arn: Some(spec.local_gateway_route_table_arn),
                local_gateway_route_table_id: Some(spec.local_gateway_route_table_id),
                owner_id: Some(::engine::emulator_account_id().to_string()),
                state: Some(spec.state),
                tags: Some(spec.tags),
            },
            pending: spec.becomes.map(PendingStateMove::starting_now),
        };
        self.add_stored_bytes(payload_bytes);
        self.local_gateway_route_table_virtual_interface_group_associations
            .insert(association_id.clone(), record);
        Some(association_id)
    }

    /// The id of the association that already joins `route_table_id` and
    /// `virtual_interface_group_id`, if the account holds one.
    ///
    /// An association IS the pair it joins, so this is what a writer asks before making a
    /// second one: without it the store would hold two association ids for one fact — the
    /// contract [`Self::local_gateway_route_table_vpc_association_of`] writes out for the
    /// VPC associations beside these. The scan is bounded by the store's own ceiling
    /// ([`MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS`]).
    pub fn local_gateway_route_table_virtual_interface_group_association_of(
        &self,
        route_table_id: &str,
        virtual_interface_group_id: &str,
    ) -> Option<&str> {
        self.local_gateway_route_table_virtual_interface_group_associations
            .iter()
            .find(|(_, record)| {
                record.view.local_gateway_route_table_id.as_deref() == Some(route_table_id)
                    && record
                        .view
                        .local_gateway_virtual_interface_group_id
                        .as_deref()
                        == Some(virtual_interface_group_id)
            })
            .map(|(association_id, _)| association_id.as_str())
    }
}

// =============================================================================
// Local gateways
// =============================================================================

/// The id prefix EC2 mints a local gateway with: `lgw-`, as the AWS Outposts and EC2
/// documentation's own ids read (for example `lgw-09b493aa7cEXAMPLE`) and as the id the
/// recorded wire corpus behind this surface sends (`lgw-` plus 17 hexadecimal digits,
/// answered with the gateway's `.NotFound`). The model puts no pattern on `LocalGatewayId`,
/// so the prefix is the one thing that says which resource an id names.
pub const LOCAL_GATEWAY_ID_PREFIX: &str = "lgw";

/// The `ResourceType` enum member a local gateway is tagged and named under:
/// `local-gateway` (`smithy_query(query="shape",shape="ResourceType")`). The resource's one
/// spelling, so the refusals that name it cannot drift from it.
pub const RESOURCE_TYPE_LOCAL_GATEWAY: &str = "local-gateway";

/// How many local gateways this emulator stores before a write must refuse.
///
/// CHOSEN, like [`MAX_LOCAL_GATEWAY_ROUTE_TABLES`] and for the same reason: nothing in this
/// emulator reclaims a stored gateway, so the store needs a ceiling that is not the
/// process's memory.
pub const MAX_LOCAL_GATEWAYS: usize = 4096;

/// `InvalidLocalGatewayID.Malformed` (HTTP 400): the request named something that is not a
/// local gateway id at all, so there is nothing to look up — the `Invalid<Kind>.Malformed`
/// code EC2 answers before it looks an id up, as
/// [`INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED`] is for a route table ON a gateway.
pub const INVALID_LOCAL_GATEWAY_ID_MALFORMED: &str = "InvalidLocalGatewayID.Malformed";

/// `InvalidLocalGatewayID.NotFound` (HTTP 400): the request named a well-formed local
/// gateway id this account holds no gateway for. The code AWS answers, as the recorded wire
/// corpus behind `issues/…-ec2-local-gateway-route-table-and-coip-pool.md` shows.
pub const INVALID_LOCAL_GATEWAY_ID_NOT_FOUND: &str = "InvalidLocalGatewayID.NotFound";

/// Whether `id` is a well-formed local gateway id — one EC2 could have issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`LOCAL_GATEWAY_ID_PREFIX`], whose doc carries the evidence for it). A route table's own
/// id (`lgw-rtb-…`) therefore reads as malformed here rather than as a gateway id, its
/// suffix not being hexadecimal.
pub fn local_gateway_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(LOCAL_GATEWAY_ID_PREFIX, id)
}

/// Refuse something that is not a local gateway id at all: HTTP 400
/// [`INVALID_LOCAL_GATEWAY_ID_MALFORMED`], worded the way AWS words a malformed id of this
/// resource and built from the request's own value.
pub fn invalid_local_gateway_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ID_MALFORMED,
        400,
        format!("The {RESOURCE_TYPE_LOCAL_GATEWAY} ID '{id}' is malformed"),
    )
}

/// The name `CreateLocalGatewayRouteTable`'s recorded refusal gives this resource: the
/// API's own `LocalGateway`, the same `<Kind>` token the error code carries
/// ([`INVALID_LOCAL_GATEWAY_ID_NOT_FOUND`]) and the same spelling the route tables ON a
/// gateway are named by ("The LocalGateway Route Table ID '…' does not exist",
/// [`invalid_local_gateway_route_table_id_not_found`]).
///
/// Distinct from [`RESOURCE_TYPE_LOCAL_GATEWAY`], the spelling the TAGGING `ResourceType`
/// enum holds, which is the one every OTHER reader of this id is recorded answering with.
pub const LOCAL_GATEWAY_API_NAME: &str = "LocalGateway";

/// Refuse a well-formed local gateway id this account holds no gateway for, for an
/// operation whose recorded refusal names the resource by the `ResourceType` member it is
/// tagged under: HTTP 400 [`INVALID_LOCAL_GATEWAY_ID_NOT_FOUND`], "The local-gateway ID
/// '…' does not exist".
///
/// This is the sentence every operation that resolves a `LocalGatewayId` answers, bar one:
/// `CreateLocalGatewayRouteTable` names the resource by its API name instead
/// ([`invalid_local_gateway_id_not_found_for_create_route_table`]), as the recorded
/// refusals of the two show. Which sentence an operation answers is a fact of that
/// operation's validator, and only the operation whose recorded refusal differs carries
/// its own writer — the division
/// [`invalid_local_gateway_route_table_id_malformed`] already makes for the route table id.
pub fn invalid_local_gateway_id_not_found<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        400,
        format!("The {RESOURCE_TYPE_LOCAL_GATEWAY} ID '{id}' does not exist"),
    )
}

/// Refuse a well-formed local gateway id this account holds no gateway for, for
/// `CreateLocalGatewayRouteTable`: HTTP 400 [`INVALID_LOCAL_GATEWAY_ID_NOT_FOUND`], "The
/// LocalGateway ID '…' does not exist" — the resource named by its API name
/// ([`LOCAL_GATEWAY_API_NAME`]), which is the sentence THAT operation's recorded refusal
/// carries.
pub fn invalid_local_gateway_id_not_found_for_create_route_table<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ID_NOT_FOUND,
        400,
        format!("The {LOCAL_GATEWAY_API_NAME} ID '{id}' does not exist"),
    )
}

/// Refuse something that is not a local gateway id at all, for a handler that has to read
/// the request's own FORM before it resolves anything — the contract
/// [`require_well_formed_local_gateway_route_table_id`] writes out for the route table id.
pub fn require_well_formed_local_gateway_id<E>(id: &str) -> Result<(), Scaffolded<E>> {
    if !local_gateway_id_is_well_formed(id) {
        return Err(invalid_local_gateway_id_malformed(id));
    }
    Ok(())
}

/// Refuse a `LocalGatewayId` an operation other than `CreateLocalGatewayRouteTable` cannot
/// resolve: a malformed one before an unknown one, the order [`require_well_formed_id`]
/// documents.
///
/// The ONE place those operations resolve a local gateway id, so each refuses the same id
/// the same way; the contract [`require_known_local_gateway_route_table`] writes out for
/// the route tables on a gateway. `CreateLocalGatewayRouteTable` resolves it through
/// [`require_known_local_gateway_for_create_route_table`] instead, which shares this
/// resolution and differs only in the sentence it words an unknown id with.
pub fn require_known_local_gateway<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_known_local_gateway_worded(store, id, invalid_local_gateway_id_not_found)
}

/// Refuse a `LocalGatewayId` `CreateLocalGatewayRouteTable` cannot resolve, with that
/// operation's own `NotFound` sentence
/// ([`invalid_local_gateway_id_not_found_for_create_route_table`]).
pub fn require_known_local_gateway_for_create_route_table<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_known_local_gateway_worded(
        store,
        id,
        invalid_local_gateway_id_not_found_for_create_route_table,
    )
}

/// Resolve a local gateway id — malformed before unknown — with `not_found` writing the
/// sentence the CALLING operation's validator words an unknown id with.
///
/// The ONE lookup and the ONE ordering behind both of this resource's `NotFound`
/// sentences, so only the wording is per-validator.
fn require_known_local_gateway_worded<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
    not_found: fn(&str) -> Scaffolded<E>,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_local_gateway_id(id)?;
    if !store.local_gateways.contains_key(id) {
        return Err(not_found(id));
    }
    Ok(())
}

/// What one local gateway is written with: the Outpost it is part of, the state it is in,
/// and its tags.
///
/// Every member the service itself owns — the gateway id and the owning account — is derived
/// by [`ComAmazonawsEc2Amazonec2State::insert_local_gateway`] rather than taken from the
/// caller, so two writers cannot report one gateway differently.
#[derive(Debug, Clone)]
pub struct LocalGatewaySpec {
    /// "The Amazon Resource Name (ARN) of the Outpost" (`LocalGateway$OutpostArn`) the
    /// gateway belongs to, which the route tables on it report as their own.
    pub outpost_arn: String,
    /// "The state of the local gateway" (`LocalGateway$State`), which the model leaves an
    /// unconstrained string.
    pub state: String,
    /// "The tags assigned to the local gateway" (`LocalGateway$Tags`).
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored local gateway holds: its tags, the Outpost ARN it
/// names and the state a writer pinned on it. Everything else on the record — the gateway
/// id and the owning account — is derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`local_gateway_route_table_payload_bytes`] is
/// for the route tables on a gateway.
pub fn local_gateway_payload_bytes(spec: &LocalGatewaySpec) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.outpost_arn.len())
        .saturating_add(spec.state.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another local gateway fits inside the store's ceiling
    /// ([`MAX_LOCAL_GATEWAYS`]).
    pub fn has_local_gateway_capacity(&self) -> bool {
        self.local_gateways.len() < MAX_LOCAL_GATEWAYS
    }

    /// Store one local gateway and answer the id it was given — or `None` when the store
    /// cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling ([`Self::has_local_gateway_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over [`local_gateway_payload_bytes`]) — the same
    /// contract as [`Self::insert_local_gateway_route_table`]. A caller that gets `None` must
    /// refuse the request rather than report a gateway it did not store.
    ///
    /// No EC2 operation creates a local gateway (one is part of an Outpost rack), so this is
    /// the seam an account's gateways are placed through rather than dead code: it is what
    /// gives [`require_known_local_gateway`] something to resolve.
    pub fn insert_local_gateway(&mut self, spec: LocalGatewaySpec) -> Option<String> {
        let payload_bytes = local_gateway_payload_bytes(&spec);
        if !self.has_local_gateway_capacity() || self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let local_gateway_id = self.next_id(LOCAL_GATEWAY_ID_PREFIX);
        let gateway = crate::ops::types::LocalGateway {
            local_gateway_id: Some(local_gateway_id.clone()),
            outpost_arn: Some(spec.outpost_arn),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            state: Some(spec.state),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(payload_bytes);
        self.local_gateways
            .insert(local_gateway_id.clone(), gateway);
        Some(local_gateway_id)
    }
}

// =============================================================================
// Local gateway route tables and the routes in them
// =============================================================================

/// The `ResourceType` enum member a local gateway route table is tagged and named under:
/// `local-gateway-route-table` (`smithy_query(query="shape",shape="ResourceType")`), which
/// is also the resource path [`local_gateway_route_table_arn`] builds its ARNs under. The
/// resource's one spelling, so the refusals that name it cannot drift from it.
pub const RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE: &str = "local-gateway-route-table";

/// The id prefix EC2 mints a local gateway route table with: `lgw-rtb-`, as the AWS
/// Outposts and EC2 documentation's own ids read (for example
/// `lgw-rtb-059615ef7dEXAMPLE`) and as the recorded wire corpus behind
/// `issues/…-ec2-createlocalgatewayroute-stub.md` shows — `lgw-rtb-0123456789abcdef0` is
/// answered with the route table's `.NotFound`, where `lgwrtb-00000000000000000` (the same
/// digits under a prefix that is not this one) is answered with its `.Malformed` before any
/// lookup. The model puts no pattern on `LocalGatewayRoutetableId`, so the prefix is the
/// one thing that says which resource an id names.
pub const LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX: &str = "lgw-rtb";

/// The id prefix a managed or Amazon-published prefix list carries: `pl-`, the form
/// `PrefixListResourceId` values take (`service_prefix_list_id` in
/// `crate::ops::describe_prefix_lists` mints exactly it). The model puts no pattern on
/// that shape, so the prefix is what says a value names a prefix list at all.
pub const PREFIX_LIST_ID_PREFIX: &str = "pl";

/// How many local gateway route tables this emulator stores before a write must refuse.
///
/// CHOSEN, like [`MAX_LOCAL_GATEWAY_VIF_GROUPS`] and for the same reason: nothing in this
/// emulator reclaims a stored route table, so the store needs a ceiling that is not the
/// process's memory.
pub const MAX_LOCAL_GATEWAY_ROUTE_TABLES: usize = 4096;

/// How many routes this emulator stores in ONE local gateway route table before a write
/// must refuse.
///
/// CHOSEN, for the reason [`MAX_LOCAL_GATEWAY_ROUTE_TABLES`] is: AWS publishes a quota for
/// the routes a local gateway route table holds, but not in anything the model or a
/// request carries, so this is a ceiling of this emulator's own rather than that quota.
pub const MAX_LOCAL_GATEWAY_ROUTES_PER_TABLE: usize = 1000;

/// A local gateway route table being created, which is what a create answers with until its
/// clock-owed move falls due: `pending`, the state the account's own tables are reported in
/// while they are being set up (`LocalGatewayRouteTable$State` is an unconstrained string
/// carrying the lifecycle `pending`, `available`, `deleting`, `deleted`).
pub const LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING: &str = "pending";

/// A local gateway route table that is in service: the state the clock owes a `pending`
/// table one [`TRANSITION_DELAY`] after the call that created it.
pub const LOCAL_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE: &str = "available";

/// A local gateway route table whose delete is under way: `deleting`, the non-terminal state
/// `DeleteLocalGatewayRouteTable` answers the table it took down in, out of the same
/// lifecycle [`LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING`] names (the model leaves
/// `LocalGatewayRouteTable$State` an unconstrained string).
pub const LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING: &str = "deleting";

/// A local gateway route table whose delete has finished: the terminal `deleted` of that
/// same lifecycle, the state the clock owes a `deleting` table one [`TRANSITION_DELAY`]
/// after the call that deleted it.
pub const LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED: &str = "deleted";

/// The mode of a local gateway route table that routes straight into the VPCs associated
/// with it: the `LocalGatewayRouteTableMode` enum's `direct-vpc-routing` member
/// (`smithy_query(query="shape",shape="LocalGatewayRouteTableMode")`, whose only other
/// member is `coip`), and the mode a create that names none writes — `coip` is the mode of a
/// table that routes through a customer-owned address pool, which is something a caller asks
/// for rather than something a create can assume.
pub const LOCAL_GATEWAY_ROUTE_TABLE_MODE_DIRECT_VPC_ROUTING: &str = "direct-vpc-routing";

/// A route "pending" insertion into the local gateway route table: the
/// `LocalGatewayRouteState` enum's `pending` member.
pub const LOCAL_GATEWAY_ROUTE_STATE_PENDING: &str = "pending";

/// A route that is carrying traffic: the `LocalGatewayRouteState` enum's `active` member.
pub const LOCAL_GATEWAY_ROUTE_STATE_ACTIVE: &str = "active";

/// A route whose target is gone, so traffic matching it is dropped: the
/// `LocalGatewayRouteState` enum's `blackhole` member.
pub const LOCAL_GATEWAY_ROUTE_STATE_BLACKHOLE: &str = "blackhole";

/// A route that has been deleted: the `LocalGatewayRouteState` enum's `deleted` member,
/// the TERMINAL state `DeleteLocalGatewayRoute` answers the route it removed in. The
/// catalog's `deleting` has no constant here because nothing writes it: this emulator owes
/// the clock no move after a delete, since the state AWS hands back from that call is
/// already terminal — the operation that needs `deleting` is the one that introduces a
/// deletion this service reports as under way.
pub const LOCAL_GATEWAY_ROUTE_STATE_DELETED: &str = "deleted";

/// A route a caller created: the `LocalGatewayRouteType` enum's `static` member, the type
/// "Creates a static route for the specified local gateway route table"
/// (`CreateLocalGatewayRoute`) writes.
pub const LOCAL_GATEWAY_ROUTE_TYPE_STATIC: &str = "static";

/// A route the local gateway learned rather than a caller created: the
/// `LocalGatewayRouteType` enum's `propagated` member.
pub const LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED: &str = "propagated";

/// `InvalidLocalGatewayRouteTableID.Malformed` (HTTP 400): the request named something
/// that is not a local gateway route table id at all, so there is nothing to look up. The
/// code AWS answers, as the recorded wire corpus behind
/// `issues/…-ec2-deletelocalgatewayroute-stub.md` shows.
pub const INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED: &str =
    "InvalidLocalGatewayRouteTableID.Malformed";

/// The code EC2 answers when a request would take the account past the local gateway route
/// tables it may hold, in the `<Resource>LimitExceeded` family this service's other quotas
/// are refused with ([`LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_LIMIT_EXCEEDED`]).
pub const LOCAL_GATEWAY_ROUTE_TABLE_LIMIT_EXCEEDED: &str = "LocalGatewayRouteTableLimitExceeded";

/// Refuse a route table create this store cannot take: HTTP 400
/// [`LOCAL_GATEWAY_ROUTE_TABLE_LIMIT_EXCEEDED`], naming the two bounds this emulator holds a
/// table against ([`MAX_LOCAL_GATEWAY_ROUTE_TABLES`] and [`MAX_STORED_BYTES`]) the way the
/// association stores name theirs.
pub fn local_gateway_route_table_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        LOCAL_GATEWAY_ROUTE_TABLE_LIMIT_EXCEEDED,
        400,
        format!(
            "This emulator stores at most {MAX_LOCAL_GATEWAY_ROUTE_TABLES} local gateway route \
             tables and at most {MAX_STORED_BYTES} bytes of caller-supplied data, and this \
             request would take it past one of the two"
        ),
    )
}

/// `InvalidLocalGatewayRouteTableID.NotFound` (HTTP 400): the request named a well-formed
/// local gateway route table id this account holds no route table for. The code AWS
/// answers, as the recorded wire corpus behind
/// `issues/…-ec2-createlocalgatewayroute-stub.md` shows.
pub const INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND: &str =
    "InvalidLocalGatewayRouteTableID.NotFound";

/// `InvalidRoute.NotFound` (HTTP 400): the route table exists, but it holds no route for
/// the destination the request named. EC2's own code for a route it cannot resolve in a
/// route table ("InvalidRoute.NotFound — The specified route does not exist", EC2 API
/// error codes), which is a different answer from the route TABLE's `.NotFound` above, so
/// a client can tell "no such route table" from "no such route in it".
pub const INVALID_ROUTE_NOT_FOUND: &str = "InvalidRoute.NotFound";

/// `RouteAlreadyExists` (HTTP 400): the route table already holds a route for the
/// destination the request named, so there is nothing to create. EC2's own code for it ("a
/// route for the specified CIDR block already exists in this route table", EC2 API error
/// codes), the counterpart of [`INVALID_ROUTE_NOT_FOUND`].
pub const ROUTE_ALREADY_EXISTS: &str = "RouteAlreadyExists";

/// Whether `id` is a well-formed local gateway route table id — one EC2 could have issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX`], whose doc carries the recorded evidence for
/// it). Anything else is `.Malformed` BEFORE any lookup, so a caller can tell "not an id of
/// this kind" from "no such route table".
pub fn local_gateway_route_table_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX, id)
}

/// Whether `id` is a well-formed prefix list id, the other form a local gateway route's
/// destination takes ("Use a prefix list in place of DestinationCidrBlock",
/// `DeleteLocalGatewayRouteRequest$DestinationPrefixListId`).
pub fn prefix_list_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(PREFIX_LIST_ID_PREFIX, id)
}

/// Whether `value` is an address prefix in CIDR notation, of either address family.
///
/// Reads the value with the two canonicalisers this crate already keeps one rule for
/// ([`canonical_ipv4_cidr`], [`canonical_ipv6_cidr`]) and keeps only the yes/no, for the
/// members that are matched EXACTLY rather than canonicalised — a local gateway route's
/// destination is one ("This must match the CIDR for the route exactly",
/// `DeleteLocalGatewayRouteRequest$DestinationCidrBlock`), so the block a request names is
/// read for its FORM and then compared as the caller wrote it.
pub fn cidr_block_is_well_formed(value: &str) -> bool {
    if value.contains(':') {
        canonical_ipv6_cidr(value).is_ok()
    } else {
        canonical_ipv4_cidr(value).is_ok()
    }
}

/// Refuse something that is not a local gateway route table id at all, for an operation whose
/// subject is the TABLE or an association with it: HTTP 400
/// `InvalidLocalGatewayRouteTableID.Malformed`, `The local-gateway-route-table ID '<id>' is
/// malformed`, built from the request's own value.
///
/// This is the sentence the operations that read a `LocalGatewayRouteTableId` as a parameter of
/// an action on the TABLE itself answer — the delete of a table, the associations with it, and
/// the search and the delete of a route in it — as their recorded refusals show, and the
/// sentence every operation answers a value that is no identifier at all with. The operations
/// that CREATE a resource belonging to the table word a value SHAPED like an id differently
/// ([`invalid_route_table_id_for_create`]). Which sentence an operation answers is a fact of
/// that operation's validator, read off its own recorded refusal.
pub fn invalid_local_gateway_route_table_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
        400,
        format!(
            "The {RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE} ID '{}' is malformed",
            echoed_resource_id(id)
        ),
    )
}

/// Refuse something that is not a local gateway route table id at all, for an operation that
/// CREATES a resource belonging to that table: HTTP 400
/// `InvalidLocalGatewayRouteTableID.Malformed`.
///
/// Which sentence the refusal carries follows the FORM of the value the request sent:
///
/// * a value shaped like an EC2 resource id ([`resource_id_shaped`]) that is none EC2 could
///   have issued for this resource is refused in the words of the service that owns the
///   local gateway — `Invalid LocalGateway Route Table id <id>`, a fixed prefix naming the
///   resource in the prose spelling [`invalid_local_gateway_route_table_id_not_found`] uses,
///   then the id the REQUEST sent, UNQUOTED and last, bounded by [`echoed_resource_id`];
/// * a value that is no identifier at all never reaches that service: it is refused at the
///   request boundary, in the boundary's own words for this resource kind
///   ([`invalid_local_gateway_route_table_id_malformed`]), the sentence the table's own
///   operations and the associations with it answer every malformed id with.
///
/// Both sentences are recorded for the creates that take this id as the table their new
/// resource belongs to — a static route (`CreateLocalGatewayRoute`) and a customer-owned
/// address pool (`CreateCoipPool`): the same operation answers a value carrying a hex
/// suffix (`lgw-rtb-` or `lgwrtb-` and digits of a length EC2 does not issue) with the
/// first and a value that is no id at all with the second, where every operation ON the
/// table answers both with the second. Nothing about the model distinguishes them, so the
/// form of the value is the only thing that chooses, and an operation added later takes the
/// sentence ITS recorded refusal carries.
pub fn invalid_route_table_id_for_create<E>(id: &str) -> Scaffolded<E> {
    if !resource_id_shaped(id) {
        return invalid_local_gateway_route_table_id_malformed(id);
    }
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED,
        400,
        format!(
            "Invalid LocalGateway Route Table id {}",
            echoed_resource_id(id)
        ),
    )
}

/// Refuse a `LocalGatewayRouteTableId` a create of a resource belonging to that table cannot
/// resolve: a malformed one with those creates' own sentence
/// ([`invalid_route_table_id_for_create`]) before an unknown one with the resource's single
/// `NotFound` sentence, the order [`require_well_formed_id`] documents.
///
/// The resolution itself is [`require_known_local_gateway_route_table_worded`]'s, shared with
/// every other reader of this id, so only the malformed sentence is these operations' own.
pub fn require_known_route_table_for_create<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_known_local_gateway_route_table_worded(store, id, invalid_route_table_id_for_create)
}

/// The form check [`require_known_route_table_for_create`] makes before it looks an id up, for
/// a create's own earlier reading of the request's FORM — a `DryRun` is answered only once that
/// form has been read.
pub fn require_well_formed_route_table_id_for_create<E>(id: &str) -> Result<(), Scaffolded<E>> {
    require_well_formed_local_gateway_route_table_id_worded(id, invalid_route_table_id_for_create)
}

/// Refuse a well-formed local gateway route table id this account holds no route table
/// for: HTTP 400 `InvalidLocalGatewayRouteTableID.NotFound`, worded the way AWS words it
/// ("The LocalGateway Route Table ID '…' does not exist").
pub fn invalid_local_gateway_route_table_id_not_found<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND,
        400,
        format!("The LocalGateway Route Table ID '{id}' does not exist"),
    )
}

/// Refuse something that is not a local gateway route table id at all, for a handler that
/// has to read the request's own FORM before it resolves anything — a `DryRun` is answered
/// only once that form has been read, and a malformed id is a malformed REQUEST rather than
/// a missing resource.
///
/// The one form check [`require_known_local_gateway_route_table`] itself makes before it
/// looks an id up, so an operation that needs it earlier asks the same question of the same
/// reader. A create of a resource belonging to the table asks it through
/// [`require_well_formed_route_table_id_for_create`], which differs only in the sentence those
/// validators word the refusal with.
pub fn require_well_formed_local_gateway_route_table_id<E>(id: &str) -> Result<(), Scaffolded<E>> {
    require_well_formed_local_gateway_route_table_id_worded(
        id,
        invalid_local_gateway_route_table_id_malformed,
    )
}

/// The form half of resolving a local gateway route table id, with `malformed` writing the
/// sentence the CALLING operation's validator words that refusal with.
///
/// The ONE reading of this id's form, so the two sentences EC2 answers this refusal class with
/// cannot drift into two readings of what a route table id IS.
pub(super) fn require_well_formed_local_gateway_route_table_id_worded<E>(
    id: &str,
    malformed: fn(&str) -> Scaffolded<E>,
) -> Result<(), Scaffolded<E>> {
    if !local_gateway_route_table_id_is_well_formed(id) {
        return Err(malformed(id));
    }
    Ok(())
}

/// Refuse a `LocalGatewayRouteTableId` the table's own operations cannot resolve — the delete of
/// a table, an association with it, and the search and the delete of a route in it: a malformed
/// one before an unknown one, the order [`require_well_formed_id`] documents.
///
/// The ONE place those operations resolve a local gateway route table id, so each refuses the
/// same id the same way; the contract [`require_known_instance`] writes out for instances.
/// A create of a resource belonging to the table resolves it through
/// [`require_known_route_table_for_create`] instead, which shares this resolution and differs
/// only in the sentence it words a malformed id with.
pub fn require_known_local_gateway_route_table<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_known_local_gateway_route_table_worded(
        store,
        id,
        invalid_local_gateway_route_table_id_malformed,
    )
}

/// Resolve a local gateway route table id — malformed before unknown — with `malformed` writing
/// the sentence the CALLING operation's validator words that refusal with.
///
/// The ONE lookup and the ONE ordering behind both of this resource's malformed sentences, so
/// only the wording is per-validator.
pub(super) fn require_known_local_gateway_route_table_worded<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
    malformed: fn(&str) -> Scaffolded<E>,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_local_gateway_route_table_id_worded(id, malformed)?;
    if !store.local_gateway_route_tables.contains_key(id) {
        return Err(invalid_local_gateway_route_table_id_not_found(id));
    }
    Ok(())
}

/// Resolve a local gateway route table an operation is about to TAKE DOWN: the id has to be
/// well formed and name a route table of the account — the two refusals
/// [`require_known_local_gateway_route_table`] answers, in that order — AND that table must
/// not be one a delete has already taken down.
///
/// A table a delete answered is still reported, moving from
/// [`LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING`] to the terminal
/// [`LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED`], so it still RESOLVES — but there is nothing
/// left to take down, and what the caller would have to change is the state of the table
/// itself, which is what EC2's `IncorrectState` names ([`incorrect_state_of`]). The ONE place
/// that guard lives, so the operations that act on a route table cannot disagree about which
/// ones admit a call; the contract [`require_live_route_server`] writes out for the resource
/// whose own state enum carries the same two delete members.
///
/// The record is resolved ONCE here, for the reason that guard resolves its own once: the
/// state the refusal names is the state the lookup read.
pub fn require_live_local_gateway_route_table<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_local_gateway_route_table_id(id)?;
    let Some(record) = store.local_gateway_route_tables.get(id) else {
        return Err(invalid_local_gateway_route_table_id_not_found(id));
    };
    let state = record.view.state.as_deref().unwrap_or_default();
    if matches!(
        state,
        LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETING | LOCAL_GATEWAY_ROUTE_TABLE_STATE_DELETED
    ) {
        return Err(incorrect_state_of(
            RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE,
            id,
            state,
        ));
    }
    Ok(())
}

/// Refuse a destination the named local gateway route table holds no route for: HTTP 400
/// [`INVALID_ROUTE_NOT_FOUND`].
pub fn local_gateway_route_not_found<E>(route_table_id: &str, destination: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_NOT_FOUND,
        400,
        format!(
            "The route '{destination}' does not exist in the local gateway route table \
             '{route_table_id}'"
        ),
    )
}

/// Refuse a destination the named local gateway route table already holds a route for: HTTP
/// 400 [`ROUTE_ALREADY_EXISTS`], worded the way AWS words a route that is already there and
/// built from the destination the request named.
pub fn local_gateway_route_already_exists<E>(destination: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        ROUTE_ALREADY_EXISTS,
        400,
        format!("The route identified by {destination} already exists."),
    )
}

/// Refuse a route create this store cannot take: HTTP 400 [`ROUTE_LIMIT_EXCEEDED`], the
/// code EC2 answers a route table that can hold no more routes, naming the two bounds this
/// emulator holds a route against ([`MAX_LOCAL_GATEWAY_ROUTES_PER_TABLE`] and
/// [`MAX_STORED_BYTES`]) the way the association stores name theirs.
pub fn local_gateway_route_limit_exceeded<E>(route_table_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        ROUTE_LIMIT_EXCEEDED,
        400,
        format!(
            "The local gateway route table '{route_table_id}' holds at most \
             {MAX_LOCAL_GATEWAY_ROUTES_PER_TABLE} routes, and this emulator at most \
             {MAX_STORED_BYTES} bytes of caller-supplied data; this request would take it \
             past one of the two"
        ),
    )
}

/// What a local gateway route MATCHES, and therefore what it is named by inside its route
/// table: one address prefix, given either literally or as a prefix list.
///
/// "You cannot use DestinationPrefixListId and DestinationCidrBlock in the same request"
/// (`DeleteLocalGatewayRouteRequest$DestinationPrefixListId`), so exactly one of the two
/// names a route and this enum is that choice. The two forms share one key space
/// ([`Self::key`]): a prefix list id can never read as a CIDR block, so no route can be
/// reached by the wrong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalGatewayRouteDestination {
    /// "The CIDR range used for destination matches"
    /// (`CreateLocalGatewayRouteRequest$DestinationCidrBlock`), as the caller wrote it:
    /// "This must match the CIDR for the route exactly"
    /// (`DeleteLocalGatewayRouteRequest$DestinationCidrBlock`) makes the comparison exact,
    /// so the value is NOT canonicalised on the way in.
    CidrBlock(String),
    /// "The ID of the prefix list. Use a prefix list in place of DestinationCidrBlock"
    /// (`CreateLocalGatewayRouteRequest$DestinationPrefixListId`).
    PrefixListId(String),
}

impl LocalGatewayRouteDestination {
    /// The key this destination names its route by inside
    /// [`LocalGatewayRouteTableRecord::routes`].
    pub fn key(&self) -> &str {
        match self {
            Self::CidrBlock(block) => block.as_str(),
            Self::PrefixListId(id) => id.as_str(),
        }
    }
}

/// The destination a request names, or the refusal EC2 answers when it names none or both.
///
/// The ONE reading of that pair of members, so every operation that takes a local gateway
/// route's destination (`CreateLocalGatewayRoute`, `ModifyLocalGatewayRoute`,
/// `DeleteLocalGatewayRoute`) refuses the same request the same way:
///
/// * both given — `InvalidParameterCombination`: "You cannot use DestinationPrefixListId
///   and DestinationCidrBlock in the same request" (`$DestinationPrefixListId`);
/// * neither given — `MissingParameter`, since a route is named by its destination and the
///   model marks neither member required (so the boundary validator passes the request);
/// * a `DestinationCidrBlock` that is not an address prefix, or a
///   `DestinationPrefixListId` that is not a prefix list id — `InvalidParameterValue`,
///   naming the parameter and the form it takes, in the one wording this service answers a
///   rejected parameter VALUE with ("Value (…) for parameter … is invalid. …", the wording
///   the other two route creates of this service answer a destination that is not a CIDR
///   block with, and [`invalid_id_parameter_value`] for an id parameter).
pub fn require_local_gateway_route_destination<E>(
    destination_cidr_block: Option<&str>,
    destination_prefix_list_id: Option<&str>,
) -> Result<LocalGatewayRouteDestination, Scaffolded<E>> {
    match (destination_cidr_block, destination_prefix_list_id) {
        (Some(_), Some(_)) => Err(invalid_parameter_combination(
            "You cannot use destinationPrefixListId and destinationCidrBlock in the same \
             request",
        )),
        (Some(cidr_block), None) => {
            if !cidr_block_is_well_formed(cidr_block) {
                return Err(invalid_parameter_value(format!(
                    "Value ({cidr_block}) for parameter destinationCidrBlock is invalid. \
                     This is not a valid CIDR block."
                )));
            }
            Ok(LocalGatewayRouteDestination::CidrBlock(
                cidr_block.to_string(),
            ))
        }
        (None, Some(prefix_list_id)) => {
            if !prefix_list_id_is_well_formed(prefix_list_id) {
                return Err(invalid_id_parameter_value(
                    "destinationPrefixListId",
                    prefix_list_id,
                    PREFIX_LIST_ID_PREFIX,
                ));
            }
            Ok(LocalGatewayRouteDestination::PrefixListId(
                prefix_list_id.to_string(),
            ))
        }
        (None, None) => Err(missing_parameter(
            "The request must contain the parameter destinationCidrBlock or \
             destinationPrefixListId",
        )),
    }
}

/// WHERE a local gateway route sends the traffic its destination matches: exactly one of the
/// two targets the operation that writes a route documents ("You must specify one of the
/// following targets: LocalGatewayVirtualInterfaceGroupId NetworkInterfaceId",
/// `CreateLocalGatewayRoute`).
///
/// The choice is read ONCE out of the pair of members a request carries
/// ([`require_local_gateway_route_target`]), so the form check, the lookup that resolves it
/// and the member the stored route reports it in all ask the same question of the same
/// answer — the arrangement [`LocalGatewayRouteDestination`] has for the pair of members
/// that NAMES a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalGatewayRouteTarget {
    /// "The ID of the virtual interface group" (`$LocalGatewayVirtualInterfaceGroupId`).
    VirtualInterfaceGroup(String),
    /// "The ID of the network interface" (`$NetworkInterfaceId`).
    NetworkInterface(String),
}

impl LocalGatewayRouteTarget {
    /// The id this target is named by, whichever of the two members carried it.
    pub fn id(&self) -> &str {
        match self {
            Self::VirtualInterfaceGroup(id) => id.as_str(),
            Self::NetworkInterface(id) => id.as_str(),
        }
    }

    /// The pair of members a stored route reports this target in — the virtual interface
    /// group and the network interface, exactly one of them set — so a route can never
    /// report two targets at once.
    pub fn members(&self) -> (Option<String>, Option<String>) {
        match self {
            Self::VirtualInterfaceGroup(id) => (Some(id.clone()), None),
            Self::NetworkInterface(id) => (None, Some(id.clone())),
        }
    }
}

/// The target a request names, or the refusal EC2 answers when it names none, both, or a
/// value that is no id of its resource at all.
///
/// The ONE reading of that pair of members, so every operation that points a local gateway
/// route at a target (`CreateLocalGatewayRoute`, `ModifyLocalGatewayRoute`) refuses the same
/// request the same way:
///
/// * both given — `InvalidParameterCombination`, a route having one target;
/// * neither given — `InvalidParameterValue`, since "You must specify ONE of the following
///   targets" (`CreateLocalGatewayRoute`) is about the VALUE a request names a target with
///   rather than about one member it left out, neither member being required on its own;
/// * a value that is no id of its resource — that resource's own `.Malformed`, answered
///   here on the request's FORM, before anything is resolved
///   ([`require_known_local_gateway_route_target`] does the resolving).
pub fn require_local_gateway_route_target<E>(
    local_gateway_virtual_interface_group_id: Option<&str>,
    network_interface_id: Option<&str>,
) -> Result<LocalGatewayRouteTarget, Scaffolded<E>> {
    match (
        local_gateway_virtual_interface_group_id,
        network_interface_id,
    ) {
        (Some(_), Some(_)) => Err(invalid_parameter_combination(
            "You cannot use localGatewayVirtualInterfaceGroupId and networkInterfaceId in \
             the same request",
        )),
        (None, None) => Err(invalid_parameter_value(
            "You must specify one of the following targets: \
             localGatewayVirtualInterfaceGroupId, networkInterfaceId",
        )),
        (Some(group_id), None) => {
            if !local_gateway_virtual_interface_group_id_is_well_formed(group_id) {
                return Err(invalid_local_gateway_virtual_interface_group_id_malformed(
                    group_id,
                ));
            }
            Ok(LocalGatewayRouteTarget::VirtualInterfaceGroup(
                group_id.to_string(),
            ))
        }
        (None, Some(interface_id)) => {
            if !network_interface_id_is_well_formed(interface_id) {
                return Err(invalid_network_interface_id_malformed(interface_id));
            }
            Ok(LocalGatewayRouteTarget::NetworkInterface(
                interface_id.to_string(),
            ))
        }
    }
}

/// Refuse a target id the account holds no such resource for with that resource's own
/// `.NotFound`, so a route is never stored pointing at nothing.
///
/// The ONE resolution of a route's target: the virtual interface group through the crate's
/// one reader of that id ([`require_known_local_gateway_virtual_interface_group`]), the
/// network interface through the store the account's interfaces live in.
pub fn require_known_local_gateway_route_target<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    target: &LocalGatewayRouteTarget,
) -> Result<(), Scaffolded<E>> {
    match target {
        LocalGatewayRouteTarget::VirtualInterfaceGroup(group_id) => {
            require_known_local_gateway_virtual_interface_group(store, group_id)
        }
        LocalGatewayRouteTarget::NetworkInterface(interface_id) => {
            if !store.network_interfaces.contains_key(interface_id) {
                return Err(invalid_network_interface_id_not_found(interface_id));
            }
            Ok(())
        }
    }
}

/// One stored local gateway route table: the modelled shape a report of the tables answers
/// with, and the routes that table holds.
///
/// The routes live HERE rather than in a map of their own because a route has no id: it is
/// named by its destination inside one route table ("Deletes the specified route from the
/// specified local gateway route table", `DeleteLocalGatewayRoute`), so the table is the
/// only thing that can key them.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableRecord {
    /// The table as every reader of it reports it.
    pub view: crate::ops::types::LocalGatewayRouteTable,
    /// The routes in the table, by the destination that names each one
    /// ([`LocalGatewayRouteDestination::key`]). Ordered by destination, the stable order a
    /// search of the routes reports and pages over.
    pub routes: BTreeMap<String, LocalGatewayRouteRecord>,
    /// The move a table still [`LOCAL_GATEWAY_ROUTE_TABLE_STATE_PENDING`] owes; `None` for
    /// one in a state the clock owes nothing on. Beside the modelled shape because
    /// `LocalGatewayRouteTable` carries no member for when its next state falls due — the
    /// arrangement [`LocalGatewayRouteRecord::pending`] has for a route in the table.
    pub pending: Option<PendingStateMove>,
}

/// One stored local gateway route: the modelled shape every reader of it reports, plus the
/// move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason
/// [`LocalGatewayRouteTableVpcAssociationRecord`] holds one; the pending move lives beside
/// it because `LocalGatewayRoute` carries no member for when the route's next state falls
/// due.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteRecord {
    /// The route as the model shapes it, carrying the state reached so far.
    pub view: crate::ops::types::LocalGatewayRoute,
    /// The move a route still `pending` owes; `None` for one in a state the clock owes
    /// nothing on.
    pub pending: Option<PendingStateMove>,
}

/// What one local gateway route table is written with: the local gateway and Outpost it
/// belongs to, the Region whose ARN namespace its ARN names, its mode and state, and its
/// tags.
///
/// Every member the service itself owns — the table id, the ARN built from the Region and
/// that id, and the owning account — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table`] rather than taken
/// from the caller, so two writers cannot report one table differently.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableSpec {
    /// "The ID of the local gateway" the table belongs to.
    pub local_gateway_id: String,
    /// "The Amazon Resource Name (ARN) of the Outpost"
    /// (`LocalGatewayRouteTable$OutpostArn`).
    pub outpost_arn: String,
    /// The Region the table's ARN names.
    pub region: String,
    /// "The mode of the local gateway route table" (`LocalGatewayRouteTable$Mode`), one of
    /// [`crate::ops::types::LOCAL_GATEWAY_ROUTE_TABLE_MODE_VALUES`].
    pub mode: String,
    /// "The state of the local gateway route table" (`LocalGatewayRouteTable$State`),
    /// which the model leaves an unconstrained string.
    pub state: String,
    /// The state the clock owes this table, [`TRANSITION_DELAY`] from the write; `None` for
    /// one written in a state nothing is owed on — the contract
    /// [`LocalGatewayRouteTableVpcAssociationSpec::becomes`] writes out for the associations
    /// made in it.
    pub becomes: Option<String>,
    /// "The tags assigned to the local gateway route table".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored route table holds: its tags, the state and mode a
/// writer pinned on it and the ids and ARNs it names. Everything else on the record — the
/// table id, its own ARN, the owning account — is derived by this module, so it is
/// fixed-size.
///
/// The ONE definition of that charge, the way
/// [`local_gateway_vif_group_association_payload_bytes`] is for the store beside it.
pub fn local_gateway_route_table_payload_bytes(spec: &LocalGatewayRouteTableSpec) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.local_gateway_id.len())
        .saturating_add(spec.outpost_arn.len())
        .saturating_add(spec.mode.len())
        .saturating_add(spec.state.len())
}

/// The one resource type a `TagSpecifications` entry on a create of a local gateway route
/// table / VPC association may name: `ResourceType`'s
/// `local-gateway-route-table-vpc-association` member
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const RESOURCE_TYPE_LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION: &str =
    "local-gateway-route-table-vpc-association";

/// The id prefix EC2 mints an association between a local gateway route table and a VPC
/// with: `lgw-vpc-assoc-`, as the AWS Outposts and EC2 documentation's own ids read (for
/// example `lgw-vpc-assoc-0e0f27af15EXAMPLE`). The model puts no pattern on
/// `LocalGatewayRouteTableVpcAssociationId`, so the prefix is the one thing that says which
/// resource an id names.
pub const LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX: &str = "lgw-vpc-assoc";

/// How many local gateway route table / VPC associations this emulator stores before a write
/// must refuse — CHOSEN, for the reason [`MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS`] is.
pub const MAX_LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATIONS: usize = 4096;

/// The code EC2 answers when a request would take the account past the VPC associations it
/// may hold, in the `<Resource>LimitExceeded` family this service's other quotas are refused
/// with ([`SECONDARY_NETWORK_LIMIT_EXCEEDED`]).
pub const LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_LIMIT_EXCEEDED: &str =
    "LocalGatewayRouteTableVpcAssociationLimitExceeded";

/// An association being carried out, which is what a create answers with until its
/// clock-owed move falls due: `associating`, the member every association-state enum of this
/// model spells it with (`smithy_query(query="shape",shape="TransitGatewayAssociationState")`
/// — `associating`, `associated`, `disassociating`, `disassociated`), the local gateway
/// association's own `State` being an unconstrained string.
pub const LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association in force: that same vocabulary's `associated`, the state the clock owes an
/// `associating` association one [`TRANSITION_DELAY`] later.
pub const LOCAL_GATEWAY_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// One association between a local gateway route table and a VPC: the modelled shape every
/// operation that reports one answers with, plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// here holds one: a member a writer sets round-trips through
/// `DescribeLocalGatewayRouteTableVpcAssociations` without a field-by-field copy that can
/// silently drop one. The pending move lives beside it because the shape carries no member
/// for when the association's next state falls due.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableVpcAssociationRecord {
    /// The association as the model shapes it, carrying the state reached so far.
    pub view: crate::ops::types::LocalGatewayRouteTableVpcAssociation,
    /// The move an association still `associating` owes; `None` for one in a state the clock
    /// owes nothing on.
    pub pending: Option<PendingStateMove>,
}

/// What one association between a local gateway route table and a VPC is written with: the
/// route table it is made in (with the ARN and local gateway the route table itself
/// reports), the VPC it joins, the state it starts in, the state the clock owes it, and its
/// tags.
///
/// Every member the service itself owns — the association id and the owning account — is
/// derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_vpc_association`] rather
/// than taken from the caller.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableVpcAssociationSpec {
    /// "The ID of the local gateway route table" the association is made in.
    pub local_gateway_route_table_id: String,
    /// "The Amazon Resource Name (ARN) of the local gateway route table for the
    /// association", as the stored route table reports it.
    pub local_gateway_route_table_arn: String,
    /// "The ID of the local gateway", as the stored route table reports it.
    pub local_gateway_id: String,
    /// "The ID of the VPC" the route table is associated with.
    pub vpc_id: String,
    /// "The state of the association" (`LocalGatewayRouteTableVpcAssociation$State`), which
    /// the model leaves an unconstrained string.
    pub state: String,
    /// The state the clock owes this association, [`TRANSITION_DELAY`] from the write;
    /// `None` for one written in a state nothing is owed on.
    pub becomes: Option<String>,
    /// "The tags assigned to the association".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored VPC association holds: its tags, the state a writer
/// pinned on it, and the ids and ARN of the resources it joins. Everything else on the record
/// — the association id, the owning account — is derived by this module.
///
/// The ONE definition of that charge, the way
/// [`local_gateway_vif_group_association_payload_bytes`] is for the virtual interface group
/// associations: the insert charges exactly it, and a removal path must give back the same
/// number.
pub fn local_gateway_route_table_vpc_association_payload_bytes(
    spec: &LocalGatewayRouteTableVpcAssociationSpec,
) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.state.len())
        .saturating_add(spec.local_gateway_route_table_id.len())
        .saturating_add(spec.local_gateway_route_table_arn.len())
        .saturating_add(spec.local_gateway_id.len())
        .saturating_add(spec.vpc_id.len())
}

/// The caller-chosen bytes one stored route holds: its destination, its target and the
/// type and state it carries. The ONE definition of that charge, so
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route`] charges exactly what
/// [`ComAmazonawsEc2Amazonec2State::remove_local_gateway_route`] gives back.
pub fn local_gateway_route_payload_bytes(route: &crate::ops::types::LocalGatewayRoute) -> usize {
    [
        route.destination_cidr_block.as_deref(),
        route.destination_prefix_list_id.as_deref(),
        route.local_gateway_virtual_interface_group_id.as_deref(),
        route.network_interface_id.as_deref(),
        route.subnet_id.as_deref(),
        route.coip_pool_id.as_deref(),
        route.r#type.as_deref(),
        route.state.as_deref(),
    ]
    .into_iter()
    .flatten()
    .fold(0usize, |bytes, value| bytes.saturating_add(value.len()))
}

/// What one local gateway route is written with: the table it goes in, the destination it
/// matches, the target traffic to that destination is sent to, and the type and state the
/// route carries.
///
/// Every member the service itself owns — the route table's ARN and the owning account —
/// is taken from the STORED route table by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route`] rather than from the
/// caller, so a route can never name a table it is not in.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteSpec {
    /// "The ID of the local gateway route table" the route goes in.
    pub local_gateway_route_table_id: String,
    /// What the route matches, and what names it inside that table.
    pub destination: LocalGatewayRouteDestination,
    /// "The ID of the virtual interface group" the route sends traffic to, for a route
    /// whose target is one.
    pub local_gateway_virtual_interface_group_id: Option<String>,
    /// "The ID of the network interface" the route sends traffic to, for a route whose
    /// target is one.
    pub network_interface_id: Option<String>,
    /// "The ID of the subnet" (`LocalGatewayRoute$SubnetId`), for a route AWS reports one
    /// for.
    pub subnet_id: Option<String>,
    /// "The ID of the customer-owned address pool" (`LocalGatewayRoute$CoipPoolId`), for a
    /// route AWS reports one for.
    pub coip_pool_id: Option<String>,
    /// "The route type" (`LocalGatewayRoute$Type`), one of
    /// [`crate::ops::types::LOCAL_GATEWAY_ROUTE_TYPE_VALUES`].
    pub route_type: String,
    /// "The state of the route" (`LocalGatewayRoute$State`), one of
    /// [`crate::ops::types::LOCAL_GATEWAY_ROUTE_STATE_VALUES`].
    pub state: String,
    /// The state the clock owes this route, [`TRANSITION_DELAY`] from the write; `None` for
    /// one written in a state nothing is owed on — the contract
    /// [`LocalGatewayRouteTableVpcAssociationSpec::becomes`] writes out.
    pub becomes: Option<String>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another local gateway route table fits inside the store's ceiling
    /// ([`MAX_LOCAL_GATEWAY_ROUTE_TABLES`]).
    pub fn has_local_gateway_route_table_capacity(&self) -> bool {
        self.local_gateway_route_tables.len() < MAX_LOCAL_GATEWAY_ROUTE_TABLES
    }

    /// Store one local gateway route table and answer the id it was given — or `None` when
    /// the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling ([`Self::has_local_gateway_route_table_capacity`]) and
    /// the byte budget ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_route_table_payload_bytes`]) — the same contract as
    /// [`Self::insert_local_gateway_route_table_virtual_interface_group_association`]. A
    /// caller that gets `None` must refuse the request rather than report a table it did
    /// not store.
    pub fn insert_local_gateway_route_table(
        &mut self,
        spec: LocalGatewayRouteTableSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_route_table_payload_bytes(&spec);
        if !self.has_local_gateway_route_table_capacity()
            || self.would_exceed_stored_bytes(payload_bytes)
        {
            return None;
        }
        let route_table_id = self.next_id(LOCAL_GATEWAY_ROUTE_TABLE_ID_PREFIX);
        let view = crate::ops::types::LocalGatewayRouteTable {
            local_gateway_route_table_id: Some(route_table_id.clone()),
            local_gateway_route_table_arn: Some(local_gateway_route_table_arn(
                &spec.region,
                &route_table_id,
            )),
            local_gateway_id: Some(spec.local_gateway_id),
            outpost_arn: Some(spec.outpost_arn),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            state: Some(spec.state),
            tags: Some(spec.tags),
            mode: Some(spec.mode),
            state_reason: None,
        };
        self.add_stored_bytes(payload_bytes);
        self.local_gateway_route_tables.insert(
            route_table_id.clone(),
            LocalGatewayRouteTableRecord {
                view,
                routes: BTreeMap::new(),
                // RECORDED, never slept for: [`Self::materialize_due_transitions`] applies
                // it once [`TRANSITION_DELAY`] has passed, so a caller asking straight after
                // the write still sees the state the table was created in.
                pending: spec.becomes.map(PendingStateMove::starting_now),
            },
        );
        Some(route_table_id)
    }

    /// The route the local gateway route table `route_table_id` holds for `destination`, if
    /// it holds one.
    ///
    /// The ONE lookup of a route, so every reader of one asks the same question of the same
    /// map: a destination is matched EXACTLY ("This must match the CIDR for the route
    /// exactly", `DeleteLocalGatewayRouteRequest$DestinationCidrBlock`).
    pub fn local_gateway_route(
        &self,
        route_table_id: &str,
        destination: &str,
    ) -> Option<&crate::ops::types::LocalGatewayRoute> {
        self.local_gateway_route_tables
            .get(route_table_id)?
            .routes
            .get(destination)
            .map(|record| &record.view)
    }

    /// Store one route in the local gateway route table its spec names, and answer it as
    /// the store now holds it — or `None` when there is no such table, it already holds a
    /// route for that destination, or the store cannot take another.
    ///
    /// The route's `LocalGatewayRouteTableArn` and `OwnerId` come from the STORED table, so
    /// a route and the table it is in can never disagree about which Region and account
    /// they belong to.
    pub fn insert_local_gateway_route(
        &mut self,
        spec: LocalGatewayRouteSpec,
    ) -> Option<crate::ops::types::LocalGatewayRoute> {
        let (cidr_block, prefix_list_id) = match &spec.destination {
            LocalGatewayRouteDestination::CidrBlock(block) => (Some(block.clone()), None),
            LocalGatewayRouteDestination::PrefixListId(id) => (None, Some(id.clone())),
        };
        let table = self
            .local_gateway_route_tables
            .get(&spec.local_gateway_route_table_id)?;
        if table.routes.len() >= MAX_LOCAL_GATEWAY_ROUTES_PER_TABLE
            || table.routes.contains_key(spec.destination.key())
        {
            return None;
        }
        let route = crate::ops::types::LocalGatewayRoute {
            destination_cidr_block: cidr_block,
            local_gateway_virtual_interface_group_id: spec
                .local_gateway_virtual_interface_group_id,
            r#type: Some(spec.route_type),
            state: Some(spec.state),
            local_gateway_route_table_id: Some(spec.local_gateway_route_table_id.clone()),
            local_gateway_route_table_arn: table.view.local_gateway_route_table_arn.clone(),
            owner_id: table.view.owner_id.clone(),
            subnet_id: spec.subnet_id,
            coip_pool_id: spec.coip_pool_id,
            network_interface_id: spec.network_interface_id,
            destination_prefix_list_id: prefix_list_id,
        };
        let payload_bytes = local_gateway_route_payload_bytes(&route);
        if self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        // The write FIRST and the charge after it, so the budget can never be charged for
        // a route that was not stored.
        self.local_gateway_route_tables
            .get_mut(&spec.local_gateway_route_table_id)?
            .routes
            .insert(
                spec.destination.key().to_string(),
                LocalGatewayRouteRecord {
                    view: route.clone(),
                    pending: spec.becomes.map(PendingStateMove::starting_now),
                },
            );
        self.add_stored_bytes(payload_bytes);
        Some(route)
    }

    /// Take the route `route_table_id` holds for `destination` out of that table, and
    /// answer it in the terminal `deleted` state EC2 reports a deleted local gateway route
    /// in — or `None` when there is no such table or route.
    ///
    /// The record LEAVES the store rather than lingering: a deleted route is not in the
    /// route table any more, so a later search of that table must not report it, and the
    /// caller-chosen bytes it held go back to the budget — exactly what
    /// [`Self::insert_local_gateway_route`] charged for it.
    pub fn remove_local_gateway_route(
        &mut self,
        route_table_id: &str,
        destination: &str,
    ) -> Option<crate::ops::types::LocalGatewayRoute> {
        let mut route = self
            .local_gateway_route_tables
            .get_mut(route_table_id)?
            .routes
            .remove(destination)?
            .view;
        self.release_stored_bytes(local_gateway_route_payload_bytes(&route));
        route.state = Some(LOCAL_GATEWAY_ROUTE_STATE_DELETED.to_string());
        Some(route)
    }

    /// Point the route the local gateway route table `route_table_id` holds for
    /// `destination` at `target`, put it in `state` with the move `becomes` owes it, and
    /// answer it as the store now holds it — or `None` when there is no such table or no
    /// such route.
    ///
    /// The route keeps its identity: the destination that names it, the table it is in and
    /// that table's own ARN and account are left exactly as they were, because this replaces
    /// WHERE the route sends traffic rather than which route it is. Exactly one of the two
    /// target members ends up set ([`LocalGatewayRouteTarget::members`]), so a retargeted
    /// route can never report the target it used to have beside the one it has now.
    ///
    /// The store's byte charge is ADJUSTED rather than re-checked: the route is already
    /// stored and one id EC2 could have issued replaces another, both bounded by the form
    /// their own readers accept, so the charge stays exact without a budget a stored route
    /// could fail. The record is taken ONCE, so the release and the charge are
    /// unconditionally paired: there is no second lookup between them that could leave the
    /// budget holding a figure for a route nothing wrote.
    pub fn retarget_local_gateway_route(
        &mut self,
        route_table_id: &str,
        destination: &str,
        target: &LocalGatewayRouteTarget,
        state: &str,
        becomes: Option<&str>,
    ) -> Option<crate::ops::types::LocalGatewayRoute> {
        let record = self
            .local_gateway_route_tables
            .get_mut(route_table_id)?
            .routes
            .get_mut(destination)?;
        let before_bytes = local_gateway_route_payload_bytes(&record.view);
        let (virtual_interface_group_id, network_interface_id) = target.members();
        record.view.local_gateway_virtual_interface_group_id = virtual_interface_group_id;
        record.view.network_interface_id = network_interface_id;
        record.view.state = Some(state.to_string());
        // RECORDED, never slept for: [`Self::materialize_due_transitions`] applies it once
        // [`TRANSITION_DELAY`] has passed, so a caller searching the table straight after
        // this write still sees the state the route was put in here.
        record.pending = becomes.map(PendingStateMove::starting_now);
        let after = record.view.clone();
        let after_bytes = local_gateway_route_payload_bytes(&after);
        self.release_stored_bytes(before_bytes);
        self.add_stored_bytes(after_bytes);
        Some(after)
    }
}

/// Refuse a local gateway route the local gateway PROPAGATED rather than a caller created,
/// for the `action` asked of it: HTTP 400 `OperationNotPermitted`.
///
/// "The route type" (`LocalGatewayRoute$Type`) is what decides whether there is anything for
/// a caller to do with a route: a `propagated` route is one the local gateway learned, while
/// a `static` one is exactly what `CreateLocalGatewayRoute` put there. The ONE wording of
/// that refusal, so the operations that change a stored route cannot drift on the same class
/// — only the `action` each of them performs differs.
pub fn require_static_local_gateway_route<E>(
    route: &crate::ops::types::LocalGatewayRoute,
    route_table_id: &str,
    destination: &str,
    action: &str,
) -> Result<(), Scaffolded<E>> {
    if route.r#type.as_deref() != Some(LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED) {
        return Ok(());
    }
    Err(Scaffolded::operation_not_permitted(format!(
        "The route '{destination}' in the local gateway route table '{route_table_id}' is a \
         {LOCAL_GATEWAY_ROUTE_TYPE_PROPAGATED} route, which cannot be {action}"
    )))
}

// =============================================================================
// Local gateway route table / VPC associations
// =============================================================================

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` local gateway route table / VPC associations fit inside the
    /// store's ceiling ([`MAX_LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATIONS`]).
    pub fn has_local_gateway_route_table_vpc_association_capacity(&self, count: usize) -> bool {
        self.local_gateway_route_table_vpc_associations
            .len()
            .saturating_add(count)
            <= MAX_LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATIONS
    }

    /// The id of the association that already joins `route_table_id` and `vpc_id`, if the
    /// account holds one.
    ///
    /// An association IS the pair it joins, so this is what a writer asks before making a
    /// second one: without it the store would hold two association ids for one fact. The
    /// scan is bounded by the store's own ceiling
    /// ([`MAX_LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATIONS`]).
    pub fn local_gateway_route_table_vpc_association_of(
        &self,
        route_table_id: &str,
        vpc_id: &str,
    ) -> Option<&str> {
        self.local_gateway_route_table_vpc_associations
            .iter()
            .find(|(_, record)| {
                record.view.local_gateway_route_table_id.as_deref() == Some(route_table_id)
                    && record.view.vpc_id.as_deref() == Some(vpc_id)
            })
            .map(|(association_id, _)| association_id.as_str())
    }

    /// Store one association between a local gateway route table and a VPC, and answer the id
    /// it was given — or `None` when the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling
    /// ([`Self::has_local_gateway_route_table_vpc_association_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_route_table_vpc_association_payload_bytes`]). A caller that gets `None`
    /// must refuse the request with
    /// [`LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_LIMIT_EXCEEDED`].
    ///
    /// The state the clock owes the association is RECORDED, never slept for:
    /// [`Self::materialize_due_transitions`] applies it once [`TRANSITION_DELAY`] has passed,
    /// so a caller asking straight after the write still sees the transient state the
    /// association starts in.
    pub fn insert_local_gateway_route_table_vpc_association(
        &mut self,
        spec: LocalGatewayRouteTableVpcAssociationSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_route_table_vpc_association_payload_bytes(&spec);
        if !self.has_local_gateway_route_table_vpc_association_capacity(1)
            || self.would_exceed_stored_bytes(payload_bytes)
        {
            return None;
        }
        let association_id = self.next_id(LOCAL_GATEWAY_ROUTE_TABLE_VPC_ASSOCIATION_ID_PREFIX);
        let record = LocalGatewayRouteTableVpcAssociationRecord {
            view: crate::ops::types::LocalGatewayRouteTableVpcAssociation {
                local_gateway_route_table_vpc_association_id: Some(association_id.clone()),
                local_gateway_route_table_id: Some(spec.local_gateway_route_table_id),
                local_gateway_route_table_arn: Some(spec.local_gateway_route_table_arn),
                local_gateway_id: Some(spec.local_gateway_id),
                vpc_id: Some(spec.vpc_id),
                owner_id: Some(::engine::emulator_account_id().to_string()),
                state: Some(spec.state),
                tags: Some(spec.tags),
            },
            pending: spec.becomes.map(PendingStateMove::starting_now),
        };
        self.add_stored_bytes(payload_bytes);
        self.local_gateway_route_table_vpc_associations
            .insert(association_id.clone(), record);
        Some(association_id)
    }
}

// =============================================================================
// Local gateway virtual interface groups
// =============================================================================

/// The id prefix EC2 mints a local gateway virtual interface group with:
/// `lgw-vif-grp-`, as the AWS Outposts and EC2 documentation's own ids read (for example
/// `lgw-vif-grp-07145b276bEXAMPLE`). The model puts no pattern on
/// `LocalGatewayVirtualInterfaceGroupId`, so the prefix is the one thing that says which
/// resource an id names.
pub const LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX: &str = "lgw-vif-grp";

/// How many local gateway virtual interface groups this emulator stores before a write
/// must refuse.
///
/// CHOSEN, like [`MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS`] and for the same reason:
/// nothing in this emulator reclaims a stored group, so the store needs a ceiling that is
/// not the process's memory. A count ceiling is no bound on the caller-chosen bytes each
/// record holds, so those are charged against [`MAX_STORED_BYTES`] too
/// ([`local_gateway_vif_group_payload_bytes`]).
pub const MAX_LOCAL_GATEWAY_VIF_GROUPS: usize = 4096;

/// The ARN of a local gateway virtual interface group:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface-group/<id>`, the form
/// `LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceGroupArn` carries ("The
/// Amazon Resource Number (ARN) of the local gateway virtual interface group").
pub fn local_gateway_virtual_interface_group_arn(region: &str, group_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("local-gateway-virtual-interface-group/{group_id}"),
    )
}

/// The `ResourceType` enum member a local gateway virtual interface group is tagged and
/// named under: `local-gateway-virtual-interface-group`
/// (`smithy_query(query="shape",shape="ResourceType")`), which is also the resource path
/// [`local_gateway_virtual_interface_group_arn`] builds its ARNs under. The resource's one
/// spelling, so the refusals that name it cannot drift from it.
pub const RESOURCE_TYPE_LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP: &str =
    "local-gateway-virtual-interface-group";

/// `InvalidLocalGatewayVirtualInterfaceGroupID.Malformed` (HTTP 400): the request named
/// something that is not a local gateway virtual interface group id at all, so there is
/// nothing to look up — the `Invalid<Kind>.Malformed` code EC2 answers before it looks an
/// id up, as [`INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_MALFORMED`] is for the route table.
pub const INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_MALFORMED: &str =
    "InvalidLocalGatewayVirtualInterfaceGroupID.Malformed";

/// `InvalidLocalGatewayVirtualInterfaceGroupID.NotFound` (HTTP 400): the request named a
/// well-formed virtual interface group id this account holds no group for. The code AWS
/// answers, as the recorded wire corpus behind
/// `issues/…-ec2-createlocalgatewayvirtualinterface-stub.md` shows.
pub const INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_NOT_FOUND: &str =
    "InvalidLocalGatewayVirtualInterfaceGroupID.NotFound";

/// The code EC2 answers when a request would take the account past the virtual interface
/// groups it may hold, in the `<Resource>LimitExceeded` family this service's other quotas
/// are refused with ([`LOCAL_GATEWAY_ROUTE_TABLE_LIMIT_EXCEEDED`]).
pub const LOCAL_GATEWAY_VIF_GROUP_LIMIT_EXCEEDED: &str =
    "LocalGatewayVirtualInterfaceGroupLimitExceeded";

/// Refuse a virtual interface group create this store cannot take: HTTP 400
/// [`LOCAL_GATEWAY_VIF_GROUP_LIMIT_EXCEEDED`], naming the two bounds this emulator holds a
/// group against ([`MAX_LOCAL_GATEWAY_VIF_GROUPS`] and [`MAX_STORED_BYTES`]) the way
/// [`local_gateway_route_table_limit_exceeded`] names the route table store's.
pub fn local_gateway_virtual_interface_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        LOCAL_GATEWAY_VIF_GROUP_LIMIT_EXCEEDED,
        400,
        format!(
            "This emulator stores at most {MAX_LOCAL_GATEWAY_VIF_GROUPS} local gateway virtual \
             interface groups and at most {MAX_STORED_BYTES} bytes of caller-supplied data, and \
             this request would take it past one of the two"
        ),
    )
}

/// A virtual interface group that is being brought up and is not carrying traffic yet:
/// `LocalGatewayVirtualInterfaceGroupConfigurationState`'s `pending`
/// ([`crate::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP_CONFIGURATION_STATE_VALUES`]
/// is the generated catalog).
pub const LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING: &str = "pending";

/// A virtual interface group that is up: that same enum's `available`. The move from
/// [`LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING`] is owed by the clock ([`TRANSITION_DELAY`],
/// through
/// [`ComAmazonawsEc2Amazonec2State::local_gateway_virtual_interface_group_available_at`]), so
/// a caller that describes the groups straight after the create still sees `pending`.
pub const LOCAL_GATEWAY_VIF_GROUP_STATE_AVAILABLE: &str = "available";

/// Whether `id` is a well-formed local gateway virtual interface group id — one EC2 could
/// have issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX`], whose doc carries the evidence for it). An
/// association's own id (`lgw-vif-grp-assoc-…`) therefore reads as malformed here rather
/// than as a group id, its suffix not being hexadecimal.
pub fn local_gateway_virtual_interface_group_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX, id)
}

/// Refuse something that is not a local gateway virtual interface group id at all: HTTP 400
/// [`INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_MALFORMED`], worded the way AWS words a malformed
/// id of this resource and built from the request's own value.
pub fn invalid_local_gateway_virtual_interface_group_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_MALFORMED,
        400,
        format!("The {RESOURCE_TYPE_LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP} ID '{id}' is malformed"),
    )
}

/// Refuse a well-formed virtual interface group id this account holds no group for: HTTP
/// 400 [`INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_NOT_FOUND`], worded the way AWS words it ("The
/// local-gateway-virtual-interface-group ID '…' does not exist").
pub fn invalid_local_gateway_virtual_interface_group_id_not_found<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_LOCAL_GATEWAY_VIF_GROUP_ID_NOT_FOUND,
        400,
        format!(
            "The {RESOURCE_TYPE_LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP} ID '{id}' does not exist"
        ),
    )
}

/// Refuse a `LocalGatewayVirtualInterfaceGroupId` this service cannot resolve: a malformed
/// one before an unknown one, the order [`require_well_formed_id`] documents.
///
/// The ONE place a virtual interface group id is resolved, so every operation that takes one
/// refuses the same id the same way; the contract
/// [`require_known_local_gateway_route_table`] writes out for the route tables these groups
/// are associated with.
pub fn require_known_local_gateway_virtual_interface_group<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<(), Scaffolded<E>> {
    if !local_gateway_virtual_interface_group_id_is_well_formed(id) {
        return Err(invalid_local_gateway_virtual_interface_group_id_malformed(
            id,
        ));
    }
    if !store.local_gateway_virtual_interface_groups.contains_key(id) {
        return Err(invalid_local_gateway_virtual_interface_group_id_not_found(
            id,
        ));
    }
    Ok(())
}

/// What one local gateway virtual interface group is written with: the local gateway it
/// belongs to, the virtual interfaces in it, its local BGP configuration, the Region whose
/// ARN namespace its ARN names, its configuration state, and its tags.
///
/// Every member the service itself owns — the group id, the ARN built from the Region and
/// that id, and the owning account — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface_group`] rather
/// than taken from the caller, so two writers cannot report one group differently.
#[derive(Debug, Clone)]
pub struct LocalGatewayVirtualInterfaceGroupSpec {
    /// "The ID of the local gateway" the group belongs to.
    pub local_gateway_id: String,
    /// "The IDs of the virtual interfaces" in the group.
    pub local_gateway_virtual_interface_ids: Vec<String>,
    /// "The Autonomous System Number(ASN) for the local Border Gateway Protocol (BGP)".
    pub local_bgp_asn: Option<i32>,
    /// "The extended 32-bit ASN for the local BGP configuration".
    pub local_bgp_asn_extended: Option<i64>,
    /// The Region the group's ARN names.
    pub region: String,
    /// "The current state of the local gateway virtual interface group"
    /// (`LocalGatewayVirtualInterfaceGroup$ConfigurationState`), one of
    /// [`crate::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_GROUP_CONFIGURATION_STATE_VALUES`].
    pub configuration_state: String,
    /// "The tags assigned to the virtual interface group".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored virtual interface group holds: its tags, the
/// configuration state a writer pinned on it, the local gateway id, the ids of the virtual
/// interfaces in it, and the Region, which the stored ARN carries. Everything else on the
/// record — the group id, the rest of the ARN, the owning account, the two ASNs — is
/// derived by this module or fixed-size.
///
/// The ONE definition of that charge, the way
/// [`local_gateway_vif_group_association_payload_bytes`] is for the association store:
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface_group`] charges
/// exactly it, and a removal path must give back the same number.
pub fn local_gateway_vif_group_payload_bytes(spec: &LocalGatewayVirtualInterfaceGroupSpec) -> usize {
    spec.local_gateway_virtual_interface_ids
        .iter()
        .fold(tag_list_bytes(&spec.tags), |total, id| {
            total.saturating_add(id.len())
        })
        .saturating_add(spec.configuration_state.len())
        .saturating_add(spec.local_gateway_id.len())
        .saturating_add(spec.region.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` local gateway virtual interface groups fit inside the
    /// store's ceiling ([`MAX_LOCAL_GATEWAY_VIF_GROUPS`]).
    ///
    /// A writer may ask this before inserting, the way
    /// [`Self::has_local_gateway_vif_group_association_capacity`] gates its own store, so
    /// it can refuse in the `<Resource>LimitExceeded` family this service's other quotas
    /// are refused with ([`SECONDARY_NETWORK_LIMIT_EXCEEDED`]) rather than discover the
    /// refusal from an absent id. The insert itself asks it too, so no path can get past
    /// it.
    ///
    /// Its only caller today is
    /// [`Self::insert_local_gateway_virtual_interface_group`], because the operations that
    /// create a virtual interface group are still scaffolds: this is the seam they must ask
    /// before minting, not dead code.
    pub fn has_local_gateway_vif_group_capacity(&self, count: usize) -> bool {
        self.local_gateway_virtual_interface_groups
            .len()
            .saturating_add(count)
            <= MAX_LOCAL_GATEWAY_VIF_GROUPS
    }

    /// Store one local gateway virtual interface group, and answer the id it was given —
    /// or `None` when the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling ([`Self::has_local_gateway_vif_group_capacity`]) and the
    /// byte budget ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_vif_group_payload_bytes`]). Nothing in this emulator reclaims a
    /// stored group, so a write that is not bounded at the seam it is written through
    /// would grow the process's heap for as long as a caller keeps asking. A caller that
    /// gets `None` must refuse the request with the `<Resource>LimitExceeded` code for
    /// this quota, which belongs with that writer rather than here.
    pub fn insert_local_gateway_virtual_interface_group(
        &mut self,
        spec: LocalGatewayVirtualInterfaceGroupSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_vif_group_payload_bytes(&spec);
        if !self.has_local_gateway_vif_group_capacity(1)
            || self.would_exceed_stored_bytes(payload_bytes)
        {
            return None;
        }
        let group_id = self.next_id(LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX);
        let group = crate::ops::types::LocalGatewayVirtualInterfaceGroup {
            local_gateway_virtual_interface_group_id: Some(group_id.clone()),
            local_gateway_virtual_interface_ids: Some(spec.local_gateway_virtual_interface_ids),
            local_gateway_id: Some(spec.local_gateway_id),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            local_bgp_asn: spec.local_bgp_asn,
            local_bgp_asn_extended: spec.local_bgp_asn_extended,
            local_gateway_virtual_interface_group_arn: Some(
                local_gateway_virtual_interface_group_arn(&spec.region, &group_id),
            ),
            tags: Some(spec.tags),
            configuration_state: Some(spec.configuration_state),
        };
        self.add_stored_bytes(payload_bytes);
        self.local_gateway_virtual_interface_groups
            .insert(group_id.clone(), group);
        Some(group_id)
    }

    /// Record the move to [`LOCAL_GATEWAY_VIF_GROUP_STATE_AVAILABLE`] the clock owes the
    /// virtual interface group `group_id`, [`TRANSITION_DELAY`] from now.
    ///
    /// A writer that answers a new group in the non-terminal
    /// [`LOCAL_GATEWAY_VIF_GROUP_STATE_PENDING`] state calls this; one that stored a group
    /// already up does not, so a group written ready owes nothing. The instant is RECORDED,
    /// never slept for: [`Self::materialize_due_transitions`] applies it once the clock has
    /// passed it, so a caller asking straight after the write still sees `pending`.
    pub fn owe_local_gateway_virtual_interface_group_available(&mut self, group_id: &str) {
        self.local_gateway_virtual_interface_group_available_at
            .insert(group_id.to_string(), Instant::now() + TRANSITION_DELAY);
    }
}

// =============================================================================
// Local gateway virtual interfaces
// =============================================================================

/// The id prefix EC2 mints a local gateway virtual interface with: `lgw-vif-`, as the AWS
/// Outposts and EC2 documentation's own ids read (for example `lgw-vif-06f8c3f5c3EXAMPLE`).
/// The model puts no pattern on `LocalGatewayVirtualInterfaceId`, so the prefix is the one
/// thing that says which resource an id names — and it is a strict prefix of the GROUP's
/// ([`LOCAL_GATEWAY_VIF_GROUP_ID_PREFIX`]), so a group id reads as malformed here rather
/// than as a virtual interface, its suffix (`grp-…`) not being hexadecimal.
pub const LOCAL_GATEWAY_VIF_ID_PREFIX: &str = "lgw-vif";

/// How many local gateway virtual interfaces this emulator stores before a write must
/// refuse.
///
/// CHOSEN, like [`MAX_LOCAL_GATEWAY_VIF_GROUPS`] and for the same reason: nothing in this
/// emulator reclaims a stored virtual interface, so the store needs a ceiling that is not
/// the process's memory. A count ceiling is no bound on the caller-chosen bytes each record
/// holds, so those are charged against [`MAX_STORED_BYTES`] too
/// ([`local_gateway_vif_payload_bytes`]).
pub const MAX_LOCAL_GATEWAY_VIFS: usize = 4096;

/// The code EC2 answers when a request would take the account past the virtual interfaces it
/// may hold, in the `<Resource>LimitExceeded` family this service's other quotas are refused
/// with ([`LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED`]).
pub const LOCAL_GATEWAY_VIF_LIMIT_EXCEEDED: &str = "LocalGatewayVirtualInterfaceLimitExceeded";

/// The `ResourceType` enum member a local gateway virtual interface is tagged and named
/// under: `local-gateway-virtual-interface`
/// (`smithy_query(query="shape",shape="ResourceType")`), which is also the resource path
/// [`local_gateway_virtual_interface_arn`] builds its ARNs under. The resource's one
/// spelling, so the refusals that name it cannot drift from it.
pub const RESOURCE_TYPE_LOCAL_GATEWAY_VIRTUAL_INTERFACE: &str = "local-gateway-virtual-interface";

/// A virtual interface that is being brought up and is not carrying traffic yet:
/// `LocalGatewayVirtualInterfaceConfigurationState`'s `pending`
/// ([`crate::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`] is the
/// generated catalog).
pub const LOCAL_GATEWAY_VIF_STATE_PENDING: &str = "pending";

/// A virtual interface that is up: that same enum's `available`. The move from
/// [`LOCAL_GATEWAY_VIF_STATE_PENDING`] is owed by the clock ([`TRANSITION_DELAY`], through
/// [`ComAmazonawsEc2Amazonec2State::local_gateway_virtual_interface_available_at`]), so a
/// caller that describes the interfaces straight after the create still sees `pending`.
pub const LOCAL_GATEWAY_VIF_STATE_AVAILABLE: &str = "available";

/// The VLAN ids a local gateway virtual interface may be created on: 1 to 4094.
///
/// "The virtual local area network (VLAN) used for the local gateway virtual interface"
/// (`CreateLocalGatewayVirtualInterfaceRequest$Vlan`) is an IEEE 802.1Q VLAN identifier,
/// whose 12-bit tag reserves 0 (no VLAN) and 4095, leaving 1 to 4094 as the ids a tagged
/// frame can carry. The model types the member a plain `Integer` with no `@range`, so the
/// bound is applied by the handler that reads it.
pub const LOCAL_GATEWAY_VIF_VLAN: ::std::ops::RangeInclusive<i32> = 1..=4094;

/// The ARN of a local gateway virtual interface:
/// `arn:aws:ec2:<region>:<account>:local-gateway-virtual-interface/<id>`, the form
/// `LocalGatewayVirtualInterface$LocalGatewayVirtualInterfaceArn` carries ("The Amazon
/// Resource Number (ARN) of the local gateway virtual interface") under the resource path
/// `ResourceType`'s own `local-gateway-virtual-interface` member names
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub fn local_gateway_virtual_interface_arn(region: &str, virtual_interface_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("local-gateway-virtual-interface/{virtual_interface_id}"),
    )
}

/// Whether `id` is a well-formed local gateway virtual interface id — one EC2 could have
/// issued.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`LOCAL_GATEWAY_VIF_ID_PREFIX`], whose doc carries the evidence for it).
pub fn local_gateway_virtual_interface_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(LOCAL_GATEWAY_VIF_ID_PREFIX, id)
}

/// What one local gateway virtual interface is written with: the local gateway and the
/// virtual interface group it belongs to, the Outpost LAG that carries it, its VLAN, the
/// local and peer addresses and BGP configuration, the Region whose ARN namespace its ARN
/// names, its configuration state, and its tags.
///
/// Every member the service itself owns — the virtual interface id, the ARN built from the
/// Region and that id, and the owning account — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface`] rather than
/// taken from the caller, so two writers cannot report one virtual interface differently.
#[derive(Debug, Clone)]
pub struct LocalGatewayVirtualInterfaceSpec {
    /// "The ID of the local gateway" the virtual interface belongs to.
    pub local_gateway_id: String,
    /// "The ID of the local gateway virtual interface group" it is a member of.
    pub local_gateway_virtual_interface_group_id: Option<String>,
    /// "The Outpost LAG ID" that connects the Outpost to the on-premises network devices.
    pub outpost_lag_id: Option<String>,
    /// "The ID of the VLAN" the virtual interface runs on.
    pub vlan: Option<i32>,
    /// "The local address" — the address on the Outpost side. Only IPv4 is supported.
    pub local_address: Option<String>,
    /// "The peer address" — the on-premises side of the link.
    pub peer_address: Option<String>,
    /// "The Border Gateway Protocol (BGP) Autonomous System Number (ASN) of the local
    /// gateway".
    pub local_bgp_asn: Option<i32>,
    /// "The peer BGP ASN".
    pub peer_bgp_asn: Option<i32>,
    /// "The extended 32-bit ASN of the BGP peer for use with larger ASN values".
    pub peer_bgp_asn_extended: Option<i64>,
    /// The Region the virtual interface's ARN names.
    pub region: String,
    /// "The current state of the local gateway virtual interface"
    /// (`LocalGatewayVirtualInterface$ConfigurationState`), one of
    /// [`crate::ops::types::LOCAL_GATEWAY_VIRTUAL_INTERFACE_CONFIGURATION_STATE_VALUES`].
    pub configuration_state: String,
    /// "The tags assigned to the virtual interface".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored virtual interface holds: its tags, the configuration
/// state a writer pinned on it, the local gateway and group ids, the Outpost LAG id, the
/// local and peer addresses, and the Region, which the stored ARN carries. Everything else
/// on the record — the virtual interface id, the rest of the ARN, the owning account, the
/// VLAN and the ASNs — is derived by this module or fixed-size, which is also why the copy of
/// the minted id the GROUP keeps in its interface list is not charged.
///
/// The ONE definition of that charge, the way [`local_gateway_vif_group_payload_bytes`] is
/// for the group store beside it:
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface`] charges exactly
/// it, and a removal path must give back the same number.
pub fn local_gateway_vif_payload_bytes(spec: &LocalGatewayVirtualInterfaceSpec) -> usize {
    [
        spec.local_gateway_virtual_interface_group_id.as_deref(),
        spec.outpost_lag_id.as_deref(),
        spec.local_address.as_deref(),
        spec.peer_address.as_deref(),
    ]
    .into_iter()
    .flatten()
    .fold(tag_list_bytes(&spec.tags), |total, value| {
        total.saturating_add(value.len())
    })
    .saturating_add(spec.configuration_state.len())
    .saturating_add(spec.local_gateway_id.len())
    .saturating_add(spec.region.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` local gateway virtual interfaces fit inside the store's
    /// ceiling ([`MAX_LOCAL_GATEWAY_VIFS`]).
    ///
    /// A writer may ask this before inserting, the way
    /// [`Self::has_local_gateway_vif_group_capacity`] gates the group store, so it can
    /// refuse in the `<Resource>LimitExceeded` family this service's other quotas are
    /// refused with ([`LOCAL_GATEWAY_VIF_LIMIT_EXCEEDED`]) rather than discover the refusal
    /// from an absent id. The insert itself asks it too, so no path can get past it.
    pub fn has_local_gateway_vif_capacity(&self, count: usize) -> bool {
        self.local_gateway_virtual_interfaces
            .len()
            .saturating_add(count)
            <= MAX_LOCAL_GATEWAY_VIFS
    }

    /// Store one local gateway virtual interface, and answer the id it was given — or
    /// `None` when the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling ([`Self::has_local_gateway_vif_capacity`]) and the byte
    /// budget ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_vif_payload_bytes`]) — the same contract as
    /// [`Self::insert_local_gateway_virtual_interface_group`]. A caller that gets `None`
    /// must refuse the request with [`LOCAL_GATEWAY_VIF_LIMIT_EXCEEDED`], the
    /// `<Resource>LimitExceeded` code for this quota.
    ///
    /// A virtual interface created IN a group is also one of "The IDs of the virtual
    /// interfaces" that group reports
    /// (`LocalGatewayVirtualInterfaceGroup$LocalGatewayVirtualInterfaceIds`), so when the spec
    /// names a group this store holds, the minted id is added to that group's list HERE, in
    /// the one place an interface is stored: a second writer cannot leave the group reporting
    /// a different set from this store. An interface created ON an Outpost LAG is likewise one
    /// of "The IDs of the local gateway virtual interfaces associated with the Outpost LAG"
    /// (`OutpostLag$LocalGatewayVirtualInterfaceIds`), so the same is done for a LAG this store
    /// holds — see [`outpost_lags`].
    pub fn insert_local_gateway_virtual_interface(
        &mut self,
        spec: LocalGatewayVirtualInterfaceSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_vif_payload_bytes(&spec);
        if !self.has_local_gateway_vif_capacity(1) || self.would_exceed_stored_bytes(payload_bytes) {
            return None;
        }
        let virtual_interface_id = self.next_id(LOCAL_GATEWAY_VIF_ID_PREFIX);
        let group_id = spec.local_gateway_virtual_interface_group_id.clone();
        let outpost_lag_id = spec.outpost_lag_id.clone();
        let virtual_interface = crate::ops::types::LocalGatewayVirtualInterface {
            local_gateway_virtual_interface_id: Some(virtual_interface_id.clone()),
            local_gateway_id: Some(spec.local_gateway_id),
            local_gateway_virtual_interface_group_id: spec.local_gateway_virtual_interface_group_id,
            local_gateway_virtual_interface_arn: Some(local_gateway_virtual_interface_arn(
                &spec.region,
                &virtual_interface_id,
            )),
            outpost_lag_id: spec.outpost_lag_id,
            vlan: spec.vlan,
            local_address: spec.local_address,
            peer_address: spec.peer_address,
            local_bgp_asn: spec.local_bgp_asn,
            peer_bgp_asn: spec.peer_bgp_asn,
            peer_bgp_asn_extended: spec.peer_bgp_asn_extended,
            owner_id: Some(::engine::emulator_account_id().to_string()),
            tags: Some(spec.tags),
            configuration_state: Some(spec.configuration_state),
        };
        self.add_stored_bytes(payload_bytes);
        // "The IDs of the virtual interfaces" the group holds, kept in step here rather than
        // by each writer: a spec naming a group this store does not hold (a seeded interface
        // of an Outpost whose group nothing stored) leaves the lists it has alone.
        if let Some(group) = group_id
            .as_deref()
            .and_then(|group_id| self.local_gateway_virtual_interface_groups.get_mut(group_id))
        {
            group
                .local_gateway_virtual_interface_ids
                .get_or_insert_with(Vec::new)
                .push(virtual_interface_id.clone());
        }
        // "The IDs of the local gateway virtual interfaces associated with the Outpost LAG"
        // (`OutpostLag$LocalGatewayVirtualInterfaceIds`), kept in step in this same one place
        // and on the same terms: a spec naming a LAG this store does not hold leaves the lists
        // it has alone, so `DescribeOutpostLags` cannot report a different set of interfaces
        // from the interface store.
        if let Some(outpost_lag) = outpost_lag_id
            .as_deref()
            .and_then(|outpost_lag_id| self.outpost_lags.get_mut(outpost_lag_id))
        {
            outpost_lag
                .local_gateway_virtual_interface_ids
                .get_or_insert_with(Vec::new)
                .push(virtual_interface_id.clone());
        }
        self.local_gateway_virtual_interfaces
            .insert(virtual_interface_id.clone(), virtual_interface);
        Some(virtual_interface_id)
    }

    /// Record the move to [`LOCAL_GATEWAY_VIF_STATE_AVAILABLE`] the clock owes the virtual
    /// interface `virtual_interface_id`, [`TRANSITION_DELAY`] from now.
    ///
    /// A writer that answers a new interface in the non-terminal
    /// [`LOCAL_GATEWAY_VIF_STATE_PENDING`] state calls this; one that stored an interface
    /// already up does not, so an interface written ready owes nothing. The instant is
    /// RECORDED, never slept for: [`Self::materialize_due_transitions`] applies it once the
    /// clock has passed it, so a caller asking straight after the write still sees `pending`.
    pub fn owe_local_gateway_virtual_interface_available(&mut self, virtual_interface_id: &str) {
        self.local_gateway_virtual_interface_available_at.insert(
            virtual_interface_id.to_string(),
            Instant::now() + TRANSITION_DELAY,
        );
    }
}
