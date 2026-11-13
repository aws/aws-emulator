// SPDX-License-Identifier: CC0-1.0

//! The store itself: the one struct every operation of this service reads and
//! writes through, and the queries and mutations it offers them — including the
//! transitions the clock owes its records, materialised on the next call.

use super::*;

/// Per-service state for com-amazonaws-eks-awswesleyfrontend. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEksAwswesleyfrontendState {
    /// Clusters by `(region, name)`.
    ///
    /// A cluster name is unique per REGION, not globally — "The name must be
    /// unique within the Amazon Web Services Region and Amazon Web Services
    /// account that you're creating the cluster in" (`CreateCluster$name`) — and
    /// every ARN and endpoint a record carries names one region, so the region a
    /// request is signed for is part of the key. A `BTreeMap` so `ListClusters`
    /// walks a stable, sorted order within a region and a page token can name the
    /// key to resume after. The account is not part of the key: this emulator
    /// serves a single account ([`::engine::mock_account_id`]). Its size is
    /// bounded by [`MAX_STORED_CLUSTERS`], which `CreateCluster` enforces.
    pub clusters: BTreeMap<(String, String), ClusterRecord>,
    /// Cluster ARN -> the `(region, name)` key of the cluster it identifies, so
    /// the tag operations resolve a `resourceArn` without scanning every record.
    /// An EKS cluster ARN already carries the region, so this is what lets a tag
    /// call name a cluster in any region. It holds exactly one entry per stored
    /// cluster, so [`MAX_STORED_CLUSTERS`] bounds it too.
    pub arn_index: BTreeMap<String, (String, String)>,
    /// Managed node groups by `(region, cluster name, nodegroup name)`.
    ///
    /// A node group name identifies a node group only WITHIN one cluster, and a
    /// cluster name only within one region (see [`Self::clusters`]), so the key
    /// is the whole triple: the same node group name may exist in another
    /// cluster or another region. A `BTreeMap` so a listing walks a stable,
    /// sorted order within a cluster. Its size is bounded by
    /// [`MAX_STORED_NODEGROUPS`], which `CreateNodegroup` enforces.
    ///
    /// Node group ARNs are deliberately NOT added to [`Self::arn_index`]: that
    /// index answers "which CLUSTER does this ARN name", while the tag plane
    /// addresses a node group by the ARN stored on its own record
    /// ([`Self::tagged_resource_for_arn`]).
    pub nodegroups: BTreeMap<(String, String, String), NodegroupRecord>,
    /// Fargate profiles by `(region, cluster name, Fargate profile name)`.
    ///
    /// A Fargate profile is a resource INSIDE one cluster — it is addressed as
    /// `/clusters/{clusterName}/fargate-profiles/{fargateProfileName}` — and a
    /// cluster name identifies a cluster only within one region (see
    /// [`Self::clusters`]), so the key is the whole triple: the same profile name
    /// may exist in another cluster or another region. A `BTreeMap` so a listing
    /// walks one cluster's profiles as a contiguous, name-sorted range. Its size
    /// is bounded by [`MAX_STORED_FARGATE_PROFILES`], which
    /// `CreateFargateProfile` enforces.
    pub fargate_profiles: BTreeMap<(String, String, String), FargateProfileRecord>,
    /// Add-ons by `(region, cluster name, add-on name)`.
    ///
    /// An add-on is a resource INSIDE one cluster — `CreateAddon`'s `clusterName`
    /// is an `@httpLabel` on `/clusters/{clusterName}/addons` and one add-on name
    /// installs at most once per cluster — so the key is the cluster's own
    /// `(region, name)` key plus the add-on name. A `BTreeMap` so a listing walks
    /// one cluster's add-ons in a stable, sorted order. Its size is bounded by
    /// [`MAX_ADDONS_PER_CLUSTER`] per cluster, which `CreateAddon` enforces,
    /// times the [`MAX_STORED_CLUSTERS`] clusters the store holds.
    ///
    /// Add-on ARNs are deliberately NOT in [`Self::arn_index`]: that index is
    /// what a CLUSTER is resolved from, while the tag plane addresses an add-on by
    /// the ARN stored on its own record ([`Self::tagged_resource_for_arn`]).
    pub addons: BTreeMap<(String, String, String), AddonRecord>,
    /// Managed capabilities by `(region, cluster name, capability name)`.
    ///
    /// A capability name is unique within its CLUSTER ("The name must be unique
    /// within your cluster", `CreateCapability$capabilityName`), and a cluster name
    /// is unique within a region, so the key is the triple. A `BTreeMap` so
    /// `ListCapabilities` walks a stable, sorted order within one cluster and a
    /// page token can name the capability to resume after. Bounded by
    /// [`MAX_CAPABILITIES_PER_CLUSTER`] per cluster and so, with
    /// [`MAX_STORED_CLUSTERS`] clusters, bounded overall.
    pub capabilities: BTreeMap<(String, String, String), CapabilityRecord>,
    /// EKS Pod Identity associations by `(region, clusterName, associationId)`.
    ///
    /// An association belongs to exactly one cluster and a cluster name only
    /// identifies a cluster within one Region, so the key carries both; the
    /// association id is unique within the cluster. A `BTreeMap` so a listing
    /// walks one cluster's associations as a contiguous, sorted range. Bounded by
    /// [`MAX_POD_IDENTITY_ASSOCIATIONS_PER_CLUSTER`] per cluster, which
    /// `CreatePodIdentityAssociation` enforces, times the bounded number of
    /// clusters.
    pub pod_identity_associations: BTreeMap<(String, String, String), PodIdentityAssociationRecord>,
    /// How many association ids this store has minted, ever.
    ///
    /// It is the uniqueness seam for [`Self::mint_association_id`]: two creates
    /// for the same cluster, namespace and service account (one after the other
    /// was deleted) must not mint the same id, so the counter — which never goes
    /// back — is part of the seed.
    pub minted_associations: u64,
    /// The Kubernetes version calendar `DescribeClusterVersions` reports, seeded
    /// once from [`VERSION_CALENDAR`] (see [`ClusterVersionRecord`]) and never
    /// written by a request: it is what Amazon EKS publishes, not per-account
    /// data. A `Vec` because the calendar has a publication ORDER (newest
    /// Kubernetes version first, see [`seed_cluster_versions`]) that the listing
    /// and its page tokens follow.
    pub cluster_versions: Vec<ClusterVersionRecord>,
    /// The add-on catalog `DescribeAddonVersions` reports and
    /// `DescribeAddonConfiguration` answers from, seeded once from
    /// [`ADDON_CATALOG`] (see [`AddonCatalogRecord`]) and never written by a
    /// request: it is what Amazon EKS publishes, not per-account data. A `Vec`
    /// because the catalog has a publication ORDER (add-on name order, see
    /// [`seed_addon_catalog`]) that the listing reports it in.
    pub addon_catalog: Vec<AddonCatalogRecord>,
    /// EKS Anywhere subscriptions by `(region, id)`.
    ///
    /// A subscription is an ACCOUNT-level support contract rather than a resource
    /// of a cluster, and it is addressed by the id the create mints
    /// (`DescribeEksAnywhereSubscription$id`), so the key is the Region the
    /// request is signed for plus that id — the Region because the name must be
    /// "unique in your Amazon Web Services account in the Amazon Web Services
    /// Region you're creating the subscription in"
    /// (`CreateEksAnywhereSubscriptionRequest$name`), so a subscription belongs to
    /// one Region as every other record here does. A `BTreeMap` so a listing walks
    /// one Region's subscriptions as a contiguous, sorted range. Its size is
    /// bounded by [`MAX_STORED_SUBSCRIPTIONS`], which
    /// `CreateEksAnywhereSubscription` enforces.
    pub eks_anywhere_subscriptions: BTreeMap<(String, String), EksAnywhereSubscriptionRecord>,
    /// How many update ids this store has minted. Mixed into the next one (see
    /// [`new_update_id`]) so two updates minted in the same nanosecond cannot
    /// collide.
    pub minted_updates: u64,
    /// The cluster deletions that are IN FLIGHT, by the deleted cluster's
    /// `(region, name)` key, each with the instant it stops being treated as one.
    ///
    /// A cluster whose delete has been accepted is being deleted while that call is
    /// still releasing what the cluster owned: a `DeleteCluster` that arrives in
    /// that window is refused as a resource in use rather than answered as a
    /// cluster that was never there. The call that started it is what clears the
    /// entry ([`Self::finish_cluster_deletion`]); the instant stored with it is the
    /// failsafe [`CLUSTER_DELETION_DELAY`] bounds, so a call that never reports
    /// back leaves nothing behind for ever and this map holds at most the
    /// deletions of that window.
    pub cluster_deletions: BTreeMap<(String, String), SystemTime>,
    // An asynchronous update is not a field here — it lives on the cluster it
    // updates ([`ClusterRecord::updates`]).
}

impl Default for ComAmazonawsEksAwswesleyfrontendState {
    /// A fresh store holds no clusters and the whole published version calendar:
    /// `DescribeClusterVersions` answers before anything has been created.
    fn default() -> Self {
        Self {
            clusters: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            nodegroups: BTreeMap::new(),
            fargate_profiles: BTreeMap::new(),
            addons: BTreeMap::new(),
            capabilities: BTreeMap::new(),
            pod_identity_associations: BTreeMap::new(),
            eks_anywhere_subscriptions: BTreeMap::new(),
            minted_associations: 0,
            cluster_versions: seed_cluster_versions(),
            addon_catalog: seed_addon_catalog(),
            minted_updates: 0,
            cluster_deletions: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsEksAwswesleyfrontendState {
    /// The calendar rows that are released at `now`, in the calendar's own
    /// publication order (newest Kubernetes version first).
    pub fn released_cluster_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &ClusterVersionRecord> + '_ {
        self.cluster_versions
            .iter()
            .filter(move |entry| entry.is_released(now))
    }

    /// The calendar rows Amazon EKS OFFERS at `now`: the released rows whose
    /// extended support has not ended.
    ///
    /// "A version whose extended support has ended is no longer one Amazon EKS
    /// offers", so a new cluster is not created at one — while
    /// `DescribeClusterVersions` still PUBLISHES its row, with the lifecycle state
    /// on the row. One reading of "offered", so the catalog and `CreateCluster`
    /// cannot disagree about which versions a cluster can run.
    pub fn offered_cluster_versions(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &ClusterVersionRecord> + '_ {
        self.released_cluster_versions(now)
            .filter(move |entry| entry.support_status(now) != SupportStatus::Unsupported)
    }

    /// Materialise every clock-owed status move across the store.
    pub fn materialize_due_transitions(&mut self, now: SystemTime) {
        // A cluster deletion nothing reported back for is no longer treated as in
        // flight once its failsafe instant has passed: the name it held is free
        // again, and that was true at that instant whether or not anybody called.
        self.cluster_deletions.retain(|_, due| now < *due);
        // An add-on whose removal the clock has finished is no longer an add-on of
        // its cluster. It goes first: the record is gone, so nothing below has to
        // reason about one that is on its way out, and the removal was due at that
        // instant whether or not anybody called.
        self.addons
            .retain(|_, addon| addon.removed_at.is_none_or(|due| now < due));
        // The ADD-ONS go first: completing an add-on's update may FAIL it (see
        // [`Self::complete_addon_update`]), and the cluster pass below is what
        // would otherwise already have reported that same update `Successful`.
        let mut completed: Vec<((String, String, String), SystemTime, PendingAddonUpdate)> =
            Vec::new();
        for (key, addon) in self.addons.iter_mut() {
            if let Some((at, pending)) = addon.materialize_due_transitions(now) {
                completed.push((key.clone(), at, pending));
            }
        }
        for (key, at, pending) in completed {
            self.complete_addon_update(&key, at, pending);
        }
        for cluster in self.clusters.values_mut() {
            cluster.materialize_due_transitions(now);
        }
        for nodegroup in self.nodegroups.values_mut() {
            nodegroup.materialize_due_transitions(now);
        }
        for profile in self.fargate_profiles.values_mut() {
            profile.materialize_due_transitions(now);
        }
        for capability in self.capabilities.values_mut() {
            capability.materialize_due_transitions(now);
        }
        // A subscription whose removal the clock has finished is no longer a
        // subscription of the account, the same way an add-on's removal above is:
        // it is "removed from list/describe API responses"
        // (`DeleteEksAnywhereSubscription`), and that was due at its stored instant
        // whether or not anybody called.
        self.eks_anywhere_subscriptions
            .retain(|_, subscription| subscription.removed_at.is_none_or(|due| now < due));
        for subscription in self.eks_anywhere_subscriptions.values_mut() {
            subscription.materialize_due_transitions(now);
        }
        self.resettle_addon_replicas(now);
    }

    /// Bring every settled add-on's status and health back in line with what the
    /// cluster can run NOW — the last pass, so it reads the node groups and
    /// Fargate profiles the passes above have just moved.
    ///
    /// Whether an add-on's replicas are running is a value DERIVED from the
    /// cluster's compute, not a fact of the add-on: a replica-less add-on
    /// recovers once the cluster has somewhere to schedule its pod (a node group
    /// finishing, a Fargate profile that selects its namespace, Auto Mode), and
    /// one whose compute has gone is short of replicas again. Recomputing it is
    /// therefore the same kind of work as materialising a clock-owed move: the
    /// answer was already true at this instant whether or not anybody called.
    ///
    /// Only the two states that are ABOUT the replicas are recomputed. An
    /// install that never came up is terminal — such an add-on is removed and
    /// installed again rather than recovering on its own — and one still
    /// settling, taking an update or being deleted has its own pass above.
    pub(super) fn resettle_addon_replicas(&mut self, now: SystemTime) {
        let settled: Vec<((String, String, String), String)> = self
            .addons
            .iter()
            .filter(|(_, addon)| {
                addon.pending_update.is_none()
                    && addon.active_at.is_none()
                    && matches!(
                        addon.status.as_str(),
                        ADDON_STATUS_ACTIVE | ADDON_STATUS_DEGRADED
                    )
            })
            .map(|(key, addon)| (key.clone(), addon.namespace.clone()))
            .collect();
        for (key, namespace) in settled {
            let outcome = self.addon_replica_outcome(&key.0, &key.1, &key.2, &namespace, now);
            if let Some(addon) = self.addons.get_mut(&key) {
                if addon.status != outcome.status {
                    addon.status = outcome.status;
                    addon.health_issues = outcome.issues;
                }
            }
        }
    }

    /// Complete the add-on update the clock owed at `at`: write the change it
    /// carries, or fail it where that change can no longer be made.
    ///
    /// An add-on update is asynchronous, so the cluster can have taken another
    /// association for a `(namespace, service account)` pair the change asks for
    /// between the call that started the update and this instant. Mapping that
    /// pair twice is the ambiguity the pod identity slice refuses, so the update
    /// FAILS — the add-on keeps every value it had, reports `UPDATE_FAILED` with
    /// the issue that says why, and the update reports `Failed` rather than the
    /// `Successful` the clock owed it.
    ///
    /// Both sides of that — the add-on, and the associations and the update record
    /// — are only reachable here, which is why this lives on the store rather than
    /// on [`AddonRecord`].
    pub(super) fn complete_addon_update(
        &mut self,
        key: &(String, String, String),
        at: SystemTime,
        pending: PendingAddonUpdate,
    ) {
        let (region, cluster_name, _) = key.clone();
        let Some(addon) = self.addons.get(key) else {
            return;
        };
        let owner_arn = addon.arn.clone();
        let namespace = addon.namespace.clone();
        // Only an association ANOTHER owner holds can conflict: the ones this
        // add-on owns are the ones the requested set replaces. Each service
        // account is kept with the IAM role it is already bound to, because that
        // role is what the conflict is reported as
        // ([`service_account_already_associated`]).
        let taken: ::std::collections::BTreeMap<String, String> = self
            .pod_identity_associations_in(&region, &cluster_name)
            .filter(|record| {
                record.namespace == namespace
                    && record.owner_arn.as_deref() != Some(owner_arn.as_str())
            })
            .map(|record| (record.service_account.clone(), record.role_arn.clone()))
            .collect();
        let conflict = pending
            .pod_identity_associations
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(|association| {
                taken
                    .get(&association.service_account)
                    .map(|role_arn| (association.service_account.clone(), role_arn.clone()))
            });

        if let Some((service_account, bound_role_arn)) = conflict {
            if let Some(addon) = self.addons.get_mut(key) {
                addon.fail_update(
                    at,
                    AddonIssue {
                        code: Some(ADDON_ISSUE_CONFIGURATION_CONFLICT.to_string()),
                        message: Some(service_account_already_associated(&bound_role_arn)),
                        resource_ids: Some(vec![service_account]),
                    },
                );
            }
            if let Some(update) = self
                .clusters
                .get_mut(&(region, cluster_name))
                .and_then(|cluster| cluster.updates.get_mut(&pending.update_id))
            {
                update.status = UPDATE_FAILED.to_string();
                update.succeeds_at = None;
            }
            return;
        }

        if let Some(addon) = self.addons.get_mut(key) {
            addon.apply_update(at, &pending);
        }
        if let Some(requested) = pending.pod_identity_associations.as_deref() {
            self.replace_addon_associations(key, requested, at);
        }
    }

    /// Make the EKS Pod Identity associations one add-on owns be exactly
    /// `requested`, which is what completing an add-on update that named them
    /// does.
    ///
    /// "An array of EKS Pod Identity associations to be updated... If an empty
    /// array is provided, existing associations owned by the add-on are deleted"
    /// (`UpdateAddonRequest$podIdentityAssociations`), so the requested list is
    /// the SET the add-on owns afterwards: the ones it owned are dropped and one
    /// record is created per requested entry — which is also what deletes them
    /// all when the list is empty.
    pub(super) fn replace_addon_associations(
        &mut self,
        key: &(String, String, String),
        requested: &[AddonPodIdentityAssociations],
        at: SystemTime,
    ) {
        let (region, cluster_name, _) = key.clone();
        let Some(addon) = self.addons.get(key) else {
            return;
        };
        let owner_arn = addon.arn.clone();
        let namespace = addon.namespace.clone();
        self.remove_associations_owned_by(&owner_arn);
        let mut association_arns = Vec::with_capacity(requested.len());
        for association in requested {
            association_arns.push(self.create_addon_owned_association(
                &region,
                &cluster_name,
                &namespace,
                &owner_arn,
                association,
                at,
            ));
        }
        if let Some(addon) = self.addons.get_mut(key) {
            addon.pod_identity_association_arns = association_arns;
        }
    }

    /// Create the EKS Pod Identity association an add-on owns for one entry of
    /// the `podIdentityAssociations` it was asked for, and answer that
    /// association's ARN.
    ///
    /// The id is minted through [`Self::mint_association_id`] — the same path
    /// `CreatePodIdentityAssociation` takes — so an association an add-on owns is
    /// unique within its cluster and has the form every other association id has,
    /// and it carries the add-on's ARN as its `ownerArn`: "If defined, the EKS Pod
    /// Identity association is owned by an Amazon EKS add-on"
    /// (`PodIdentityAssociation$ownerArn`). One path for both the install that
    /// creates such an association and the update that replaces it, so the two
    /// cannot write different records for the same request member.
    pub fn create_addon_owned_association(
        &mut self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
        owner_arn: &str,
        association: &AddonPodIdentityAssociations,
        at: SystemTime,
    ) -> String {
        let association_id =
            self.mint_association_id(region, cluster_name, namespace, &association.service_account);
        let association_arn = derive_association_arn(region, cluster_name, &association_id);
        let record = PodIdentityAssociationRecord {
            cluster_name: cluster_name.to_string(),
            association_id: association_id.clone(),
            association_arn: association_arn.clone(),
            namespace: namespace.to_string(),
            service_account: association.service_account.clone(),
            role_arn: association.role_arn.clone(),
            // An add-on asks only for a service account and a role, so an
            // association it owns names no target role — and `externalId` exists
            // to be matched in a TARGET role's trust policy, so there is nothing
            // to condition — no permissions policy and no session-tag override.
            target_role_arn: None,
            external_id: None,
            policy: None,
            disable_session_tags: None,
            // "Tags don't propagate to any other cluster or Amazon Web Services
            // resources" (`CreateAddon$tags`), so the add-on's tags are not
            // copied onto the associations it owns.
            tags: BTreeMap::new(),
            created_at: at,
            modified_at: at,
            // The association is created by the add-on rather than by a call of
            // the pod identity slice, so no association-level idempotency token
            // applies to it.
            client_request_token: None,
            update_request_token: None,
            owner_arn: Some(owner_arn.to_string()),
        };
        self.pod_identity_associations.insert(
            (region.to_string(), cluster_name.to_string(), association_id),
            record,
        );
        association_arn
    }

    /// The names of the add-ons installed on one cluster, in sorted order.
    ///
    /// The map is keyed `(region, cluster, addon)`, so its iteration order
    /// already sorts each cluster's add-on names; this only drops the other
    /// clusters.
    pub fn addon_names_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.addons
            .keys()
            .filter(move |(key_region, key_cluster, _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, _, addon_name)| addon_name.as_str())
    }

    /// The add-ons the catalog publishes at `now`, in the catalog's own
    /// publication order (add-on name order).
    ///
    /// An add-on none of whose builds Amazon EKS has published yet is not one
    /// the catalog offers, so it is not reported at all.
    pub fn published_addons(
        &self,
        now: SystemTime,
    ) -> impl Iterator<Item = &AddonCatalogRecord> + '_ {
        self.addon_catalog
            .iter()
            .filter(move |addon| addon.published_versions(now).next().is_some())
    }

    /// The catalog entry for one add-on name, when the catalog publishes it at
    /// `now`.
    pub fn published_addon(
        &self,
        addon_name: &str,
        now: SystemTime,
    ) -> Option<&AddonCatalogRecord> {
        self.published_addons(now)
            .find(|addon| addon.addon_name == addon_name)
    }

    /// The catalog entry and the published build one `(addonName, addonVersion)`
    /// pair names, or `None` when the catalog publishes no such build at `now`.
    ///
    /// It is the pair `DescribeAddonConfiguration` resolves, so whatever reads a
    /// build's configuration schema reads the same build that operation
    /// publishes it for.
    pub fn published_addon_build(
        &self,
        addon_name: &str,
        addon_version: &str,
        now: SystemTime,
    ) -> Option<(&AddonCatalogRecord, &AddonCatalogVersionRecord)> {
        let addon = self.published_addon(addon_name, now)?;
        let version = addon
            .published_versions(now)
            .find(|version| version.addon_version == addon_version)?;
        Some((addon, version))
    }

    /// Whether the catalog publishes any build of one add-on that supports one
    /// Kubernetes version at `now`.
    ///
    /// This is what "the name must match one of the names returned by
    /// `DescribeAddonVersions`" (`CreateAddon$addonName`) means for a request
    /// against one cluster: the names that listing reports when it is narrowed
    /// to that cluster's Kubernetes version, which is exactly the add-ons that
    /// have a build for it.
    pub fn publishes_addon_for(
        &self,
        addon_name: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> bool {
        self.published_addon(addon_name, now).is_some_and(|addon| {
            addon
                .published_versions(now)
                .any(|version| version.supports(kubernetes_version))
        })
    }

    /// Whether the catalog publishes the build one `(addonName, addonVersion)`
    /// pair names AND lists one Kubernetes version among that build's
    /// compatibilities at `now`.
    ///
    /// This is what "the version must match one of the versions returned by
    /// `DescribeAddonVersions`" (`CreateAddon$addonVersion`) means for a request
    /// against one cluster: a build the catalog does not publish at all, and a
    /// published build whose `compatibilities` do not carry the cluster's
    /// Kubernetes version, are both builds that listing does not offer for it.
    pub fn publishes_addon_build_for(
        &self,
        addon_name: &str,
        addon_version: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> bool {
        self.published_addon_build(addon_name, addon_version, now)
            .is_some_and(|(_, build)| build.supports(kubernetes_version))
    }

    /// The version the catalog publishes as the DEFAULT build of one add-on for
    /// one Kubernetes version — the version `CreateAddon` installs when the
    /// caller names none — or `None` when this catalog publishes no build of
    /// that add-on for that Kubernetes version.
    pub fn default_addon_version(
        &self,
        addon_name: &str,
        kubernetes_version: &str,
        now: SystemTime,
    ) -> Option<String> {
        self.published_addon(addon_name, now)?
            .published_versions(now)
            .find(|version| version.is_default_for(kubernetes_version))
            .map(|version| version.addon_version.clone())
    }

    /// The add-on one `(region, cluster, addonName)` triple names, when that
    /// cluster holds it.
    pub fn installed_addon(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
    ) -> Option<&AddonRecord> {
        self.addons.get(&(
            region.to_string(),
            cluster_name.to_string(),
            addon_name.to_string(),
        ))
    }

    /// Whether `cluster_name` has compute a pod in `namespace` can be scheduled
    /// onto.
    ///
    /// Three things give a cluster somewhere to run a pod: EKS Auto Mode, which
    /// provisions nodes for the pods the cluster is asked to run; a managed node
    /// group that is RUNNING and scaled to at least one node; and a Fargate
    /// profile that is running and SELECTS the pod's namespace, since Fargate
    /// runs only the pods a profile selects. A cluster with none of them runs
    /// nothing.
    ///
    /// The status matters as much as the count on both: a node group or profile
    /// the cluster is still creating carries no pod yet, which is why this is
    /// re-read rather than answered once (see [`Self::materialize_due_transitions`]).
    pub fn cluster_can_place_pods(
        &self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
    ) -> bool {
        if self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
            .is_some_and(ClusterRecord::is_auto_mode)
        {
            return true;
        }
        if self.nodegroups_of(region, cluster_name).any(|nodegroup| {
            nodegroup.scaling_config.desired_size.unwrap_or_default() > 0
                && matches!(nodegroup.status.as_str(), STATUS_ACTIVE | STATUS_UPDATING)
        }) {
            return true;
        }
        self.fargate_profiles_of(region, cluster_name)
            .any(|profile| {
                profile.status == FARGATE_PROFILE_STATUS_ACTIVE
                    && profile
                        .selectors
                        .iter()
                        .any(|selector| selector.namespace.as_deref() == Some(namespace))
            })
    }

    /// The terminal state an install of `addon_name` into `namespace` on one
    /// cluster is owed, decided from that cluster's own state.
    ///
    /// An add-on install is asynchronous and settles in the state that reflects
    /// what the cluster could actually do with it, which is four questions in
    /// order, each about one thing the install needs:
    ///
    /// 1. an add-on sold through the AWS Marketplace needs a SUBSCRIPTION to it
    ///    before its software may be pulled at all. This emulator serves no
    ///    Marketplace, so it holds no subscription to any listing and such an
    ///    install never STARTS ([`ADDON_ISSUE_SUBSCRIPTION_NEEDED`]): it is
    ///    settled at the call that asked for it
    ///    ([`AddonInstallOutcome::refused`]), because this precondition is the
    ///    ADD-ON's own and no span of the clock can change it; the remediation
    ///    is the one Amazon EKS states, outside this API;
    /// 2. the add-ons its resources are applied against must be ON the cluster,
    ///    otherwise the Kubernetes resources it needs are not there and the
    ///    install fails ([`ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND`]);
    /// 3. one that is installed but is NOT RUNNING cannot answer the admission
    ///    review the API server sends its webhook, so every resource that
    ///    webhook gates is refused and the install fails
    ///    ([`ADDON_ISSUE_ADMISSION_REQUEST_DENIED`]);
    /// 4. an add-on with a replica to schedule needs the cluster to have compute
    ///    to schedule it onto; without it the resources are applied but no pod
    ///    of the add-on runs, which is a running add-on that is not healthy
    ///    ([`ADDON_ISSUE_INSUFFICIENT_REPLICAS`]).
    ///
    /// An add-on this catalog does not publish carries none of these
    /// requirements, so its install runs.
    pub fn addon_install_outcome(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        namespace: &str,
        now: SystemTime,
    ) -> AddonInstallOutcome {
        let Some(published) = self.published_addon(addon_name, now) else {
            return AddonInstallOutcome::running();
        };
        if published.owner == ADDON_OWNER_MARKETPLACE {
            return AddonInstallOutcome::refused(AddonIssue {
                code: Some(ADDON_ISSUE_SUBSCRIPTION_NEEDED.to_string()),
                message: Some(ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE.to_string()),
                // Nothing was applied to the cluster, so there is no resource of
                // this add-on for the issue to name.
                resource_ids: None,
            });
        }
        let missing: Vec<String> = published
            .required_addons
            .iter()
            .filter(|required| {
                self.installed_addon(region, cluster_name, required)
                    .is_none()
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            return AddonInstallOutcome::failed(AddonIssue {
                code: Some(ADDON_ISSUE_K8S_RESOURCE_NOT_FOUND.to_string()),
                message: Some(format!(
                    "The Kubernetes resources add-on {addon_name} is installed against are \
                     not on cluster {cluster_name}: {} must be installed first.",
                    missing.join(", ")
                )),
                resource_ids: Some(self.addon_resource_ids(&missing, now)),
            });
        }
        let denied: Vec<String> = published
            .required_addons
            .iter()
            .filter(|required| {
                self.published_addon(required, now)
                    .is_some_and(|entry| entry.admission_webhook)
                    && self
                        .installed_addon(region, cluster_name, required)
                        .is_some_and(|record| !record.is_running())
            })
            .cloned()
            .collect();
        if !denied.is_empty() {
            return AddonInstallOutcome::failed(AddonIssue {
                code: Some(ADDON_ISSUE_ADMISSION_REQUEST_DENIED.to_string()),
                message: Some(format!(
                    "The admission webhook of add-on {} on cluster {cluster_name} is not \
                     running, so the Kubernetes resources of add-on {addon_name} were denied \
                     at admission.",
                    denied.join(", ")
                )),
                resource_ids: Some(self.addon_resource_ids(&denied, now)),
            });
        }
        self.addon_replica_outcome(region, cluster_name, addon_name, namespace, now)
    }

    /// The Kubernetes resources of each of `addon_names`, as an `AddonIssue`
    /// names the resources an issue is about: `<namespace>/<add-on>`, the
    /// namespace being the one the catalog installs that add-on into.
    pub(super) fn addon_resource_ids(&self, addon_names: &[String], now: SystemTime) -> Vec<String> {
        addon_names
            .iter()
            .map(|addon_name| {
                let namespace = self
                    .published_addon(addon_name, now)
                    .map_or(ADDON_DEFAULT_NAMESPACE, |published| {
                        published.default_namespace.as_str()
                    });
                format!("{namespace}/{addon_name}")
            })
            .collect()
    }

    /// The state an add-on whose resources ARE on the cluster is in: running, or
    /// running without the replica it needs.
    ///
    /// An add-on with a replica to schedule needs the cluster to have compute to
    /// schedule it onto; without it the resources are applied but no pod of the
    /// add-on runs, which is a running add-on that is not healthy. An add-on that
    /// runs one pod per node has nothing to place, and so is healthy on a cluster
    /// with no nodes.
    ///
    /// It is the question an install asks last, and the one every later read asks
    /// again ([`Self::resettle_addon_replicas`]): an add-on reports what the
    /// cluster can do with the resources it carries, at the instant it is read.
    pub fn addon_replica_outcome(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        namespace: &str,
        now: SystemTime,
    ) -> AddonInstallOutcome {
        let replicated = self
            .published_addon(addon_name, now)
            .is_some_and(|published| published.workload == AddonWorkload::Replicated);
        if replicated && !self.cluster_can_place_pods(region, cluster_name, namespace) {
            return AddonInstallOutcome::degraded(AddonIssue {
                code: Some(ADDON_ISSUE_INSUFFICIENT_REPLICAS.to_string()),
                message: Some(ADDON_ISSUE_INSUFFICIENT_REPLICAS_MESSAGE.to_string()),
                // The issue is about the add-on's OWN pods, which are not
                // resources this emulator holds — it runs no Kubernetes — so it
                // names none. The issues that are about resources name them (see
                // [`Self::addon_install_outcome`]).
                resource_ids: None,
            });
        }
        AddonInstallOutcome::running()
    }

    /// The instant an install of `addon_name` on one cluster is owed its
    /// terminal state.
    ///
    /// One span ([`ADDON_ACTIVATION_DELAY`]) from now for an add-on that waits
    /// for nothing. An add-on cannot come up before the things it is applied
    /// against are themselves ready, so its span starts from the last of their
    /// due instants:
    ///
    /// * the CLUSTER it is installed on — an add-on of a cluster Amazon EKS is
    ///   still provisioning is recorded straight away
    ///   ([`CLUSTER_STATUSES_AN_ADDON_IS_MANAGED_IN`]) and settles once that
    ///   cluster is running;
    /// * each add-on it is applied against that is still settling.
    ///
    /// A dependency level therefore costs a span, which is what makes the
    /// install of a chain take one span per level.
    pub fn addon_install_settles_at(
        &self,
        region: &str,
        cluster_name: &str,
        addon_name: &str,
        now: SystemTime,
    ) -> SystemTime {
        let required = self
            .published_addon(addon_name, now)
            .map(|published| published.required_addons.as_slice())
            .unwrap_or_default();
        let last_prerequisite = required
            .iter()
            .filter_map(|required| self.installed_addon(region, cluster_name, required))
            .filter_map(|record| record.active_at)
            .max();
        let cluster_running = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
            .and_then(|cluster| cluster.active_at);
        let waits_for = [cluster_running, last_prerequisite]
            .into_iter()
            .flatten()
            .max();
        waits_for.unwrap_or(now).max(now) + ADDON_ACTIVATION_DELAY
    }

    /// Drop every add-on installed on one cluster.
    ///
    /// An add-on only exists inside its cluster, so whatever removes the cluster
    /// removes its add-ons with it — otherwise a cluster created again under the
    /// same name would inherit the add-ons of the one that is gone.
    pub fn remove_cluster_addons(&mut self, region: &str, cluster_name: &str) {
        self.addons.retain(|(key_region, key_cluster, _), _| {
            key_region != region || key_cluster != cluster_name
        });
    }

    /// The ARNs of the EKS Pod Identity associations this store HOLDS for one
    /// owner, which is what an add-on may report owning.
    ///
    /// The associations live in this store's own map, so this is the only place
    /// that can answer whether one still exists: a caller can delete an
    /// association directly, and an add-on must not go on reporting one that is
    /// gone (see [`AddonRecord::to_addon`]).
    pub fn association_arns_owned_by(&self, owner_arn: &str) -> ::std::collections::BTreeSet<String> {
        self.pod_identity_associations
            .values()
            .filter(|record| record.owner_arn.as_deref() == Some(owner_arn))
            .map(|record| record.association_arn.clone())
            .collect()
    }

    /// Drop the EKS Pod Identity associations one owner holds.
    ///
    /// An association an add-on owns exists only for that add-on
    /// (`PodIdentityAssociation$ownerArn`), so whatever stops the add-on owning
    /// it — the update that replaces the set it owns, or the delete that takes
    /// the add-on off its cluster — removes it through here, so both write the
    /// same thing.
    pub fn remove_associations_owned_by(&mut self, owner_arn: &str) {
        self.pod_identity_associations
            .retain(|_, record| record.owner_arn.as_deref() != Some(owner_arn));
    }

    /// Start taking one add-on off its cluster and answer the record as the
    /// delete reports it, or `None` when the cluster holds no add-on under that
    /// key.
    ///
    /// A removal is ASYNCHRONOUS: `DeleteAddon` answers with the add-on in
    /// `DELETING`, and the add-on is still an add-on of its cluster — readable,
    /// listed, and in the way of installing the same one again — until the clock
    /// reaches [`ADDON_DELETION_DELAY`] and [`Self::materialize_due_transitions`]
    /// drops it.
    ///
    /// Removing an add-on removes what Amazon EKS manages FOR it, which is the
    /// EKS Pod Identity associations it owns; they go as the removal starts,
    /// because Amazon EKS stops managing them the moment the delete is accepted.
    /// `preserve` is the request member that asks for the add-on software to stay
    /// on the cluster with Amazon EKS no longer managing its settings, and then
    /// "if an IAM account is associated with the add-on, it isn't removed" — so
    /// those associations are left in place.
    ///
    /// A change an update still in flight carries can no longer be made once the
    /// add-on it would change is being removed, so that update stops being owed
    /// its completion and reports the same status as any other change that could
    /// not be made (see [`Self::complete_addon_update`]).
    pub fn start_addon_removal(
        &mut self,
        key: &(String, String, String),
        preserve: bool,
        at: SystemTime,
    ) -> Option<AddonRecord> {
        if !preserve {
            let arn = self.addons.get(key)?.arn.clone();
            self.remove_associations_owned_by(&arn);
        }
        let pending = self.addons.get_mut(key)?.pending_update.take();
        if let Some(pending) = pending {
            if let Some(update) = self
                .clusters
                .get_mut(&(key.0.clone(), key.1.clone()))
                .and_then(|cluster| cluster.updates.get_mut(&pending.update_id))
            {
                update.status = UPDATE_FAILED.to_string();
                update.succeeds_at = None;
            }
        }
        let record = self.addons.get_mut(key)?;
        record.status = ADDON_STATUS_DELETING.to_string();
        // The add-on is on its way out, so the status move the clock owed its
        // install is no longer owed to anybody; what it is owed now is the end of
        // the removal.
        record.active_at = None;
        record.removed_at = Some(at + ADDON_DELETION_DELAY);
        record.modified_at = at;
        Some(record.clone())
    }

    /// The next value to mix into a minted update id, counted from this store's
    /// own total rather than from any one cluster's.
    pub fn next_update_sequence(&mut self) -> u64 {
        self.minted_updates = self.minted_updates.wrapping_add(1);
        self.minted_updates
    }

    /// The `(region, name)` key of the cluster `arn` identifies, if this store
    /// holds it.
    pub fn cluster_key_for_arn(&self, arn: &str) -> Option<(String, String)> {
        self.arn_index.get(arn).cloned()
    }

    /// The names of the clusters this store holds in `region`, in sorted order.
    ///
    /// The map is keyed `(region, name)`, so its iteration order already sorts
    /// each region's names; this only drops the other regions. EVERY cluster is
    /// counted, connected ones included, because what this feeds is the quota and
    /// the uniqueness of a name within a region — neither of which distinguishes
    /// how the cluster got there.
    pub fn cluster_names_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.clusters
            .keys()
            .filter(move |(key_region, _)| key_region == region)
            .map(|(_, name)| name.as_str())
    }

    /// The names of the node groups this store holds in `cluster_name` in
    /// `region`, in sorted order.
    ///
    /// The map is keyed `(region, cluster, nodegroup)`, so its iteration order
    /// already sorts each cluster's node group names; this only drops the other
    /// clusters and regions.
    pub fn nodegroup_names_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.nodegroups
            .keys()
            .filter(move |(key_region, key_cluster, _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, _, name)| name.as_str())
    }

    /// The node groups this store holds in `cluster_name` in `region`, in
    /// node-group-name order.
    ///
    /// The map is keyed `(region, cluster, nodegroup)`, so one cluster's node
    /// groups are a contiguous, name-sorted RANGE of it — the same walk
    /// [`Self::capabilities_of`] makes over its own child resource. This is the
    /// records; [`Self::nodegroup_names_in`] is the names.
    pub fn nodegroups_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a NodegroupRecord> + 'a {
        self.nodegroups
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// The add-ons installed on one cluster, in add-on-name order.
    ///
    /// The map is keyed `(region, cluster, addon)`, so one cluster's add-ons are a
    /// contiguous, name-sorted RANGE of it, the same walk [`Self::nodegroups_of`]
    /// makes over its own child resource. This is the records;
    /// [`Self::addon_names_in`] is the names.
    pub fn addons_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a AddonRecord> + 'a {
        self.addons
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// The managed node groups of one cluster whose Kubernetes version is not
    /// `cluster_version`, each as `name (version)`.
    ///
    /// "If your cluster has managed node groups attached to it, all of your node
    /// groups' Kubernetes versions must match the cluster's Kubernetes version in
    /// order to update the cluster to a new Kubernetes version"
    /// (`UpdateClusterVersion`), so this is the one walk both the operation that
    /// REFUSES such an update and the cluster's own upgrade-readiness insight read
    /// the mismatch from — rather than two copies of one rule that can drift.
    pub fn mismatched_nodegroup_versions(
        &self,
        region: &str,
        cluster_name: &str,
        cluster_version: &str,
    ) -> Vec<String> {
        self.nodegroups_of(region, cluster_name)
            .filter(|nodegroup| nodegroup.version != cluster_version)
            .map(|nodegroup| format!("{} ({})", nodegroup.nodegroup_name, nodegroup.version))
            .collect()
    }

    /// Every insight Amazon EKS checks for against one cluster, in
    /// [`InsightCheck::all`] order.
    ///
    /// COMPUTED from what this store holds — the cluster record, the node groups and
    /// add-ons inside it and the add-on catalog — and writing nothing: an insight is
    /// the result of a check, so `ListInsights` and `DescribeInsight` both report
    /// this and cannot disagree. A cluster this store does not hold has no insights
    /// at all, which is what the empty vector says.
    pub fn cluster_insights(&self, region: &str, cluster_name: &str) -> Vec<InsightView> {
        let Some(cluster) = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
        else {
            return Vec::new();
        };
        let last_refresh_time = cluster.insights_refreshed_at();
        InsightCheck::all()
            .into_iter()
            .map(|check| {
                let evaluated = self.evaluate_insight(check, region, cluster);
                InsightView {
                    id: derive_insight_id(&cluster.arn, check.key()),
                    check,
                    kubernetes_version: evaluated.kubernetes_version,
                    status: evaluated.status.to_string(),
                    reason: evaluated.reason,
                    last_refresh_time,
                    last_transition_time: evaluated.last_transition_time,
                }
            })
            .collect()
    }

    /// The resources one check of one cluster examined, each with the status it has
    /// against that check — `Insight$resources`, "The details about each resource
    /// listed in the insight check result".
    ///
    /// Only a check whose subject is a SET of resources inside the cluster lists any:
    /// the node group check lists the cluster's managed node groups and the add-on
    /// check its installed add-ons, each with its own ARN and its own status, so a
    /// caller reading one insight sees which resource the finding is about. A check
    /// whose subject is the cluster itself lists none — the cluster is not a resource
    /// listed INSIDE its own insight.
    pub fn insight_resources(
        &self,
        check: InsightCheck,
        region: &str,
        cluster_name: &str,
    ) -> Vec<InsightResourceDetail> {
        let Some(cluster) = self
            .clusters
            .get(&(region.to_string(), cluster_name.to_string()))
        else {
            return Vec::new();
        };
        // `kubernetesResourceUri` is "The Kubernetes resource URI if applicable": a
        // node group and an add-on are Amazon EKS resources addressed by ARN, not
        // objects of the Kubernetes API, so neither carries one.
        match check {
            InsightCheck::NodegroupVersions => {
                let Some(cluster_version) = cluster.version.as_deref() else {
                    return Vec::new();
                };
                self.nodegroups_of(region, &cluster.name)
                    .map(|nodegroup| {
                        let matches = nodegroup.version == cluster_version;
                        InsightResourceDetail {
                            insight_status: Some(InsightStatus {
                                status: Some(
                                    if matches {
                                        INSIGHT_STATUS_PASSING
                                    } else {
                                        INSIGHT_STATUS_ERROR
                                    }
                                    .to_string(),
                                ),
                                reason: Some(if matches {
                                    format!(
                                        "The node group runs the cluster's Kubernetes version \
                                         {cluster_version}."
                                    )
                                } else {
                                    format!(
                                        "The node group runs Kubernetes version {} instead of the \
                                         cluster's {cluster_version}.",
                                        nodegroup.version
                                    )
                                }),
                            }),
                            kubernetes_resource_uri: None,
                            arn: Some(nodegroup.arn.clone()),
                        }
                    })
                    .collect()
            }
            InsightCheck::AddonVersions => {
                let Some(target) = cluster
                    .version
                    .as_deref()
                    .and_then(next_kubernetes_minor_version)
                else {
                    return Vec::new();
                };
                self.addons_of(region, &cluster.name)
                    .map(|addon| {
                        let published = self
                            .addon_catalog
                            .iter()
                            .find(|entry| entry.addon_name == addon.addon_name)
                            .and_then(|entry| {
                                entry
                                    .versions
                                    .iter()
                                    .find(|build| build.addon_version == addon.addon_version)
                            });
                        let (status, reason) = match published {
                            Some(build) if !build.supports(&target) => (
                                INSIGHT_STATUS_ERROR,
                                format!(
                                    "Version {} of the add-on is not published for Kubernetes \
                                     version {target}.",
                                    addon.addon_version
                                ),
                            ),
                            Some(_) => (
                                INSIGHT_STATUS_PASSING,
                                format!(
                                    "Version {} of the add-on is published for Kubernetes version \
                                     {target}.",
                                    addon.addon_version
                                ),
                            ),
                            // A build the catalog does not publish is one whose
                            // compatibilities this service does not know, which is
                            // what `UNKNOWN` says.
                            None => (
                                INSIGHT_STATUS_UNKNOWN,
                                format!(
                                    "Version {} of the add-on is not one Amazon EKS publishes, so \
                                     the versions it supports are not known.",
                                    addon.addon_version
                                ),
                            ),
                        };
                        InsightResourceDetail {
                            insight_status: Some(InsightStatus {
                                status: Some(status.to_string()),
                                reason: Some(reason),
                            }),
                            kubernetes_resource_uri: None,
                            arn: Some(addon.arn.clone()),
                        }
                    })
                    .collect()
            }
            InsightCheck::HybridNodeNetworks | InsightCheck::VersionRollback => Vec::new(),
        }
    }

    /// Evaluate ONE check against one cluster.
    ///
    /// Each check reports the status its own subject justifies, the reason naming
    /// what it found in this store, the Kubernetes version the check is ABOUT (the
    /// version the cluster would move to for an upgrade check, the version it would
    /// return to for the rollback check, and none for a check that is about no
    /// version), and the instant that subject last changed — which is when the
    /// check's status can last have changed.
    pub(super) fn evaluate_insight(
        &self,
        check: InsightCheck,
        region: &str,
        cluster: &ClusterRecord,
    ) -> InsightEvaluation {
        // "A cluster moves one minor version per update", so the version an upgrade
        // check is about is the minor version right after the cluster's own.
        let upgrade_target = cluster
            .version
            .as_deref()
            .and_then(next_kubernetes_minor_version);
        match check {
            InsightCheck::NodegroupVersions => {
                // The node groups are the subject, so the check's status can only
                // have changed when one of them was created or last changed.
                let last_transition_time = self
                    .nodegroups_of(region, &cluster.name)
                    .map(|nodegroup| nodegroup.modified_at)
                    .chain(std::iter::once(cluster.created_at))
                    .max()
                    .unwrap_or(cluster.created_at);
                let Some(cluster_version) = cluster.version.as_deref() else {
                    return InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster reports no Kubernetes version, so its node groups \
                                 could not be compared against one."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    };
                };
                let mismatched =
                    self.mismatched_nodegroup_versions(region, &cluster.name, cluster_version);
                if mismatched.is_empty() {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "Every managed node group of the cluster runs Kubernetes version \
                             {cluster_version}."
                        ),
                        kubernetes_version: upgrade_target,
                        last_transition_time,
                    }
                } else {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_ERROR,
                        reason: format!(
                            "Managed node groups whose Kubernetes version does not match the \
                             cluster's {cluster_version}: {}.",
                            mismatched.join(", ")
                        ),
                        kubernetes_version: upgrade_target,
                        last_transition_time,
                    }
                }
            }
            InsightCheck::AddonVersions => {
                let last_transition_time = self
                    .addons_of(region, &cluster.name)
                    .map(|addon| addon.modified_at)
                    .chain(std::iter::once(cluster.created_at))
                    .max()
                    .unwrap_or(cluster.created_at);
                let Some(target) = upgrade_target else {
                    return InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster reports no Kubernetes version, so no add-on could be \
                                 checked against the version it would be updated to."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    };
                };
                // An installed build the catalog does not publish is not one this
                // service knows the compatibilities of, so it is not reported as
                // incompatible: only a build the catalog carries can fail the check.
                let incompatible: Vec<String> = self
                    .addons_of(region, &cluster.name)
                    .filter(|addon| {
                        self.addon_catalog
                            .iter()
                            .find(|entry| entry.addon_name == addon.addon_name)
                            .and_then(|entry| {
                                entry
                                    .versions
                                    .iter()
                                    .find(|build| build.addon_version == addon.addon_version)
                            })
                            .is_some_and(|build| !build.supports(&target))
                    })
                    .map(|addon| format!("{} ({})", addon.addon_name, addon.addon_version))
                    .collect();
                if incompatible.is_empty() {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "Every Amazon EKS add-on installed on the cluster is published for \
                             Kubernetes version {target}."
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    }
                } else {
                    InsightEvaluation {
                        status: INSIGHT_STATUS_ERROR,
                        reason: format!(
                            "Amazon EKS add-ons installed at a version that is not published for \
                             Kubernetes version {target}: {}.",
                            incompatible.join(", ")
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    }
                }
            }
            InsightCheck::HybridNodeNetworks => {
                let last_transition_time =
                    cluster.configuration_changed_at(UPDATE_TYPE_REMOTE_NETWORK_CONFIG);
                let remote = cluster.remote_network_config.as_ref();
                let has_node_networks = remote
                    .and_then(|config| config.remote_node_networks.as_ref())
                    .is_some_and(|networks| !networks.is_empty());
                let has_pod_networks = remote
                    .and_then(|config| config.remote_pod_networks.as_ref())
                    .is_some_and(|networks| !networks.is_empty());
                // No remote node networks means the cluster runs no hybrid nodes, so
                // there is no setup to find a misconfiguration in: the check has no
                // subject rather than a passing one.
                let (status, reason) = if !has_node_networks {
                    (
                        INSIGHT_STATUS_UNKNOWN,
                        "The cluster has no remote node networks, so no hybrid nodes setup was \
                         evaluated."
                            .to_string(),
                    )
                } else if !has_pod_networks {
                    (
                        INSIGHT_STATUS_WARNING,
                        "The cluster has remote node networks for hybrid nodes but no remote pod \
                         networks, so pods running on those nodes are not reachable from the rest \
                         of the cluster."
                            .to_string(),
                    )
                } else {
                    (
                        INSIGHT_STATUS_PASSING,
                        "The cluster has both remote node networks and remote pod networks \
                         configured for its hybrid nodes."
                            .to_string(),
                    )
                };
                InsightEvaluation {
                    status,
                    reason,
                    // A hybrid nodes misconfiguration is about the cluster's network
                    // configuration, not about a Kubernetes version.
                    kubernetes_version: None,
                    last_transition_time,
                }
            }
            InsightCheck::VersionRollback => {
                let latest_change = cluster.latest_version_change();
                let last_transition_time = latest_change
                    .map(|(_, changed_at)| changed_at.max(cluster.created_at))
                    .unwrap_or(cluster.created_at);
                let rollback_target = cluster
                    .version
                    .as_deref()
                    .and_then(previous_kubernetes_minor_version);
                match (latest_change, rollback_target) {
                    (Some((UPDATE_TYPE_VERSION, _)), Some(target)) => InsightEvaluation {
                        status: INSIGHT_STATUS_PASSING,
                        reason: format!(
                            "The cluster completed a Kubernetes version update, so its version \
                             can be rolled back to {target}."
                        ),
                        kubernetes_version: Some(target),
                        last_transition_time,
                    },
                    (Some((UPDATE_TYPE_VERSION_ROLLBACK, _)), _) => InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster's last completed Kubernetes version change was a \
                                 rollback, so it has no version update to roll back."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    },
                    _ => InsightEvaluation {
                        status: INSIGHT_STATUS_UNKNOWN,
                        reason: "The cluster has not completed a Kubernetes version update, so it \
                                 has no earlier version to roll back to."
                            .to_string(),
                        kubernetes_version: None,
                        last_transition_time,
                    },
                }
            }
        }
    }

    /// The capabilities one cluster holds, in capability-name order.
    ///
    /// The map is keyed `(region, cluster, capability)`, so one cluster's
    /// capabilities are a contiguous, name-sorted RANGE of it: the walk starts at
    /// that cluster's first possible key and stops at its last, the way
    /// [`Self::pod_identity_associations_in`] does for its own child resource.
    pub fn capabilities_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a CapabilityRecord> + 'a {
        self.capabilities
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// The EKS Anywhere subscriptions one Region holds, in subscription-id order.
    ///
    /// The map is keyed `(region, id)`, so one Region's subscriptions are a
    /// contiguous, id-sorted RANGE of it — the walk
    /// `ListEksAnywhereSubscriptions` pages and `CreateEksAnywhereSubscription`
    /// checks a name against.
    pub fn eks_anywhere_subscriptions_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a EksAnywhereSubscriptionRecord> + 'a {
        self.eks_anywhere_subscriptions
            .range((
                std::ops::Bound::Included((region.to_string(), String::new())),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, _), _)| key_region == region)
            .map(|(_, record)| record)
    }

    /// The names `ListClusters` projects for `region`, in sorted order.    ///
    /// A connected cluster is listed only when the caller asked for it:
    /// `include` "indicates whether external clusters are included in the
    /// returned list. Use 'all' to return connected clusters, or blank to return
    /// only Amazon EKS clusters" (`ListClustersRequest$include`).
    pub fn listed_cluster_names_in<'a>(
        &'a self,
        region: &'a str,
        include_connected: bool,
    ) -> impl Iterator<Item = &'a str> + 'a {
        self.clusters
            .iter()
            .filter(move |((key_region, _), record)| {
                key_region == region && (include_connected || !record.is_connected())
            })
            .map(|((_, name), _)| name.as_str())
    }

    /// Every EKS Pod Identity association of one cluster, in association-id order.
    ///
    /// The map is keyed `(region, clusterName, associationId)`, so one cluster's
    /// associations are a contiguous range of it — which is also the ascending
    /// id order a page token resumes after.
    pub fn pod_identity_associations_in<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a PodIdentityAssociationRecord> + 'a {
        self.pod_identity_associations
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// Drop every capability of one cluster, for a caller that is removing the
    /// cluster itself.
    ///
    /// A capability only exists inside a cluster — its ARN and its key both name
    /// one — so a cluster that leaves the store takes its capabilities with it.
    /// Without this, a later cluster created with the same name in the same region
    /// would inherit the dead cluster's capabilities.
    pub fn remove_capabilities_of(&mut self, region: &str, cluster_name: &str) {
        self.capabilities.retain(|(key_region, key_cluster, _), _| {
            key_region != region || key_cluster != cluster_name
        });
    }

    /// Mint an association id that no association of this cluster already holds.
    ///
    /// The seed carries the cluster, the pair the association is for and the
    /// store's monotonic mint counter, so a re-create after a delete mints a NEW
    /// id rather than resurrecting the old one. The loop is the collision guard:
    /// a digest can repeat, and an id must be unique within its cluster.
    pub fn mint_association_id(
        &mut self,
        region: &str,
        cluster_name: &str,
        namespace: &str,
        service_account: &str,
    ) -> String {
        loop {
            self.minted_associations = self.minted_associations.wrapping_add(1);
            let id = derive_association_id(&format!(
                "{region}/{cluster_name}/{namespace}/{service_account}/{}",
                self.minted_associations
            ));
            let key = (region.to_string(), cluster_name.to_string(), id.clone());
            if !self.pod_identity_associations.contains_key(&key) {
                return id;
            }
        }
    }

    /// Drop every EKS Pod Identity association of one cluster.
    ///
    /// An association is a child of its cluster: once the cluster is gone the
    /// associations are gone with it, so a cluster later created with the same
    /// name in the same Region does not inherit them.
    pub fn remove_pod_identity_associations_for_cluster(
        &mut self,
        region: &str,
        cluster_name: &str,
    ) {
        self.pod_identity_associations
            .retain(|(key_region, key_cluster, _), _| {
                key_region != region || key_cluster != cluster_name
            });
    }

    /// Drop every managed node group of one cluster, and answer the capacity those
    /// node groups ran on.
    ///
    /// A node group is a child of its cluster — it is addressed as
    /// `/clusters/{clusterName}/node-groups/{nodegroupName}` — so once the
    /// cluster is gone its node groups are gone with it, and a cluster later
    /// created with the same name in the same Region does not inherit them. The
    /// capacity each of them ran on is reported back because nothing holds it any
    /// more either, and it lives in other services for the caller of this to
    /// release ([`release_nodegroup_capacity`]).
    pub fn remove_nodegroups_for_cluster(
        &mut self,
        region: &str,
        cluster_name: &str,
    ) -> Vec<NodegroupCapacity> {
        let mut released = Vec::new();
        self.nodegroups
            .retain(|(key_region, key_cluster, _), nodegroup| {
                if key_region == region && key_cluster == cluster_name {
                    released.push(nodegroup.capacity.clone());
                    return false;
                }
                true
            });
        released
    }

    /// The Fargate profiles this store holds in `cluster_name` in `region`, in
    /// profile-name order.
    ///
    /// The map is keyed `(region, cluster, profile)`, so one cluster's profiles
    /// are a contiguous, name-sorted RANGE of it — the same walk
    /// [`Self::nodegroups_of`] makes over its own child resource.
    pub fn fargate_profiles_of<'a>(
        &'a self,
        region: &'a str,
        cluster_name: &'a str,
    ) -> impl Iterator<Item = &'a FargateProfileRecord> + 'a {
        self.fargate_profiles
            .range((
                std::ops::Bound::Included((
                    region.to_string(),
                    cluster_name.to_string(),
                    String::new(),
                )),
                std::ops::Bound::Unbounded,
            ))
            .take_while(move |((key_region, key_cluster, _), _)| {
                key_region == region && key_cluster == cluster_name
            })
            .map(|(_, record)| record)
    }

    /// Drop every Fargate profile of one cluster.
    ///
    /// A Fargate profile is a child of its cluster, so once the cluster is gone
    /// its profiles are gone with it, and a cluster later created with the same
    /// name in the same Region does not inherit them.
    pub fn remove_fargate_profiles_for_cluster(&mut self, region: &str, cluster_name: &str) {
        self.fargate_profiles
            .retain(|(key_region, key_cluster, _), _| {
                key_region != region || key_cluster != cluster_name
            });
    }

    /// Why the compute attached to one cluster keeps it from being deleted, or
    /// `None` when nothing is attached to it.
    ///
    /// "If you have managed node groups or Fargate profiles attached to the
    /// cluster, you must delete them first" (`DeleteCluster`): the cluster's
    /// control plane cannot be torn down while workloads it schedules still run,
    /// so a delete is refused while either child exists and the cluster stays as
    /// it was. The node groups are reported first, because they are the compute a
    /// cluster normally carries.
    ///
    /// The cluster's OTHER children — add-ons, capabilities, access entries, EKS
    /// Pod Identity associations, identity provider configurations — do not keep
    /// it: Amazon EKS deletes those with the cluster, which is what
    /// `DeleteCluster` already does here.
    pub fn attached_compute_refusal(&self, region: &str, cluster_name: &str) -> Option<&'static str> {
        if self.nodegroups_of(region, cluster_name).next().is_some() {
            return Some(CLUSTER_HAS_NODEGROUPS_ATTACHED);
        }
        if self.fargate_profiles_of(region, cluster_name).next().is_some() {
            return Some(CLUSTER_HAS_FARGATE_PROFILES_ATTACHED);
        }
        None
    }

    /// Record that the deletion of the cluster `key` names is in flight until the
    /// call that started it reports back, or until `failsafe` at the latest, which
    /// is what makes a `DeleteCluster` arriving in the meantime find a deletion
    /// rather than an absent cluster.
    ///
    /// Entries whose failsafe instant has already passed at `now` are dropped here
    /// as well as in [`Self::materialize_due_transitions`] — the same test
    /// [`Self::cluster_deletion_in_flight`] applies, so a deletion of ANOTHER
    /// cluster that is still genuinely in flight is left alone — which bounds the
    /// map at the deletions of the last [`CLUSTER_DELETION_DELAY`] however many
    /// clusters are deleted.
    pub fn start_cluster_deletion(
        &mut self,
        key: (String, String),
        now: SystemTime,
        failsafe: SystemTime,
    ) {
        self.cluster_deletions.retain(|_, at| now < *at);
        self.cluster_deletions.insert(key, failsafe);
    }

    /// Record that the deletion of the cluster `key` names has FINISHED: everything
    /// the cluster owned is released, so the cluster is now absent rather than
    /// being deleted and the next `DeleteCluster` for that name is a not-found.
    pub fn finish_cluster_deletion(&mut self, key: &(String, String)) {
        self.cluster_deletions.remove(key);
    }

    /// Whether the deletion of the cluster `key` names is still in flight at
    /// `now`.
    pub fn cluster_deletion_in_flight(&self, key: &(String, String), now: SystemTime) -> bool {
        self.cluster_deletions
            .get(key)
            .is_some_and(|due| now < *due)
    }
}
