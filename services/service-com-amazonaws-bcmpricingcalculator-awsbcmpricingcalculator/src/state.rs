// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator service.
//!
//! The bill scenario is this service's root resource: a bill estimate is derived from a
//! scenario, and a usage modification only exists inside one, so every modification is
//! stored on its parent scenario rather than in a second map that could drift from it.
//!
//! The workload estimate is the other root resource, kept in its own insertion-ordered
//! collection beside the scenarios, and sharing their id space (every `ResourceId` this
//! service mints is unique across both) and their idempotency map.
//!
//! Beside the resources, the store carries the two facts about the ACCOUNT this service
//! answers for: its rate-type [`Preferences`], and its [`AccountClass`], which decides
//! whether the bill-planning surface is open to the caller at all.
//!
//! It also carries the PRICE LIST this emulator quotes from — [`RATE_CARD`], the
//! published on-demand rates [`price_usage`] prices a usage line against — here rather
//! than in one handler, because every operation that stores or changes a usage line must
//! price it the same way, and the cost of an estimate is the sum over those lines.

use std::collections::{BTreeMap, HashMap};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BillInterval, Expression, HistoricalUsageEntity, ResourceNotFoundException, UsageAmount,
    UsageQuantity, ValidationException, WorkloadEstimateUsageItem, WorkloadEstimateUsageQuantity,
};

/// The region segment every ARN this service mints carries: NONE.
///
/// `AWSBCMPricingCalculator` is a PARTITIONAL service — its model carries
/// `aws.endpoints#standardPartitionalEndpoints`, so a resource of it belongs to the
/// account inside a partition rather than to a region — and its `Arn` shape's pattern
/// leaves the region field possibly empty (smithy_query `shape="Arn"`:
/// `^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$`).
/// That is the form the service's own ARNs carry, and the form a client holding one
/// sends back: `arn:aws:bcm-pricing-calculator::<account>:workload-estimate/<id>`.
///
/// Because the region segment does not select among this service's resources, it is
/// also not part of the identity [`parse_resource_arn`] resolves an ARN by.
pub const ARN_REGION: &str = "";

/// This service's ARN namespace, as the Smithy model's `aws.api#service` declares it
/// (`arnNamespace: bcm-pricing-calculator`).
pub const ARN_NAMESPACE: &str = "bcm-pricing-calculator";

/// How long after creation a bill scenario reports it will expire.
///
/// CHOSEN, not documented: neither the Smithy model nor the API reference states a
/// retention span for a bill scenario, so this emulator picks one, reports it
/// consistently through `expiresAt` on create, get and list, and enforces it in
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::purge_expired`].
pub const BILL_SCENARIO_RETENTION_SECS: i64 = 15 * 24 * 60 * 60;

/// The page size a `@paginated` list operation whose page-size member is `MaxResults`
/// uses when the caller sends no `maxResults`: the model's own maximum for that member
/// (`range 1..25`).
pub const MAX_PAGE_SIZE: i32 = 25;

/// The page size `ListWorkloadEstimateUsage` uses when the caller sends no
/// `maxResults`. Same rule as [`MAX_PAGE_SIZE`] — the modelled maximum of that
/// operation's own page-size member — but that member is `WorkloadEstimateUsageMaxResults`
/// (smithy_query: `shape="WorkloadEstimateUsageMaxResults"`, `range 1..300`) rather than
/// `MaxResults`.
pub const WORKLOAD_ESTIMATE_USAGE_MAX_PAGE_SIZE: i32 = 300;

/// The greatest number of bill scenarios this emulator holds at once.
///
/// CHOSEN, not documented: AWS publishes no bill-scenario quota in the model or the
/// API reference. A cap is still required rather than optional, for two reasons: the
/// model declares `ServiceQuotaExceededException` (HTTP 402, "The request would cause
/// you to exceed your service quota") on `CreateBillScenario`, which is unreachable
/// without one; and without it a caller could grow a long-lived emulator's store
/// without bound.
pub const MAX_BILL_SCENARIOS: usize = 1000;

/// The greatest number of usage modifications ONE bill scenario holds at once. Chosen
/// for the same two reasons as [`MAX_BILL_SCENARIOS`], and answered with the same
/// modelled `ServiceQuotaExceededException`.
pub const MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO: usize = 2000;

/// How long after creation a workload estimate reports it will expire.
///
/// CHOSEN, not documented, for the same reason as [`BILL_SCENARIO_RETENTION_SECS`]:
/// neither the Smithy model nor the API reference states a retention span for a
/// workload estimate. The same window is used for both resources so one store ages
/// uniformly, it is reported through `expiresAt` on create and get, and it is enforced
/// in [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::purge_expired`].
pub const WORKLOAD_ESTIMATE_RETENTION_SECS: i64 = BILL_SCENARIO_RETENTION_SECS;

/// The greatest number of workload estimates this emulator holds at once. Chosen for
/// the same two reasons as [`MAX_BILL_SCENARIOS`] — `CreateWorkloadEstimate` models
/// `ServiceQuotaExceededException` (HTTP 402), which is unreachable without a cap, and
/// an uncapped store grows for the life of the process — and answered with that same
/// modelled error.
pub const MAX_WORKLOAD_ESTIMATES: usize = 1000;

/// The status a workload estimate whose whole cost is resolved carries:
/// `WorkloadEstimateStatus`'s terminal success value (smithy_query:
/// `shape="WorkloadEstimateStatus"`, values `UPDATING | VALID | INVALID |
/// ACTION_NEEDED`).
///
/// It is the status a created estimate is already in, and the one it returns to after
/// every change to its usage: the cost of an estimate is computed from the usage it
/// holds and the rates it is measured against ([`RATE_CARD`]), both of which are in hand
/// by the time the call that changed them answers, so there is no recomputation left
/// outstanding for a clock to finish. Nothing here reports `UPDATING`: a status is
/// transient only where the service really is still working, and inventing a wait a
/// caller must then poll out of would hang every client that gates on a terminal
/// status.
pub const WORKLOAD_ESTIMATE_STATUS_VALID: &str = "VALID";

/// The status a workload estimate carries when at least one usage line it holds could
/// not be priced, so its `totalCost` does not account for that line:
/// `WorkloadEstimateStatus`'s own "the caller must do something about this" value.
///
/// It is reported rather than `VALID` because `VALID` over a total that silently omits a
/// line would report a workload as cheaper than the usage it models, and rather than
/// `INVALID` because the estimate itself is well-formed — what is missing is a published
/// rate for that line's usage type, which the caller resolves by modelling usage this
/// service carries rates for. The `failureMessage` beside it names the usage types.
pub const WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED: &str = "ACTION_NEEDED";

/// What the `resourceType` member of a modelled error names a workload estimate as:
/// the resource's own Smithy shape name (`com.amazonaws.bcmpricingcalculator#\
/// WorkloadEstimate`), as the bill-scenario handlers name theirs `BillScenario`.
pub const WORKLOAD_ESTIMATE_RESOURCE_TYPE: &str = "WorkloadEstimate";

/// What the `resourceType` member of a modelled error names ONE usage line of a
/// workload estimate as: that line's own Smithy shape name
/// (`com.amazonaws.bcmpricingcalculator#WorkloadEstimateUsageItem`), as the
/// bill-scenario handlers name theirs `BillScenarioUsageModification`.
pub const WORKLOAD_ESTIMATE_USAGE_RESOURCE_TYPE: &str = "WorkloadEstimateUsageItem";

/// The greatest number of usage lines ONE workload estimate holds at once. Chosen for
/// the same two reasons as [`MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO`] —
/// `BatchCreateWorkloadEstimateUsage` models `ServiceQuotaExceededException` (HTTP 402),
/// which is unreachable without a cap, and an uncapped store grows for the life of the
/// process — and answered with that same modelled error.
pub const MAX_USAGE_PER_WORKLOAD_ESTIMATE: usize = 2000;

/// The only `CurrencyCode` the model declares (smithy_query: `shape="CurrencyCode"`,
/// values `USD`), so it is the currency every cost this service reports is in.
pub const COST_CURRENCY: &str = "USD";

/// The `WorkloadEstimateCostStatus` a cost that is current carries (smithy_query:
/// `shape="WorkloadEstimateCostStatus"`, values `VALID | INVALID | STALE`). Held here
/// rather than in one handler, so every reader of a usage line's cost qualifies it the
/// same way (see [`WorkloadEstimateUsage::cost_status`]).
pub const COST_STATUS_VALID: &str = "VALID";

/// The `WorkloadEstimateCostStatus` a usage line whose cost could not be established
/// carries: the cost of that line is not a valid one, because no rate is published for
/// its usage type at its location (see [`price_usage`]). Reported rather than left unset
/// so a caller reading a line with no `cost` is told THAT line's cost is the problem,
/// which is also what the estimate's `ACTION_NEEDED` status reports over the whole.
pub const COST_STATUS_INVALID: &str = "INVALID";

/// How many decimal places a cost this service reports is rounded to.
///
/// A cost is a rate times an amount, and binary floating point leaves a product like
/// `24 × 0.0104` a few ulps away from the decimal value it denotes. Rounding to a fixed
/// number of places makes the reported cost a function of the usage alone, so the same
/// usage always reports the same number and a sum does not depend on the order it was
/// added in. Ten places is far finer than any published rate (the finest AWS publishes
/// are per-request rates with eight decimals) and far coarser than the error it removes.
pub const COST_DECIMAL_PLACES: i32 = 10;

/// `value` rounded to [`COST_DECIMAL_PLACES`].
pub fn round_cost(value: f64) -> f64 {
    let scale = 10f64.powi(COST_DECIMAL_PLACES);
    (value * scale).round() / scale
}

/// The pricing location a usage type that carries no region prefix belongs to.
///
/// AWS's usage types name their region with a prefix (`USW2-BoxUsage:...`) and leave it
/// off for the region the service was first offered in, `us-east-1` — the convention the
/// AWS Billing user guide describes under "Understanding your usage types" and the one a
/// Cost and Usage Report row follows. See [`REGION_PREFIXES`].
pub const DEFAULT_USAGE_LOCATION: &str = "us-east-1";

/// The region each usage-type prefix names, as the AWS Billing user guide's usage-type
/// documentation lists them (the prefix of a usage type is the region abbreviation, e.g.
/// `USW2-BoxUsage:t3.micro` is `us-west-2`; `EU-` is the legacy spelling of `EUW1-`).
///
/// Only prefixes whose region is unambiguous are listed: a usage type carrying a prefix
/// this table does not name has NO location this service can report (see
/// [`split_usage_type`]), rather than being silently read as [`DEFAULT_USAGE_LOCATION`].
pub const REGION_PREFIXES: &[(&str, &str)] = &[
    ("USE1", "us-east-1"),
    ("USE2", "us-east-2"),
    ("USW1", "us-west-1"),
    ("USW2", "us-west-2"),
    ("CAN1", "ca-central-1"),
    ("EU", "eu-west-1"),
    ("EUW1", "eu-west-1"),
    ("EUW2", "eu-west-2"),
    ("EUW3", "eu-west-3"),
    ("EUC1", "eu-central-1"),
    ("EUN1", "eu-north-1"),
    ("APN1", "ap-northeast-1"),
    ("APN2", "ap-northeast-2"),
    ("APS1", "ap-southeast-1"),
    ("APS2", "ap-southeast-2"),
    ("SAE1", "sa-east-1"),
];

/// One published on-demand rate of the AWS price list this emulator carries: what one
/// unit of a usage type costs, per location, and the unit it is measured in.
///
/// Each row's `per_location` rates are the PUBLISHED on-demand (undiscounted,
/// `BEFORE_DISCOUNTS`) prices of that usage type, in [`COST_CURRENCY`], as the service's
/// pricing pages state them. They are not computed from anything: a price list is data,
/// and this is the slice of it this emulator ships (see [`RATE_CARD`]).
#[derive(Debug, Clone, Copy)]
pub struct PublishedRate {
    /// The `ServiceCode` this rate belongs to, e.g. `AmazonEC2`.
    pub service_code: &'static str,
    /// The usage type WITHOUT its region prefix, e.g. `BoxUsage:t3.micro` — the prefix
    /// selects the location, which `per_location` keys the rate by.
    pub usage_type: &'static str,
    /// The `Operation` dimension of the rate: a usage type priced under one operation is
    /// a different rate from the same usage type under another (a Linux instance-hour is
    /// `RunInstances`, a Windows one is not), so it is part of the key rather than
    /// ignored.
    pub operation: &'static str,
    /// The unit of measure one `amount` of this usage type counts, as the price list
    /// states it (`Hrs`, `GB-Mo`, ...). It is a property of the usage type, so it is
    /// reported for a line of this usage type even at a location this row publishes no
    /// rate for.
    pub unit: &'static str,
    /// `(location, price of one unit in USD)`.
    pub per_location: &'static [(&'static str, f64)],
}

/// The price list this emulator carries.
///
/// SOURCE: the published AWS on-demand price lists — Amazon EC2
/// (<https://aws.amazon.com/ec2/pricing/on-demand/>, Linux/UNIX, shared tenancy),
/// Amazon S3 (<https://aws.amazon.com/s3/pricing/>, S3 Standard, first 50 TB/month) and
/// Amazon RDS (<https://aws.amazon.com/rds/mysql/pricing/>, Single-AZ MySQL) — written
/// out here because a price list is DATA the real service reads and nothing in this
/// emulator can derive it.
///
/// It is deliberately a slice rather than the whole list: a usage type it does not
/// publish a rate for is answered as a line whose cost could not be established
/// ([`COST_STATUS_INVALID`]) on an estimate that needs the caller's attention
/// ([`WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED`]), never with a made-up number.
pub const RATE_CARD: &[PublishedRate] = &[
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:t3.micro",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.0104),
            ("us-west-2", 0.0104),
            ("eu-west-1", 0.0114),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:t3.small",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.0208),
            ("us-west-2", 0.0208),
            ("eu-west-1", 0.0228),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:t3.medium",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.0416),
            ("us-west-2", 0.0416),
            ("eu-west-1", 0.0456),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.large",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.096),
            ("us-west-2", 0.096),
            ("eu-west-1", 0.107),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.xlarge",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.192),
            ("us-west-2", 0.192),
            ("eu-west-1", 0.214),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:c5.large",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.085),
            ("us-west-2", 0.085),
            ("eu-west-1", 0.096),
        ],
    },
    PublishedRate {
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:c5.xlarge",
        operation: "RunInstances",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.17),
            ("us-west-2", 0.17),
            ("eu-west-1", 0.192),
        ],
    },
    PublishedRate {
        service_code: "AmazonS3",
        usage_type: "TimedStorage-ByteHrs",
        operation: "StandardStorage",
        unit: "GB-Mo",
        per_location: &[
            ("us-east-1", 0.023),
            ("us-west-2", 0.023),
            ("eu-west-1", 0.023),
        ],
    },
    PublishedRate {
        service_code: "AmazonRDS",
        usage_type: "InstanceUsage:db.t3.micro",
        operation: "CreateDBInstance",
        unit: "Hrs",
        per_location: &[
            ("us-east-1", 0.017),
            ("us-west-2", 0.017),
            ("eu-west-1", 0.018),
        ],
    },
];

/// A usage type split into the location its prefix names and the usage type the
/// [`RATE_CARD`] keys a rate by.
#[derive(Debug, Clone, Copy)]
pub struct SplitUsageType<'a> {
    /// The region the prefix names, [`DEFAULT_USAGE_LOCATION`] when there is no prefix,
    /// and `None` when the prefix is one [`REGION_PREFIXES`] does not name — nothing can
    /// be said about that location, so nothing is.
    pub location: Option<&'static str>,
    /// The usage type with any region prefix removed.
    pub usage_type: &'a str,
}

/// Whether `segment` is shaped like a usage type's region abbreviation: two to five
/// characters, every one an upper-case letter or a digit, as every prefix
/// [`REGION_PREFIXES`] lists is. A usage type whose first segment is anything else
/// (`TimedStorage-ByteHrs`, `DataTransfer-Out-Bytes`) carries no prefix at all.
fn is_region_prefix_shaped(segment: &str) -> bool {
    (2..=5).contains(&segment.len())
        && segment
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && segment.chars().any(|c| c.is_ascii_uppercase())
}

/// [`SplitUsageType`] for `usage_type`: the location its region prefix names and the
/// rest of the usage type, by the convention [`DEFAULT_USAGE_LOCATION`] describes.
pub fn split_usage_type(usage_type: &str) -> SplitUsageType<'_> {
    match usage_type.split_once('-') {
        Some((prefix, rest)) if is_region_prefix_shaped(prefix) => SplitUsageType {
            location: REGION_PREFIXES
                .iter()
                .find(|(candidate, _)| *candidate == prefix)
                .map(|(_, region)| *region),
            usage_type: rest,
        },
        _ => SplitUsageType {
            location: Some(DEFAULT_USAGE_LOCATION),
            usage_type,
        },
    }
}

/// The [`RATE_CARD`] row for a usage type (region prefix already removed) under an
/// operation, or `None` when this emulator publishes no rate for that usage.
pub fn published_rate(
    service_code: &str,
    usage_type: &str,
    operation: &str,
) -> Option<&'static PublishedRate> {
    RATE_CARD.iter().find(|rate| {
        rate.service_code == service_code
            && rate.usage_type == usage_type
            && rate.operation == operation
    })
}

/// What this service can say about the price of one usage line.
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePricing {
    /// The pricing location of the line, from its usage type's region prefix.
    pub location: Option<String>,
    /// The unit of measure its quantity counts, from the [`RATE_CARD`] row for its usage
    /// type.
    pub unit: Option<String>,
    /// The cost of `amount` units at that location: the published rate times the amount,
    /// rounded to [`COST_DECIMAL_PLACES`], and `None` when no rate is published for this
    /// usage type at this location.
    pub cost: Option<f64>,
}

/// Price one usage line: the location its usage type names, the unit that usage type is
/// measured in, and what `amount` of it costs at the published rate.
///
/// ONE function for every usage line of this service, so the batch that creates a line
/// and every later reader of it report the same price for the same usage.
pub fn price_usage(
    service_code: &str,
    usage_type: &str,
    operation: &str,
    amount: f64,
) -> UsagePricing {
    let split = split_usage_type(usage_type);
    let rate = published_rate(service_code, split.usage_type, operation);
    let unit_price = rate.zip(split.location).and_then(|(rate, location)| {
        rate.per_location
            .iter()
            .find(|(candidate, _)| *candidate == location)
            .map(|(_, price)| *price)
    });
    UsagePricing {
        location: split.location.map(str::to_string),
        unit: rate.map(|rate| rate.unit.to_string()),
        cost: unit_price.map(|price| round_cost(price * amount)),
    }
}

/// What the `resourceType` member of a modelled error names a bill scenario as: the
/// resource's own Smithy shape name (`com.amazonaws.bcmpricingcalculator#BillScenario`),
/// the name the bill-scenario handlers already answer with.
pub const BILL_SCENARIO_RESOURCE_TYPE: &str = "BillScenario";

/// The resource-type segment a bill scenario's ARN carries, as `CreateBillScenario`
/// mints it: `arn:aws:bcm-pricing-calculator::<account>:bill-scenario/<id>`.
pub const BILL_SCENARIO_ARN_TYPE: &str = "bill-scenario";

/// The resource-type segment a workload estimate's ARN carries, as
/// `CreateWorkloadEstimate` mints it:
/// `arn:aws:bcm-pricing-calculator::<account>:workload-estimate/<id>`.
pub const WORKLOAD_ESTIMATE_ARN_TYPE: &str = "workload-estimate";

/// Every resource kind an ARN of this service names, as `(ARN segment, Smithy shape
/// name)`: the segment is what an `Arn` carries on the wire, the shape name is what a
/// modelled error's `resourceType` reports.
///
/// `bill-estimate` is listed although `CreateBillEstimate` is still a scaffold stub, so
/// an ARN naming a bill estimate is reported as the kind it names rather than as an
/// unrecognised string — the store holds none, so such an ARN resolves to nothing
/// exactly as an ARN naming a deleted resource does.
const ARN_RESOURCE_TYPES: [(&str, &str); 3] = [
    (BILL_SCENARIO_ARN_TYPE, BILL_SCENARIO_RESOURCE_TYPE),
    (WORKLOAD_ESTIMATE_ARN_TYPE, WORKLOAD_ESTIMATE_RESOURCE_TYPE),
    ("bill-estimate", "BillEstimate"),
];

/// The greatest number of tags ONE resource carries at once, INFERRED from the model's
/// maximum for the `Tags` map every tagging operation reads and writes (smithy_query:
/// `shape="Tags"`, `length 0..200`).
///
/// It is an inference from that payload bound, not a service quota AWS states: neither
/// the model nor the API reference publishes a per-resource tag quota for this service
/// (AWS's general tagging guidance is lower, 50 tags per resource). The payload bound is
/// nevertheless a true ceiling on what a resource can carry here, because
/// `ListTagsForResource` reports a resource's tags as a single `Tags` map, so a resource
/// holding more could not be read back. A cap is required rather than optional:
/// `TagResource` models `ServiceQuotaExceededException` (HTTP 402), which is unreachable
/// without one, and without it a caller could grow one resource's tag map without bound.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// The tag-key prefix AWS reserves for its own tags.
///
/// AWS's tagging rules state that the `aws:` prefix is reserved for AWS use and that a
/// caller can neither create nor modify a tag key carrying it, so `TagResource` refuses
/// such a key instead of storing it. The comparison is case-insensitive, because the
/// prefix is reserved however it is cased.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is a tag key AWS reserves for itself (see
/// [`RESERVED_TAG_KEY_PREFIX`]).
///
/// `str::get` rather than a byte-range slice (`key[..4]`), which PANICS when the request's
/// key carries a multi-byte character straddling that byte index — the modelled key
/// pattern `^[\w\s:+=@/-]+$` is matched with a Unicode-aware `\w`, so such a key passes
/// validation and reaches this helper. `get` answers `None` for a key shorter than the
/// prefix or one whose boundary falls mid-character, which is exactly "not reserved".
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// Which resource one of this service's ARNs addresses: every segment of it that
/// SELECTS a resource.
///
/// The region is deliberately absent. This service is partitional (see [`ARN_REGION`]),
/// so a resource of it is not region-scoped and the region segment names nothing to
/// choose between: the same resource is addressed whether the caller sends the empty
/// region its own ARNs carry or writes a region into that field, which the `Arn`
/// pattern permits. Everything else — partition, account and the resource type and id —
/// is part of the identity, so another partition's or another account's ARN addresses a
/// different resource and resolves to nothing here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceArn<'a> {
    pub partition: &'a str,
    pub account_id: &'a str,
    /// The resource-type segment (`bill-scenario`, `workload-estimate`, …).
    pub resource_type: &'a str,
    /// The id after the type's `/`, empty when the ARN carries no `/`.
    pub resource_id: &'a str,
}

/// The resource one of this service's ARNs addresses, or `None` when the string is not
/// an ARN of this service at all.
///
/// The form every ARN this service mints carries, and the one its `Arn` shape pins:
/// `arn:<partition>:bcm-pricing-calculator:<region>:<account>:<type>/<id>`. The resource
/// portion is split off with `splitn(6, ':')` rather than `split`, because the pattern
/// admits `:` INSIDE that portion, so a resource segment carrying one must stay whole.
pub fn parse_resource_arn(arn: &str) -> Option<ResourceArn<'_>> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    if fields.next()? != ARN_NAMESPACE {
        return None;
    }
    // The region field: read so the fields after it are reached, and dropped because it
    // is not part of the identity (see [`ResourceArn`]).
    let _region = fields.next()?;
    let account_id = fields.next()?;
    let resource = fields.next()?;
    let (resource_type, resource_id) = resource.split_once('/').unwrap_or((resource, ""));
    Some(ResourceArn {
        partition,
        account_id,
        resource_type,
        resource_id,
    })
}

/// Whether `requested` addresses the resource whose own ARN is `minted`: the two name
/// the same [`ResourceArn`] identity.
///
/// An ARN this service never mints (another partition's, another account's, another
/// kind's, another id's) addresses something else, and a string that is not an ARN of
/// this service addresses nothing.
pub fn arn_addresses(minted: &str, requested: &str) -> bool {
    match (parse_resource_arn(minted), parse_resource_arn(requested)) {
        (Some(minted), Some(requested)) => minted == requested,
        _ => false,
    }
}

/// The resource-type segment `arn` names (`bill-scenario`, `workload-estimate`, …), or
/// `None` when the string does not have the ARN form this service's `Arn` shape pins.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    parse_resource_arn(arn).map(|parsed| parsed.resource_type)
}

/// What a modelled error's `resourceType` names the resource `arn` addresses as: the
/// resource kind's own Smithy shape name when the ARN names one of this service's kinds
/// ([`ARN_RESOURCE_TYPES`]), and otherwise the segment as the ARN wrote it, since that
/// segment is all that names the thing the caller asked for.
pub fn arn_resource_label(arn: &str) -> String {
    let Some(segment) = arn_resource_type(arn) else {
        // Unreachable through the wire: `Arn`'s pattern pins the ARN form before a
        // handler runs. Reported as the whole string rather than invented, so a
        // hand-built value reaching here is visible instead of silently relabelled.
        return arn.to_string();
    };
    ARN_RESOURCE_TYPES
        .iter()
        .find(|(arn_type, _)| *arn_type == segment)
        .map_or_else(|| segment.to_string(), |(_, shape)| (*shape).to_string())
}

/// One stored usage modification of a bill scenario.
#[derive(Debug, Clone)]
pub struct UsageModification {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// The `key` the batch entry that created this modification carried, so a later
    /// batch response can report an error against the same key.
    pub key: Option<String>,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    pub usage_account_id: String,
    pub availability_zone: Option<String>,
    /// The pricing location of this line. Only resolvable from the pricing catalog,
    /// which this emulator does not carry, so it stays unset.
    pub location: Option<String>,
    pub group: Option<String>,
    pub amounts: Vec<UsageAmount>,
    pub historical_usage: Option<HistoricalUsageEntity>,
}

impl UsageModification {
    /// The modified usage quantities, one per requested `UsageAmount`.
    ///
    /// `unit` stays unset: a usage type's unit of measure comes from the pricing
    /// catalog, which this emulator does not carry, and `UsageQuantity$unit` is
    /// optional precisely so it can be absent.
    pub fn quantities(&self) -> Option<Vec<UsageQuantity>> {
        if self.amounts.is_empty() {
            return None;
        }
        Some(
            self.amounts
                .iter()
                .map(|amount| UsageQuantity {
                    start_hour: Some(amount.start_hour),
                    unit: None,
                    amount: Some(amount.amount),
                })
                .collect(),
        )
    }

    /// The value a `ListUsageFilter` name selects on this modification, or `None` when
    /// this modification carries nothing for it: [`usage_filter_value`] over this
    /// modification's attributes.
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        usage_filter_value(
            &UsageFilterFields {
                usage_account_id: self.usage_account_id.as_str(),
                service_code: self.service_code.as_str(),
                usage_type: self.usage_type.as_str(),
                operation: self.operation.as_str(),
                location: self.location.as_deref(),
                group: self.group.as_deref(),
                historical_usage: self.historical_usage.as_ref(),
            },
            name,
        )
    }
}

/// The attributes a `ListUsageFilter` can select on, borrowed from one stored usage row.
///
/// It is a struct rather than a parameter list so [`usage_filter_value`] names what it
/// reads: this service has two kinds of usage row (a bill scenario's
/// [`UsageModification`] and a workload estimate's [`WorkloadEstimateUsage`]) and both
/// are filtered through the SAME `ListUsageFilters` shape, so both must answer one
/// filter table.
pub struct UsageFilterFields<'a> {
    pub usage_account_id: &'a str,
    pub service_code: &'a str,
    pub usage_type: &'a str,
    pub operation: &'a str,
    /// The row's pricing location: the region its usage type names, for a row whose
    /// creator resolves one (see [`split_usage_type`]). A row with none is selected out
    /// by a `LOCATION` filter rather than refusing it.
    pub location: Option<&'a str>,
    pub group: Option<&'a str>,
    pub historical_usage: Option<&'a HistoricalUsageEntity>,
}

/// The value a `ListUsageFilter` name selects on a usage row, or `None` when the row
/// carries nothing for it (which selects the row out).
///
/// ONE table for every usage row this service filters, so a `ListUsageFilterName` the
/// model adds is handled in one place instead of drifting between two copies. The names
/// are that enum's own values (smithy_query: `shape="ListUsageFilterName"`); the
/// `HISTORICAL_*` half reads the row's `historicalUsage`.
pub fn usage_filter_value<'a>(row: &UsageFilterFields<'a>, name: &str) -> Option<&'a str> {
    let historical = row.historical_usage;
    match name {
        "USAGE_ACCOUNT_ID" => Some(row.usage_account_id),
        "SERVICE_CODE" => Some(row.service_code),
        "USAGE_TYPE" => Some(row.usage_type),
        "OPERATION" => Some(row.operation),
        "LOCATION" => row.location,
        "USAGE_GROUP" => row.group,
        "HISTORICAL_USAGE_ACCOUNT_ID" => historical.map(|h| h.usage_account_id.as_str()),
        "HISTORICAL_SERVICE_CODE" => historical.map(|h| h.service_code.as_str()),
        "HISTORICAL_USAGE_TYPE" => historical.map(|h| h.usage_type.as_str()),
        "HISTORICAL_OPERATION" => historical.map(|h| h.operation.as_str()),
        "HISTORICAL_LOCATION" => historical.and_then(|h| h.location.as_deref()),
        _ => None,
    }
}

/// One stored usage line of a workload estimate: the usage a
/// `BatchCreateWorkloadEstimateUsage` entry asked to model, stored on its parent
/// estimate rather than in a second map that could drift from it (as a bill scenario's
/// [`UsageModification`]s are stored on their scenario).
#[derive(Debug, Clone)]
pub struct WorkloadEstimateUsage {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// The `key` the batch entry that created this line carried, so a later batch
    /// response can report an error against the same key.
    pub key: Option<String>,
    pub service_code: String,
    pub usage_type: String,
    pub operation: String,
    pub usage_account_id: String,
    /// The pricing location of this line: the region its usage type's prefix names (see
    /// [`split_usage_type`]), resolved once when the line is stored so every reader
    /// reports the same one.
    pub location: Option<String>,
    pub group: Option<String>,
    /// The usage amount the entry asked to model, which is what this line's
    /// `quantity.amount` reports back.
    pub amount: f64,
    /// The unit of measure [`Self::amount`] counts, from the [`RATE_CARD`] row for this
    /// line's usage type, and unset when this emulator's price list does not carry that
    /// usage type.
    pub unit: Option<String>,
    /// What this line's usage costs, in [`COST_CURRENCY`]: the published rate for its
    /// usage type at its location times its amount ([`price_usage`]).
    ///
    /// Unset when this emulator's price list publishes no rate for that usage at that
    /// location — a cost nothing established is not reported as a number. It is what
    /// [`WorkloadEstimate::cost_over_usage`] sums, and a line without one is what puts
    /// its estimate in [`WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED`].
    pub cost: Option<f64>,
    pub historical_usage: Option<HistoricalUsageEntity>,
}

impl WorkloadEstimateUsage {
    /// The modelled quantity of this line's usage: the amount asked for, in the unit the
    /// price list measures that usage type in.
    pub fn quantity(&self) -> WorkloadEstimateUsageQuantity {
        WorkloadEstimateUsageQuantity {
            unit: self.unit.clone(),
            amount: Some(self.amount),
        }
    }

    /// The value a `ListUsageFilter` name selects on this usage line, or `None` when
    /// this line carries nothing for it: [`usage_filter_value`] over this line's
    /// attributes, the same table a bill scenario's [`UsageModification`] answers,
    /// since both are filtered through the SAME `ListUsageFilters` shape.
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        usage_filter_value(
            &UsageFilterFields {
                usage_account_id: self.usage_account_id.as_str(),
                service_code: self.service_code.as_str(),
                usage_type: self.usage_type.as_str(),
                operation: self.operation.as_str(),
                location: self.location.as_deref(),
                group: self.group.as_deref(),
                historical_usage: self.historical_usage.as_ref(),
            },
            name,
        )
    }

    /// The currency a reader reports this line's cost in: [`COST_CURRENCY`] exactly when
    /// a `cost` is reported, since `currency` says which currency THAT cost is in and has
    /// nothing to qualify without one.
    pub fn cost_currency(&self) -> Option<String> {
        self.cost.map(|_| COST_CURRENCY.to_string())
    }

    /// The `WorkloadEstimateCostStatus` a reader reports for this line: it qualifies the
    /// line's cost, so a line priced off the published rate reports
    /// [`COST_STATUS_VALID`], and one this emulator's price list carries no rate for
    /// reports [`COST_STATUS_INVALID`] — the cost of that line is not valid, which is
    /// exactly what the caller needs to know about an item carrying no `cost`.
    pub fn cost_status(&self) -> Option<String> {
        Some(
            if self.cost.is_some() {
                COST_STATUS_VALID
            } else {
                COST_STATUS_INVALID
            }
            .to_string(),
        )
    }

    /// This line as the modelled `WorkloadEstimateUsageItem`.
    ///
    /// Both operations that report a STORED line through that shape
    /// (`ListWorkloadEstimateUsage` and `BatchUpdateWorkloadEstimateUsage`) project it
    /// here, so one line cannot be reported two ways. `BatchCreateWorkloadEstimateUsage`
    /// answers in its own item shape (which also carries the entry's `key`) and reads the
    /// same accessors.
    pub fn usage_item(&self) -> WorkloadEstimateUsageItem {
        WorkloadEstimateUsageItem {
            id: Some(self.id.clone()),
            service_code: self.service_code.clone(),
            usage_type: self.usage_type.clone(),
            operation: self.operation.clone(),
            location: self.location.clone(),
            group: self.group.clone(),
            usage_account_id: Some(self.usage_account_id.clone()),
            quantity: Some(self.quantity()),
            cost: self.cost,
            currency: self.cost_currency(),
            status: self.cost_status(),
            historical_usage: self.historical_usage.clone(),
        }
    }

    /// Re-price this line after a change to the usage it models: the location, unit and
    /// cost are a function of its usage type, operation and amount, so they are resolved
    /// again through the one [`price_usage`] every creator of a line uses.
    pub fn reprice(&mut self) {
        let pricing = price_usage(
            &self.service_code,
            &self.usage_type,
            &self.operation,
            self.amount,
        );
        self.location = pricing.location;
        self.unit = pricing.unit;
        self.cost = pricing.cost;
    }

    /// A stored line for one batch entry's usage, priced from this service's rate card.
    ///
    /// Every operation that stores a usage line goes through here, so the location, unit
    /// and cost of a line are resolved in ONE place: two creators of the same usage
    /// cannot price it differently.
    #[allow(clippy::too_many_arguments)]
    pub fn priced(
        id: String,
        key: Option<String>,
        service_code: String,
        usage_type: String,
        operation: String,
        usage_account_id: String,
        group: Option<String>,
        amount: f64,
        historical_usage: Option<HistoricalUsageEntity>,
    ) -> Self {
        let pricing = price_usage(&service_code, &usage_type, &operation, amount);
        Self {
            id,
            key,
            service_code,
            usage_type,
            operation,
            usage_account_id,
            location: pricing.location,
            group,
            amount,
            unit: pricing.unit,
            cost: pricing.cost,
            historical_usage,
        }
    }
}

/// One stored workload estimate.
#[derive(Debug, Clone)]
pub struct WorkloadEstimate {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// `arn:aws:bcm-pricing-calculator::<account>:workload-estimate/<id>`, built through
    /// `engine::emulator_arn` so the account segment is the canonical one and the region
    /// segment is empty ([`ARN_REGION`]).
    ///
    /// This is what the operations that address an estimate by ARN (`TagResource`,
    /// `UntagResource`, `ListTagsForResource`) resolve against, through
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::resource_tags`].
    pub arn: String,
    pub name: String,
    /// A `WorkloadEstimateStatus` value: [`WORKLOAD_ESTIMATE_STATUS_VALID`] while every
    /// usage line it holds is priced, [`WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED`] while
    /// one of them is not ([`Self::recompute_cost`] maintains it).
    pub status: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    /// The `WorkloadEstimateRateType` this estimate is measured on: what the request
    /// asked for, or [`DEFAULT_WORKLOAD_ESTIMATE_RATE_TYPE`] when it asked for none.
    pub rate_type: Option<String>,
    /// The instant of the pricing rates this estimate is measured against — the moment
    /// it was created, since that is when the rates were read.
    pub rate_timestamp: DateTime,
    /// The estimated cost of this estimate's usage, in [`COST_CURRENCY`].
    ///
    /// Maintained by the operations that change that usage: each of them calls
    /// [`Self::recompute_cost`], which lands [`Self::cost_over_usage`] here. An estimate
    /// holding no usage costs `0.0`.
    pub total_cost: f64,
    pub failure_message: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// This estimate's usage lines, insertion-ordered so a paginated read of them is
    /// stable across calls.
    pub usage: Vec<WorkloadEstimateUsage>,
}

impl WorkloadEstimate {
    /// The value a `ListWorkloadEstimatesFilter` name selects on this estimate, or
    /// `None` when this estimate carries nothing for it.
    ///
    /// Names are the `ListWorkloadEstimatesFilterName` enum's own values (smithy_query:
    /// `shape="ListWorkloadEstimatesFilterName"`, values `STATUS | NAME`).
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        match name {
            "STATUS" => Some(self.status.as_str()),
            "NAME" => Some(self.name.as_str()),
            _ => None,
        }
    }

    /// The usage line with this id, if this estimate holds it.
    pub fn usage_line(&self, id: &str) -> Option<&WorkloadEstimateUsage> {
        self.usage.iter().find(|line| line.id == id)
    }

    /// The cost of this estimate's usage: the sum over the costs of its usage lines,
    /// rounded to [`COST_DECIMAL_PLACES`] so the total of a set of lines does not depend
    /// on the order they were added in.
    ///
    /// A line whose cost this emulator's price list cannot resolve contributes nothing
    /// (see [`WorkloadEstimateUsage::cost`]) and is what puts the estimate in
    /// [`WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED`], so a total that omits a line is never
    /// reported as a complete one.
    pub fn cost_over_usage(&self) -> f64 {
        round_cost(self.usage.iter().filter_map(|line| line.cost).sum())
    }

    /// The usage types of the lines this estimate holds that carry no cost, in stored
    /// order and without repeats.
    fn unpriced_usage_types(&self) -> Vec<String> {
        let mut unpriced: Vec<String> = Vec::new();
        for line in &self.usage {
            if line.cost.is_none() && !unpriced.contains(&line.usage_type) {
                unpriced.push(line.usage_type.clone());
            }
        }
        unpriced
    }

    /// Recompute what this estimate costs and what status that leaves it in.
    ///
    /// Called by every operation that changes the usage this estimate holds, and by the
    /// create that mints it: the cost of an estimate is a function of its usage lines and
    /// the rates they were priced at, so it is landed by the call that changes them
    /// rather than left for a later reader to work out.
    pub fn recompute_cost(&mut self) {
        self.total_cost = self.cost_over_usage();
        let unpriced = self.unpriced_usage_types();
        if unpriced.is_empty() {
            self.status = WORKLOAD_ESTIMATE_STATUS_VALID.to_string();
            self.failure_message = None;
        } else {
            self.status = WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED.to_string();
            self.failure_message = Some(format!(
                "no rate is published for usage type {}, so its cost is not included in \
                 totalCost",
                unpriced.join(", ")
            ));
        }
    }
}

/// One stored bill scenario.
#[derive(Debug, Clone)]
pub struct BillScenario {
    /// The server-assigned `ResourceId` (a lowercase-hex v4 UUID).
    pub id: String,
    /// `arn:aws:bcm-pricing-calculator::<account>:bill-scenario/<id>`, built through
    /// `engine::emulator_arn` so the account segment is the canonical one and the region
    /// segment is empty ([`ARN_REGION`]).
    ///
    /// This is what the operations that address a scenario by ARN (`TagResource`,
    /// `UntagResource`, `ListTagsForResource`) resolve against, through
    /// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::resource_tags`].
    pub arn: String,
    pub name: String,
    /// A `BillScenarioStatus` value (`READY` | `LOCKED` | `STALE` | `FAILED`).
    pub status: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub bill_interval_start: DateTime,
    pub bill_interval_end: DateTime,
    pub failure_message: Option<String>,
    pub group_sharing_preference: Option<String>,
    pub cost_category_group_sharing_preference_arn: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Insertion-ordered, so a paginated read of this scenario's modifications is
    /// stable across calls.
    pub usage_modifications: Vec<UsageModification>,
}

impl BillScenario {
    /// The time period this scenario covers, as the modelled structure.
    pub fn bill_interval(&self) -> BillInterval {
        BillInterval {
            start: Some(self.bill_interval_start),
            end: Some(self.bill_interval_end),
        }
    }

    /// The value a `ListBillScenariosFilter` name selects on this scenario, or `None`
    /// when this scenario carries nothing for it.
    ///
    /// Names are the `ListBillScenariosFilterName` enum's own values (smithy_query:
    /// `shape="ListBillScenariosFilterName"`).
    pub fn filter_value(&self, name: &str) -> Option<&str> {
        match name {
            "STATUS" => Some(self.status.as_str()),
            "NAME" => Some(self.name.as_str()),
            "GROUP_SHARING_PREFERENCE" => self.group_sharing_preference.as_deref(),
            "COST_CATEGORY_ARN" => self.cost_category_group_sharing_preference_arn.as_deref(),
            _ => None,
        }
    }

    /// The modification with this id, if this scenario holds it.
    pub fn usage_modification(&self, id: &str) -> Option<&UsageModification> {
        self.usage_modifications.iter().find(|m| m.id == id)
    }
}

/// The rate-type selection an account that has never written one has in effect.
///
/// CHOSEN, not documented: neither the Smithy model nor the API reference states the
/// selection the service applies before anybody writes one. A selection there has to be,
/// though, and it cannot be the empty list: `GetPreferences` is a read of the account's
/// EFFECTIVE preferences — the rate types Pricing Calculator quotes with, which exist
/// from the moment the account does — and `RateTypes` is `@length 1..3` (smithy_query:
/// `shape="RateTypes"`), so an empty or absent selection is not a value the service can
/// legitimately report. The value picked is the un-discounted list rate, `RateType`'s
/// `BEFORE_DISCOUNTS`: the only rate a calculator can quote knowing nothing about an
/// account's discounts or commitments.
pub const DEFAULT_RATE_TYPE_SELECTION: &str = "BEFORE_DISCOUNTS";

/// The `WorkloadEstimateRateType` an estimate is created on when the request names none.
///
/// `CreateWorkloadEstimate$rateType` is optional and the model declares no `@default`,
/// but the estimate is nevertheless priced on SOME rate basis — `totalCost` and every
/// line's `cost` are only meaningful against one — and the response reports which, so
/// the effective basis is resolved here instead of being left unsaid. The value is the
/// un-discounted list rate, `WorkloadEstimateRateType`'s `BEFORE_DISCOUNTS`: the only
/// basis a calculator can quote knowing nothing about an account's discounts or
/// commitments, which is the same reason [`DEFAULT_RATE_TYPE_SELECTION`] is the
/// rate-type preference an account that has written none has in effect, and it is the
/// basis the rates in [`RATE_CARD`] are published on.
pub const DEFAULT_WORKLOAD_ESTIMATE_RATE_TYPE: &str = "BEFORE_DISCOUNTS";

/// Which kind of account, in AWS Organizations terms, this service presents its caller
/// as. It decides the account-class gate in
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::member_account_refusal`],
/// so it is stored rather than re-derived per call: every operation of one store answers
/// for the same account.
///
/// The model carries this three-way split itself — `GetPreferences` and
/// `UpdatePreferences` report one rate-type selection per account class
/// (management/member/standalone), and `ValidationExceptionReason` has the value
/// `invalidRequestFromMember` for a request a member account may not make.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountClass {
    /// An account whose bills another account in its organization pays. This is the
    /// DEFAULT, and it is what this emulator presents unless a store is built stating
    /// otherwise: it is the account class the recorded behaviour of this service was
    /// observed in, so the refusals a member account gets are the ones a caller of an
    /// unconfigured emulator sees.
    #[default]
    Member,
    /// The payer (management) account of an organization: it pays its members' bills, so
    /// it may plan them.
    Management,
    /// An account in no organization at all, which pays only for itself.
    Standalone,
}

impl AccountClass {
    /// Whether an account of this class may reach the bill-planning surface — the bill
    /// scenarios and the bill estimates derived from them, and the write of the
    /// account-level rate-type preferences.
    ///
    /// A member account may not: those subjects belong to whichever account pays the
    /// bill being modelled, which is the organization's payer account or, for an account
    /// in no organization, itself.
    pub fn may_plan_bills(self) -> bool {
        matches!(self, Self::Management | Self::Standalone)
    }
}

/// `ValidationExceptionReason`'s value for a request the caller's account class does not
/// permit (smithy_query: `shape="ValidationExceptionReason"` → `unknownOperation |
/// cannotParse | fieldValidationFailed | invalidRequestFromMember | disallowedRate |
/// other`), as the MODEL spells it. The account-class gate puts it on the wire in the
/// spelling [`gate_reason`] renders, which is what a client reads.
pub const VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER: &str = "invalidRequestFromMember";

/// The fixed sentence the account-class gate refuses with.
///
/// RECORDED behaviour: the gate's message is operation-INDEPENDENT — it is byte-identical
/// across every recorded refusal, of seven different operations — so it names no operation
/// and is one constant rather than a per-operation rendering.
pub const MEMBER_ACCOUNT_REFUSAL_MESSAGE: &str = "Operation not permitted for member \
     accounts. This API is only allowed for regular or payer accounts.";

/// A `ValidationExceptionReason` value in the spelling the ACCOUNT-CLASS GATE puts it on
/// the wire in: the SCREAMING_SNAKE_CASE rendering of the model's lowerCamel enum value
/// (`invalidRequestFromMember` → `INVALID_REQUEST_FROM_MEMBER`).
///
/// RECORDED behaviour, and not a property of the enum: the gate sits in FRONT of the
/// modelled application layer and spells the reason it names this way for every operation
/// it refuses, while a refusal raised by the modelled input validator carries the model's
/// own spelling (`fieldValidationFailed`) — see `crate::service`. Rendering it rather than
/// restating a second literal keeps the model's value the single source: the value named
/// at a call site is always one of the enum's own.
///
/// Each boundary between a lowercase (or digit) character and an uppercase one becomes an
/// `_`, which is the inverse of the lowerCamel spelling Smithy enum values use; a value
/// already in that form is returned unchanged in upper case.
pub fn gate_reason(modelled_value: &str) -> String {
    let mut rendered = String::with_capacity(modelled_value.len() + 4);
    let mut previous_was_lower = false;
    for character in modelled_value.chars() {
        if character.is_uppercase() && previous_was_lower {
            rendered.push('_');
        }
        previous_was_lower = character.is_lowercase() || character.is_numeric();
        rendered.extend(character.to_uppercase());
    }
    rendered
}

/// The account's Pricing Calculator preferences: the preferred rate types for each
/// kind of account, written by `UpdatePreferences` and reported by `GetPreferences`.
///
/// One record, not one per account: the preference belongs to the account the request
/// is made for, and this emulator serves a single account (`engine::mock_account_id`),
/// so a second key would only ever hold that one record.
///
/// Every selection is held as a NON-EMPTY list rather than an option, because the
/// account's effective preferences exist whether or not anybody wrote them: the record a
/// fresh store holds is [`DEFAULT_RATE_TYPE_SELECTION`] for each of the three account
/// classes, and a write replaces a selection rather than bringing it into being. That is
/// what lets `GetPreferences` report all three members for an account that never called
/// `UpdatePreferences`, which a `@length 1..3` list could not do with an empty or absent
/// value.
#[derive(Debug, Clone)]
pub struct Preferences {
    /// `RateTypes` (len 1..3, of `RateType`: `BEFORE_DISCOUNTS` | `AFTER_DISCOUNTS` |
    /// `AFTER_DISCOUNTS_AND_COMMITMENTS`) — smithy_query: `shape="RateType"`.
    pub management_account_rate_type_selections: Vec<String>,
    pub member_account_rate_type_selections: Vec<String>,
    pub standalone_account_rate_type_selections: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        let default_selection = || vec![DEFAULT_RATE_TYPE_SELECTION.to_string()];
        Self {
            management_account_rate_type_selections: default_selection(),
            member_account_rate_type_selections: default_selection(),
            standalone_account_rate_type_selections: default_selection(),
        }
    }
}

impl Preferences {
    /// Apply one `UpdatePreferences` request: each selection the request supplies
    /// REPLACES the stored one (the member is a whole list, so there is nothing to
    /// merge inside it), and each selection the request omits keeps its stored value.
    /// Every member is individually optional, so an omitted one states nothing about
    /// that preference rather than clearing it.
    pub fn merge(
        &mut self,
        management_account_rate_type_selections: Option<Vec<String>>,
        member_account_rate_type_selections: Option<Vec<String>>,
        standalone_account_rate_type_selections: Option<Vec<String>>,
    ) {
        if let Some(selections) = management_account_rate_type_selections {
            self.management_account_rate_type_selections = selections;
        }
        if let Some(selections) = member_account_rate_type_selections {
            self.member_account_rate_type_selections = selections;
        }
        if let Some(selections) = standalone_account_rate_type_selections {
            self.standalone_account_rate_type_selections = selections;
        }
    }
}

/// What a previous request carrying an `@idempotencyToken` did.
#[derive(Debug, Clone)]
pub struct IdempotentRecord {
    /// A digest of the request that made this record, so the same token replayed with a
    /// DIFFERENT request is recognised as a conflict rather than answered with this
    /// request's result.
    pub fingerprint: String,
    /// The resource ids that request created.
    pub ids: Vec<String>,
}

/// What [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::replay`] found
/// for a request's `@idempotencyToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replay {
    /// No record: a first request, or no token at all.
    Fresh,
    /// This exact request was already served; these are the ids it created.
    Replayed(Vec<String>),
    /// This token was already used for a DIFFERENT request; these are the ids that
    /// request created, so the conflict can name the resource it already made.
    Mismatch(Vec<String>),
}

/// Per-service state for com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// Every live bill scenario, in creation order so a paginated read is stable.
    pub bill_scenarios: Vec<BillScenario>,
    /// Every live workload estimate, in creation order so a paginated read is stable.
    pub workload_estimates: Vec<WorkloadEstimate>,
    /// `<operation>:<clientToken>` -> what that request created, so a replayed
    /// `@idempotencyToken` answers with the first result instead of minting a second
    /// resource. The key carries the operation name because the same token may
    /// legitimately be reused for a different operation.
    pub idempotency: HashMap<String, IdempotentRecord>,
    /// The rate-type preferences `UpdatePreferences` last stored, which
    /// `GetPreferences` reports. Default-constructed: an account that never called
    /// `UpdatePreferences` has the effective selections [`Preferences::default`] holds.
    pub preferences: Preferences,
    /// Which kind of account this store answers for; see [`AccountClass`].
    pub account_class: AccountClass,
}

impl ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState {
    /// The refusal the caller's account class owes this request, or `None` when that
    /// class may make it.
    ///
    /// The bill-planning surface — the bill scenarios, the bill estimates derived from
    /// them, and the write of the account-level rate-type preferences — belongs to the
    /// account that pays the bill being modelled
    /// ([`AccountClass::may_plan_bills`]). A request for one of those subjects from a
    /// member account is refused whatever it carries, so the refusal is answered BEFORE
    /// the request is read and before anything is written, and reports the modelled
    /// `reason` ([`VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER`]) so a client can tell
    /// an account-class refusal from a malformed-field one.
    ///
    /// It takes no operation: the gate's refusal is operation-INDEPENDENT, in both halves
    /// a client reads — the `reason` and the fixed [`MEMBER_ACCOUNT_REFUSAL_MESSAGE`] —
    /// so there is nothing per-operation for a caller to pass in. `fieldList` is left
    /// unset: no field of the request is at fault.
    pub fn member_account_refusal(&self) -> Option<ValidationException> {
        if self.account_class.may_plan_bills() {
            return None;
        }
        Some(ValidationException {
            message: MEMBER_ACCOUNT_REFUSAL_MESSAGE.to_string(),
            reason: Some(gate_reason(
                VALIDATION_REASON_INVALID_REQUEST_FROM_MEMBER,
            )),
            field_list: None,
        })
    }

    /// The scenario with this id, or `None` when it was never created or was deleted.
    pub fn bill_scenario(&self, id: &str) -> Option<&BillScenario> {
        self.bill_scenarios.iter().find(|s| s.id == id)
    }

    /// The scenario with this id, mutably.
    pub fn bill_scenario_mut(&mut self, id: &str) -> Option<&mut BillScenario> {
        self.bill_scenarios.iter_mut().find(|s| s.id == id)
    }

    /// The workload estimate with this id, or `None` when it was never created or was
    /// deleted.
    pub fn workload_estimate(&self, id: &str) -> Option<&WorkloadEstimate> {
        self.workload_estimates.iter().find(|e| e.id == id)
    }

    /// The workload estimate with this id, mutably.
    pub fn workload_estimate_mut(&mut self, id: &str) -> Option<&mut WorkloadEstimate> {
        self.workload_estimates.iter_mut().find(|e| e.id == id)
    }

    /// The tags of the resource `arn` addresses, or `None` when this store holds no
    /// resource with that ARN.
    ///
    /// The ARN is matched by the identity it names ([`parse_resource_arn`]): partition,
    /// account, resource type and resource id, which is everything that selects a
    /// resource of this partitional service. So another account's or another kind's ARN
    /// resolves to nothing, while the region field — which names nothing to choose
    /// between here — does not keep an ARN from resolving. A well-formed ARN naming a
    /// resource this store does not hold (one that was deleted, one that expired, or one
    /// of a kind this service cannot yet create) resolves to nothing, which every tagging
    /// operation answers with the modelled not-found payload under HTTP 400
    /// ([`unresolvable_arn_refusal`]).
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(scenario) = self
            .bill_scenarios
            .iter()
            .find(|s| arn_addresses(&s.arn, arn))
        {
            return Some(&scenario.tags);
        }
        self.workload_estimates
            .iter()
            .find(|e| arn_addresses(&e.arn, arn))
            .map(|estimate| &estimate.tags)
    }

    /// The tags of the resource `arn` addresses, to be changed in place. `None` for an
    /// ARN this store holds no resource for; see [`Self::resource_tags`].
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(scenario) = self
            .bill_scenarios
            .iter_mut()
            .find(|s| arn_addresses(&s.arn, arn))
        {
            return Some(&mut scenario.tags);
        }
        self.workload_estimates
            .iter_mut()
            .find(|e| arn_addresses(&e.arn, arn))
            .map(|estimate| &mut estimate.tags)
    }

    /// Drop the workload estimate with this id. `true` when one was removed.
    pub fn remove_workload_estimate(&mut self, id: &str) -> bool {
        let before = self.workload_estimates.len();
        self.workload_estimates.retain(|e| e.id != id);
        let removed = before != self.workload_estimates.len();
        if removed {
            self.prune_idempotency();
        }
        removed
    }

    /// One fresh `ResourceId`, as [`Self::mint_resource_ids`] mints a batch of them.
    pub fn mint_resource_id(&self) -> String {
        // `mint_resource_ids` returns exactly as many ids as it is asked for, so the
        // fallback is unreachable; it is here so a handler needs no unwrap.
        self.mint_resource_ids(1)
            .pop()
            .unwrap_or_else(candidate_resource_id)
    }

    /// `count` fresh `ResourceId`s, none of which a live scenario, a live usage
    /// modification, or another id of this batch already carries.
    ///
    /// The live-id set is built ONCE per call rather than per candidate, so minting a
    /// batch costs one pass over the store instead of one per row. The rejection loop
    /// is what makes uniqueness exact: a candidate the store already holds is discarded
    /// rather than overwriting somebody's resource, so a caller never loses a row to a
    /// repeated id.
    pub fn mint_resource_ids(&self, count: usize) -> Vec<String> {
        let mut live: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
        for scenario in &self.bill_scenarios {
            live.insert(scenario.id.as_str());
            live.extend(
                scenario
                    .usage_modifications
                    .iter()
                    .map(|modification| modification.id.as_str()),
            );
        }
        for estimate in &self.workload_estimates {
            live.insert(estimate.id.as_str());
            live.extend(estimate.usage.iter().map(|line| line.id.as_str()));
        }
        let mut minted: Vec<String> = Vec::with_capacity(count);
        while minted.len() < count {
            let candidate = candidate_resource_id();
            if !live.contains(candidate.as_str()) && !minted.contains(&candidate) {
                minted.push(candidate);
            }
        }
        minted
    }

    /// Drop the scenario with this id (and, with it, its usage modifications).
    /// `true` when one was removed.
    pub fn remove_bill_scenario(&mut self, id: &str) -> bool {
        let before = self.bill_scenarios.len();
        self.bill_scenarios.retain(|s| s.id != id);
        let removed = before != self.bill_scenarios.len();
        if removed {
            self.prune_idempotency();
        }
        removed
    }

    /// Materialise the expiry the clock has already made due: drop every bill scenario
    /// and workload estimate whose `expiresAt` has passed, and the idempotency entries
    /// that named them.
    ///
    /// The expiry is owed at `now_secs` whether or not anybody called, so even a
    /// reporting operation may apply it. Without it the retention window every reader
    /// advertises through `expiresAt` would never be enforced, and the store would grow
    /// for the life of the process.
    pub fn purge_expired(&mut self, now_secs: i64) {
        let before = (self.bill_scenarios.len(), self.workload_estimates.len());
        self.bill_scenarios
            .retain(|scenario| scenario.expires_at.secs() > now_secs);
        self.workload_estimates
            .retain(|estimate| estimate.expires_at.secs() > now_secs);
        if before != (self.bill_scenarios.len(), self.workload_estimates.len()) {
            self.prune_idempotency();
        }
    }

    /// Materialise everything the clock has already made due at `now_millis`: the expiry
    /// [`Self::purge_expired`] applies.
    ///
    /// That change was owed at `now_millis` whether or not anybody called, which is why a
    /// reporting operation may apply it; every operation that resolves a resource of this
    /// store calls this first, so no reader reports a resource whose advertised
    /// `expiresAt` has passed.
    ///
    /// Nothing else is settled here. A workload estimate's cost and the status over it
    /// are landed by the call that changes its usage ([`WorkloadEstimate::recompute_cost`]),
    /// so there is never a transition outstanding for a later reader to apply.
    pub fn settle(&mut self, now_millis: i64) {
        self.purge_expired(now_millis.div_euclid(1000));
    }

    /// Forget every resource id an idempotency record names that no longer exists, and
    /// drop a record left naming nothing.
    ///
    /// A consequence worth stating: once the resources a token created are deleted (or
    /// expired), that token no longer replays, so the same `clientToken` sent again
    /// creates a new resource rather than answering with the vanished one. The
    /// alternative — keeping records for a retention period of their own — would let a
    /// caller grow this map without bound by creating and deleting under fresh tokens,
    /// and would have a replay promise nothing can honour, since the resource it names
    /// is gone.
    ///
    /// Pruning per id rather than per record keeps the record and the replay it
    /// promises in agreement: a replay answers with exactly the resources that still
    /// exist, instead of falling through (because one id of a batch vanished) and
    /// minting a second set under the same token. And a record naming nothing could
    /// never answer a replay at all, so keeping it would only let a caller grow this
    /// map by repeatedly creating and deleting under fresh tokens.
    pub fn prune_idempotency(&mut self) {
        // Nothing to prune, and the live-id set is the expensive part: the delete and
        // expiry paths reach here on every call.
        if self.idempotency.is_empty() {
            return;
        }
        // Destructured so the live set can BORROW the scenario ids while the records
        // are mutated: they are disjoint fields, which a whole-`self` borrow would not
        // let the compiler see.
        let Self {
            bill_scenarios,
            workload_estimates,
            idempotency,
            preferences: _,
            account_class: _,
        } = self;
        let live: std::collections::HashSet<&str> = bill_scenarios
            .iter()
            .flat_map(|scenario| {
                std::iter::once(scenario.id.as_str()).chain(
                    scenario
                        .usage_modifications
                        .iter()
                        .map(|modification| modification.id.as_str()),
                )
            })
            .chain(workload_estimates.iter().flat_map(|estimate| {
                std::iter::once(estimate.id.as_str())
                    .chain(estimate.usage.iter().map(|line| line.id.as_str()))
            }))
            .collect();
        idempotency.retain(|_, record| {
            record.ids.retain(|id| live.contains(id.as_str()));
            !record.ids.is_empty()
        });
    }

    /// What a request's `@idempotencyToken` says about this request: a first request, a
    /// genuine retry of the same one, or the same token used for a different one.
    pub fn replay(
        &self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
    ) -> Replay {
        let Some(token) = client_token else {
            return Replay::Fresh;
        };
        match self.idempotency.get(&idempotency_key(operation, token)) {
            None => Replay::Fresh,
            Some(record) if record.fingerprint == fingerprint => {
                Replay::Replayed(record.ids.clone())
            }
            Some(record) => Replay::Mismatch(record.ids.clone()),
        }
    }

    /// Record what this request created against its `@idempotencyToken`.
    ///
    /// A request that carried no token, or created nothing, records nothing: a record
    /// naming no resource could never answer a replay and would never be pruned.
    pub fn record_idempotent(
        &mut self,
        operation: &str,
        client_token: Option<&str>,
        fingerprint: &str,
        ids: Vec<String>,
    ) {
        if ids.is_empty() {
            return;
        }
        if let Some(token) = client_token {
            self.idempotency.insert(
                idempotency_key(operation, token),
                IdempotentRecord {
                    fingerprint: fingerprint.to_string(),
                    ids,
                },
            );
        }
    }
}

fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// A stable digest of the request parts an `@idempotencyToken` is scoped to, and of
/// the selection a page of results was measured against.
///
/// It is what lets a genuine retry (the same token AND the same request) be told from a
/// token reused for a different request, which is a conflict rather than a replay. The
/// parts are length-prefixed, so `["a", "bc"]` and `["ab", "c"]` do not collide.
///
/// Digesting rather than keeping the parts matters for the second use: the digest
/// travels inside a `NextPageToken`, which the model caps at 2048 characters, while a
/// request's `filters` are unbounded.
pub fn request_fingerprint<S: AsRef<str>>(parts: &[S]) -> String {
    use sha2::Digest;

    let mut digest = sha2::Sha256::new();
    for part in parts {
        let part = part.as_ref();
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// How deep a `filterExpression` is digested before the digest stops descending.
///
/// `Expression` nests through caller-supplied `and`/`or`/`not`, so
/// [`push_expression_digest`] is bounded rather than trusting the request's depth. A
/// request deeper than this digests to the same value past the cap, which costs only
/// that two such requests could share one idempotency token — never a crash.
pub const MAX_EXPRESSION_DEPTH: usize = 32;

/// Push the digest parts of one `HistoricalUsageEntity`, the shape every batch entry of
/// this service carries, so each of those operations fingerprints it the same way.
///
/// Written member by member rather than from a derived `Debug`, so the digest is a
/// contract: ADD A LINE HERE when the model gives `HistoricalUsageEntity` another
/// member, or two requests differing only in that member will share one fingerprint.
/// [`request_fingerprint`] length-prefixes every part, so parts cannot run together.
pub fn push_historical_usage_digest(parts: &mut Vec<String>, historical: &HistoricalUsageEntity) {
    parts.push(historical.service_code.clone());
    parts.push(historical.usage_type.clone());
    parts.push(historical.operation.clone());
    parts.push(historical.location.clone().unwrap_or_default());
    parts.push(historical.usage_account_id.clone());
    parts.push(
        historical
            .bill_interval
            .start
            .map(|start| start.secs())
            .unwrap_or_default()
            .to_string(),
    );
    parts.push(
        historical
            .bill_interval
            .end
            .map(|end| end.secs())
            .unwrap_or_default()
            .to_string(),
    );
    push_expression_digest(parts, &historical.filter_expression, 0);
}

/// Push the digest parts of a filter expression, descending at most
/// [`MAX_EXPRESSION_DEPTH`] levels.
pub fn push_expression_digest(parts: &mut Vec<String>, expression: &Expression, depth: usize) {
    if depth > MAX_EXPRESSION_DEPTH {
        return;
    }
    for (member, filter) in [
        ("costCategories", &expression.cost_categories),
        ("dimensions", &expression.dimensions),
        ("tags", &expression.tags),
    ] {
        if let Some(filter) = filter {
            parts.push(member.to_string());
            parts.push(filter.key.clone().unwrap_or_default());
            parts.extend(filter.match_options.clone().unwrap_or_default());
            parts.extend(filter.values.clone().unwrap_or_default());
        }
    }
    for (member, branch) in [("and", &expression.and), ("or", &expression.or)] {
        for child in branch.iter().flatten() {
            parts.push(member.to_string());
            push_expression_digest(parts, child, depth + 1);
        }
    }
    if let Some(negated) = &expression.not {
        parts.push("not".to_string());
        push_expression_digest(parts, negated, depth + 1);
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState>)
}

/// Wall-clock seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Wall-clock milliseconds since the Unix epoch, the resolution
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::settle`] takes.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// A candidate `ResourceId`: the lowercase-hex v4 UUID form this service's
/// `ResourceId` shape declares
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, len 36..36), with
/// RFC 9562's version nibble and variant bits set, so it is a well-formed v4 value and
/// not merely 32 hex characters.
///
/// The 128 bits come from the standard library's `RandomState`, which the OS seeds, and
/// a process-wide sequence number so two ids minted in the same nanosecond differ.
/// Uniqueness is then made EXACT rather than probabilistic by
/// [`ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState::mint_resource_id`],
/// which rejects a candidate the store already holds — the property the service needs,
/// since a repeated id would overwrite a caller's resource.
fn candidate_resource_id() -> String {
    use ::std::hash::{BuildHasher, Hasher};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let half = |salt: u64| -> u64 {
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u64(sequence);
        hasher.write_u128(nanos);
        hasher.finish()
    };
    // The version nibble (4) and the two variant bits (0b10) are the only fixed bits of
    // a v4 UUID; everything else is random.
    let high = (half(0) & 0xffff_ffff_ffff_0fff) | 0x0000_0000_0000_4000;
    let low = (half(1) & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff,
    )
}

/// The UTC calendar month containing `secs`, as `[start, end)` epoch seconds — the
/// billing period a bill scenario covers.
pub fn month_bounds(secs: i64) -> (i64, i64) {
    use ::aws_smithy_types::date_time::Format;

    let bounds = || -> Option<(i64, i64)> {
        // The calendar reading and the conversion back both come from
        // `aws_smithy_types::DateTime`, the crate this service already serialises its
        // timestamps with: `Format::DateTime` renders the instant as RFC 3339
        // (`2026-10-22T09:30:00Z`), whose first seven characters are the calendar year
        // and month, and `DateTime::from_str` turns the first instant of a month back
        // into epoch seconds. The only arithmetic written here is the month rollover.
        let text = DateTime::from_secs(secs).fmt(Format::DateTime).ok()?;
        let year: i32 = text.get(..4)?.parse().ok()?;
        let month: u32 = text.get(5..7)?.parse().ok()?;
        let first_instant_of = |year: i32, month: u32| -> Option<i64> {
            let text = format!("{year:04}-{month:02}-01T00:00:00Z");
            Some(DateTime::from_str(&text, Format::DateTime).ok()?.secs())
        };
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        Some((
            first_instant_of(year, month)?,
            first_instant_of(next_year, next_month)?,
        ))
    };
    // An instant no calendar can render (a year outside RFC 3339's four digits) has no
    // calendar month; report the instant itself rather than a fabricated interval.
    bounds().unwrap_or((secs, secs))
}

/// The `ResourceNotFoundException` this service answers a request that names a resource
/// of `resource_type` it does not hold with.
///
/// THE MESSAGE IS FIXED PER RESOURCE TYPE and never carries the requested identifier.
/// The recorded service answers the same string for two different operations reading the
/// same kind of resource (`GetWorkloadEstimate` and `ListWorkloadEstimateUsage`), which
/// is what makes it identifier- and operation-independent: it reports the service's
/// condition ("there is no resource of this kind by that id") rather than reflecting the
/// request back. The requested identifier is still reported, in the member modelled for
/// it (`resourceId`), beside the kind in `resourceType`.
///
/// `resource_type` is the resource's Smithy shape name, as that member reports it
/// ([`WORKLOAD_ESTIMATE_RESOURCE_TYPE`], [`BILL_SCENARIO_RESOURCE_TYPE`], …).
pub fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!(
            "The requested {resource_type} was not found. Check the {resource_type} ID \
             and try again."
        ),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The reason a line of one request whose `key` repeats an earlier line's is reported
/// with.
///
/// INFERRED, not modelled: no shape of this service declares a uniqueness constraint on
/// a `key`; the requirement is read off the documentation of the member itself — a key is
/// "a unique identifier" a caller matches one line of a request to its own result by,
/// which only holds if one key names one line. It is scoped to the request, as that
/// documentation scopes it: a key may be reused by a later call.
///
/// ONE sentence for every operation of this service that answers per line, so a repeated
/// key is reported the same way whichever of them a caller sent it to.
pub fn duplicate_key_message(key: &str) -> String {
    format!("key {key} appears more than once in this request; each entry's key must be unique")
}

/// [`resource_not_found`] for a workload estimate `identifier` names no live estimate.
pub fn workload_estimate_not_found(identifier: &str) -> ResourceNotFoundException {
    resource_not_found(WORKLOAD_ESTIMATE_RESOURCE_TYPE, identifier)
}

/// [`resource_not_found`] for a bill scenario `identifier` names no live scenario.
pub fn bill_scenario_not_found(identifier: &str) -> ResourceNotFoundException {
    resource_not_found(BILL_SCENARIO_RESOURCE_TYPE, identifier)
}

/// The message the tagging surface refuses an unresolvable `arn` with — fixed, and the
/// same for all three tagging operations, as the recorded service answers it.
pub const UNRESOLVABLE_ARN_MESSAGE: &str = "Resource not found";

/// The HTTP status the tagging surface answers an unresolvable `arn` under.
///
/// It is NOT the 404 the `ResourceNotFoundException` shape's `@httpError` states: the
/// recorded service answers that shape's payload under a 400 on this surface, where the
/// resource is named by a caller-built ARN and an ARN that resolves to nothing is bad
/// INPUT — a non-retryable error the caller must fix — rather than the 404 that tells a
/// client to create the resource and retry. The status and the error code are
/// independent facts of the wire, so the pair is answered as recorded
/// ([`crate::scaffolded::Scaffolded::ModeledWithStatus`]).
pub const UNRESOLVABLE_ARN_STATUS: u16 = 400;

/// The refusal `ListTagsForResource`, `TagResource` and `UntagResource` answer an `arn`
/// that addresses no resource this account holds with: the modelled
/// `ResourceNotFoundException` payload, under [`UNRESOLVABLE_ARN_STATUS`], carrying
/// [`UNRESOLVABLE_ARN_MESSAGE`].
///
/// The message is FIXED and never echoes the ARN, for the same reason
/// [`resource_not_found`]'s is: it reports the service's condition rather than
/// reflecting the request. The requested ARN is still reported in the member modelled
/// for it (`resourceId`), with the kind that ARN names beside it in `resourceType`.
pub fn unresolvable_arn_refusal(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: UNRESOLVABLE_ARN_MESSAGE.to_string(),
        resource_id: arn.to_string(),
        resource_type: arn_resource_label(arn),
    }
}

/// Whether `candidate` satisfies a filter's `values` under its `matchOption`.
///
/// The `MatchOption` enum's values are `EQUALS` | `STARTS_WITH` | `CONTAINS`
/// (smithy_query: `shape="MatchOption"`); a filter that sends no `matchOption` is an
/// exact match. The values of one filter are OR-ed.
pub fn matches_filter(candidate: &str, values: &[String], match_option: Option<&str>) -> bool {
    values.iter().any(|value| match match_option {
        Some("STARTS_WITH") => candidate.starts_with(value.as_str()),
        Some("CONTAINS") => candidate.contains(value.as_str()),
        _ => candidate == value.as_str(),
    })
}

/// Whether `moment` falls inside a `FilterTimestamp` window, which is inclusive at
/// both ends and open at an end the caller left unset.
///
/// The inclusivity of each bound is CHOSEN, not documented: `FilterTimestamp`'s members
/// say only "Include results after/before this timestamp" (smithy_query:
/// `shape="FilterTimestamp"`), which does not settle whether the bound itself is
/// included. Both bounds are inclusive here, consistently for every window this service
/// applies, and that choice is pinned by a test row on each boundary instant.
pub fn within_window(
    moment: DateTime,
    after: Option<DateTime>,
    before: Option<DateTime>,
) -> bool {
    let key = (moment.secs(), moment.subsec_nanos());
    if let Some(after) = after {
        if key < (after.secs(), after.subsec_nanos()) {
            return false;
        }
    }
    if let Some(before) = before {
        if key > (before.secs(), before.subsec_nanos()) {
            return false;
        }
    }
    true
}

/// The prefix every pagination token this service emits carries, so a token minted by
/// another service (or a hand-written one) is rejected rather than silently read as an
/// offset.
const PAGE_TOKEN_PREFIX: &str = "bcmpc-offset:";

/// What a caller is told about a token this service did not issue for THIS request.
const UNISSUED_PAGE_TOKEN: &str = "nextToken is not a token this service issued for this request";

/// The opaque `nextToken` naming the item at `offset` in an insertion-ordered
/// collection, bound to `scope`.
///
/// `scope` is a [`request_fingerprint`] of the request the page belongs to (its
/// operation and the selection it asked for), so an offset is only ever applied to the
/// list it was measured against: a token from another operation, or from the same
/// operation under different `filters`, is refused rather than silently skipping or
/// repeating items.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{scope}:{offset}"))
}

/// The offset [`encode_page_token`] encoded for `scope`, or `Err` with the reason the
/// token is not one this service issued for this request.
///
/// A token is meaningful only to the process that minted it: the offset indexes an
/// in-memory collection, so it is refused anywhere else, which is the safe direction.
pub fn decode_page_token(token: &str, scope: &str) -> Result<usize, String> {
    let decoded = hex::decode(token).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let text = String::from_utf8(decoded).map_err(|_| UNISSUED_PAGE_TOKEN.to_string())?;
    let body = text
        .strip_prefix(PAGE_TOKEN_PREFIX)
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    let (token_scope, offset) = body
        .rsplit_once(':')
        .ok_or_else(|| UNISSUED_PAGE_TOKEN.to_string())?;
    if token_scope != scope {
        return Err(UNISSUED_PAGE_TOKEN.to_string());
    }
    offset
        .parse::<usize>()
        .map_err(|_| UNISSUED_PAGE_TOKEN.to_string())
}

/// One page of `items`, starting at the offset `next_token` names.
///
/// `max_results` is the caller's page size, defaulting to the model's maximum, and
/// `scope` binds the token to this request (see [`encode_page_token`]). The returned
/// token is `Some` whenever this page FILLED the page size the caller asked for — see
/// [`paginate_sized`] for why a full last page still carries one — and an offset past the
/// end is an empty last page rather than an error, because the item a live token named
/// may have been deleted in between.
pub fn paginate<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), String> {
    paginate_sized(items, next_token, max_results, MAX_PAGE_SIZE, scope)
}

/// [`paginate`], for an operation whose `maxResults` member is modelled with a
/// different maximum than [`MAX_PAGE_SIZE`] — `default_page_size` is the page size a
/// caller that sends no `maxResults` gets, which this service takes to be that
/// operation's own modelled maximum.
///
/// A FULL page always carries a continuation token, even when it happens to have
/// returned the last item: a page that filled the requested size says nothing about
/// whether anything follows it, and the recorded service answers a full last page with a
/// token whose next page comes back empty. A page SHORT of the requested size is the end
/// of the collection, so it carries none and a client's walk terminates.
pub fn paginate_sized<T: Clone>(
    items: &[T],
    next_token: Option<&str>,
    max_results: Option<i32>,
    default_page_size: i32,
    scope: &str,
) -> Result<(Vec<T>, Option<String>), String> {
    let offset = match next_token {
        Some(token) if !token.is_empty() => decode_page_token(token, scope)?,
        _ => 0,
    };
    // Every `maxResults` member of this service is modelled with a minimum of 1, so the
    // boundary validator has already refused anything below it; refuse rather than
    // clamp, so a page size that somehow reached here is reported instead of silently
    // becoming 1.
    let page_size = match max_results {
        Some(requested) if requested < 1 => {
            return Err(format!("maxResults must be at least 1, got {requested}"))
        }
        Some(requested) => requested as usize,
        None => default_page_size as usize,
    };
    if offset >= items.len() {
        return Ok((Vec::new(), None));
    }
    let end = (offset + page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = (page.len() == page_size).then(|| encode_page_token(scope, end));
    Ok((page, token))
}
