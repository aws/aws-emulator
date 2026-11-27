// SPDX-License-Identifier: CC0-1.0

//! What Amazon MSK admits as the CAPACITY of a provisioned cluster: the broker
//! types it runs brokers on, how a broker count relates to the Availability Zones
//! the cluster's client subnets span, and which direction that count may move in.
//!
//! Each rule here is a function of a request (and, for the count, of what the
//! cluster already holds) rather than of anything the registry owns, which is why
//! this module sits beside the state module rather than in it. One place for all
//! three, because the same rule is applied by more than one operation: a broker
//! count is set by a create AND by `UpdateBrokerCount`, and a broker type is named
//! by both creates.

/// Every Amazon EC2 instance type Amazon MSK runs brokers on, grouped as the
/// service offers them: the Express broker category (`express.m7g.*`) and the
/// standard families (`kafka.m5.*`, `kafka.m7g.*`, and the one burstable size
/// `kafka.t3.small`).
///
/// Source: the Amazon MSK Developer Guide, "Broker types"
/// (<https://docs.aws.amazon.com/msk/latest/developerguide/msk-brokers.html>),
/// together with the valid-value list the service itself renders when it refuses
/// an instance type ([`unsupported_instance_type_message`]) — this table holds
/// EXACTLY the types that list names, since a type the service does not offer
/// names no broker it could run, and a type missing from here would be advertised
/// as retryable by a refusal of the very value it names.
///
/// `BrokerNodeGroupInfo$InstanceType`'s own documentation lists the `kafka.m5`
/// sizes only; the families AWS added after that prose (`kafka.m7g.*`, the
/// Express category) are offered by the service all the same, so the guide is what
/// this table follows.
pub const BROKER_INSTANCE_TYPE_CATALOG: &[&str] = &[
    // The Express broker category.
    "express.m7g.large",
    "express.m7g.xlarge",
    "express.m7g.2xlarge",
    "express.m7g.4xlarge",
    "express.m7g.8xlarge",
    "express.m7g.12xlarge",
    "express.m7g.16xlarge",
    // The standard families.
    "kafka.t3.small",
    "kafka.m5.large",
    "kafka.m5.xlarge",
    "kafka.m5.2xlarge",
    "kafka.m5.4xlarge",
    "kafka.m5.8xlarge",
    "kafka.m5.12xlarge",
    "kafka.m5.16xlarge",
    "kafka.m5.24xlarge",
    "kafka.m7g.large",
    "kafka.m7g.xlarge",
    "kafka.m7g.2xlarge",
    "kafka.m7g.4xlarge",
    "kafka.m7g.8xlarge",
    "kafka.m7g.12xlarge",
    "kafka.m7g.16xlarge",
];

/// Whether `value` is a broker type Amazon MSK offers — a member of
/// [`BROKER_INSTANCE_TYPE_CATALOG`], and so a type the service could really place
/// brokers of.
pub fn is_catalogued_broker_instance_type(value: &str) -> bool {
    BROKER_INSTANCE_TYPE_CATALOG.contains(&value)
}

/// The valid-value list the refusal below names, in the order the service names
/// them in: ascending by the type's own name, which is one sequence over both
/// categories rather than the grouped order [`BROKER_INSTANCE_TYPE_CATALOG`] is
/// written in.
///
/// Derived from that catalog once, so a type the service gains or loses is gained
/// or lost here too and the list can never offer a type the gate below refuses.
fn valid_broker_instance_types() -> &'static [&'static str] {
    static ORDERED: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        let mut ordered: Vec<&'static str> = BROKER_INSTANCE_TYPE_CATALOG.to_vec();
        ordered.sort_unstable();
        ordered
    });
    &ORDERED
}

/// The sentence Amazon MSK answers a request naming a broker type it does not
/// offer with: the fixed text `Unsupported InstanceType specified. Valid values:
/// [`, `]` around the one variable part — the types that ARE valid, read from
/// [`valid_broker_instance_types`].
///
/// The rejected value is NOT echoed: the service names the member's own name in
/// the fixed sentence and then says which types it does offer, which is the
/// actionable half of the refusal and so is derived from the catalog rather than
/// written out beside it.
pub fn unsupported_instance_type_message() -> String {
    format!(
        "Unsupported InstanceType specified. Valid values: [{}]",
        valid_broker_instance_types().join(", ")
    )
}

/// The input member a request names a cluster's broker type in, as the API
/// documents it on the wire — the camelCase spelling the request itself carries,
/// which is what a refusal of that value reports as the parameter at fault.
pub const INSTANCE_TYPE_MEMBER: &str = "instanceType";

/// Why a create naming `instance_type` for its brokers describes no cluster the
/// service could run, or `None` when the service offers that type.
///
/// Amazon MSK places brokers on instance types of its OWN catalogue
/// ([`BROKER_INSTANCE_TYPE_CATALOG`]), so a value outside it names no broker
/// hardware and the create is refused from the REQUEST alone, before anything the
/// request names is resolved against this account's network or registry.
pub fn create_instance_type_violation(instance_type: &str) -> Option<String> {
    (!is_catalogued_broker_instance_type(instance_type)).then(unsupported_instance_type_message)
}

/// The broker types of the Express category, in the order
/// [`BROKER_INSTANCE_TYPE_CATALOG`] offers them — which is ascending by size.
fn express_instance_types() -> &'static [&'static str] {
    static EXPRESS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
        BROKER_INSTANCE_TYPE_CATALOG
            .iter()
            .filter(|instance_type| instance_type.starts_with("express."))
            .copied()
            .collect()
    });
    &EXPRESS
}

/// The EC2 instance SIZES the broker catalog names, smallest first.
///
/// Source: the size segments of [`BROKER_INSTANCE_TYPE_CATALOG`] itself, in the
/// order EC2 orders those names in; written out because the order of two size
/// names is not something their text implies (`2xlarge` sorts before `large`).
const BROKER_INSTANCE_SIZES: &[&str] = &[
    "small", "large", "xlarge", "2xlarge", "4xlarge", "8xlarge", "12xlarge", "16xlarge",
    "24xlarge",
];

/// Where `instance_type`'s size sits on [`BROKER_INSTANCE_SIZES`], or `None` for
/// a type whose size the catalog does not name.
fn instance_size_rank(instance_type: &str) -> Option<usize> {
    let size = instance_type.rsplit('.').next()?;
    BROKER_INSTANCE_SIZES
        .iter()
        .position(|known| *known == size)
}

/// The Express broker type a cluster running `instance_type` would move to: the
/// Express type of the SAME size when the Express category offers that size, and
/// otherwise the nearest size it does offer — its largest for a size above the
/// Express range, its smallest for one below.
///
/// One function of the catalog, so a recommendation can only ever name a type the
/// service really offers ([`is_catalogued_broker_instance_type`]). `None` for a
/// value that is not a broker type at all: there is no size to match.
pub fn express_instance_type_for(instance_type: &str) -> Option<&'static str> {
    let rank = instance_size_rank(instance_type)?;
    let express = express_instance_types();
    if let Some(same_size) = express
        .iter()
        .find(|candidate| instance_size_rank(candidate) == Some(rank))
    {
        return Some(same_size);
    }
    let largest = express.last().copied()?;
    match instance_size_rank(largest) {
        Some(largest_rank) if rank > largest_rank => Some(largest),
        _ => express.first().copied(),
    }
}

/// What Amazon MSK answers a list that names the same item twice.
///
/// Entirely fixed text: neither the repeated value nor the member carrying it is
/// echoed back, and the member at fault is reported in the refusal's
/// `invalidParameter` by the handler that knows it.
pub const DUPLICATE_LIST_ITEMS_MESSAGE: &str = "The list provided contains duplicate items.";

/// Why the subnet list `subnet_ids` cannot place a resource, or `None` when it
/// can.
///
/// A placement names ONE subnet per Availability Zone — Amazon MSK puts a broker
/// group's elastic network interfaces in distinct zones — so the same subnet id
/// twice names no second zone: the list is refused as a duplicate list, from the
/// request alone, before anything is minted.
pub fn duplicate_placement_violation(subnet_ids: &[String]) -> Option<String> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    subnet_ids
        .iter()
        .any(|id| !seen.insert(id.as_str()))
        .then(|| DUPLICATE_LIST_ITEMS_MESSAGE.to_string())
}

/// What Amazon MSK answers a broker count that does not spread evenly over the
/// Availability Zones the cluster's client subnets span.
///
/// Entirely fixed text — the example it ends with is the service's own and names
/// no value of the request, so neither the count asked for nor the number of zones
/// found is interpolated.
pub const BROKER_COUNT_ZONE_MULTIPLE_MESSAGE: &str =
    "The target number of broker nodes must be a multiple of the number of Availability Zones \
     in the Client subnets parameter. For example, for a cluster with subnets in 2 Availability \
     Zones, the target number of brokers must be a multiple of 2.";

/// Why `target` brokers cannot be spread over `zones` Availability Zones, or
/// `None` when they can.
///
/// Amazon MSK gives every Availability Zone of a cluster's client subnets the SAME
/// number of brokers, so a count that is not a multiple of the number of those
/// zones describes a cluster it could not lay out. `zones` is what the cluster's
/// client subnets really span, so a placement whose zones are not known
/// (nothing resolved them) is no ground for this refusal: the caller passes a zone
/// count only when it has one.
pub fn broker_count_zone_violation(target: i32, zones: usize) -> Option<String> {
    let zones = i32::try_from(zones).unwrap_or(i32::MAX);
    (zones > 0 && target % zones != 0).then(|| BROKER_COUNT_ZONE_MULTIPLE_MESSAGE.to_string())
}

/// What Amazon MSK answers a request that asks for FEWER brokers than the cluster
/// holds.
///
/// Entirely fixed text, naming neither the count asked for nor the count the
/// cluster is at: the service cannot take brokers out of a cluster, and the
/// sentence says what the caller would have to do about the partitions on them
/// before it could.
pub const BROKER_COUNT_DECREASE_MESSAGE: &str =
    "Amazon MSK cannot proceed with decreasing the cluster's broker count. Make sure that at \
     least one broker in each Availability Zone has no user partitions and try again. Allow five \
     minutes after partition reassignments.";

/// Why a broker count may not move from `current` to `target`, or `None` when it
/// may.
///
/// Amazon MSK scales a cluster's broker count UP only: the brokers of a
/// provisioned cluster hold partition replicas, so removing one would have to move
/// the data on it, which the service does not do on the caller's behalf. A target
/// the cluster is already at moves nothing and is admitted, so the rule is about
/// going DOWN rather than about repeating a count.
pub fn broker_count_decrease_violation(current: i32, target: i32) -> Option<String> {
    (target < current).then(|| BROKER_COUNT_DECREASE_MESSAGE.to_string())
}
