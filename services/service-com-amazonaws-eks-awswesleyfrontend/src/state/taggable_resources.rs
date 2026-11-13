// SPDX-License-Identifier: CC0-1.0

//! Which resource of this service a `resourceArn` addresses, and where that
//! resource's tags live — the one resolution the tag plane (`TagResource`,
//! `UntagResource`, `ListTagsForResource`) reads a caller-supplied ARN with.
//!
//! The tag operations take "the Amazon Resource Name (ARN) of the resource"
//! rather than a cluster, so what they can address is every resource THIS
//! service mints an ARN for and keeps tags on: a cluster, a node group, a
//! Fargate profile, an add-on, a managed capability, an EKS Pod Identity
//! association, an EKS Anywhere subscription, an access entry and an identity
//! provider config. One resolution serves all three operations, so a resource
//! that can be tagged can also be untagged and read back.

use super::*;

/// Where the resource an EKS resource ARN addresses lives in this store.
///
/// A locator rather than a borrow, so the one walk below serves both the READ
/// (`ListTagsForResource`) and the two WRITES: the tag map is then reached
/// through [`ComAmazonawsEksAwswesleyfrontendState::tags_for`] or
/// [`ComAmazonawsEksAwswesleyfrontendState::tags_for_mut`] with the mutability
/// the caller needs, instead of the walk being written twice.
///
/// Each variant carries the key of the map the record is stored under, which is
/// the whole identity of the resource: a child resource's key spells its parents
/// (Region, cluster), because a name only identifies it within them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A cluster, by its `(region, name)` key.
    Cluster((String, String)),
    /// A managed node group, by its `(region, cluster, nodegroup)` key.
    Nodegroup((String, String, String)),
    /// A Fargate profile, by its `(region, cluster, profile)` key.
    FargateProfile((String, String, String)),
    /// An add-on, by its `(region, cluster, add-on)` key.
    Addon((String, String, String)),
    /// A managed capability, by its `(region, cluster, capability)` key.
    Capability((String, String, String)),
    /// An EKS Pod Identity association, by its `(region, cluster, id)` key.
    PodIdentityAssociation((String, String, String)),
    /// An EKS Anywhere subscription, by its `(region, id)` key.
    EksAnywhereSubscription((String, String)),
    /// An access entry, by its cluster's key and the principal ARN it is held
    /// under.
    AccessEntry {
        /// The `(region, name)` key of the cluster holding the entry.
        cluster: (String, String),
        /// The IAM principal the entry is for, which is its key within that
        /// cluster.
        principal_arn: String,
    },
    /// An identity provider config, by its cluster's key and the name it is held
    /// under.
    IdentityProviderConfig {
        /// The `(region, name)` key of the cluster holding the config.
        cluster: (String, String),
        /// The config's own name, which is its key within that cluster.
        name: String,
    },
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// The resource `arn` addresses, or `None` when no resource this store holds
    /// carries that ARN.
    ///
    /// The ARN a resource carries is the one this service MINTED for it and
    /// stored on its record, so the comparison is against that stored value
    /// rather than against an ARN recomposed from the request: a resource is
    /// addressable by exactly the ARN its own create answered with. Every ARN
    /// already names its Region, so a tag call resolves a resource in any Region
    /// without the request's own credential scope.
    ///
    /// The clusters are looked up through [`Self::arn_index`] and the rest are
    /// walked; each of those maps is bounded by the ceiling its own create
    /// enforces, so the walk is bounded too.
    pub fn tagged_resource_for_arn(&self, arn: &str) -> Option<TaggedResource> {
        if let Some(key) = self.arn_index.get(arn) {
            return Some(TaggedResource::Cluster(key.clone()));
        }
        let child = |(key, record_arn): (&(String, String, String), &str)| {
            (record_arn == arn).then(|| key.clone())
        };
        if let Some(key) = self
            .nodegroups
            .iter()
            .map(|(key, record)| (key, record.arn.as_str()))
            .find_map(child)
        {
            return Some(TaggedResource::Nodegroup(key));
        }
        if let Some(key) = self
            .fargate_profiles
            .iter()
            .map(|(key, record)| (key, record.arn.as_str()))
            .find_map(child)
        {
            return Some(TaggedResource::FargateProfile(key));
        }
        if let Some(key) = self
            .addons
            .iter()
            .map(|(key, record)| (key, record.arn.as_str()))
            .find_map(child)
        {
            return Some(TaggedResource::Addon(key));
        }
        if let Some(key) = self
            .capabilities
            .iter()
            .map(|(key, record)| (key, record.arn.as_str()))
            .find_map(child)
        {
            return Some(TaggedResource::Capability(key));
        }
        if let Some(key) = self
            .pod_identity_associations
            .iter()
            .map(|(key, record)| (key, record.association_arn.as_str()))
            .find_map(child)
        {
            return Some(TaggedResource::PodIdentityAssociation(key));
        }
        if let Some(key) = self
            .eks_anywhere_subscriptions
            .iter()
            .find_map(|(key, record)| (record.arn == arn).then(|| key.clone()))
        {
            return Some(TaggedResource::EksAnywhereSubscription(key));
        }
        // The two collections a cluster holds INSIDE its own record: their ARNs
        // are minted the same way and stored the same way, so they are addressable
        // the same way.
        for (cluster, record) in &self.clusters {
            if let Some(entry) = record
                .access_entries
                .values()
                .find(|entry| entry.arn == arn)
            {
                return Some(TaggedResource::AccessEntry {
                    cluster: cluster.clone(),
                    principal_arn: entry.principal_arn.clone(),
                });
            }
            if let Some(name) = record
                .identity_provider_configs
                .iter()
                .find_map(|(name, config)| (config.arn == arn).then(|| name.clone()))
            {
                return Some(TaggedResource::IdentityProviderConfig {
                    cluster: cluster.clone(),
                    name,
                });
            }
        }
        None
    }

    /// The tags the resource `resource` locates carries, or `None` when the
    /// record it locates is no longer there.
    pub fn tags_for(&self, resource: &TaggedResource) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::Cluster(key) => self.clusters.get(key).map(|record| &record.tags),
            TaggedResource::Nodegroup(key) => self.nodegroups.get(key).map(|record| &record.tags),
            TaggedResource::FargateProfile(key) => {
                self.fargate_profiles.get(key).map(|record| &record.tags)
            }
            TaggedResource::Addon(key) => self.addons.get(key).map(|record| &record.tags),
            TaggedResource::Capability(key) => {
                self.capabilities.get(key).map(|record| &record.tags)
            }
            TaggedResource::PodIdentityAssociation(key) => self
                .pod_identity_associations
                .get(key)
                .map(|record| &record.tags),
            TaggedResource::EksAnywhereSubscription(key) => self
                .eks_anywhere_subscriptions
                .get(key)
                .map(|record| &record.tags),
            TaggedResource::AccessEntry {
                cluster,
                principal_arn,
            } => self
                .clusters
                .get(cluster)
                .and_then(|record| record.access_entries.get(principal_arn))
                .map(|entry| &entry.tags),
            TaggedResource::IdentityProviderConfig { cluster, name } => self
                .clusters
                .get(cluster)
                .and_then(|record| record.identity_provider_configs.get(name))
                .map(|config| &config.tags),
        }
    }

    /// The tags the resource `resource` locates carries, to write — the same map
    /// [`Self::tags_for`] reads, so a tag written through one operation is the tag
    /// every other reports.
    pub fn tags_for_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::Cluster(key) => {
                self.clusters.get_mut(key).map(|record| &mut record.tags)
            }
            TaggedResource::Nodegroup(key) => {
                self.nodegroups.get_mut(key).map(|record| &mut record.tags)
            }
            TaggedResource::FargateProfile(key) => self
                .fargate_profiles
                .get_mut(key)
                .map(|record| &mut record.tags),
            TaggedResource::Addon(key) => self.addons.get_mut(key).map(|record| &mut record.tags),
            TaggedResource::Capability(key) => self
                .capabilities
                .get_mut(key)
                .map(|record| &mut record.tags),
            TaggedResource::PodIdentityAssociation(key) => self
                .pod_identity_associations
                .get_mut(key)
                .map(|record| &mut record.tags),
            TaggedResource::EksAnywhereSubscription(key) => self
                .eks_anywhere_subscriptions
                .get_mut(key)
                .map(|record| &mut record.tags),
            TaggedResource::AccessEntry {
                cluster,
                principal_arn,
            } => self
                .clusters
                .get_mut(cluster)
                .and_then(|record| record.access_entries.get_mut(principal_arn))
                .map(|entry| &mut entry.tags),
            TaggedResource::IdentityProviderConfig { cluster, name } => self
                .clusters
                .get_mut(cluster)
                .and_then(|record| record.identity_provider_configs.get_mut(name))
                .map(|config| &mut config.tags),
        }
    }
}
