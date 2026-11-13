// SPDX-License-Identifier: CC0-1.0

//! The IPAM stores: the IPAM itself, its scopes and pools, the resource discoveries it is
//! associated with, and the prefix-list resolvers and their targets.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Store a transit gateway attachment and answer the `TransitGatewayAttachmentId` it
    /// was given — or `None` when the store is full.
    ///
    /// The id is minted here rather than taken from `view`, the way
    /// [`Self::insert_vpc_endpoint_connection`] mints a connection id, and the
    /// caller-chosen bytes the record holds are charged here on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, so no path can store a
    /// record uncharged and a tag-free attachment is not free against the byte budget.
    ///
    /// `None` means the store is at its ceiling and NOTHING was stored; a writer that sees
    /// it refuses the request with EC2's own quota code rather than reporting an attachment
    /// it does not hold. The ceiling is enforced HERE as well as by
    /// [`Self::has_transit_gateway_attachment_capacity`], so a handler that forgets the
    /// gate cannot grow the store without bound either.
    pub fn insert_transit_gateway_attachment(
        &mut self,
        mut view: TransitGatewayAttachmentView,
    ) -> Option<String> {
        if !self.has_transit_gateway_attachment_capacity() {
            return None;
        }
        let attachment_id = self.next_id(TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);
        view.set_attachment_id(&attachment_id);
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + view.payload_bytes());
        self.transit_gateway_attachments.insert(
            attachment_id.clone(),
            TransitGatewayAttachmentRecord {
                view,
                pending: None,
            },
        );
        Some(attachment_id)
    }

    /// Store a transit gateway Connect attachment in the non-terminal state a create leaves
    /// it in, with the move to `available` the clock owes it, and answer the
    /// `TransitGatewayAttachmentId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    ///
    /// The id, the state and the owed move are minted HERE rather than taken from `view`, the
    /// way [`Self::insert_transit_gateway_connect_peer`] mints a peer's, so no path can report
    /// a Connect attachment in a state the store does not hold; the id and the byte charge are
    /// the shared [`Self::insert_transit_gateway_attachment`]'s, so a Connect attachment is
    /// stored and charged exactly like every other kind.
    pub fn insert_transit_gateway_connect(
        &mut self,
        mut view: TransitGatewayConnect,
    ) -> Option<String> {
        view.state = Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING.to_string());
        let attachment_id =
            self.insert_transit_gateway_attachment(TransitGatewayAttachmentView::Connect(view))?;
        if let Some(record) = self.transit_gateway_attachments.get_mut(&attachment_id) {
            record.pending = Some(PendingStateMove::starting_now(
                TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
            ));
        }
        Some(attachment_id)
    }

    /// Store a transit gateway VPC attachment in the non-terminal state a create leaves it
    /// in, with the move to `available` the clock owes it, and answer the
    /// `TransitGatewayAttachmentId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    ///
    /// The id, the state and the owed move are minted HERE rather than taken from `view`, on
    /// the contract of [`Self::insert_transit_gateway_connect`], so no path can report a VPC
    /// attachment in a state the store does not hold; the id and the byte charge are the
    /// shared [`Self::insert_transit_gateway_attachment`]'s, so a VPC attachment is stored
    /// and charged exactly like every other kind.
    ///
    /// An attachment carries traffic only once the transit gateway it is on carries traffic,
    /// so the move it owes falls due one [`TRANSITION_DELAY`] after the gateway's own move to
    /// `available` is due ([`Self::transit_gateway_available_at`]) — and one span after this
    /// call for a gateway that is in service already. Nothing is slept for either way.
    pub fn insert_transit_gateway_vpc_attachment(
        &mut self,
        mut view: TransitGatewayVpcAttachment,
    ) -> Option<String> {
        view.state = Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING.to_string());
        let now = Instant::now();
        let gateway_ready_at = view
            .transit_gateway_id
            .as_deref()
            .and_then(|transit_gateway_id| self.transit_gateway_available_at.get(transit_gateway_id))
            .copied()
            .unwrap_or(now);
        let due_at = now.max(gateway_ready_at) + TRANSITION_DELAY;
        let attachment_id =
            self.insert_transit_gateway_attachment(TransitGatewayAttachmentView::Vpc(view))?;
        if let Some(record) = self.transit_gateway_attachments.get_mut(&attachment_id) {
            record.pending = Some(PendingStateMove {
                to: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string(),
                due_at,
            });
        }
        Some(attachment_id)
    }

    /// Store a transit gateway multicast domain and answer the
    /// `TransitGatewayMulticastDomainId` it was given — or `None` when the store is at its
    /// ceiling and nothing was stored.
    pub fn insert_transit_gateway_multicast_domain(
        &mut self,
        mut record: TransitGatewayMulticastDomainRecord,
    ) -> Option<String> {
        if !self.has_transit_gateway_multicast_domain_capacity() {
            return None;
        }
        let domain_id = self.next_id(TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX);
        record.transit_gateway_multicast_domain_id = domain_id.clone();
        // The caller-chosen bytes a domain holds are the ids of the associations written
        // with it, charged here like every other writer's — on top of the fixed overhead
        // the record itself costs — so no path can store them uncharged and a domain with
        // no association is not free against the byte budget either.
        let association_bytes: usize = record
            .associations
            .iter()
            .map(|association| {
                association.transit_gateway_attachment_id.len()
                    + association.resource_id.len()
                    + association.subnet_id.len()
            })
            .sum();
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + association_bytes);
        self.transit_gateway_multicast_domains
            .insert(domain_id.clone(), record);
        Some(domain_id)
    }

    /// Store a transit gateway route table announcement and answer the
    /// `TransitGatewayRouteTableAnnouncementId` it was given — or `None` when the store is
    /// at its ceiling and nothing was stored.
    pub fn insert_transit_gateway_route_table_announcement(
        &mut self,
        mut view: TransitGatewayRouteTableAnnouncement,
    ) -> Option<String> {
        if !self.has_transit_gateway_route_table_announcement_capacity() {
            return None;
        }
        let announcement_id = self.next_id(TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX);
        view.transit_gateway_route_table_announcement_id = Some(announcement_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.transit_gateway_route_table_announcements.insert(
            announcement_id.clone(),
            TransitGatewayRouteTableAnnouncementRecord {
                view,
                pending: None,
            },
        );
        Some(announcement_id)
    }

    /// Store one of the account's own VPC endpoint service configurations and answer the
    /// `ServiceId` it was given — or `None` when the store is at its ceiling and nothing
    /// was stored.
    pub fn insert_vpc_endpoint_service(
        &mut self,
        mut view: ServiceConfiguration,
    ) -> Option<String> {
        if !self.has_vpc_endpoint_service_capacity() {
            return None;
        }
        let service_id = self.next_id(VPC_ENDPOINT_SERVICE_ID_PREFIX);
        view.service_id = Some(service_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.vpc_endpoint_services.insert(service_id.clone(), view);
        Some(service_id)
    }

    /// Store a VPC endpoint and answer the `VpcEndpointId` it was given — or `None` when
    /// the store is at its ceiling and nothing was stored.
    pub fn insert_vpc_endpoint(&mut self, mut view: VpcEndpoint) -> Option<String> {
        if !self.has_vpc_endpoint_capacity() {
            return None;
        }
        let endpoint_id = self.next_id(VPC_ENDPOINT_ID_PREFIX);
        view.vpc_endpoint_id = Some(endpoint_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.vpc_endpoints.insert(endpoint_id.clone(), view);
        Some(endpoint_id)
    }

    /// Store an IPAM pool and answer the `IpamPoolId` it was given — or `None` when the
    /// store is at its ceiling and nothing was stored.
    ///
    /// "The Amazon Resource Name (ARN) of the IPAM pool" (`IpamPool$IpamPoolArn`) is built by
    /// the one writer of every IPAM-family ARN ([`ipam_resource_arn`]), so a pool's ARN is
    /// spelled the way the scope's and the IPAM's own are — with an EMPTY Region segment, the
    /// pool's Region being reported in `IpamPool$IpamRegion` instead.
    pub fn insert_ipam_pool(&mut self, mut view: IpamPool) -> Option<String> {
        if !self.has_ipam_pool_capacity() {
            return None;
        }
        let pool_id = self.next_id(IPAM_POOL_ID_PREFIX);
        view.ipam_pool_id = Some(pool_id.clone());
        view.ipam_pool_arn = Some(ipam_resource_arn(RESOURCE_TYPE_IPAM_POOL, &pool_id));
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default())
                + view.description.as_deref().unwrap_or_default().len(),
        );
        self.ipam_pools.insert(pool_id.clone(), view);
        Some(pool_id)
    }

    /// Whether another IPAM scope fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]) — the same contract as
    /// [`Self::has_ipam_pool_capacity`].
    pub fn has_ipam_scope_capacity(&self) -> bool {
        self.ipam_scopes.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an IPAM scope and answer the `IpamScopeId` it was given — or `None` when the
    /// store is at its ceiling and nothing was stored.
    ///
    /// The ONE way a scope is stored, so the members the SERVICE owns cannot be built two
    /// ways: the id, the ARN it reports, the account that owns it and the number of pools in
    /// it are all derived here — a scope nothing has created a pool in holds none — rather
    /// than taken from a caller, exactly as [`Self::insert_ipam_pool`] derives a pool's.
    pub fn insert_ipam_scope(&mut self, mut view: IpamScope) -> Option<String> {
        if !self.has_ipam_scope_capacity() {
            return None;
        }
        let scope_id = self.next_id(IPAM_SCOPE_ID_PREFIX);
        view.ipam_scope_id = Some(scope_id.clone());
        // "The Amazon Resource Name (ARN) of the scope" (`IpamScope$IpamScopeArn`): an EC2
        // IPAM-family ARN under this resource's modelled type, whose Region segment is empty
        // for the reason [`ipam_resource_arn`] records — the scope's Region is reported in
        // `IpamScope$IpamRegion` instead.
        view.ipam_scope_arn = Some(ipam_resource_arn(RESOURCE_TYPE_IPAM_SCOPE, &scope_id));
        // "The Amazon Web Services account ID of the owner of the scope"
        // (`IpamScope$OwnerId`): this emulator serves one account.
        view.owner_id = Some(::engine::emulator_account_id().to_string());
        // "The number of pools in the scope" (`IpamScope$PoolCount`), which
        // [`Self::recount_ipam_scope_pools`] keeps as pools are created in it.
        view.pool_count = Some(0);
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default())
                + view.description.as_deref().unwrap_or_default().len(),
        );
        self.ipam_scopes.insert(scope_id.clone(), view);
        Some(scope_id)
    }

    /// Store an IPAM scope a create has just made in the IPAM `ipam_id` names, and answer
    /// the `IpamScopeId` it was given — or `None` when the store is at its ceiling and
    /// nothing was stored.
    ///
    /// The ONE way a CREATED scope is stored, the counterpart of
    /// [`Self::insert_creating_ipam_pool`] for the container a pool lives in: the scope is
    /// answered in the non-terminal `create-in-progress` state, so the clock owes it
    /// [`IPAM_STATE_CREATE_COMPLETE`] one [`TRANSITION_DELAY`] later
    /// ([`Self::ipam_scope_create_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]); `client_token` names the scope from here on so
    /// a replay of the same request answers it instead of creating a second one; the IPAM's
    /// own count of the scopes in it is recounted; and every caller-chosen byte the record
    /// retains — the external authority's resource identifier and the token beside what
    /// [`Self::insert_ipam_scope`] already charges — is charged to the store's budget.
    pub fn insert_created_ipam_scope(
        &mut self,
        ipam_id: &str,
        client_token: Option<&str>,
        mut view: IpamScope,
    ) -> Option<String> {
        view.state = Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string());
        // "Defines if the scope is the default scope or not" (`IpamScope$IsDefault`): only
        // the two scopes an IPAM comes up with are default ones, so a scope a caller asked
        // for is not.
        view.is_default = Some(false);
        let retained = client_token.unwrap_or_default().len()
            + view
                .external_authority_configuration
                .as_ref()
                .and_then(|configuration| configuration.external_resource_identifier.as_deref())
                .unwrap_or_default()
                .len();
        let scope_id = self.insert_ipam_scope(view)?;
        self.add_stored_bytes(retained);
        self.ipam_scope_create_complete_at
            .insert(scope_id.clone(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.ipam_scope_client_tokens
                .insert(token.to_string(), scope_id.clone());
        }
        self.recount_ipam_scopes(ipam_id);
        Some(scope_id)
    }

    /// Set the IPAM's `ScopeCount` to the number of scopes the store now holds in it.
    ///
    /// "The number of scopes in the IPAM" (`Ipam$ScopeCount`) is DERIVED from the scopes
    /// themselves, so it is recounted from them rather than incremented — the same contract
    /// [`Self::recount_ipam_scope_pools`] has for a scope's pool count. A scope names its
    /// IPAM by `IpamArn` (the modelled [`IpamScope`] carries no IPAM id at all), which is
    /// what the scopes of this IPAM are counted by.
    ///
    /// A scope whose deletion has been asked for ([`ipam_scope_is_deleting`]) is not one of
    /// them: the record stays in the store until the clock removes it so its state can still
    /// be reported, but the IPAM no longer HOLDS it — exactly the reading
    /// [`Self::recount_ipam_scope_pools`] has for a pool a delete has acted on.
    pub(super) fn recount_ipam_scopes(&mut self, ipam_id: &str) {
        let Some(ipam_arn) = self.ipams.get(ipam_id).and_then(|ipam| ipam.ipam_arn.clone()) else {
            return;
        };
        let scopes = self
            .ipam_scopes
            .values()
            .filter(|scope| {
                scope.ipam_arn.as_deref() == Some(ipam_arn.as_str())
                    && !ipam_scope_is_deleting(scope)
            })
            .count();
        if let Some(ipam) = self.ipams.get_mut(ipam_id) {
            ipam.scope_count = i32::try_from(scopes).ok();
        }
    }

    /// Store an IPAM pool a create has just made in the scope `scope_id` names, and answer
    /// the `IpamPoolId` it was given — or `None` when the store is at its ceiling and
    /// nothing was stored.
    ///
    /// The ONE way a CREATED pool is stored, so no path can leave the store disagreeing with
    /// itself about one: the pool is answered in the non-terminal `create-in-progress` state,
    /// so the clock owes it [`IPAM_POOL_STATE_CREATE_COMPLETE`] one [`TRANSITION_DELAY`]
    /// later ([`Self::ipam_pool_create_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]); `client_token` names the pool from here on so
    /// a replay of the same request answers it instead of creating a second one; the scope's
    /// own count of the pools in it is recounted; and every caller-chosen byte the record
    /// retains — the allocation resource tags and the token beside what
    /// [`Self::insert_ipam_pool`] already charges — is charged to the store's budget.
    pub fn insert_creating_ipam_pool(
        &mut self,
        scope_id: &str,
        client_token: Option<&str>,
        mut view: IpamPool,
    ) -> Option<String> {
        view.state = Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS.to_string());
        let retained = ipam_resource_tag_list_bytes(
            view.allocation_resource_tags.as_deref().unwrap_or_default(),
        ) + client_token.unwrap_or_default().len();
        let pool_id = self.insert_ipam_pool(view)?;
        self.add_stored_bytes(retained);
        self.ipam_pool_create_complete_at
            .insert(pool_id.clone(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.ipam_pool_client_tokens
                .insert(token.to_string(), pool_id.clone());
        }
        self.recount_ipam_scope_pools(scope_id);
        Some(pool_id)
    }

    /// Set the scope's `PoolCount` to the number of pools the store now holds in it.
    ///
    /// "The number of pools in the scope" (`IpamScope$PoolCount`) is DERIVED from the pools
    /// themselves, so it is recounted from them rather than incremented — two writers cannot
    /// then leave the scope claiming a pool count the store does not hold. A pool names its
    /// scope by `IpamScopeArn` (the modelled [`IpamPool`] carries no scope id at all), which
    /// is what the pools of this scope are counted by.
    ///
    /// A pool whose deletion has been asked for ([`ipam_pool_is_deleting`]) is not one of
    /// them: the record stays in the store so its state can still be reported, but the scope
    /// no longer HOLDS it, so a delete recounts the scope exactly as a create does.
    pub(super) fn recount_ipam_scope_pools(&mut self, scope_id: &str) {
        let Some(scope_arn) = self
            .ipam_scopes
            .get(scope_id)
            .and_then(|scope| scope.ipam_scope_arn.clone())
        else {
            return;
        };
        let pools = self
            .ipam_pools
            .values()
            .filter(|pool| {
                pool.ipam_scope_arn.as_deref() == Some(scope_arn.as_str())
                    && !ipam_pool_is_deleting(pool)
            })
            .count();
        if let Some(scope) = self.ipam_scopes.get_mut(scope_id) {
            scope.pool_count = i32::try_from(pools).ok();
        }
    }

    /// The `IpamScopeId` of the scope the pool `pool_id` lives in, found by the `IpamScopeArn`
    /// the pool names it by — the modelled [`IpamPool`] carries no scope id at all.
    pub(super) fn ipam_scope_id_of_pool(&self, pool_id: &str) -> Option<String> {
        let scope_arn = self.ipam_pools.get(pool_id)?.ipam_scope_arn.clone()?;
        self.ipam_scopes
            .iter()
            .find(|(_, scope)| scope.ipam_scope_arn.as_deref() == Some(scope_arn.as_str()))
            .map(|(scope_id, _)| scope_id.clone())
    }

    /// The pools created WITHIN the pool `pool_id` names, deepest first, excluding any whose
    /// own deletion has already been asked for.
    ///
    /// "The ID of the source IPAM pool. You can use this option to create an IPAM pool within
    /// an existing source pool" (`IpamPool$SourceIpamPoolId`) is what makes one pool a pool
    /// inside another, so the tree is walked by that member — transitively, because a pool
    /// inside a pool inside this one is still "within that pool"
    /// (`DeleteIpamPoolRequest$Cascade`). Deepest first, so a caller acting on the answer
    /// deletes a child before its parent.
    ///
    /// Each pool is visited once, so the walk is bounded by the number of pools the store
    /// holds ([`MAX_STORED_RECORDS_PER_STORE`]) however the `SourceIpamPoolId` members point
    /// at one another.
    pub fn ipam_pools_within(&self, pool_id: &str) -> Vec<String> {
        let mut frontier = vec![pool_id.to_string()];
        let mut seen: BTreeSet<String> = BTreeSet::from([pool_id.to_string()]);
        let mut within: Vec<String> = Vec::new();
        while let Some(parent) = frontier.pop() {
            for child in self
                .ipam_pools
                .iter()
                .filter(|(_, pool)| {
                    pool.source_ipam_pool_id.as_deref() == Some(parent.as_str())
                        && !ipam_pool_is_deleting(pool)
                })
                .map(|(child_id, _)| child_id.clone())
            {
                if !seen.insert(child.clone()) {
                    continue;
                }
                frontier.push(child.clone());
                within.push(child);
            }
        }
        // The walk above appends a parent before its own children, so reversing it answers
        // the deepest pools first.
        within.reverse();
        within
    }

    /// The CIDRs provisioned to the pool `pool_id` names, as the store spells each range.
    ///
    /// A range `ProvisionIpamPoolCidr` provisioned to the pool is one of them
    /// ([`Self::ipam_pool_cidr_records`]), and so is a BYOIP address range IPAM manages,
    /// which names the pool managing it (`ByoipCidr$IpamPoolId`) and which
    /// `MoveByoipCidrToIpam` is what writes. Together they are the CIDRs provisioned to a pool
    /// that "you cannot delete an IPAM pool if there are … provisioned to it"
    /// (`DeleteIpamPool`) speaks of, which is why this is the ONE reader of that question: a
    /// pool cannot be judged empty by one path and occupied by another.
    ///
    /// A range whose deprovisioning has already been asked for is not one
    /// ([`ipam_pool_cidr_holds_space`]).
    pub fn ipam_pool_provisioned_cidrs(&self, pool_id: &str) -> Vec<String> {
        let mut provisioned: Vec<String> = self
            .ipam_pool_cidr_records(pool_id)
            .into_iter()
            .filter(|record| ipam_pool_cidr_holds_space(record.view.state.as_deref()))
            .filter_map(|record| record.view.cidr.clone())
            .collect();
        provisioned.extend(
            self.byoip_cidrs
                .iter()
                .filter(|(_, record)| record.byoip_cidr.ipam_pool_id.as_deref() == Some(pool_id))
                .map(|(cidr, _)| cidr.clone()),
        );
        provisioned.sort();
        provisioned.dedup();
        provisioned
    }

    /// Deprovision every CIDR provisioned to the pool `pool_id` names, and answer how many
    /// ranges that was.
    ///
    /// What `DeleteIpamPool`'s `Cascade` — "including provisioned CIDRs" — and `DeleteIpam`'s
    /// do to a pool's ranges: every range the pool holds, through the ONE reader of that
    /// question above and the ONE writer that takes one back
    /// ([`Self::deprovision_ipam_pool_cidr`]), so a cascade and an explicit
    /// `DeprovisionIpamPoolCidr` leave a range in exactly the same state.
    pub fn deprovision_ipam_pool_cidrs(&mut self, pool_id: &str) -> usize {
        let provisioned = self.ipam_pool_provisioned_cidrs(pool_id);
        for cidr in &provisioned {
            self.deprovision_ipam_pool_cidr(pool_id, cidr);
        }
        provisioned.len()
    }

    /// Start deleting the IPAM pool `pool_id` names, and answer it as the store then holds it
    /// — `None` when the account holds no such pool.
    ///
    /// "Delete an IPAM pool" is work AWS reports as under way: the `IpamPoolState` catalog
    /// carries both `delete-in-progress` and `delete-complete`, so the pool is left in the
    /// TRANSIENT state with the clock owing it the terminal one
    /// ([`Self::ipam_pool_delete_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]) — the same arrangement, on the same clock, as
    /// the `create-in-progress` the pool started in. The span is never zero, so a caller
    /// acting straight after this call still sees `delete-in-progress`.
    ///
    /// The record STAYS in the store, because a pool in either delete state still has a state
    /// to report ([`IPAM_POOL_STATE_DELETE_COMPLETE`]) — so its caller-chosen bytes stay
    /// charged to the store's budget, which still holds them, the same arrangement as
    /// [`Self::begin_ipam_policy_deletion`]. The scope it was in no longer HOLDS it, so its
    /// "number of pools in the scope" is recounted exactly as a create recounts it.
    pub fn begin_ipam_pool_deletion(&mut self, pool_id: &str) -> Option<IpamPool> {
        let scope_id = self.ipam_scope_id_of_pool(pool_id);
        let pool = self.ipam_pools.get_mut(pool_id)?;
        pool.state = Some(IPAM_POOL_STATE_DELETE_IN_PROGRESS.to_string());
        let view = pool.clone();
        // A pool on its way out is not going to land in the state its creation was moving it
        // to, so the delete supersedes whatever move was owed it.
        self.ipam_pool_create_complete_at.remove(pool_id);
        self.ipam_pool_delete_complete_at
            .insert(pool_id.to_string(), Instant::now() + TRANSITION_DELAY);
        if let Some(scope_id) = scope_id {
            self.recount_ipam_scope_pools(&scope_id);
        }
        Some(view)
    }
}

// =============================================================================
// IPAM
// =============================================================================

/// The id prefix a stored IPAM is minted with.
///
/// The model publishes no pattern for `IpamId` (`smithy_query(query="shape",shape="Ipam")`
/// gives a plain `String` target), so the id takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`], with the prefix AWS gives the resource.
pub const IPAM_ID_PREFIX: &str = "ipam";

/// The id prefix a stored IPAM resource discovery is minted with — see [`IPAM_ID_PREFIX`].
pub const IPAM_RESOURCE_DISCOVERY_ID_PREFIX: &str = "ipam-res-disco";

/// The id prefix a stored IPAM resource discovery association is minted with — see
/// [`IPAM_ID_PREFIX`].
pub const IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX: &str = "ipam-res-disco-assoc";

/// The ARN resource type of an IPAM, which is also the `ResourceType` a `TagSpecification`
/// names to tag one.
pub const RESOURCE_TYPE_IPAM: &str = "ipam";

/// The ARN resource type of an IPAM resource discovery.
pub const RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY: &str = "ipam-resource-discovery";

/// The ARN resource type of an IPAM resource discovery association.
pub const RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION: &str =
    "ipam-resource-discovery-association";

/// The ARN of one IPAM resource: `arn:aws:ec2::<account>:<resource_type>/<id>`.
///
/// The Region segment is EMPTY, unlike every other EC2 ARN this crate builds: the Service
/// Authorization Reference spells the EC2 `ipam` resource
/// `arn:${Partition}:ec2::${Account}:ipam/${IpamId}`, and its `ipam-scope`,
/// `ipam-resource-discovery` and `ipam-resource-discovery-association` resources the same
/// way, an IPAM being a multi-Region resource whose home Region is reported in
/// `Ipam$IpamRegion` instead. [`::engine::emulator_arn`] takes an empty `region` for exactly
/// this ("Global-service ARNs simply pass an empty `region`").
pub fn ipam_resource_arn(resource_type: &str, id: &str) -> String {
    ::engine::emulator_arn("ec2", "", &format!("{resource_type}/{id}"))
}

/// An IPAM, scope or resource discovery AWS is still creating: the `create-in-progress`
/// member shared by `IpamState`, `IpamScopeState` and `IpamResourceDiscoveryState`.
pub const IPAM_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// The state those three enums reach once the creation is done: `create-complete`.
pub const IPAM_STATE_CREATE_COMPLETE: &str = "create-complete";

/// An IPAM whose configuration a `ModifyIpam` is still applying: the `IpamState` enum's
/// `modify-in-progress` member.
pub const IPAM_STATE_MODIFY_IN_PROGRESS: &str = "modify-in-progress";

/// The state an IPAM reaches once that modification is applied: `modify-complete`.
pub const IPAM_STATE_MODIFY_COMPLETE: &str = "modify-complete";

/// A resource discovery association AWS is still making: the
/// `IpamResourceDiscoveryAssociationState` enum's `associate-in-progress` member.
pub const IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS: &str = "associate-in-progress";

/// The state an association reaches once it is made: `associate-complete`.
pub const IPAM_ASSOCIATION_STATE_ASSOCIATE_COMPLETE: &str = "associate-complete";

/// "`active` - Connection or permissions required to read the results of the resource
/// discovery are intact" (`IpamResourceDiscoveryAssociation$ResourceDiscoveryStatus`): the
/// status of a discovery the IPAM owns itself, which nothing can stop sharing.
pub const IPAM_RESOURCE_DISCOVERY_STATUS_ACTIVE: &str = "active";

/// The tier an IPAM that names none gets: "IPAM is offered in a Free Tier and an Advanced
/// Tier" (`IpamTier`, one of `free` | `advanced` — a value outside it is refused by the
/// boundary validator, so only the DEFAULT is a decision this crate makes), and the Free
/// Tier is the one an account has without asking.
pub const DEFAULT_IPAM_TIER: &str = "free";

/// The metered account an IPAM that names none gets: "`ipam-owner` (default): The Amazon
/// Web Services account which owns the IPAM is charged for all active IP addresses managed
/// in IPAM" (`CreateIpamRequest$MeteredAccount`). The other member of `IpamMeteredAccount`
/// is `resource-owner`, and a value outside the enum is refused by the boundary validator.
pub const DEFAULT_IPAM_METERED_ACCOUNT: &str = "ipam-owner";

/// How many scopes a freshly created IPAM has: "An IPAM contains two default scopes"
/// (`IpamScope`), the public one and the private one, and nothing else has created a scope
/// in it yet.
pub const DEFAULT_IPAM_SCOPE_COUNT: i32 = 2;

/// How many resource discoveries a freshly created IPAM has associated: "When you create an
/// IPAM, a default resource discovery is created for your IPAM and it's associated with your
/// IPAM" (`IpamResourceDiscoveryAssociation$IsDefault`) — exactly that one.
pub const DEFAULT_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_COUNT: i32 = 1;

/// How many IPAMs this emulator stores.
///
/// A ceiling of this store, not a published AWS quota: like [`MAX_VPCS`] it exists so
/// repeated creates cannot grow memory without bound, and a create asks
/// [`ComAmazonawsEc2Amazonec2State::has_ipam_capacity`] before inserting.
pub const MAX_IPAMS: usize = 1_000;

/// `IpamLimitExceeded`: the create would take the account past the IPAMs it may hold.
pub const IPAM_LIMIT_EXCEEDED: &str = "IpamLimitExceeded";

/// `InvalidIpamId.NotFound`: the request named an IPAM id this account holds no IPAM for.
pub const INVALID_IPAM_ID_NOT_FOUND: &str = "InvalidIpamId.NotFound";

/// `InvalidIpamId.Malformed`: the request named something that is not an IPAM id at all, so
/// there is nothing to look up — the counterpart of [`INVALID_IPAM_SCOPE_ID_MALFORMED`] for
/// the IPAM itself.
pub const INVALID_IPAM_ID_MALFORMED: &str = "InvalidIpamId.Malformed";

/// Refuse a create that would take the store past [`MAX_IPAMS`]: HTTP 400
/// `IpamLimitExceeded`.
pub fn ipam_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        IPAM_LIMIT_EXCEEDED,
        400,
        format!("This emulator holds {MAX_IPAMS} IPAMs, which is as many as it stores"),
    )
}

/// Refuse a request that named an IPAM the account holds none of: HTTP 400
/// `InvalidIpamId.NotFound`.
///
/// The wording is this resource's own — "The IPAM (…) does not exist.", the sentence AWS
/// answers a request naming an IPAM that does not exist with, built from the id the REQUEST
/// named. The parenthesised id is why this resource does not go through [`id_not_found`],
/// whose sentence quotes the id instead; [`invalid_ipam_scope_id_not_found`] words the
/// scope's own refusal the same way.
pub fn invalid_ipam_id_not_found<E>(ipam_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPAM_ID_NOT_FOUND,
        400,
        format!("The IPAM ({ipam_id}) does not exist."),
    )
}

/// The Region names one operating-Region list of a request carries, validated and as a SET.
///
/// The ONE reader every operation of this crate resolving an `AddIpamOperatingRegionSet` or a
/// `RemoveIpamOperatingRegionSet` goes through — `CreateIpam`'s `OperatingRegions` and
/// `ModifyIpam`'s `AddOperatingRegions` and `RemoveOperatingRegions` — so no two of them can
/// disagree about which Region names an IPAM may operate in, the same contract
/// [`tags_from_specifications`] has for a tag payload. `member` is the parameter the refusal
/// names, since that is all that differs between them.
///
/// "The name of the operating Region" (`AddIpamOperatingRegion$RegionName` /
/// `RemoveIpamOperatingRegion$RegionName`) is the only member either shape has, so an entry
/// without one names no Region at all and is refused with `MissingParameter`. A name that IS
/// given is held to the CATALOG of Regions of the partition this emulator's ARNs name
/// ([`commercial_regions`]) rather than to the grammar, the way an instance type is held to
/// the offered catalog: a Region AWS does not publish can neither be given to an IPAM at
/// creation, added to one, nor removed from one. Operating Regions are a SET
/// (`Ipam$OperatingRegions`), so a name repeated within one list says nothing the first
/// mention does not and is kept once, in the order the request named them.
pub fn ipam_operating_region_names<'a, E>(
    requested: impl IntoIterator<Item = Option<&'a str>>,
    member: &str,
) -> Result<Vec<String>, Scaffolded<E>> {
    let catalog = commercial_regions();
    let mut regions: Vec<String> = Vec::new();
    for region_name in requested {
        let Some(region_name) = region_name.filter(|name| !name.is_empty()) else {
            return Err(missing_parameter(
                "An operating region must have a region name",
            ));
        };
        if !catalog.contains(&region_name) {
            return Err(invalid_parameter_value(format!(
                "Value ({region_name}) for parameter {member} is invalid. \
                 This is not a valid Region name."
            )));
        }
        if !regions.iter().any(|held| held == region_name) {
            regions.push(region_name.to_string());
        }
    }
    Ok(regions)
}

/// What one stored IPAM is created with, beside everything the store derives itself (every
/// id, every ARN, the states, the scope and association counts).
///
/// The spec of the resource, in the same shape [`VpcEndpointConnectionSpec`] has for a
/// connection: `CreateIpam` fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_ipam`] turns it into the modelled [`Ipam`] the
/// describe reports — plus the two default scopes, the default resource discovery and that
/// discovery's association — so no two paths can create an IPAM two ways.
#[derive(Debug, Clone)]
pub struct IpamSpec {
    /// The IPAM's home Region: the Region the creating request was signed for
    /// (`Ipam$IpamRegion`).
    pub region: String,
    /// "A description for the IPAM" (`CreateIpamRequest$Description`), or `None`.
    pub description: Option<String>,
    /// The Region names of `Ipam$OperatingRegions`, already validated and deduplicated by
    /// the caller, including the home Region.
    pub operating_regions: Vec<String>,
    /// The `IpamTier` the IPAM is in.
    pub tier: String,
    /// The `IpamMeteredAccount` the IPAM meters to.
    pub metered_account: String,
    /// "Enable this option to use your own GUA ranges as private IPv6 addresses"
    /// (`Ipam$EnablePrivateGua`).
    pub enable_private_gua: bool,
    /// The tags of an `ipam` tag specification.
    pub tags: Vec<Tag>,
    /// The request's `ClientToken`, which [`ComAmazonawsEc2Amazonec2State::insert_ipam`]
    /// records against the IPAM it mints so a replay of the same request answers with that
    /// IPAM, or `None` when the request carried none.
    pub client_token: Option<String>,
}

/// The bytes one `CreateIpam` adds to the store THROUGH
/// [`ComAmazonawsEc2Amazonec2State::insert_ipam`] itself: the caller-chosen ones — its
/// description, the Region names it operates in, its tags and the client token the token map
/// keeps — plus the fixed [`STORED_RECORD_OVERHEAD_BYTES`] each of the three records that
/// writer inserts costs. Every other member is an id, an ARN, a state or a count this module
/// derives, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a removal path must give back the same
/// number. The two default scopes a create also writes are charged by
/// [`ComAmazonawsEc2Amazonec2State::insert_ipam_scope`], the one writer of that store, and so
/// are deliberately NOT counted here — counting them in both places would make a later
/// delete refund less than was charged. The resource discovery keeps its own copy of the
/// description and the Region names, which is why those are counted twice.
pub fn ipam_payload_bytes(spec: &IpamSpec) -> usize {
    let regions: usize = spec.operating_regions.iter().map(String::len).sum();
    let description = spec.description.as_deref().unwrap_or_default().len();
    // The IPAM keeps the description and the Regions; the resource discovery keeps a copy
    // of both. A request that carries no client token is charged nothing for one. Beside
    // those, the THREE records this writer inserts (the IPAM, its resource discovery and
    // that discovery's association) each cost the fixed [`STORED_RECORD_OVERHEAD_BYTES`],
    // so a create with no description, Regions or tags is not free against
    // [`MAX_STORED_BYTES`].
    tag_list_bytes(&spec.tags)
        .saturating_add(2 * (regions + description))
        .saturating_add(spec.client_token.as_deref().unwrap_or_default().len())
        .saturating_add(3 * STORED_RECORD_OVERHEAD_BYTES)
}

/// What one `ModifyIpam` asks to be changed about a stored IPAM.
///
/// A member left `None` here is one the REQUEST did not carry, which leaves the stored value
/// exactly as it was rather than returning it to the default a create would have given it —
/// the same contract [`TransitGatewayModification`] has for a gateway. The operating Regions
/// are two SETS rather than one value, because the request names the ones to add and the ones
/// to remove instead of the list it wants to end up with.
#[derive(Debug, Clone)]
pub struct IpamModification {
    /// "The ID of the IPAM you want to modify" (`ModifyIpamRequest$IpamId`).
    pub ipam_id: String,
    /// "The description of the IPAM you want to modify" (`ModifyIpamRequest$Description`), or
    /// `None` when the request named none.
    pub description: Option<String>,
    /// The Region names of `ModifyIpamRequest$AddOperatingRegions`, already validated and
    /// deduplicated by the caller.
    pub add_operating_regions: Vec<String>,
    /// The Region names of `ModifyIpamRequest$RemoveOperatingRegions`, likewise.
    pub remove_operating_regions: Vec<String>,
    /// The `IpamTier` the IPAM is moved to, or `None` to leave it in the one it is in.
    pub tier: Option<String>,
    /// The `IpamMeteredAccount` the IPAM meters to, or `None` to leave it as it is.
    pub metered_account: Option<String>,
    /// "Enable this option to use your own GUA ranges as private IPv6 addresses"
    /// (`ModifyIpamRequest$EnablePrivateGua`), or `None` to leave the setting as it is.
    pub enable_private_gua: Option<bool>,
}

impl IpamModification {
    /// The IPAM this modification leaves behind, from the one it is applied to.
    ///
    /// The ONE definition of what a modification DOES to an IPAM, so the figure a handler
    /// charges to the store's byte budget is exactly the one the write stores. Every member
    /// the request did not carry is copied across unchanged, and every member the IPAM's own
    /// creation derived — its ids, its ARN, its owner, its home Region, its counts — is
    /// carried over untouched, since none of them follows from this request. The state is
    /// left as it stands here; [`ComAmazonawsEc2Amazonec2State::modify_ipam`] is what moves
    /// it, so no path can store a state the clock knows nothing about.
    pub fn applied_to(&self, ipam: &Ipam) -> Ipam {
        let mut applied = ipam.clone();
        if self.description.is_some() {
            applied.description = self.description.clone();
        }
        if self.tier.is_some() {
            applied.tier = self.tier.clone();
        }
        if self.metered_account.is_some() {
            applied.metered_account = self.metered_account.clone();
        }
        if self.enable_private_gua.is_some() {
            applied.enable_private_gua = self.enable_private_gua;
        }
        applied.operating_regions = Some(
            self.operating_regions_after(ipam)
                .into_iter()
                .map(|region_name| IpamOperatingRegion {
                    region_name: Some(region_name),
                })
                .collect(),
        );
        applied
    }

    /// The Region names the IPAM operates in once this modification is applied: the ones it
    /// holds now, less the ones to remove, plus the ones to add.
    ///
    /// "Operating Regions are Amazon Web Services Regions where the IPAM is allowed to manage
    /// IP address CIDRs" (`Ipam$OperatingRegions`) — a SET, so a Region already held is added
    /// no second time and one the IPAM does not hold is removed without effect, and the Region
    /// names the IPAM already reports keep the order they were stored in so an addition is
    /// reported after them.
    pub fn operating_regions_after(&self, ipam: &Ipam) -> Vec<String> {
        let mut regions: Vec<String> = ipam
            .operating_regions
            .iter()
            .flatten()
            .filter_map(|operating| operating.region_name.clone())
            .filter(|held| !self.remove_operating_regions.contains(held))
            .collect();
        for added in &self.add_operating_regions {
            if !regions.iter().any(|held| held == added) {
                regions.push(added.clone());
            }
        }
        regions
    }
}

/// The caller-chosen bytes of a stored IPAM that a `ModifyIpam` can change: its description
/// and the names of the Regions it operates in, each counted twice because the IPAM's default
/// resource discovery keeps a copy of both — exactly the share [`ipam_payload_bytes`] charges
/// a create for them.
///
/// The ONE definition of that charge, so what [`ComAmazonawsEc2Amazonec2State::modify_ipam`]
/// gives back for the record it replaces is the same number a later modification of the
/// replacement gives back for it. The tags, the client token and the fixed per-record
/// overhead are deliberately NOT counted: no modification changes them, so counting them
/// would charge and refund the same figure twice.
pub fn ipam_modifiable_bytes(ipam: &Ipam) -> usize {
    let regions: usize = ipam
        .operating_regions
        .iter()
        .flatten()
        .filter_map(|operating| operating.region_name.as_deref())
        .map(str::len)
        .sum();
    let description = ipam.description.as_deref().unwrap_or_default().len();
    2 * (regions + description)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPAM fits inside the store's ceiling ([`MAX_IPAMS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses with
    /// [`ipam_limit_exceeded`] when it answers `false`.
    pub fn has_ipam_capacity(&self) -> bool {
        self.ipams.len() < MAX_IPAMS
    }

    /// Store an IPAM with the two default scopes, the default resource discovery and that
    /// discovery's association AWS creates with it, and answer the `IpamId` it was given —
    /// or `None` when this store cannot hold the IPAM and both of its default scopes, which
    /// a handler answers with [`ipam_limit_exceeded`].
    ///
    /// Every id and every ARN is minted HERE rather than taken from `spec`, so the ids the
    /// `Ipam` reports (`PublicDefaultScopeId`, `PrivateDefaultScopeId`,
    /// `DefaultResourceDiscoveryId`, `DefaultResourceDiscoveryAssociationId`) always name
    /// records this store holds — the two scopes through [`Self::insert_ipam_scope`], the
    /// ONE writer of [`Self::ipam_scopes`], so a scope this path creates and a scope
    /// `CreateIpamScope` creates cannot be built two ways. All four records start in their
    /// non-terminal state and the due instant of their completion is recorded in
    /// [`Self::ipam_create_complete_at`], so a caller that reads straight back still sees
    /// `create-in-progress`. The caller-chosen bytes are charged against the store's byte
    /// budget here, so no path can store them uncharged; a create still asks
    /// [`Self::has_ipam_capacity`] and [`Self::would_exceed_stored_bytes`] first so it can
    /// refuse instead of overshooting.
    pub fn insert_ipam(&mut self, spec: IpamSpec) -> Option<String> {
        // Room for the IPAM itself AND for the two default scopes that come up with it,
        // asked before anything is written: a create stores all of it or none of it, so no
        // IPAM can end up naming a default scope this store refused to hold.
        let default_scopes = usize::try_from(DEFAULT_IPAM_SCOPE_COUNT).unwrap_or_default();
        if !self.has_ipam_capacity()
            || self.ipam_scopes.len() + default_scopes > MAX_STORED_RECORDS_PER_STORE
        {
            return None;
        }
        let payload_bytes = ipam_payload_bytes(&spec);
        let ipam_id = self.next_id(IPAM_ID_PREFIX);
        let ipam_arn = ipam_resource_arn(RESOURCE_TYPE_IPAM, &ipam_id);
        let owner_id = ::engine::emulator_account_id().to_string();
        let operating_regions: Vec<IpamOperatingRegion> = spec
            .operating_regions
            .iter()
            .map(|region_name| IpamOperatingRegion {
                region_name: Some(region_name.clone()),
            })
            .collect();

        // The two default scopes. "The public scope is intended for all public IP address
        // space" and the private one for private space (`IpamScope`); both are default
        // scopes of this IPAM, in its home Region, holding no pool yet. The room for both
        // was taken above, so neither write can be refused here.
        let public_scope_id =
            self.insert_default_ipam_scope(IPAM_SCOPE_TYPE_PUBLIC, &ipam_arn, &spec.region)?;
        let private_scope_id =
            self.insert_default_ipam_scope(IPAM_SCOPE_TYPE_PRIVATE, &ipam_arn, &spec.region)?;

        // The default resource discovery, and its association with this IPAM: "a default
        // resource discovery is created for your IPAM and it's associated with your IPAM"
        // (`IpamResourceDiscoveryAssociation$IsDefault`). It discovers in the IPAM's own
        // operating Regions, and it excludes no organizational unit.
        let resource_discovery_id = self.next_id(IPAM_RESOURCE_DISCOVERY_ID_PREFIX);
        self.ipam_resource_discoveries.insert(
            resource_discovery_id.clone(),
            IpamResourceDiscovery {
                owner_id: Some(owner_id.clone()),
                ipam_resource_discovery_id: Some(resource_discovery_id.clone()),
                ipam_resource_discovery_arn: Some(ipam_resource_arn(
                    RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY,
                    &resource_discovery_id,
                )),
                ipam_resource_discovery_region: Some(spec.region.clone()),
                description: spec.description.clone(),
                operating_regions: Some(operating_regions.clone()),
                is_default: Some(true),
                state: Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string()),
                // The tags of the request are the IPAM's; nothing tagged the discovery.
                tags: Some(Vec::new()),
                organizational_unit_exclusions: Some(Vec::new()),
            },
        );
        let association_id = self.next_id(IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX);
        self.ipam_resource_discovery_associations.insert(
            association_id.clone(),
            ipam_resource_discovery_association_view(
                &association_id,
                IpamResourceDiscoveryLink {
                    ipam_id: &ipam_id,
                    ipam_arn: Some(ipam_arn.clone()),
                    ipam_region: Some(spec.region.clone()),
                    discovery_id: &resource_discovery_id,
                    owner_id: Some(owner_id.clone()),
                },
                true,
                // The tags of the request are the IPAM's; nothing tagged this association.
                Vec::new(),
            ),
        );

        let ipam = Ipam {
            owner_id: Some(owner_id),
            ipam_id: Some(ipam_id.clone()),
            ipam_arn: Some(ipam_arn),
            ipam_region: Some(spec.region),
            public_default_scope_id: Some(public_scope_id),
            private_default_scope_id: Some(private_scope_id),
            scope_count: Some(DEFAULT_IPAM_SCOPE_COUNT),
            description: spec.description,
            operating_regions: Some(operating_regions),
            state: Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string()),
            tags: Some(spec.tags),
            default_resource_discovery_id: Some(resource_discovery_id),
            default_resource_discovery_association_id: Some(association_id),
            resource_discovery_association_count: Some(
                DEFAULT_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_COUNT,
            ),
            // "The state message" (`Ipam$StateMessage`): nothing has gone wrong, so there
            // is none.
            state_message: None,
            tier: Some(spec.tier),
            enable_private_gua: Some(spec.enable_private_gua),
            metered_account: Some(spec.metered_account),
        };
        self.add_stored_bytes(payload_bytes);
        self.ipams.insert(ipam_id.clone(), ipam);
        // The token that minted this IPAM, so a replay of the same request answers with it
        // rather than creating a second one. Recorded HERE, with the record itself, so no
        // path can store one without the other.
        if let Some(token) = spec.client_token {
            self.ipam_client_tokens.insert(token, ipam_id.clone());
        }
        // What the clock now owes all four records: their completed state, one
        // TRANSITION_DELAY from now — the same span, off the same monotonic clock, as a
        // pending VPC's move to `available`.
        self.ipam_create_complete_at
            .insert(ipam_id.clone(), Instant::now() + TRANSITION_DELAY);
        Some(ipam_id)
    }

    /// One default scope of an IPAM, stored, and the `IpamScopeId` it was given — or `None`
    /// when the scope store is at its ceiling.
    ///
    /// Written through [`Self::insert_ipam_scope`], the one writer of that store, which
    /// derives the id, the ARN it reports, the owning account and its pool count; what a
    /// DEFAULT scope adds is only what the IPAM it belongs to decides.
    pub(super) fn insert_default_ipam_scope(
        &mut self,
        scope_type: &str,
        ipam_arn: &str,
        region: &str,
    ) -> Option<String> {
        self.insert_ipam_scope(IpamScope {
            ipam_arn: Some(ipam_arn.to_string()),
            ipam_region: Some(region.to_string()),
            ipam_scope_type: Some(scope_type.to_string()),
            // "Defines if the scope is the default scope or not" (`IpamScope$IsDefault`):
            // this is one of the two an IPAM is created with.
            is_default: Some(true),
            // A default scope carries no description: nothing has set one, and the
            // request's description is the IPAM's.
            description: None,
            state: Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string()),
            tags: Some(Vec::new()),
            // "The external authority configuration for this IPAM scope, IF CONFIGURED"
            // (`IpamScope$ExternalAuthorityConfiguration`): a scope IPAM created itself is
            // linked to no external authority.
            external_authority_configuration: None,
            // Derived by `insert_ipam_scope`, which is why they are left unset here.
            ipam_scope_id: None,
            ipam_scope_arn: None,
            owner_id: None,
            pool_count: None,
        })
    }

    /// Move one IPAM and everything created with it to its completed state.
    ///
    /// Called only by [`Self::materialize_due_transitions`], once the instant recorded in
    /// [`Self::ipam_create_complete_at`] has passed. Each record is moved only out of the
    /// state the create left it in, so a record something else has since moved is left
    /// alone.
    pub(super) fn complete_ipam_creation(&mut self, ipam_id: &str) {
        let Some(ipam) = self.ipams.get_mut(ipam_id) else {
            return;
        };
        if ipam.state.as_deref() == Some(IPAM_STATE_CREATE_IN_PROGRESS) {
            ipam.state = Some(IPAM_STATE_CREATE_COMPLETE.to_string());
        }
        let scope_ids = [
            ipam.public_default_scope_id.clone(),
            ipam.private_default_scope_id.clone(),
        ];
        let resource_discovery_id = ipam.default_resource_discovery_id.clone();
        let association_id = ipam.default_resource_discovery_association_id.clone();

        for scope_id in scope_ids.into_iter().flatten() {
            if let Some(scope) = self.ipam_scopes.get_mut(&scope_id) {
                if scope.state.as_deref() == Some(IPAM_STATE_CREATE_IN_PROGRESS) {
                    scope.state = Some(IPAM_STATE_CREATE_COMPLETE.to_string());
                }
            }
        }
        if let Some(discovery) = resource_discovery_id
            .and_then(|discovery_id| self.ipam_resource_discoveries.get_mut(&discovery_id))
        {
            if discovery.state.as_deref() == Some(IPAM_STATE_CREATE_IN_PROGRESS) {
                discovery.state = Some(IPAM_STATE_CREATE_COMPLETE.to_string());
            }
        }
        if let Some(association) = association_id.and_then(|association_id| {
            self.ipam_resource_discovery_associations
                .get_mut(&association_id)
        }) {
            if association.state.as_deref() == Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS)
            {
                association.state =
                    Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_COMPLETE.to_string());
            }
        }
    }

    /// Apply one `ModifyIpam` to the IPAM it names and answer the record it leaves behind —
    /// or `None` when this store holds no IPAM of that id and nothing was written.
    ///
    /// The ONE writer of a modification, so what `ModifyIpam` answers is the stored record a
    /// later `DescribeIpams` of the same IPAM reports. What the modification CHANGES is
    /// [`IpamModification::applied_to`]; what this adds is everything the store owns:
    ///
    /// * the IPAM is left in the NON-TERMINAL [`IPAM_STATE_MODIFY_IN_PROGRESS`] with the move
    ///   to [`IPAM_STATE_MODIFY_COMPLETE`] recorded in [`Self::ipam_modify_complete_at`] one
    ///   [`TRANSITION_DELAY`] out and applied by [`Self::materialize_due_transitions`], so a
    ///   caller acting straight after the call still sees the transient state and nothing is
    ///   slept for;
    /// * the IPAM's default resource discovery follows it — "IPAM only discovers and monitors
    ///   resources in the Amazon Web Services Regions you select as operating Regions"
    ///   (`Ipam$OperatingRegions`), so the discovery that does that monitoring for this IPAM
    ///   operates in the same Regions it does, which is also the invariant
    ///   [`Self::insert_ipam`] establishes and [`ipam_payload_bytes`] charges for;
    /// * the store's byte budget gives back what the replaced record held and is charged what
    ///   the new one holds ([`ipam_modifiable_bytes`]), so the budget tracks exactly the
    ///   caller-chosen text that is stored.
    ///
    /// A caller must have asked [`Self::would_exceed_stored_bytes_after_refund`] first: this
    /// method is the single write path, and it is not where a request is refused.
    pub fn modify_ipam(&mut self, modification: &IpamModification) -> Option<Ipam> {
        let current = self.ipams.get(&modification.ipam_id)?.clone();
        let mut applied = modification.applied_to(&current);
        applied.state = Some(IPAM_STATE_MODIFY_IN_PROGRESS.to_string());

        self.release_stored_bytes(ipam_modifiable_bytes(&current));
        self.add_stored_bytes(ipam_modifiable_bytes(&applied));

        if let Some(discovery) = current
            .default_resource_discovery_id
            .as_deref()
            .and_then(|discovery_id| self.ipam_resource_discoveries.get_mut(discovery_id))
        {
            discovery.description = applied.description.clone();
            discovery.operating_regions = applied.operating_regions.clone();
        }

        self.ipams
            .insert(modification.ipam_id.clone(), applied.clone());
        self.ipam_modify_complete_at
            .insert(modification.ipam_id.clone(), Instant::now() + TRANSITION_DELAY);
        Some(applied)
    }

    /// Whether another resource discovery association fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_ipam_scope_capacity`]: nothing in this emulator
    /// reclaims an association, so a writer asks first and refuses when it answers `false`.
    pub fn has_ipam_resource_discovery_association_capacity(&self) -> bool {
        self.ipam_resource_discovery_associations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The association this store holds between the IPAM `ipam_id` names and the resource
    /// discovery `discovery_id` names, if it holds one.
    ///
    /// One IPAM-and-discovery PAIR holds at most one association — an association is the link
    /// between exactly those two resources, so a second link between the same pair would say
    /// nothing the first does not. The ONE definition of that question, so the writer below
    /// and every operation that asks it read the same pair.
    pub fn ipam_resource_discovery_association(
        &self,
        ipam_id: &str,
        discovery_id: &str,
    ) -> Option<&IpamResourceDiscoveryAssociation> {
        self.ipam_resource_discovery_associations
            .values()
            .find(|association| {
                association.ipam_id.as_deref() == Some(ipam_id)
                    && association.ipam_resource_discovery_id.as_deref() == Some(discovery_id)
            })
    }

    /// Store the association a caller asked for between the IPAM `ipam_id` names and the
    /// resource discovery `discovery_id` names, and answer the
    /// `IpamResourceDiscoveryAssociationId` it was given — or `None` when the store is at its
    /// ceiling, or when either resource is not held, and nothing was stored.
    ///
    /// The ONE way a CALLER-MADE association is stored, the counterpart of
    /// [`Self::insert_created_ipam_scope`] for the link between an IPAM and a discovery: the
    /// id and the ARN are minted here, "the IPAM ARN" and "the IPAM home Region"
    /// (`IpamResourceDiscoveryAssociation$IpamArn` / `$IpamRegion`) are taken from the stored
    /// IPAM and "the Amazon Web Services account ID of the resource discovery owner"
    /// (`$OwnerId`) from the stored discovery, so an association can never report a resource
    /// this store does not hold. The association is answered in the non-terminal
    /// `associate-in-progress` state, so the clock owes it
    /// [`IPAM_ASSOCIATION_STATE_ASSOCIATE_COMPLETE`] one [`TRANSITION_DELAY`] later
    /// ([`Self::ipam_resource_discovery_association_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]); `client_token` names the association from here
    /// on so a replay of the same request answers it instead of making a second one; the
    /// IPAM's own count of the associations against it is recounted; and every caller-chosen
    /// byte the record retains is charged to the store's byte budget.
    pub fn insert_ipam_resource_discovery_association(
        &mut self,
        ipam_id: &str,
        discovery_id: &str,
        client_token: Option<&str>,
        tags: Vec<Tag>,
    ) -> Option<String> {
        if !self.has_ipam_resource_discovery_association_capacity() {
            return None;
        }
        let ipam = self.ipams.get(ipam_id)?;
        let ipam_arn = ipam.ipam_arn.clone();
        let ipam_region = ipam.ipam_region.clone();
        let owner_id = self
            .ipam_resource_discoveries
            .get(discovery_id)?
            .owner_id
            .clone();
        let retained = tag_list_bytes(&tags)
            + client_token.unwrap_or_default().len()
            + STORED_RECORD_OVERHEAD_BYTES;
        let association_id = self.next_id(IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX);
        self.ipam_resource_discovery_associations.insert(
            association_id.clone(),
            ipam_resource_discovery_association_view(
                &association_id,
                IpamResourceDiscoveryLink {
                    ipam_id,
                    ipam_arn,
                    ipam_region,
                    discovery_id,
                    owner_id,
                },
                // "When you create an IPAM, a default resource discovery is created for your
                // IPAM and it's associated with your IPAM"
                // (`IpamResourceDiscoveryAssociation$IsDefault`): only that one association
                // is a default one, so one a caller asked for is not.
                false,
                tags,
            ),
        );
        self.add_stored_bytes(retained);
        self.ipam_resource_discovery_association_complete_at
            .insert(association_id.clone(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.ipam_resource_discovery_association_client_tokens
                .insert(token.to_string(), association_id.clone());
        }
        self.recount_ipam_resource_discovery_associations(ipam_id);
        Some(association_id)
    }

    /// Set the IPAM's `ResourceDiscoveryAssociationCount` to the number of associations this
    /// store now holds against it.
    ///
    /// "The IPAM's resource discovery association count"
    /// (`Ipam$ResourceDiscoveryAssociationCount`) is DERIVED from the associations
    /// themselves, so it is recounted from them rather than incremented — the same contract
    /// [`Self::recount_ipam_scopes`] has for an IPAM's scope count.
    pub(super) fn recount_ipam_resource_discovery_associations(&mut self, ipam_id: &str) {
        let associations = self
            .ipam_resource_discovery_associations
            .values()
            .filter(|association| association.ipam_id.as_deref() == Some(ipam_id))
            .count();
        if let Some(ipam) = self.ipams.get_mut(ipam_id) {
            ipam.resource_discovery_association_count = i32::try_from(associations).ok();
        }
    }
}

/// What one resource discovery association LINKS, read off the two stored resources it names:
/// the IPAM (its id, its ARN and its home Region) and the resource discovery (its id and the
/// account that owns it).
///
/// Carried as one value so [`ipam_resource_discovery_association_view`] takes the two sides of
/// the link rather than five loose members, and so neither writer can pass the ARN of one IPAM
/// beside the id of another.
pub(super) struct IpamResourceDiscoveryLink<'facts> {
    /// "The IPAM ID" (`IpamResourceDiscoveryAssociation$IpamId`).
    pub(super) ipam_id: &'facts str,
    /// "The IPAM ARN" (`$IpamArn`), as the stored IPAM reports it.
    pub(super) ipam_arn: Option<String>,
    /// "The IPAM home Region" (`$IpamRegion`), as the stored IPAM reports it.
    pub(super) ipam_region: Option<String>,
    /// "The resource discovery ID" (`$IpamResourceDiscoveryId`).
    pub(super) discovery_id: &'facts str,
    /// "The Amazon Web Services account ID of the resource discovery owner" (`$OwnerId`), as
    /// the stored discovery reports it.
    pub(super) owner_id: Option<String>,
}

/// One resource discovery association as the model shapes it, built from the facts of the two
/// resources it links.
///
/// The ONE constructor of that shape, called by both of its writers —
/// [`ComAmazonawsEc2Amazonec2State::insert_ipam`] for the default association an IPAM comes up
/// with and
/// [`ComAmazonawsEc2Amazonec2State::insert_ipam_resource_discovery_association`] for one a
/// caller asked for — so the two cannot drift apart over a member one of them sets and the
/// other forgets. The ARN is derived from the id here rather than passed in, and every
/// association starts in the non-terminal state of an association being made
/// ([`IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS`]) with the discovery's results readable
/// ("`active` - Connection or permissions required to read the results of the resource
/// discovery are intact", `$ResourceDiscoveryStatus`), which is true of every discovery this
/// store holds.
pub(super) fn ipam_resource_discovery_association_view(
    association_id: &str,
    link: IpamResourceDiscoveryLink<'_>,
    is_default: bool,
    tags: Vec<Tag>,
) -> IpamResourceDiscoveryAssociation {
    IpamResourceDiscoveryAssociation {
        owner_id: link.owner_id,
        ipam_resource_discovery_association_id: Some(association_id.to_string()),
        ipam_resource_discovery_association_arn: Some(ipam_resource_arn(
            RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION,
            association_id,
        )),
        ipam_resource_discovery_id: Some(link.discovery_id.to_string()),
        ipam_id: Some(link.ipam_id.to_string()),
        ipam_arn: link.ipam_arn,
        ipam_region: link.ipam_region,
        is_default: Some(is_default),
        resource_discovery_status: Some(IPAM_RESOURCE_DISCOVERY_STATUS_ACTIVE.to_string()),
        state: Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS.to_string()),
        tags: Some(tags),
    }
}

/// `InvalidIpamResourceDiscoveryId.Malformed` (HTTP 400): the request named something that is
/// not an IPAM resource discovery id at all, so there is nothing to look up — the counterpart
/// of [`INVALID_IPAM_ID_MALFORMED`] for the discovery.
pub const INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED: &str =
    "InvalidIpamResourceDiscoveryId.Malformed";

/// `InvalidIpamResourceDiscoveryId.NotFound` (HTTP 400): the request named a well-formed IPAM
/// resource discovery id this account holds no discovery for — the code AWS answers an
/// `AssociateIpamResourceDiscovery` naming an unknown discovery with, as the recorded wire
/// corpus behind this operation's stub issue shows.
pub const INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND: &str =
    "InvalidIpamResourceDiscoveryId.NotFound";

/// Whether `id` is a well-formed IPAM resource discovery id — one this service could have
/// ISSUED.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`IPAM_RESOURCE_DISCOVERY_ID_PREFIX`]). An ASSOCIATION id is therefore not one:
/// `ipam-res-disco-assoc-…` leaves a suffix that is no run of hex digits, so the two
/// resources of that shared prefix cannot be confused.
pub fn ipam_resource_discovery_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(IPAM_RESOURCE_DISCOVERY_ID_PREFIX, id)
}

/// Refuse something that is not an IPAM resource discovery id at all: HTTP 400
/// `InvalidIpamResourceDiscoveryId.Malformed`, worded the way this crate words a malformed id
/// of an IPAM resource ([`invalid_ipam_policy_id_malformed`]) and built from the request's own
/// value.
pub fn invalid_ipam_resource_discovery_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        400,
        format!("The {RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY} ID '{id}' is malformed"),
    )
}

/// Refuse a well-formed IPAM resource discovery id this account holds no discovery for: HTTP
/// 400 `InvalidIpamResourceDiscoveryId.NotFound`, worded by the one writer of that refusal
/// ([`id_not_found`]).
pub fn invalid_ipam_resource_discovery_id_not_found<E>(id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY} ID"),
        id,
    )
}

/// Refuse an `IpamResourceDiscoveryId` whose FORM is not one this service issues: HTTP 400
/// `InvalidIpamResourceDiscoveryId.Malformed`.
///
/// The first of the two questions an operation taking a discovery id asks, and the one that is
/// asked BEFORE `DryRun`: "not an id of this kind" is a fact about the request itself, while
/// "no such discovery" is a fact about the account — the order
/// [`require_well_formed_id`] documents for every other resource of this module.
pub fn require_well_formed_ipam_resource_discovery_id<E>(id: &str) -> Result<(), Scaffolded<E>> {
    if ipam_resource_discovery_id_is_well_formed(id) {
        return Ok(());
    }
    Err(invalid_ipam_resource_discovery_id_malformed(id))
}

// =============================================================================
// IPAM prefix list resolvers and the targets created against them
// =============================================================================

/// The ARN resource type of an IPAM prefix list resolver, which is also the `ResourceType` a
/// `TagSpecification` names to tag one — from the modelled catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES` carries `ipam-prefix-list-resolver`).
pub const RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER: &str = "ipam-prefix-list-resolver";

/// The ARN resource type of an IPAM prefix list resolver TARGET, from that same catalog.
pub const RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET: &str =
    "ipam-prefix-list-resolver-target";

/// The id prefix a stored IPAM prefix list resolver is minted with.
///
/// The model publishes no pattern for `IpamPrefixListResolverId`
/// (`smithy_query(query="shape",shape="IpamPrefixListResolverId")` is a plain `String`), so
/// the id takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`] with the prefix AWS gives the resource:
/// `ipam-plr-<hex>`, as the ids in the wire corpus's prefix list resolver calls read. The
/// resource's full modelled type is what its ARN carries
/// ([`RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER`]), exactly as an IPAM resource discovery's
/// `ipam-res-disco-` id sits beside its `ipam-resource-discovery` ARN type.
pub const IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX: &str = "ipam-plr";

/// The id prefix a stored IPAM prefix list resolver TARGET is minted with: `ipam-plrt-<hex>`,
/// from the same source as [`IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX`].
pub const IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_PREFIX: &str = "ipam-plrt";

/// How many rules one IPAM prefix list resolver may carry.
///
/// A ceiling of this store, not a published AWS quota: the rules a create names are kept for
/// the resolver's lifetime, so their COUNT is bounded the way every other list this store
/// retains is, and the caller-chosen bytes within them are charged beside it.
pub const MAX_IPAM_PREFIX_LIST_RESOLVER_RULES: usize = 50;

/// A resolver target whose creation has been asked for and has not settled yet: the
/// `IpamPrefixListResolverTargetState` enum's `create-in-progress` member
/// (`smithy_query(query="shape",shape="IpamPrefixListResolverTargetState")`), which is what
/// `CreateIpamPrefixListResolverTarget` answers with.
pub const IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// The state that enum reaches once the creation has settled: `create-complete`, which the
/// clock owes a [`IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS`] target one
/// [`TRANSITION_DELAY`] after the call that created it.
pub const IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_COMPLETE: &str = "create-complete";

/// `InvalidIpamPrefixListResolverId.NotFound` (HTTP 400): the request named a prefix list
/// resolver id this account holds no resolver for.
pub const INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND: &str =
    "InvalidIpamPrefixListResolverId.NotFound";

/// `InvalidIpamPrefixListResolverTargetId.NotFound` (HTTP 400): the request named a resolver
/// target id this account holds no target for.
///
/// There is no `.Malformed` counterpart for either of these two id kinds: the model publishes
/// no pattern for `IpamPrefixListResolverId` or `IpamPrefixListResolverTargetId` (both are
/// plain `String`s) and the `<prefix>-<hex>` form this module mints them in is its own, so an
/// id a request names is answered on whether the account HOLDS that resource and nothing else.
pub const INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND: &str =
    "InvalidIpamPrefixListResolverTargetId.NotFound";

/// The sentence this family's `NotFound` refusal is worded with: `The specified <resource> does
/// not exist.`, where `<resource>` is the resource's own name as the API spells it.
///
/// The sentence is FIXED: this pair of resources is refused with the "specified" wording and
/// names NO id — neither the id the request sent nor any other value varies in it — which is
/// why the id a caller asked about is not a parameter of this template. That is what separates
/// it both from the parenthesised `The IPAM (…) does not exist.` the IPAM and its scopes answer
/// ([`invalid_ipam_id_not_found`], [`invalid_ipam_scope_id_not_found`]) and from the quoted
/// `The <kind> '<id>' does not exist` the rest of EC2 answers ([`id_not_found`]). Only the
/// resource's name differs between the two refusals below, so the template lives here once.
pub(super) fn ipam_prefix_list_resolver_family_not_found<E>(
    code: &'static str,
    resource: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        code,
        400,
        format!("The specified {resource} does not exist."),
    )
}

/// The resource name this family's resolver refusal spells, which is the resource as the API
/// titles it rather than the lower-case prose the rest of this module's sentences use.
const IPAM_PREFIX_LIST_RESOLVER_REFUSAL_NAME: &str = "IPAM Prefix List Resolver";

/// The same for the resolver TARGET: the resolver's own name with the target's word on the end.
const IPAM_PREFIX_LIST_RESOLVER_TARGET_REFUSAL_NAME: &str = "IPAM Prefix List Resolver Target";

/// Refuse a prefix list resolver id this account holds no resolver for: HTTP 400
/// [`INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND`], worded by the template above.
pub fn invalid_ipam_prefix_list_resolver_id_not_found<E>() -> Scaffolded<E> {
    ipam_prefix_list_resolver_family_not_found(
        INVALID_IPAM_PREFIX_LIST_RESOLVER_ID_NOT_FOUND,
        IPAM_PREFIX_LIST_RESOLVER_REFUSAL_NAME,
    )
}

/// Refuse a resolver target id this account holds no target for: HTTP 400
/// [`INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND`], worded by that same template with
/// the target's own name.
pub fn invalid_ipam_prefix_list_resolver_target_id_not_found<E>() -> Scaffolded<E> {
    ipam_prefix_list_resolver_family_not_found(
        INVALID_IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_NOT_FOUND,
        IPAM_PREFIX_LIST_RESOLVER_TARGET_REFUSAL_NAME,
    )
}

/// The caller-chosen bytes one resolver's CIDR selection rules hold for the process's
/// lifetime: every string a rule and its conditions carry. The rule and condition types
/// themselves are fixed-size enums the model publishes, so only the text counts.
///
/// The ONE definition of that charge, asked by
/// [`ComAmazonawsEc2Amazonec2State::insert_creating_ipam_prefix_list_resolver`] and by the
/// create that calls it.
pub fn ipam_prefix_list_resolver_rule_bytes(rules: &[IpamPrefixListResolverRule]) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    rules
        .iter()
        .map(|rule| {
            text(&rule.rule_type)
                + text(&rule.static_cidr)
                + text(&rule.ipam_scope_id)
                + text(&rule.resource_type)
                + rule
                    .conditions
                    .iter()
                    .flatten()
                    .map(|condition| {
                        text(&condition.operation)
                            + text(&condition.ipam_pool_id)
                            + text(&condition.resource_id)
                            + text(&condition.resource_owner)
                            + text(&condition.resource_region)
                            + text(&condition.cidr)
                            + condition.resource_tag.as_ref().map_or(0, |tag| {
                                tag.key.as_ref().map_or(0, String::len)
                                    + tag.value.as_ref().map_or(0, String::len)
                            })
                    })
                    .sum::<usize>()
        })
        .sum()
}

/// The bytes one stored resolver target holds for the process's lifetime: the caller-chosen
/// ones — the prefix list it names, the Region that list is in, its tags and the client token
/// the token map keeps — plus the fixed [`STORED_RECORD_OVERHEAD_BYTES`] the record itself
/// costs. Its id, its ARN, the owning account, its state and its versions are derived or
/// numeric, so they are bounded already and not counted here.
///
/// The ONE definition of that charge: the create asks it before the write, off the values it
/// has in hand, and [`ComAmazonawsEc2Amazonec2State::insert_creating_ipam_prefix_list_resolver_target`]
/// charges exactly it.
pub fn ipam_prefix_list_resolver_target_payload_bytes(
    prefix_list_id: &str,
    prefix_list_region: &str,
    tags: &[Tag],
    client_token: Option<&str>,
) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(prefix_list_id.len())
        .saturating_add(prefix_list_region.len())
        .saturating_add(tag_list_bytes(tags))
        .saturating_add(client_token.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPAM prefix list resolver fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]) — the same contract as
    /// [`Self::has_ipam_scope_capacity`].
    pub fn has_ipam_prefix_list_resolver_capacity(&self) -> bool {
        self.ipam_prefix_list_resolvers.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an IPAM prefix list resolver and answer the `IpamPrefixListResolverId` it was
    /// given — or `None` when the store is at its ceiling and nothing was stored.
    ///
    /// The ONE way a resolver is stored, so the members the SERVICE owns cannot be built two
    /// ways: the id, the ARN it reports and the account that owns it are derived here rather
    /// than taken from a caller, exactly as [`Self::insert_ipam_scope`] derives a scope's.
    /// [`Self::insert_creating_ipam_prefix_list_resolver`] is what `CreateIpamPrefixListResolver`
    /// reaches it through.
    pub fn insert_ipam_prefix_list_resolver(
        &mut self,
        mut view: IpamPrefixListResolver,
    ) -> Option<String> {
        if !self.has_ipam_prefix_list_resolver_capacity() {
            return None;
        }
        let resolver_id = self.next_id(IPAM_PREFIX_LIST_RESOLVER_ID_PREFIX);
        view.ipam_prefix_list_resolver_id = Some(resolver_id.clone());
        // "The Amazon Resource Name (ARN) of the IPAM prefix list resolver"
        // (`IpamPrefixListResolver$IpamPrefixListResolverArn`): an EC2 IPAM-family ARN under
        // this resource's modelled type, whose Region segment is empty for the reason
        // [`ipam_resource_arn`] records — the resolver's Region is reported in
        // `IpamPrefixListResolver$IpamRegion` instead.
        view.ipam_prefix_list_resolver_arn = Some(ipam_resource_arn(
            RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER,
            &resolver_id,
        ));
        // "The ID of the Amazon Web Services account that owns the IPAM prefix list resolver"
        // (`IpamPrefixListResolver$OwnerId`): this emulator serves one account.
        view.owner_id = Some(::engine::emulator_account_id().to_string());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default())
                + view.description.as_deref().unwrap_or_default().len(),
        );
        self.ipam_prefix_list_resolvers
            .insert(resolver_id.clone(), view);
        Some(resolver_id)
    }

    /// Store a resolver a create has just made, and answer the `IpamPrefixListResolverId` it
    /// was given — or `None` when the store is at its ceiling and nothing was stored.
    ///
    /// The ONE way a CREATED resolver is stored, the counterpart of
    /// [`Self::insert_created_ipam_scope`]: the resolver is answered in the non-terminal
    /// `create-in-progress` state, so the clock owes it [`IPAM_STATE_CREATE_COMPLETE`] one
    /// [`TRANSITION_DELAY`] later ([`Self::ipam_prefix_list_resolver_create_complete_at`],
    /// applied by [`Self::materialize_due_transitions`]); the CIDR selection `rules` the create
    /// accepted are kept beside it, with the caller-chosen bytes they and the token retain
    /// charged to the store's budget; and `client_token` names the resolver from here on so a
    /// replay of the same request answers it instead of creating a second one.
    pub fn insert_creating_ipam_prefix_list_resolver(
        &mut self,
        client_token: Option<&str>,
        rules: Vec<IpamPrefixListResolverRule>,
        mut view: IpamPrefixListResolver,
    ) -> Option<String> {
        view.state = Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string());
        let retained = client_token.unwrap_or_default().len()
            + ipam_prefix_list_resolver_rule_bytes(&rules);
        let resolver_id = self.insert_ipam_prefix_list_resolver(view)?;
        self.add_stored_bytes(retained);
        self.ipam_prefix_list_resolver_create_complete_at
            .insert(resolver_id.clone(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.ipam_prefix_list_resolver_client_tokens
                .insert(token.to_string(), resolver_id.clone());
        }
        self.ipam_prefix_list_resolver_rules
            .insert(resolver_id.clone(), rules);
        Some(resolver_id)
    }

    /// Whether another resolver target fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    pub fn has_ipam_prefix_list_resolver_target_capacity(&self) -> bool {
        self.ipam_prefix_list_resolver_targets.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The resolver target the prefix list `prefix_list_id` names is already associated with,
    /// if it has one.
    ///
    /// "The ID of the IPAM prefix list resolver target associated with this managed prefix
    /// list" (`ManagedPrefixList$IpamPrefixListResolverTargetId`) is a SINGLE id, so a prefix
    /// list is the target of at most one resolver — and the list itself is where that fact
    /// lives, rather than in a second index beside it.
    pub fn ipam_prefix_list_resolver_target_of_prefix_list(
        &self,
        prefix_list_id: &str,
    ) -> Option<String> {
        self.managed_prefix_lists
            .get(prefix_list_id)?
            .view
            .ipam_prefix_list_resolver_target_id
            .clone()
    }

    /// Store a resolver target a create has just made against the prefix list
    /// `prefix_list_id` names, and answer the `IpamPrefixListResolverTargetId` it was given —
    /// or `None` when the store is at its ceiling and nothing was stored.
    ///
    /// The ONE way a CREATED target is stored, the counterpart of
    /// [`Self::insert_created_ipam_scope`]: the id, the ARN and the owning account are derived
    /// here; the target is answered in the non-terminal
    /// [`IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS`] state, so the clock owes
    /// it [`IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_COMPLETE`] one [`TRANSITION_DELAY`]
    /// later ([`Self::ipam_prefix_list_resolver_target_create_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]); nothing has synchronized a version of the
    /// prefix list yet, so the target reports no last synced version and no state message;
    /// `client_token` names the target from here on so a replay of the same request answers it
    /// instead of creating a second one; and the prefix list becomes IPAM managed, which is
    /// the list's own record of this association.
    pub fn insert_creating_ipam_prefix_list_resolver_target(
        &mut self,
        prefix_list_id: &str,
        client_token: Option<&str>,
        mut view: IpamPrefixListResolverTarget,
    ) -> Option<String> {
        if !self.has_ipam_prefix_list_resolver_target_capacity() {
            return None;
        }
        let target_id = self.next_id(IPAM_PREFIX_LIST_RESOLVER_TARGET_ID_PREFIX);
        view.ipam_prefix_list_resolver_target_id = Some(target_id.clone());
        view.ipam_prefix_list_resolver_target_arn = Some(ipam_resource_arn(
            RESOURCE_TYPE_IPAM_PREFIX_LIST_RESOLVER_TARGET,
            &target_id,
        ));
        view.owner_id = Some(::engine::emulator_account_id().to_string());
        view.state = Some(IPAM_PREFIX_LIST_RESOLVER_TARGET_STATE_CREATE_IN_PROGRESS.to_string());
        // "The version of the prefix list that was last successfully synchronized by this
        // target" (`IpamPrefixListResolverTarget$LastSyncedVersion`) and "a message describing
        // the current state … including any error information"
        // (`IpamPrefixListResolverTarget$StateMessage`): no synchronization has run on a
        // target this call is still creating, so it reports neither.
        view.last_synced_version = None;
        view.state_message = None;
        self.add_stored_bytes(ipam_prefix_list_resolver_target_payload_bytes(
            view.prefix_list_id.as_deref().unwrap_or_default(),
            view.prefix_list_region.as_deref().unwrap_or_default(),
            view.tags.as_deref().unwrap_or_default(),
            client_token,
        ));
        self.ipam_prefix_list_resolver_target_create_complete_at
            .insert(target_id.clone(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.ipam_prefix_list_resolver_target_client_tokens
                .insert(token.to_string(), target_id.clone());
        }
        // "The ID of the IPAM prefix list resolver target associated with this managed prefix
        // list. When set, this prefix list becomes an IPAM managed prefix list … its CIDRs are
        // automatically synchronized based on the IPAM prefix list resolver's CIDR selection
        // rules" (`ManagedPrefixList$IpamPrefixListResolverTargetId`), which is what the list's
        // own `IpamPrefixListResolverSyncEnabled` reports — so the list the target names
        // records the association rather than only the target knowing about it.
        if let Some(record) = self.managed_prefix_lists.get_mut(prefix_list_id) {
            record.view.ipam_prefix_list_resolver_target_id = Some(target_id.clone());
            record.view.ipam_prefix_list_resolver_sync_enabled = Some(true);
        }
        self.ipam_prefix_list_resolver_targets
            .insert(target_id.clone(), view);
        Some(target_id)
    }
}

// =============================================================================
// The tag keys registered for scheduled event notifications
// =============================================================================

/// How many Regions this emulator holds a scheduled event notification registration for
/// before a register signed for a further one is refused.
///
/// CHOSEN, and defined as the crate's one ceiling for a store nothing reclaims
/// ([`MAX_STORED_RECORDS_PER_STORE`]), the way the other per-Region store
/// ([`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`]) defines itself: the Region an entry is keyed
/// by is read off the REQUEST's own SigV4 credential scope, so a caller can name a Region
/// the store has never seen on every call, and nothing here reclaims an entry. A ceiling on
/// the entry count is what the byte budget cannot give on its own, because a registration in
/// the include-all mode holds no tag key. It is well above the number of Regions AWS has, so
/// no caller signing for real Regions can meet it.
pub const MAX_EVENT_NOTIFICATION_REGIONS: usize = MAX_STORED_RECORDS_PER_STORE;

/// The tag keys registered to appear in scheduled event notifications for the resources of
/// ONE Region.
///
/// `IncludeAllTagsOfInstance` is the MODE the registration is in — "true indicates that all
/// tag keys in the current Region are registered"
/// (`InstanceTagNotificationAttribute$IncludeAllTagsOfInstance`) — and the key set holds the
/// individually registered keys the other mode reports ("The registered tag keys",
/// `InstanceTagNotificationAttribute$InstanceTagKeys`). Storing the two together, rather
/// than as independent members, is what keeps the store out of a combination the model has
/// no reading for: either every tag key of the Region is registered, or exactly the keys in
/// the set are.
///
/// The set is a [`BTreeSet`], so a key a request names twice is registered once and the
/// order it is reported in is stable across reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceEventTagRegistration {
    /// Whether every tag key of the Region is registered.
    pub include_all_tags_of_instance: bool,
    /// The individually registered tag keys — empty while
    /// [`Self::include_all_tags_of_instance`] holds, since "all tag keys in the current
    /// Region are registered" leaves no key to name.
    pub instance_tag_keys: BTreeSet<String>,
}

impl InstanceEventTagRegistration {
    /// This registration as the modelled
    /// [`crate::ops::types::InstanceTagNotificationAttribute`] every
    /// operation that reports one answers with, so a register, a deregister and a describe
    /// cannot report one Region's registration differently.
    pub fn view(&self) -> crate::ops::types::InstanceTagNotificationAttribute {
        crate::ops::types::InstanceTagNotificationAttribute {
            include_all_tags_of_instance: Some(self.include_all_tags_of_instance),
            instance_tag_keys: Some(self.instance_tag_keys.iter().cloned().collect()),
        }
    }

    /// The registration with every tag key of the Region registered: "Indicates whether to
    /// register all tag keys in the current Region. Specify `true` to register all tag keys"
    /// (`RegisterInstanceTagAttributeRequest$IncludeAllTagsOfInstance`).
    ///
    /// An associated function, not a method, because the registration it answers with does
    /// NOT depend on the one it replaces: the individually registered keys are dropped
    /// because they are subsumed — the reported attribute has no key left to name once all
    /// of them are registered — which is also what makes the registration idempotent.
    pub fn registering_all() -> Self {
        Self {
            include_all_tags_of_instance: true,
            instance_tag_keys: BTreeSet::new(),
        }
    }

    /// This registration with `keys` registered individually: "The tag keys to register"
    /// (`RegisterInstanceTagAttributeRequest$InstanceTagKeys`) are UNIONED into the keys
    /// already registered, so registering one key does not deregister another.
    ///
    /// Naming keys is the other mode of the attribute, so it also leaves the include-all
    /// mode off: the registration then reports exactly the keys it holds.
    pub fn registering(&self, keys: &[String]) -> Self {
        let mut registered = self.instance_tag_keys.clone();
        registered.extend(keys.iter().cloned());
        Self {
            include_all_tags_of_instance: false,
            instance_tag_keys: registered,
        }
    }

    /// This registration with `keys` no longer registered: "Information about the tag keys
    /// to deregister" (`DeregisterInstanceTagAttributeRequest$InstanceTagKeys`) are taken
    /// OUT of the individually registered keys and every other key stays where it is — the
    /// mirror of [`Self::registering`]. A key the registration does not hold is removed
    /// without effect, since a set holds a key once or not at all.
    ///
    /// Naming keys says nothing about the include-all MODE, so the mode is left as it
    /// stands: turning it off is what `IncludeAllTagsOfInstance` is for on this request
    /// ([`Self::deregistering_all`]), exactly as the register turns it on.
    pub fn deregistering(&self, keys: &[String]) -> Self {
        let mut registered = self.instance_tag_keys.clone();
        for key in keys {
            registered.remove(key);
        }
        Self {
            include_all_tags_of_instance: self.include_all_tags_of_instance,
            instance_tag_keys: registered,
        }
    }

    /// The registration with no tag key registered at all: "Indicates whether to deregister
    /// all tag keys in the current Region. Specify `false` to deregister all tag keys"
    /// (`DeregisterInstanceTagAttributeRequest$IncludeAllTagsOfInstance`) leaves neither the
    /// include-all mode nor an individually registered key, which is
    /// [`Self::default`] — what a Region nothing has registered in reports.
    ///
    /// An associated function for the same reason [`Self::registering_all`] is one: what it
    /// answers does not depend on the registration it replaces.
    pub fn deregistering_all() -> Self {
        Self::default()
    }

    /// Whether this registration holds nothing: not the include-all mode and no
    /// individually registered key — the registration a Region with no stored entry
    /// reports, so a store holding it as an entry would charge for a fact it already
    /// answers ([`ComAmazonawsEc2Amazonec2State::instance_event_tag_registration`]).
    pub fn is_empty(&self) -> bool {
        !self.include_all_tags_of_instance && self.instance_tag_keys.is_empty()
    }

    /// The attribute a deregister answers with: the tag keys that call took out of the
    /// registration, beside the mode the registration is LEFT in.
    ///
    /// Not the registration as it now stands, which is what the register answers and what
    /// the describe reports: the recorded wire answer to a deregister of the one key a
    /// Region had registered carries THAT key, which the registration no longer holds, and
    /// the API reference's own example of this operation answers the keys the request
    /// named. So the keys a deregister reports are its own request's, as a set.
    pub fn deregistered(keys: &[String], left: &Self) -> Self {
        Self {
            include_all_tags_of_instance: left.include_all_tags_of_instance,
            instance_tag_keys: keys.iter().cloned().collect(),
        }
    }

    /// The caller-chosen bytes this registration holds: the registered keys themselves.
    /// The mode is one bit the service stores, so it is fixed-size.
    ///
    /// The bytes of the REGION it is keyed by are charged too, by
    /// [`instance_event_registration_payload_bytes`] — this method is only the key set's
    /// share, and the include-all mode holds no key at all, so it must not be the whole
    /// charge of a stored entry.
    pub fn payload_bytes(&self) -> usize {
        self.instance_tag_keys.iter().map(String::len).sum()
    }
}

/// The caller-chosen bytes one STORED registration holds: the Region name it is keyed by
/// and the keys registered for it.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and replacing or removing a registration
/// gives back exactly what the old one held. The Region is part of it because the key comes
/// from the request's own SigV4 credential scope, so an entry in the include-all mode — whose
/// key set is empty — must still cost the bytes it occupies.
pub fn instance_event_registration_payload_bytes(
    region: &str,
    registration: &InstanceEventTagRegistration,
) -> usize {
    region.len().saturating_add(registration.payload_bytes())
}

impl ComAmazonawsEc2Amazonec2State {
    /// What `region` has registered right now, or the empty registration for a Region
    /// nothing has registered in.
    ///
    /// A pure read: a Region that is only described gains no entry, so describing twice
    /// cannot change what a later caller sees.
    pub fn instance_event_tag_registration(&self, region: &str) -> InstanceEventTagRegistration {
        self.instance_event_tag_registrations
            .get(region)
            .cloned()
            .unwrap_or_default()
    }

    /// The caller-chosen bytes `region`'s stored registration holds right now, or 0 for a
    /// Region with no entry — what [`Self::set_instance_event_tag_registration`] gives back
    /// when it replaces that entry, so a writer can work out what its write ADDS.
    pub fn instance_event_registration_bytes(&self, region: &str) -> usize {
        self.instance_event_tag_registrations
            .get(region)
            .map_or(0, |registration| {
                instance_event_registration_payload_bytes(region, registration)
            })
    }

    /// Whether a registration for `region` fits inside the store's ceiling
    /// ([`MAX_EVENT_NOTIFICATION_REGIONS`]) — always true for a Region that already has an
    /// entry, since replacing one adds no entry.
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a writer asks first and refuses when
    /// it answers `false`, so the number of entries this map can hold never depends on how
    /// many Regions a caller signs requests for.
    pub fn has_instance_event_region_capacity(&self, region: &str) -> bool {
        self.instance_event_tag_registrations.contains_key(region)
            || self.instance_event_tag_registrations.len() < MAX_EVENT_NOTIFICATION_REGIONS
    }

    /// Replace `region`'s registration, re-charging the store's byte budget: what the old
    /// entry held is given back and what the new one holds is charged
    /// ([`instance_event_registration_payload_bytes`]), so the budget tracks exactly the
    /// Region names and keys that are stored.
    ///
    /// A writer must have asked [`Self::has_instance_event_region_capacity`] first: this
    /// method is the single write path, and it is not where a request is refused.
    pub fn set_instance_event_tag_registration(
        &mut self,
        region: &str,
        registration: InstanceEventTagRegistration,
    ) {
        let released = self.instance_event_registration_bytes(region);
        self.release_stored_bytes(released);
        self.add_stored_bytes(instance_event_registration_payload_bytes(
            region,
            &registration,
        ));
        self.instance_event_tag_registrations
            .insert(region.to_string(), registration);
    }

    /// Drop `region`'s registration, giving its bytes back: a Region whose last registered
    /// tag key has been deregistered is one nothing has registered in, which is exactly
    /// what [`Self::instance_event_tag_registration`] answers for a Region with no entry —
    /// so keeping an emptied entry would charge the store for a fact it already reports.
    ///
    /// A no-op for a Region that has no entry, which is what makes a deregister against
    /// such a Region store NOTHING.
    pub fn clear_instance_event_tag_registration(&mut self, region: &str) {
        let released = self.instance_event_registration_bytes(region);
        if self
            .instance_event_tag_registrations
            .remove(region)
            .is_some()
        {
            self.release_stored_bytes(released);
        }
    }
}
