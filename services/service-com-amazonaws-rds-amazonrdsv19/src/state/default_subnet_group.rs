// SPDX-License-Identifier: CC0-1.0

//! The account's `default` DB subnet group: the group Amazon RDS provides alongside the
//! account's default VPC, and the network it covers.
//!
//! No request creates it — `CreateDBSubnetGroup` refuses the name `default` for exactly
//! that reason — so it is not a stored group. What it reports is a fact of ANOTHER
//! service: "when we create a default VPC, we create a default subnet in each
//! Availability Zone" (Amazon VPC User Guide, "Default VPCs"), and those default subnets
//! are the ones the default DB subnet group covers. So the network is READ from the owner
//! of subnets ([`default_vpc_network`], the same in-process hop
//! [`super::ec2_subnets`] makes for the subnets a request names) and kept on this store
//! ([`ComAmazonawsRdsAmazonrdsv19State::record_default_subnet_group_network`]), so every
//! later read of the group — on a DB instance's envelope and from
//! `DescribeDBSubnetGroups` — reports the one answer EC2 gave.
//!
//! A deployment where EC2 cannot be reached, or an account whose default VPC has been
//! deleted, has no such network: the group then reports its name, description, status and
//! the VPC its region resolves into, and NO subnets, rather than subnet identifiers
//! nobody minted.

use super::*;

/// The `DescribeSubnets` filter that selects the default subnets of the account's default
/// VPC: `default-for-az` — "indicates whether this is the default subnet for the
/// Availability Zone" — which only a default VPC's own subnets carry.
const DEFAULT_FOR_AZ_FILTER: &str = "default-for-az";

/// The network the account's `default` DB subnet group covers: the default VPC and its
/// default subnets, as the owner of subnets reported them.
#[derive(Debug, Clone, Default)]
pub struct DefaultSubnetGroupNetwork {
    /// The VPC those subnets belong to, as EC2 reported it; `None` where it named none.
    pub vpc_id: Option<String>,
    /// The default subnets, in the Availability Zone order EC2 reported them in.
    pub subnet_ids: Vec<String>,
    /// The Availability Zone of each of [`Self::subnet_ids`], as EC2 reported it.
    pub subnet_zones: BTreeMap<String, String>,
}

/// The default subnets of the account's default VPC in `region`, as Amazon EC2 — the
/// service that owns subnets — reports them.
///
/// `None` where the hop could not be made or answered nothing: this service then knows no
/// network for the group, which it reports as no subnets rather than as a network of its
/// own choosing. Nothing is cached here; the CALLER records the answer on the store so a
/// later read reports the same group.
pub async fn default_vpc_network(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<DefaultSubnetGroupNetwork> {
    let client = ec2_subnets::ec2(ctx, region)?;
    let default_for_az = ::aws_sdk_ec2::types::Filter::builder()
        .name(DEFAULT_FOR_AZ_FILTER)
        .values("true")
        .build();
    let answered = client
        .describe_subnets()
        .filters(default_for_az)
        .send()
        .await
        .inspect_err(|error| {
            // At `warn`: a client reads the default group to tell which subnets and zones
            // an instance it created without naming a group was placed in, so a deployment
            // where this hop fails is one where the group reports none — worth seeing.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
                "the default VPC's subnets could not be resolved against EC2"
            );
        })
        .ok()?;
    let mut network = DefaultSubnetGroupNetwork::default();
    for subnet in answered.subnets() {
        let Some(subnet_id) = subnet.subnet_id() else {
            continue;
        };
        network.subnet_ids.push(subnet_id.to_string());
        if let Some(zone) = subnet.availability_zone() {
            network
                .subnet_zones
                .insert(subnet_id.to_string(), zone.to_string());
        }
        // A subnet belongs to one VPC, and every default subnet belongs to the default
        // one, so the VPC of any of them is the group's.
        if let Some(vpc_id) = subnet.vpc_id() {
            network.vpc_id = Some(vpc_id.to_string());
        }
    }
    // The zones a default VPC's subnets lie in are what orders them, so two reads of the
    // group report the subnets in one order whatever order EC2 answered in.
    network
        .subnet_ids
        .sort_by_key(|subnet_id| network.subnet_zones.get(subnet_id).cloned());
    (!network.subnet_ids.is_empty()).then_some(network)
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Whether this store already holds what the owner of subnets answered about the
    /// default VPC of `region` — so a create that places a resource in the `default` group
    /// asks EC2 once per region rather than on every call
    /// ([`Self::record_default_subnet_group_network`] keeps the first answer).
    #[must_use]
    pub fn knows_default_subnet_group_network(&self, region: &str) -> bool {
        self.default_subnet_group_networks.contains_key(region)
    }

    /// Keep what the owner of subnets answered about the default VPC of `region`, so the
    /// account's `default` DB subnet group reports that network from now on.
    ///
    /// Recorded once per region and not refreshed on every read: the group an instance was
    /// placed in reports the network it was placed in, and nothing in this service changes
    /// a default VPC.
    ///
    /// The region a request names is the caller's, so this cache is held to the same region
    /// ceiling every stored record is ([`Self::has_room_for_region`], which counts this
    /// map's own keys): a caller naming a fresh region on each call cannot open more buckets
    /// here than that, and a region past the ceiling simply keeps no cached network — the
    /// group then reports the network of the region it does know, or none.
    pub fn record_default_subnet_group_network(
        &mut self,
        region: &str,
        network: DefaultSubnetGroupNetwork,
    ) {
        if !self.has_room_for_region(region) {
            return;
        }
        self.default_subnet_group_networks
            .entry(region.to_string())
            .or_insert(network);
    }

    /// The account's `default` DB subnet group in `region`, carrying the network of the
    /// default VPC where this store has read it ([`default_vpc_network`]).
    pub fn account_default_db_subnet_group(&self, region: &str) -> DbSubnetGroupRecord {
        let mut group = DbSubnetGroupRecord::account_default(region);
        if let Some(network) = self.default_subnet_group_networks.get(region) {
            group.subnet_ids = network.subnet_ids.clone();
            group.subnet_zones = network.subnet_zones.clone();
            group.vpc_id = network.vpc_id.clone();
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One network, as the owner of subnets would have answered for a default VPC.
    fn a_network(region: &str) -> DefaultSubnetGroupNetwork {
        DefaultSubnetGroupNetwork {
            vpc_id: Some(region_vpc_id(region)),
            subnet_ids: vec![format!("subnet-{region}")],
            subnet_zones: BTreeMap::new(),
        }
    }

    /// The cached answer is held to the store's region ceiling: a caller naming a fresh
    /// region on every call opens at most [`MAX_REGIONS`] buckets here, after which the
    /// cache stops growing — and the regions it already answered for keep their network.
    #[test]
    fn the_cached_network_cannot_open_more_region_buckets_than_the_cap() {
        let mut store = ComAmazonawsRdsAmazonrdsv19State::default();
        let region_at = |index: usize| format!("us-west-{index}");

        for index in 0..MAX_REGIONS + 8 {
            let region = region_at(index);
            store.record_default_subnet_group_network(&region, a_network(&region));
            assert!(
                store.default_subnet_group_networks.len() <= MAX_REGIONS,
                "after caching {} region(s) the store holds {} buckets, over the cap of \
                 {MAX_REGIONS}",
                index + 1,
                store.default_subnet_group_networks.len()
            );
        }
        assert_eq!(
            store.default_subnet_group_networks.len(),
            MAX_REGIONS,
            "every region up to the cap is cached, and none past it"
        );

        // A region the cache already answered for is still answered for, and re-recording
        // it is accepted rather than refused as a new bucket.
        let first = region_at(0);
        store.record_default_subnet_group_network(&first, a_network(&first));
        assert!(
            store.knows_default_subnet_group_network(&first),
            "a region inside the cap keeps the network it was given"
        );
        assert!(
            !store.knows_default_subnet_group_network(&region_at(MAX_REGIONS + 1)),
            "a region past the cap caches nothing, so the group reports no subnets for it"
        );
    }
}
