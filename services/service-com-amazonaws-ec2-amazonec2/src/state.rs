// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ec2-amazonec2 service.
//!
//! This module owns the EC2 instance lifecycle: the reservations `RunInstances`
//! mints, the instances inside them, their tags, and the clock-owed state
//! transitions `Start`/`Stop`/`TerminateInstances` start. The projection of a
//! stored instance into the modelled `Instance` shape lives here too, so
//! `RunInstances` and `DescribeInstances` cannot report the same instance
//! differently.
//!
//! Each operation's own validation and orchestration stays in its
//! `src/ops/<operation>.rs` file; only the data model and the primitives several
//! operations share live here.
//!
//! Every resource is stored as the MODELLED shape the describe operations return,
//! beside the few facts those shapes carry no member for, keyed by the same resource
//! id ([`ComAmazonawsEc2Amazonec2State::vpc_available_at`] is when a `pending` VPC's
//! `available` transition falls due, on the same monotonic clock and the same
//! [`TRANSITION_DELAY`] as an instance's [`PendingTransition`]). This module also holds what several operations
//! share: the resource-id mint, the CIDR canonicalisation the `CidrBlock` members
//! document, the Availability Zone facts, the EC2 filter-value matcher, the page-token
//! codec, and the EC2 error channel (EC2's model declares no error shapes at all, so
//! every documented error code is returned through [`crate::scaffolded`]).
//!
//! ## Availability Zones are facts of a Region, not stored resources
//!
//! Nothing in the EC2 API creates, renames or deletes an Availability Zone: AWS
//! reports the zones the Region a request was made in *has*. So this module derives
//! a Region's zone set from the Region name — the one thing a request says about the
//! Region it is asking in, in its SigV4 credential scope — instead of keeping a copy
//! of it in [`ComAmazonawsEc2Amazonec2State`]. `DescribeAvailabilityZones` therefore
//! writes nothing at all: two calls signed for the same Region report the same zones,
//! and a read can never change what a later caller sees.
//!
//! The instance types a location offers are facts of that location in exactly the
//! same way — no EC2 operation creates or withdraws an offering — so
//! [`instance_type_offerings`] derives them from the Region and the modelled
//! `InstanceType` catalog, and `DescribeInstanceTypeOfferings` writes nothing either.
//!
//! An instance TYPE itself is a fact of the same kind: nothing in the EC2 API creates a
//! type, renames one or changes its vCPUs. So [`instance_type_record`] and
//! [`instance_type_info`] derive everything about a type from its name — the family,
//! generation, capability letters and size [`InstanceTypeName`] parses out of it, read
//! against the per-family ratios AWS publishes — and `DescribeInstanceTypes` writes
//! nothing: two calls describe one type identically, and the catalog cannot drift from
//! the one `DescribeInstanceTypeOfferings` offers.
//!
//! The REGIONS themselves are facts of the account's partition in the same way — no EC2
//! operation creates a Region, and opting one in or out is the Account API's job — so
//! [`regions_for_request`] derives them from the Region catalog and the Region the request
//! was signed for, and `DescribeRegions` writes nothing either.
//!
//! ## Resources callers create are stored
//!
//! [`ComAmazonawsEc2Amazonec2State::vpcs`] holds the account's VPCs as the modelled
//! [`Vpc`] shape itself rather than a parallel record, so every member a writer sets
//! round-trips through `DescribeVpcs` without a field-by-field copy that can silently
//! drop one. The store is keyed by `VpcId` in a [`BTreeMap`], which gives the describe
//! path one stable order and lets a pagination token name the id the next page starts
//! at. [`ComAmazonawsEc2Amazonec2State::subnets`] holds the account's subnets the same
//! way, keyed by `SubnetId`. [`ComAmazonawsEc2Amazonec2State::security_groups`] holds the
//! account's security groups the same way, keyed by `GroupId`, and
//! [`ComAmazonawsEc2Amazonec2State::images`] the images it can see, keyed by `ImageId`.
//! Further caller-created resources belong beside them as they are implemented, and should
//! be validated against [`availability_zones_in_region`] rather than against a second zone
//! list.
//!
//! ## A prefix list is either stored or derived, by who owns it
//!
//! [`ComAmazonawsEc2Amazonec2State::managed_prefix_lists`] holds the prefix lists the
//! ACCOUNT created, keyed by `PrefixListId`, as [`ManagedPrefixListRecord`]s: the modelled
//! `ManagedPrefixList` shape beside the facts it carries no member for — the list's entries
//! and the `create-complete` move the clock owes it.
//! [`ComAmazonawsEc2Amazonec2State::insert_managed_prefix_list`] is the one way one is
//! stored. The prefix lists AMAZON WEB SERVICES publishes for the services of a Region are
//! NOT stored: no EC2 operation creates or withdraws one, so
//! [`service_prefix_lists_in_region`] derives them from the Region the way
//! [`availability_zones_in_region`] derives its zones. Both reports of a prefix list read
//! the one derivation, so `DescribePrefixLists` and `DescribeManagedPrefixLists` cannot
//! disagree about one published list's id.
//!
//! An endpoint SERVICE is split the same way, by who offers it:
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_services`] holds the service configurations
//! the ACCOUNT created, while the services Amazon Web Services offers in a Region are NOT
//! stored — no EC2 operation creates or withdraws one — so [`endpoint_services_in_region`]
//! derives them from the Region. Both are projected into one [`EndpointServiceRecord`], so
//! `DescribeVpcEndpointServices` reports whichever source a service came from the same way.
//!
//! [`ComAmazonawsEc2Amazonec2State::local_gateway_virtual_interface_groups`] holds the
//! account's local gateway virtual interface groups the same way, keyed by
//! `LocalGatewayVirtualInterfaceGroupId`, so a pagination token can name the id the next
//! page starts at and `DescribeLocalGatewayVirtualInterfaceGroups` reports one group
//! exactly as its writer stored it. It starts EMPTY, like the association store beside it:
//! a local gateway and the virtual interface groups on it belong to an Outpost, so an
//! account without one has no group to report.
//! [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface_group`] is the
//! one way a group is stored, and it derives every member the service owns (the group id,
//! its ARN and the owning account) rather than taking it from a caller.
//!
//! [`ComAmazonawsEc2Amazonec2State::local_gateway_virtual_interfaces`] holds the virtual
//! interfaces themselves the same way, keyed by `LocalGatewayVirtualInterfaceId`, so
//! `DescribeLocalGatewayVirtualInterfaces` reports one exactly as its writer stored it and
//! pages over the same id order. It starts EMPTY for the reason the group store does, and
//! [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_virtual_interface`] is the one way
//! one is stored — which is also where the minted id is added to its group's
//! `LocalGatewayVirtualInterfaceIds` and, for an interface a writer answered in the
//! non-terminal `pending` configuration state, where the clock's move to `available` is
//! recorded ([`ComAmazonawsEc2Amazonec2State::local_gateway_virtual_interface_available_at`],
//! the way [`ComAmazonawsEc2Amazonec2State::vpc_available_at`] records a `pending` VPC's).
//!
//! [`ComAmazonawsEc2Amazonec2State::outpost_lags`] holds the Outpost link aggregation groups of
//! that same Outpost, keyed by `OutpostLagId`, and
//! [`ComAmazonawsEc2Amazonec2State::service_link_virtual_interfaces`] the service link virtual
//! interfaces that run over them, keyed by `ServiceLinkVirtualInterfaceId` — both as the
//! modelled shape itself, both starting EMPTY because no EC2 call racks an Outpost, and both
//! written through the one seam the [`outpost_lags`] submodule documents. A LAG's lists of the
//! virtual interfaces over it are kept in step by the inserts that store those interfaces, so
//! `DescribeOutpostLags` and the interface stores cannot report one association differently.
//!
//! [`ComAmazonawsEc2Amazonec2State::local_gateway_route_table_vpc_associations`] holds the
//! associations between a local gateway route table of
//! [`ComAmazonawsEc2Amazonec2State::local_gateway_route_tables`] and a VPC, keyed by
//! `LocalGatewayRouteTableVpcAssociationId`, as the modelled shape beside the move the clock
//! owes it: an association is answered in the non-terminal `associating` state, so the clock
//! owes it `associated` one [`TRANSITION_DELAY`] later
//! ([`LocalGatewayRouteTableVpcAssociationRecord::pending`]).
//! [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_vpc_association`] is the
//! one way an association is stored, and
//! [`require_known_local_gateway_route_table`] the one way the route table it is made in is
//! resolved — which answers [`INVALID_LOCAL_GATEWAY_ROUTE_TABLE_ID_NOT_FOUND`] for every
//! well-formed id while the route table store is empty.
//!
//! [`ComAmazonawsEc2Amazonec2State::local_gateways`] holds the local gateways themselves,
//! keyed by `LocalGatewayId`, as the modelled shape. It starts EMPTY for the reason the two
//! stores above it do — a local gateway is part of an Outpost rack, which no EC2 call
//! creates — so [`require_known_local_gateway`], the one way that id is resolved, answers
//! [`INVALID_LOCAL_GATEWAY_ID_NOT_FOUND`] for every well-formed id until
//! [`ComAmazonawsEc2Amazonec2State::insert_local_gateway`] has stored one. A route table is
//! created ON a gateway, so `CreateLocalGatewayRouteTable` reads the Outpost the table
//! belongs to off that stored gateway rather than from the request.
//!
//! ## A security group's rules are objects of their own
//!
//! A security group's rules have two shapes in the EC2 API: the aggregated
//! `IpPermissions`/`IpPermissionsEgress` lists `DescribeSecurityGroups` reports, and the
//! individual [`SecurityGroupRule`] objects — one per destination, each with its own
//! `SecurityGroupRuleId` — that `AuthorizeSecurityGroup{Ingress,Egress}` answers with and
//! `DescribeSecurityGroupRules`, `ModifySecurityGroupRules` and the revokes address by id.
//! [`ComAmazonawsEc2Amazonec2State::security_group_rules`] holds those objects, keyed by
//! rule id, and [`ComAmazonawsEc2Amazonec2State::insert_security_group_rules`] is the ONLY
//! writer of a rule: it expands one `IpPermission` into one rule per grant it names and
//! ends by recomputing the aggregated lists of the group it belongs to
//! ([`ComAmazonawsEc2Amazonec2State::resync_security_group_permissions`]), so the two views
//! of one rule cannot drift apart. The rules a group is created with (the allow-all
//! outbound rule every group carries, the default group's self-referencing inbound rule)
//! are stored through that same writer, so every rule the store holds has an id.
//!
//! A security group can also be made usable in a VPC it was not created in:
//! [`ComAmazonawsEc2Amazonec2State::security_group_vpc_associations`] holds those
//! associations, keyed by the (`GroupId`, `VpcId`) pair an association IS, as a
//! [`SecurityGroupVpcAssociationRecord`] — the facts the modelled
//! `SecurityGroupVpcAssociation` shape carries no member for, the instants its `associating`
//! -> `associated` and, once a disassociate has asked for one, its `disassociating` ->
//! `disassociated` moves fall due, on the same monotonic clock and the same
//! [`TRANSITION_DELAY`] as every other move this store owes. The two accounts the shape
//! reports are NOT copied onto the record: they are the owners of the group and of the VPC,
//! which this store already holds, so
//! [`ComAmazonawsEc2Amazonec2State::security_group_vpc_association_view`] derives them on
//! read and `AssociateSecurityGroupVpc`, `DisassociateSecurityGroupVpc` and
//! `DescribeSecurityGroupVpcAssociations` cannot report one association differently. An
//! association cannot outlive either side of the
//! pair, so [`ComAmazonawsEc2Amazonec2State::remove_security_group`] and
//! [`ComAmazonawsEc2Amazonec2State::remove_vpc`] drop the ones they orphan.
//!
//! A fact of a VPC that the modelled [`Vpc`] shape carries no member for is stored beside
//! it under the same id rather than on it:
//! [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] holds the `VpcAttributeName`
//! attributes `ModifyVpcAttribute` sets and `DescribeVpcAttribute` reports, so those two
//! operations and `CreateVpc`'s defaults cannot disagree about one VPC.
//!
//! The DHCP option set a VPC reports is a RESOURCE of its own, not a fact of the VPC, so it
//! lives in [`ComAmazonawsEc2Amazonec2State::dhcp_options`] keyed by `DhcpOptionsId` and
//! the VPC record only names it. An account has one default option set per Region and every
//! VPC it creates there inherits that same set, which is what
//! [`ComAmazonawsEc2Amazonec2State::default_dhcp_options_id`] answers — so a VPC's
//! `DhcpOptionsId` names a set `DescribeDhcpOptions` reports, and two VPCs of one Region
//! report the same one.
//!
//! ## Network interfaces
//!
//! [`ComAmazonawsEc2Amazonec2State::network_interfaces`] holds the account's elastic
//! network interfaces the same way, keyed by `NetworkInterfaceId`, and starts EMPTY: AWS
//! gives a fresh account a default VPC with its subnets and security group, but no network
//! interface exists until something creates one. The addresses an interface holds are the
//! store's own: a caller-named one is judged by [`is_assignable_subnet_address`] and taken
//! with [`ComAmazonawsEc2Amazonec2State::reserve_private_ip`], and a service-chosen one
//! comes from [`ComAmazonawsEc2Amazonec2State::mint_private_ip_in_cidr`] — the one scan of
//! a subnet's free addresses, judged against the one definition of "held" — and they are
//! counted off the subnet's `AvailableIpAddressCount`, so the subnet, interface, instance
//! and route server stores cannot disagree about which addresses are in use.
//!
//! ## EC2 Fleets
//!
//! [`ComAmazonawsEc2Amazonec2State::fleets`] holds the account's EC2 Fleets as
//! [`FleetRecord`]s: the modelled [`FleetData`](crate::ops::types::FleetData) every
//! operation that reports a fleet answers with, plus the instances the fleet has launched
//! ([`FleetInstanceRecord`]), which the modelled shape carries no member for outside an
//! `instant` fleet. Keyed by `FleetId`
//! in a [`BTreeMap`], the same stable order the other stores are kept in.
//!
//! A fleet record names its instances by id rather than copying them: the instance itself
//! stays in [`ComAmazonawsEc2Amazonec2State::instances`], so [`fleet_active_instances`] —
//! the projection of a fleet's instances into the modelled
//! [`ActiveInstance`](crate::ops::types::ActiveInstance) shape every operation that reports
//! one answers with — reads the instance type and the state out of the SAME record
//! `DescribeInstances` reports, and the two can never disagree about one instance.
//!
//! ## Windows fast launch
//!
//! [`ComAmazonawsEc2Amazonec2State::fast_launch_images`] holds the Windows fast launch
//! configuration of each image that has one, keyed by the `ImageId` it belongs to, as
//! [`FastLaunchRecord`]s: the modelled [`DescribeFastLaunchImagesSuccessItem`] every
//! operation that reports one answers with, plus the move the clock owes it
//! ([`FastLaunchTransition`]) — `enabling` -> `enabled` after `EnableFastLaunch`, and the
//! removal of the configuration after `DisableFastLaunch`, since `FastLaunchStateCode` has
//! no word for "not enabled" and an image whose pre-provisioned snapshots are gone is
//! simply not a fast launch enabled image any more. Both fall due on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as an instance's [`PendingTransition`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them.
//!
//! A configuration is keyed by an image of [`ComAmazonawsEc2Amazonec2State::images`], so
//! the store cannot hold more configurations than there are images, and every string one
//! retains is either a word the service chose or an id read back out of another store —
//! nothing caller-sized — so no byte charge is made for one.
//!
//! ## Fast snapshot restores
//!
//! [`ComAmazonawsEc2Amazonec2State::fast_snapshot_restores`] holds the fast snapshot
//! restore of each (snapshot, Availability Zone) pair that has one, keyed by both halves of
//! that pair ([`fast_snapshot_restore_key`]) as [`FastSnapshotRestoreRecord`]s: the pair,
//! the account that enabled it and the instants its lifecycle moves fall due on, from which
//! [`FastSnapshotRestoreRecord::view_at`] derives the state, the state transition reason and
//! the phase timestamps every operation that reports one answers with. See
//! [`fast_snapshot_restores`] for that slice's own documentation.
//!
//! ## Carrier gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::carrier_gateways`] holds the account's carrier
//! gateways as the modelled `CarrierGateway` shape itself, keyed by `CarrierGatewayId`
//! in a [`BTreeMap`] — the same arrangement, and for the same reasons, as the VPC and
//! subnet stores: every member `CreateCarrierGateway` sets round-trips through
//! `DescribeCarrierGateways` without a field-by-field copy, and the id order is both the
//! describe's order and what a pagination token names a position in. A carrier gateway
//! `CreateCarrierGateway` answers is `pending` (`CarrierGatewayState`), so
//! [`ComAmazonawsEc2Amazonec2State::carrier_gateway_available_at`] records when the clock
//! owes it `available`, the subnet store's
//! [`ComAmazonawsEc2Amazonec2State::subnet_available_at`] arrangement on the same
//! [`TRANSITION_DELAY`].
//!
//! ## AMI store tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::store_image_tasks`] holds the AMI store tasks
//! `CreateStoreImageTask` starts, as [`StoreImageTaskRecord`]s: the destination the request
//! named (the AMI, the bucket and the object key the service derives from the AMI's id),
//! the instant the task started, and the instant its upload finishes — on the same
//! monotonic clock and the same [`TRANSITION_DELAY`] as every other clock-owed move in
//! this module. The record holds no `StoreTaskState` and no `ProgressPercentage`: both are
//! facts of the task's age, so [`StoreImageTaskRecord::view_at`] derives them on read the
//! way [`SecondaryNetworkRecord::view_at`] derives a network's state, and a read writes
//! nothing.
//!
//! The store is keyed by a MINTED, monotonically increasing internal key, so iterating it
//! backwards is the "reverse chronological order" `DescribeStoreImageTasks` lists tasks in
//! and a pagination token can name the key the next page starts at. A task leaves the
//! account's view 31 days after it started ("Currently, only tasks from the past 31 days
//! can be viewed", `DescribeStoreImageTasks`), which is a move the clock owes it too, so
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] drops it then.
//!
//! ## Launch templates
//!
//! [`ComAmazonawsEc2Amazonec2State::launch_templates`] holds the account's launch
//! templates as [`LaunchTemplateRecord`]s: the modelled [`LaunchTemplate`] every operation
//! that reports one answers with, plus the versions of the template, because "deleting a
//! launch template deletes all of its versions" (`DeleteLaunchTemplate`) — so the versions
//! have to be part of the one record the delete removes. The store is keyed by
//! `LaunchTemplateId` in a [`BTreeMap`], the same stable order the VPC and subnet stores
//! are kept in, and a template is reached by name through
//! [`ComAmazonawsEc2Amazonec2State::launch_template_id_for_name`] because the API lets a
//! caller name either.
//!
//! ## EC2 Instance Connect Endpoints
//!
//! [`ComAmazonawsEc2Amazonec2State::instance_connect_endpoints`] holds the account's EC2
//! Instance Connect Endpoints as [`InstanceConnectEndpointRecord`]s, on exactly the terms
//! the volume store holds a volume: the modelled `Ec2InstanceConnectEndpoint` every
//! operation that reports one answers with, plus the instant the clock owes the endpoint
//! its `create-in-progress` -> `create-complete` move, and the instant a delete that has
//! been asked for owes it its `delete-in-progress` -> `delete-complete` move. Both are
//! facts of the endpoint's age rather than of the request and so the state is derived on
//! read
//! ([`InstanceConnectEndpointRecord::view_at`]) instead of stored. Keyed by
//! `InstanceConnectEndpointId` in a [`BTreeMap`], so a pagination token can name the id
//! the next page starts at.
//!
//! ## EBS volumes
//!
//! [`ComAmazonawsEc2Amazonec2State::volumes`] holds the account's EBS volumes as
//! [`VolumeRecord`]s: the modelled [`VolumeView`] (`com.amazonaws.ec2#Volume`) every
//! operation that reports one answers with, plus the instant the clock owes the volume its
//! `creating` -> `available` move — the one member of the view that is a fact of the
//! volume's age rather than of the request, and so derived on read instead of stored.
//! Keyed by `VolumeId` in a [`BTreeMap`], the same stable order the VPC and subnet stores
//! are kept in, so a pagination token can name the id the next page starts at.
//!
//! ## VM Import/Export snapshot import tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::import_snapshot_tasks`] holds the account's snapshot
//! import tasks as the modelled [`ImportSnapshotTask`] shape itself, keyed by
//! `ImportTaskId` in a [`BTreeMap`] — the same arrangement as the VPC and volume stores,
//! and for the same two reasons: a member a writer sets round-trips through
//! `DescribeImportSnapshotTasks` without a field-by-field copy, and the store's own order
//! is the order that describe reports and a page token names a position in. The store is
//! EMPTY for a fresh account, unlike the image store: nothing creates an import task until
//! a caller asks for one ([`ComAmazonawsEc2Amazonec2State::insert_import_snapshot_task`]),
//! and the describe answers "a list of zero or more import snapshot tasks".
//!
//! What the CLOCK owes a task an import is still running behind — the EBS snapshot its disk
//! becomes, and the `active` -> `completed` move that snapshot being ready earns it — is
//! kept beside the store as an [`ImportSnapshotConversion`], because `SnapshotTaskDetail`
//! carries no member for an instant; see
//! [`ComAmazonawsEc2Amazonec2State::complete_due_import_snapshot_conversions`].
//!
//! ## EBS volume modifications
//!
//! [`ComAmazonawsEc2Amazonec2State::volume_modifications`] holds, per `VolumeId`, the most
//! recent modification of that volume as a [`VolumeModificationRecord`]: the modelled
//! [`VolumeModification`] every operation that reports one answers with, plus the instants
//! the clock owes it its `modifying` -> `optimizing` -> `completed` moves. The state, the
//! progress and the end time are facts of the modification's age rather than of the request
//! that started it, so [`VolumeModificationRecord::view_at`] derives all three on read
//! instead of storing a word two writers could disagree about — the same arrangement
//! [`VolumeRecord`] uses for a volume's own `State`, on the same monotonic clock and the
//! same [`TRANSITION_DELAY`].
//!
//! One volume has at most ONE record: "Describes the most recent volume modification
//! request for the specified EBS volumes" (`DescribeVolumesModifications`), so a later
//! modification of the same volume replaces the record rather than adding one beside it.
//!
//! ## Verified Access trust providers
//!
//! [`ComAmazonawsEc2Amazonec2State::verified_access_trust_providers`] holds the account's
//! Verified Access trust providers as the modelled
//! [`crate::ops::types::VerifiedAccessTrustProvider`] shape itself — the one
//! `CreateVerifiedAccessTrustProvider` answers with and
//! `DescribeVerifiedAccessTrustProviders` reports — keyed by
//! `VerifiedAccessTrustProviderId` in a [`BTreeMap`], the same stable order the VPC and
//! subnet stores are kept in, so a pagination token can name the id the next page starts
//! at. The shape carries no status member: AWS hands a trust provider back ready, so the
//! clock owes it no transition and there is nothing to derive on read.
//!
//! [`ComAmazonawsEc2Amazonec2State::remove_verified_access_trust_provider`] is the one way
//! a trust provider leaves that store: it takes the create token that minted it along with
//! it, gives the record's caller-chosen bytes back to the byte budget, and — when the
//! delete named an idempotency token — keeps the removed record under that token
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_trust_provider_deletion`]) so a
//! retried delete answers with what the first one answered. That history is bounded by
//! entry count and drops its oldest entry to make room, because a delete frees the live
//! record's slot and so leaves no ceiling that would bound it. It also brings every instance
//! that had the trust provider ATTACHED back in step with that store
//! ([`ComAmazonawsEc2Amazonec2State::resync_verified_access_trust_provider_attachments`]), so
//! no instance is left reporting an id the account holds no trust provider for.
//!
//! Which trust providers an instance has attached is the instance's own
//! `VerifiedAccessTrustProviders` set, so there is one place an attachment is read from — the
//! set `DescribeVerifiedAccessInstances` reports —
//! and [`ComAmazonawsEc2Amazonec2State::set_verified_access_trust_provider_attachment`] is
//! the one way it CHANGES: `AttachVerifiedAccessTrustProvider` adds the entry,
//! `DetachVerifiedAccessTrustProvider` takes it away, both stamp the instance's
//! `LastUpdatedTime`, and each entry is
//! [`verified_access_trust_provider_condensed`] of the STORED trust provider rather than of a
//! request, so an instance cannot report a trust provider the trust provider store describes
//! differently.
//!
//! ## Verified Access instances, groups and endpoints
//!
//! [`ComAmazonawsEc2Amazonec2State::verified_access_instances`] holds the account's Verified
//! Access instances as the modelled [`crate::ops::types::VerifiedAccessInstance`] shape
//! itself, keyed by `VerifiedAccessInstanceId` in a [`BTreeMap`], and
//! [`ComAmazonawsEc2Amazonec2State::insert_verified_access_instance`] is the one way an
//! instance is stored. A group resolves the instance it is created on there, so an id that
//! names no instance of the account is refused
//! ([`invalid_verified_access_instance_id_not_found`]) rather than stored as a dangling
//! reference. The store starts EMPTY — a fresh account has created no Verified Access
//! instance — and `CreateVerifiedAccessInstance` is what lands on that writer, so the
//! instance a caller created is the one every reader of an instance resolves.
//! [`ComAmazonawsEc2Amazonec2State::modify_verified_access_instance`] is the one way a stored
//! instance CHANGES: it applies the members a `ModifyVerifiedAccessInstance` request carried,
//! leaves the ones it did not alone, and stamps the instance's `LastUpdatedTime`, so the
//! modify and a later describe cannot report one instance differently.
//! [`ComAmazonawsEc2Amazonec2State::remove_verified_access_instance`] is the one way an
//! instance LEAVES that store, on the same terms a group leaves its own: the create token
//! that minted it goes with it, the logging configuration kept beside it goes with it, the
//! bytes both held go back to the byte budget, and a delete that named an idempotency token
//! keeps the removed record under it
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_instance_deletion`]) so a retried delete
//! answers what the first one answered. A group or an endpoint that still names the instance
//! is a dependency ([`ComAmazonawsEc2Amazonec2State::verified_access_instance_dependencies`]),
//! so the delete is refused while one is stored rather than leaving a record naming an
//! instance the account no longer holds.
//!
//! An instance's LOGGING configuration is a fact of the instance the modelled
//! `VerifiedAccessInstance` shape carries no member for, so it is kept beside the instance in
//! [`ComAmazonawsEc2Amazonec2State::verified_access_instance_logging_configurations`], as the
//! modelled [`crate::ops::types::VerifiedAccessLogs`] shape a describe of the logging
//! configurations reports, keyed by the same `VerifiedAccessInstanceId`.
//! [`ComAmazonawsEc2Amazonec2State::set_verified_access_instance_logging_configuration`] is
//! the one way one is written and
//! [`ComAmazonawsEc2Amazonec2State::verified_access_instance_logging_configuration`] the one
//! way one is read, so the modify of a configuration and a later describe of it cannot
//! disagree. An instance nothing has configured has NO entry there and carries
//! [`default_verified_access_logs`] — a read reports that configuration rather than storing
//! it, so describing an instance leaves the store exactly as it was.
//!
//! [`ComAmazonawsEc2Amazonec2State::verified_access_groups`] holds the account's Verified
//! Access groups as [`VerifiedAccessGroupRecord`]s, keyed by `VerifiedAccessGroupId`, and
//! [`ComAmazonawsEc2Amazonec2State::insert_verified_access_group`] is the one way a group is
//! stored: it derives every member the service owns (the group id, its ARN and the owning
//! account) rather than taking them from a caller, so `CreateVerifiedAccessGroup` and a
//! describe of the groups cannot report one group differently. The record is the modelled
//! [`crate::ops::types::VerifiedAccessGroup`] shape the create answers with, plus the
//! group-level policy document that shape carries no member for
//! ([`VerifiedAccessGroupRecord::policy_document`]), which `GetVerifiedAccessGroupPolicy`
//! reports — kept beside the view exactly as an endpoint's own document is.
//! An endpoint resolves the group it is attached to here, and reads the instance the group
//! belongs to off that record, so one endpoint cannot name an instance its group does not.
//!
//! [`ComAmazonawsEc2Amazonec2State::remove_verified_access_group`] is the one way a group
//! LEAVES that store, on the same terms a trust provider leaves its own: the create token
//! that minted it goes with it, the record's caller-chosen bytes go back to the byte budget,
//! the removed record is stamped with the instant it was deleted (the group shape's
//! `DeletionTime`, which only a delete sets), and a delete that named an idempotency token
//! keeps that record under it
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_group_deletion`]) so a retried delete
//! answers what the first one answered. That report is of the group as the store HELD it, so
//! a description no caller ever set reads as empty text there rather than as an absent
//! member, while the create answers the request it was given.
//!
//! [`ComAmazonawsEc2Amazonec2State::verified_access_endpoints`] holds the account's Verified
//! Access endpoints as [`VerifiedAccessEndpointRecord`]s, keyed by
//! `VerifiedAccessEndpointId` in a [`BTreeMap`] — the same stable order every other store
//! here is kept in, so a pagination token can name the id the next page starts at. The
//! record is the modelled [`crate::ops::types::VerifiedAccessEndpoint`] shape
//! `CreateVerifiedAccessEndpoint` answers with and `DescribeVerifiedAccessEndpoints`
//! reports, plus the two facts that shape carries no member for: the `pending` -> `active`
//! move the clock owes an endpoint AWS hands back in the non-terminal `pending` status
//! ([`VerifiedAccessEndpointRecord::pending`], the [`ClockTransition`] every other record of
//! this store carries, on the same monotonic clock and the same [`TRANSITION_DELAY`] span as
//! a `pending` VPC's `available` move, applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]), and the endpoint-level
//! policy document the create takes ([`VerifiedAccessEndpointRecord::policy_document`]),
//! which `GetVerifiedAccessEndpointPolicy` reports and which the endpoint shape itself does
//! not carry.
//!
//! An endpoint LEAVES that store the way a transit gateway does rather than the way a group
//! does: `DeleteVerifiedAccessEndpoint` leaves it in the non-terminal `deleting` status the
//! `VerifiedAccessEndpointStatusCode` catalog carries
//! ([`ComAmazonawsEc2Amazonec2State::begin_verified_access_endpoint_deletion`]) and the clock
//! owes it the REMOVAL one [`TRANSITION_DELAY`] later
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_endpoint_delete_at`]), at which point
//! [`ComAmazonawsEc2Amazonec2State::remove_verified_access_endpoint`] gives the record's
//! caller-chosen bytes and its create token back; a delete that named an idempotency token
//! keeps the record it answered under it
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_endpoint_deletion`]) so a retried delete
//! answers what the first one answered even after that removal.
//!
//! ## Secondary networks
//!
//! [`ComAmazonawsEc2Amazonec2State::secondary_networks`] holds the account's secondary
//! networks as [`SecondaryNetworkRecord`]s, the same shape of record the volume store
//! keeps: the modelled [`SecondaryNetwork`] every operation that reports one answers with,
//! plus the instant the clock owes the network its `create-in-progress` ->
//! `create-complete` move and, once a delete has asked for it, the instant it owes it its
//! `delete-in-progress` -> `delete-complete` move. The network's `State` and the `State` of
//! each of its IPv4 CIDR block associations are facts of its age rather than of the
//! request, so [`SecondaryNetworkRecord::view_at`] derives both on read instead of storing
//! them; a deleted network keeps its record, because `DescribeSecondaryNetworks` reports
//! the delete states too. Keyed
//! by `SecondaryNetworkId` in a [`BTreeMap`], the same stable order the VPC, subnet and
//! volume stores are kept in, so a pagination token can name the id the next page starts
//! at.
//!
//! ## Network interfaces
//!
//! [`ComAmazonawsEc2Amazonec2State::network_interfaces`] holds the account's elastic
//! network interfaces as the modelled [`NetworkInterface`] shape itself — the one
//! `CreateNetworkInterface` answers with and `DescribeNetworkInterfaces` reports — keyed by
//! `NetworkInterfaceId` in a [`BTreeMap`], the same stable order the VPC and subnet stores
//! are kept in, so a pagination token can name the id the next page starts at. The shape
//! carries the one fact a delete turns on itself, its `Attachment`: "you must detach the
//! network interface before you can delete it" (`DeleteNetworkInterface`), so nothing is
//! stored beside the record.
//!
//! An interface AWS hands back is ready (`available`, or `in-use` once something holds it),
//! so a create owes it no transition and there is nothing to derive on read.
//! [`ComAmazonawsEc2Amazonec2State::insert_network_interface`] is the one writer seam every
//! path that creates one lands on — `CreateNetworkInterface` and a launch's own primary
//! interface alike — so an interface is one record rather than a second copy beside it.
//!
//! An interface's ATTACHMENT is what does owe the clock a move: `AttachNetworkInterface`
//! leaves it `attaching` on its way to `attached`, and `DetachNetworkInterface` leaves it
//! `detaching`, after which the interface holds nothing and is `available` again. Each move
//! falls due one [`TRANSITION_DELAY`] after the call that started it, as the
//! [`PendingStateMove`] in
//! [`ComAmazonawsEc2Amazonec2State::network_interface_attachment_move_at`] beside the record
//! — the way a `pending` VPC's move is kept — and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it. So a caller that
//! reads the interface straight after either call still sees the transient state, and nothing
//! is slept for.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_interface_permissions`] holds the permissions the
//! account has granted over those interfaces, as the modelled [`NetworkInterfacePermission`]
//! shape itself, keyed by `NetworkInterfacePermissionId` in a [`BTreeMap`] for the same
//! reason every other store here is: one stable order a pagination token can name a position
//! in. A permission is recorded in the non-terminal `pending` state, so the clock owes it
//! `granted` one [`TRANSITION_DELAY`] later; the due instant sits beside the record in
//! [`ComAmazonawsEc2Amazonec2State::network_interface_permission_granted_at`], the way a
//! `pending` VPC's does, and [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]
//! applies it. A permission names an interface, so
//! [`ComAmazonawsEc2Amazonec2State::remove_network_interface`] drops the permissions over the
//! interface it removes rather than leaving a grant over something the account no longer
//! holds.
//!
//! ## BYOIP address ranges
//!
//! [`ComAmazonawsEc2Amazonec2State::byoip_cidrs`] holds the address ranges the account has
//! brought to AWS as [`ByoipCidrRecord`]s: the modelled [`ByoipCidr`] every operation that
//! reports one answers with, plus the two moves the clock can owe one — the state move a
//! `ProvisionByoipCidr` or an `AdvertiseByoipCidr` left non-terminal
//! ([`ByoipCidrRecord::pending`], the [`PendingStateMove`] every other record of this store
//! carries) and the release a `DeprovisionByoipCidr` has started
//! ([`ByoipCidrRecord::released_at`], beside it because WHAT a release ends as is decided at
//! its due instant rather than when it starts). Both run on the same monotonic clock and the
//! same [`TRANSITION_DELAY`] span as a `pending` VPC's `available` move, and are applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]. Keyed by
//! the range itself in its canonical form ([`canonical_byoip_cidr`]), because the range IS
//! the name the API takes — every BYOIP operation is addressed by `Cidr`, not by a minted id
//! — which also gives the describe path one stable order, the order `DescribeByoipCidrs`
//! reports and pages over. That key comes from a REQUEST, so
//! the store is bounded like every other one here: a new range has to fit inside the record
//! ceiling ([`ComAmazonawsEc2Amazonec2State::has_byoip_cidr_capacity`]) and the byte budget,
//! and [`byoip_cidr_payload_bytes`] is the one definition of what a range is charged, given
//! back when the clock's release drops it.
//!
//! ## BYOASN: the ASNs brought to IPAM, and their associations with those ranges
//!
//! [`ComAmazonawsEc2Amazonec2State::byoasns`] holds the Autonomous System Numbers the
//! account has brought to IPAM as [`ByoasnRecord`]s — the modelled [`Byoasn`] every
//! operation that reports one answers with, plus the provisioning move the clock owes it —
//! keyed by the ASN as the API spells it (`Byoasn$Asn` is a `String`, not a number), which
//! is the only name `AssociateIpamByoasn`, `DisassociateIpamByoasn` and
//! `DeprovisionIpamByoasn` take for one. `ProvisionIpamByoasn` is the operation that brings
//! an ASN to IPAM and `DeprovisionIpamByoasn` the one that gives it back; the provision is
//! still a scaffold, so a fresh account holds none — which is why `AssociateIpamByoasn`
//! answers [`INVALID_BYOASN_NOT_FOUND`] for every ASN until one is written, exactly as the
//! real service does for an ASN that has not been brought to IPAM.
//!
//! The ASSOCIATION of such an ASN with a BYOIP range lives on the RANGE, in
//! `ByoipCidr$AsnAssociations` — "You can view the association with DescribeByoipCidrs"
//! (`AssociateIpamByoasn`) — rather than in a map beside it, so the associating operation
//! and that describe cannot report one association differently. Only the state move the
//! clock owes an association it left in `pending-association` has no member to live in, so
//! it is kept beside the ranges in
//! [`ComAmazonawsEc2Amazonec2State::asn_association_moves`], keyed by the canonical range
//! and the ASN together — the pair that names one association — and applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] on the same clock and
//! span as every other move this store owes.
//!
//! The ASN store has ONE insert — [`ComAmazonawsEc2Amazonec2State::insert_byoasn`], behind
//! [`ComAmazonawsEc2Amazonec2State::has_byoasn_capacity`] and [`byoasn_payload_bytes`] —
//! the same ceiling-and-budget contract every other store of this module is written
//! through, so an ASN cannot be filed two ways and a key a REQUEST names cannot grow this
//! process's memory without bound. `DeprovisionIpamByoasn` gives an ASN back through
//! [`ComAmazonawsEc2Amazonec2State::begin_byoasn_release`], which leaves it in the
//! non-terminal `pending-deprovision` with the clock owing the outcome; WHAT that release
//! ends as is decided at the due instant, from the associations the ASN still holds THEN
//! ([`ComAmazonawsEc2Amazonec2State::byoip_cidrs_associated_with_asn`]), exactly as an
//! address range's release is decided from the addresses still allocated out of it. The
//! operation that brings an ASN to IPAM is still a scaffold, so the only other writer today
//! is a test seeding one.
//!
//! ## Public IPv4 address pools
//!
//! "Provisioning an IPv4 or IPv6 address range … creates a corresponding address pool"
//! (`ProvisionByoipCidr`), so a provisioned IPv4 range gives the account one public IPv4
//! pool, held in [`ComAmazonawsEc2Amazonec2State::public_ipv4_pools`] as a
//! [`PublicIpv4PoolRecord`] under the `ipv4pool-ec2-…` id the provision minted — the same id
//! the range reports as its `ByoipCidr$PoolId`, so the range and the pool cannot name each
//! other differently, and the pool goes away with the range when the clock's release drops
//! it. What the two stores CAN differ on is contents: `DeprovisionPublicIpv4PoolCidr` takes a
//! range out of the pool while `DeprovisionByoipCidr` is still what gives the address range
//! itself back, which is the order AWS documents, so between those two calls the account
//! holds a range whose pool is empty.
//!
//! A record holds the pool's own facts (its description, its network border group, the pool
//! tags `ProvisionByoipCidrRequest$PoolTagSpecifications` asked for) beside the ADDRESS
//! RANGES provisioned to it, each a canonical IPv4 CIDR block
//! ([`canonical_public_ipv4_pool_cidr`]) — the one source of truth for what the pool holds.
//! Everything the modelled `crate::ops::types::PublicIpv4Pool` reports about those ranges is
//! DERIVED from them
//! on read by [`ComAmazonawsEc2Amazonec2State::public_ipv4_pool_view`] — each range's first
//! and last address, its address count, and how many of those addresses are still available,
//! which is the count less the account's Elastic IP addresses that fall inside the block
//! ([`ComAmazonawsEc2Amazonec2State::addresses_in_byoip_cidr`]) — so a count can never drift
//! from the ranges and the addresses it is a count of. `DeprovisionPublicIpv4PoolCidr` takes
//! one range back out of a pool, and [`public_ipv4_pool_payload_bytes`] is the one definition
//! of what a pool is charged against the byte budget.
//!
//! ## IPv6 address pools
//!
//! The other family of the same sentence: "provisioning an IPv4 or IPv6 address range …
//! creates a corresponding address pool" (`ProvisionByoipCidr`), so a provisioned IPv6 range
//! gives the account one IPv6 pool, held in [`ComAmazonawsEc2Amazonec2State::ipv6_pools`] as
//! an [`Ipv6PoolRecord`] under the `ipv6pool-ec2-…` id the provision minted — the same id the
//! range reports as its `ByoipCidr$PoolId`, so the range and the pool cannot name each other
//! differently, and the pool goes away with the range when the clock's release drops it.
//! Keyed by that id in a [`BTreeMap`], the arrangement every other pool store here uses: that
//! id order is the one stable order `DescribeIpv6Pools` reports and pages over, so a
//! pagination token can name the id the next page starts at.
//!
//! A record holds the pool's own facts (its description and the pool tags
//! `ProvisionByoipCidrRequest$PoolTagSpecifications` asked for) beside the ADDRESS RANGES
//! provisioned to it, each a canonical IPv6 CIDR block ([`canonical_ipv6_cidr`]) — the one
//! source of truth for what the pool holds, which
//! [`ComAmazonawsEc2Amazonec2State::ipv6_pool_view`] reports as the modelled
//! `Ipv6Pool$PoolCidrBlocks` in address order. [`ipv6_pool_payload_bytes`] is the one
//! definition of what such a pool is charged against the byte budget, given back when the
//! clock's release drops it.
//!
//! ## Elastic IP addresses
//!
//! [`ComAmazonawsEc2Amazonec2State::addresses`] holds the account's Elastic IP addresses
//! as the modelled [`Address`] shape itself, keyed by `AllocationId`, for the same reason
//! the VPC store holds [`Vpc`]s: `AllocateAddress` and `DescribeAddresses` cannot report
//! one address differently. The address itself is minted out of Amazon's pool by
//! [`ComAmazonawsEc2Amazonec2State::mint_elastic_ip`], never taken from the request, and
//! an allocation is reached by its public address through
//! [`ComAmazonawsEc2Amazonec2State::allocation_id_for_public_ip`] because the API lets a
//! caller name either. An Elastic IP address AWS hands back is usable at once — it has no
//! non-terminal status — so no entry beside it records a clock-owed transition.
//!
//! An address's ASSOCIATION with one of the account's resources lives on that same record
//! — `Address$AssociationId`, `Address$InstanceId` and `Address$PrivateIpAddress` — rather
//! than in a map beside it, so `AssociateAddress` and `DescribeAddresses` cannot disagree
//! about what an address is attached to. The instance reports the same address through
//! `Instance$PublicIpAddress`, so the ONE primitive that ends an association
//! ([`ComAmazonawsEc2Amazonec2State::detach_elastic_ip`]) clears both sides, and
//! [`ComAmazonawsEc2Amazonec2State::allocation_id_associated_with_instance`] answers which
//! address an instance holds.
//!
//! The address's `domain-name` attribute — its reverse DNS (PTR) record — is NOT on that
//! record, because the [`Address`] shape carries no member for it: it lives in
//! [`ComAmazonawsEc2Amazonec2State::address_ptr_records`] as an [`AddressPtrRecord`], the
//! record in effect beside the update a caller started and the instant that update falls
//! due. Unlike the allocation itself, such an update is NOT immediate — the modelled
//! `AddressAttribute$PtrRecordUpdate` is what reports one that has not been applied yet —
//! so the move is recorded ([`ComAmazonawsEc2Amazonec2State::start_address_ptr_record_update`])
//! and applied at its instant by
//! [`ComAmazonawsEc2Amazonec2State::apply_due_ptr_record_updates`] rather than slept for,
//! and while one is owed a second request on the same record is refused
//! ([`pending_request_already_exists`]).
//! [`ComAmazonawsEc2Amazonec2State::address_attribute_view`] is the ONE projection of that
//! state into the modelled `AddressAttribute` shape, so the operations that set, reset and
//! report the attribute cannot describe one address differently.
//!
//! ## Elastic IP address transfers
//!
//! [`ComAmazonawsEc2Amazonec2State::address_transfers`] holds the Elastic IP address
//! transfers the source account can see, as [`AddressTransferRecord`]s: the modelled
//! [`AddressTransfer`] every operation that reports one answers with, plus the instant the
//! record stops being one of that account's transfers
//! ([`AddressTransferRecord::visible_until`]), which the shape carries no member for.
//! Keyed by the `AllocationId` of the address being transferred, because an address has at
//! most one transfer at a time — `EnableAddressTransfer` starts the one offer on it — which
//! also gives the describe path one stable order to report and page over.
//!
//! The status on the record is what the two-step handshake writes:
//! [`ComAmazonawsEc2Amazonec2State::withdraw_address_transfer`] leaves a withdrawn offer
//! `disabled` and [`ComAmazonawsEc2Amazonec2State::accept_address_transfer`] leaves a taken
//! one `accepted` with the instant it was taken on it. An offer is found BY ADDRESS for the
//! accepting side ([`ComAmazonawsEc2Amazonec2State::address_transfer_on_offer`]), which is
//! what the accept names the transfer by.
//!
//! Unlike a `pending` VPC's move, those instants are DOCUMENTED spans rather than chosen
//! ones ([`ADDRESS_TRANSFER_OFFER_WINDOW`], [`ADDRESS_TRANSFER_ACCEPTED_VISIBILITY`]), so
//! they are kept on the wall clock the reported timestamps are on;
//! [`ComAmazonawsEc2Amazonec2State::expire_due_address_transfers`] drops a record whose
//! instant has passed, from [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`].
//!
//! ## Dedicated Hosts
//!
//! [`ComAmazonawsEc2Amazonec2State::dedicated_hosts`] holds the account's Dedicated Hosts
//! as the modelled [`Host`] shape itself, keyed by `HostId`, for the same reason the VPC
//! store holds [`Vpc`]s: `AllocateHosts` and the describe operations cannot report one
//! host differently. Which of those hosts is an EC2 Mac Dedicated Host is a fact of the
//! host's instance family rather than a stored flag ([`is_mac_instance_family`]), and the
//! macOS releases one can launch are a fact of that family's hardware
//! ([`mac_os_supported_versions`]) — so [`mac_host_view`] derives the `MacHost` shape
//! `DescribeMacHosts` reports from the one stored host, and no second record can disagree
//! with it.
//!
//! ## VPC endpoint connections
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_connections`] holds the connections to
//! the account's OWN VPC endpoint services — "the VPC endpoint connections to your VPC
//! endpoint services" (`DescribeVpcEndpointConnections`) — as the modelled
//! [`VpcEndpointConnection`] shape itself, keyed by `VpcEndpointId` in a [`BTreeMap`], for
//! the same reasons the VPC store holds [`Vpc`]s: one endpoint connects to exactly one
//! service, so the endpoint id identifies the connection, every member a writer sets
//! round-trips through the describe without a field-by-field copy, and the map's order is
//! the one order the describe reports and pages over. A fresh account has created no
//! endpoint service, so it has no connection either and the store starts empty.
//! [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_connection`] is the one way a
//! connection is stored, so the id it carries, the bytes it charges and the ceiling it
//! fits inside cannot be decided two ways: a connection is keyed by a REQUEST-SUPPLIED
//! endpoint id and nothing here reclaims one, so the insert refuses past
//! [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] (a writer asks
//! [`ComAmazonawsEc2Amazonec2State::has_vpc_endpoint_connection_capacity`] first and
//! refuses with EC2's own quota code) and every record costs at least
//! [`STORED_RECORD_OVERHEAD_BYTES`] against the byte budget, so repeated writes
//! cannot grow the store for free. A connection sits in `pendingAcceptance` until the
//! SERVICE OWNER accepts or rejects it (`AcceptVpcEndpointConnections` /
//! `RejectVpcEndpointConnections`), which is an action rather than an instant, so —
//! unlike a `pending` VPC — nothing beside it records a clock-owed transition until the
//! owner acts: once `AcceptVpcEndpointConnections` has, the connection is `pending` and
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_connection_moves`] holds the instant the
//! clock owes it `available`.
//!
//! ## VPC endpoint associations
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_associations`] holds the resources
//! associated with the account's VPC endpoints — "the VPC resources, VPC endpoint services,
//! Lattice services, or service networks associated with the VPC endpoint"
//! (`DescribeVpcEndpointAssociations`) — as the modelled [`VpcEndpointAssociation`] shape
//! itself, keyed by that shape's `Id` in a [`BTreeMap`], for the same reasons the
//! connection store above holds [`VpcEndpointConnection`]s: the id names the resource, every
//! member a writer sets round-trips through the describe, and the map's order is the one
//! order that describe reports and pages over.
//! [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_association`] is the one way one is
//! stored — it mints the association id itself rather than taking it from a caller, answers
//! `None` past [`MAX_STORED_RECORDS_PER_STORE`]
//! ([`ComAmazonawsEc2Amazonec2State::has_vpc_endpoint_association_capacity`] is the gate a
//! writer asks first) and charges [`vpc_endpoint_association_payload_bytes`] against the
//! byte budget, so repeated writes cannot grow the store for free.
//!
//! The store starts EMPTY, like the local gateway stores above and for the same kind of
//! reason: an association exists only for an endpoint that points at a resource
//! configuration or a service network, and `CreateVpcEndpoint` holds no store for either
//! today (it answers 501 for an endpoint of those types), so an account has no association
//! to report until it does. The insert is the seam that writer must go through when it
//! lands, not dead code.
//!
//! ## Connection notifications for endpoints and endpoint services
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_connection_notifications`] holds the
//! account's connection notifications — "the connection notifications for VPC endpoints and
//! VPC endpoint services" (`DescribeVpcEndpointConnectionNotifications`) — as the modelled
//! [`ConnectionNotification`] shape itself, keyed by `ConnectionNotificationId` in a
//! [`BTreeMap`], for the same reasons the connection store above holds
//! [`VpcEndpointConnection`]s: the id names the resource, every member a writer sets
//! round-trips through the describe, and the map's order is the one order that describe
//! reports and pages over. A notification is a resource a request creates, so a fresh
//! account holds none and the store starts empty.
//! [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_connection_notification`] is the one
//! way one is stored, and it mints the id itself rather than taking it from a caller; it
//! answers `None` past [`MAX_STORED_RECORDS_PER_STORE`] and charges
//! [`connection_notification_payload_bytes`] against the byte budget, so repeated writes
//! cannot grow the store for free. A notification has no non-terminal state — it is
//! `Enabled` or `Disabled`, and only `ModifyVpcEndpointConnectionNotification` moves it —
//! so nothing beside the store records a clock-owed transition for one.
//!
//! That pair of bounds — a count ceiling the writer itself re-checks, and a per-record byte
//! floor so a record carrying no tags is not free — is the rule for EVERY store nothing in
//! this emulator reclaims, not just this one. The writers of the transit gateway
//! attachments, multicast domains and route table announcements, the VPC endpoint services
//! and endpoints, and the IPAM pools answer `None` past [`MAX_STORED_RECORDS_PER_STORE`]
//! and charge [`STORED_RECORD_OVERHEAD_BYTES`] on top of the caller-chosen bytes they hold,
//! so repeated writes cannot grow this process's memory without bound whatever the records
//! carry. The Allowed AMIs settings are bounded the same way
//! ([`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] and [`image_criteria_payload_bytes`]).
//!
//! ## VPC endpoints, endpoint services, transit gateway attachments and IPAM pools
//!
//! Each of these is held as the modelled shape its own operations report —
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoints`],
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_services`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_attachments`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_multicast_domains`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_route_table_announcements`] and
//! [`ComAmazonawsEc2Amazonec2State::ipam_pools`] — keyed in a [`BTreeMap`] by the id that
//! names it, for the same reasons the VPC store
//! holds [`Vpc`]s. A fresh account holds none of them: every one is created by a request,
//! so each store starts empty, and the `insert_*` writer beside each is the ONE way a
//! record is stored so its id and its byte charge cannot be minted two ways. Where an
//! operation leaves a resource in a NON-TERMINAL state — an accepted attachment, an
//! announcement a delete started on, a range whose advertisement was asked for — the
//! record carries the [`PendingStateMove`] the clock owes it, which
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies — the BYOIP range
//! above included, whose record carries one beside the release it may owe.
//!
//! ## IPAM scopes and the pools created in them
//!
//! [`ComAmazonawsEc2Amazonec2State::ipam_scopes`] holds the account's IPAM scopes as the
//! modelled [`IpamScope`] shape itself, keyed by `IpamScopeId`, beside the
//! [`ComAmazonawsEc2Amazonec2State::ipam_pools`] created in them: a scope is "the
//! highest-level container within IPAM" (`IpamScope`), so `CreateIpamPool` resolves its
//! `IpamScopeId` there and takes the pool's IPAM, Region and scope type from the scope it
//! finds rather than from the request. A scope the account does not hold is refused with
//! [`invalid_ipam_scope_id_not_found`], and a scope's own "number of pools in the scope"
//! (`IpamScope$PoolCount`) is RECOUNTED from the pool store
//! ([`ComAmazonawsEc2Amazonec2State::insert_creating_ipam_pool`]) rather than tracked
//! separately, so the two cannot drift.
//!
//! A pool `CreateIpamPool` answers with is `create-in-progress`, a non-terminal
//! `IpamPoolState`, so the clock owes it `create-complete` one [`TRANSITION_DELAY`] later;
//! that due instant is a fact the modelled [`IpamPool`] carries no member for, so it lives
//! beside the pool under the same id in
//! [`ComAmazonawsEc2Amazonec2State::ipam_pool_create_complete_at`], the way a `pending`
//! VPC's does. The other end of the same pool's life is kept the same way: a pool
//! `DeleteIpamPool` left `delete-in-progress`
//! ([`ComAmazonawsEc2Amazonec2State::begin_ipam_pool_deletion`]) is owed `delete-complete`
//! through [`ComAmazonawsEc2Amazonec2State::ipam_pool_delete_complete_at`], and the pools
//! within it ([`ComAmazonawsEc2Amazonec2State::ipam_pools_within`]) and the CIDRs
//! provisioned to it ([`ComAmazonawsEc2Amazonec2State::ipam_pool_provisioned_cidrs`]) are
//! what that delete is held against; the scope that held it stops counting it
//! (`IpamScope$PoolCount`). A scope `CreateIpamScope` creates is the same story one level
//! up: it is
//! answered in the non-terminal `create-in-progress` `IpamScopeState` and its due instant
//! lives in [`ComAmazonawsEc2Amazonec2State::ipam_scope_create_complete_at`] — the two
//! DEFAULT scopes an IPAM comes up with are not listed there, because the one entry
//! [`ComAmazonawsEc2Amazonec2State::ipam_create_complete_at`] holds for the IPAM already
//! covers them.
//!
//! ## Network Access Scopes and their analyses
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_access_scopes`] holds the account's
//! Network Access Scopes as the modelled [`NetworkInsightsAccessScope`] shape itself, keyed
//! by `NetworkInsightsAccessScopeId`: it is where
//! `StartNetworkInsightsAccessScopeAnalysis` resolves the scope a caller asks to analyze,
//! so a scope the account does not hold is refused rather than analyzed.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_access_scope_contents`] holds the
//! CONTENT of each of those scopes — the modelled [`NetworkInsightsAccessScopeContent`]
//! shape itself — under the SAME `NetworkInsightsAccessScopeId`, because
//! [`NetworkInsightsAccessScope`] carries no member for the paths a scope is made of while
//! `CreateNetworkInsightsAccessScope` and `GetNetworkInsightsAccessScopeContent` both answer
//! with them. One writer stores the scope and its content together
//! ([`ComAmazonawsEc2Amazonec2State::insert_network_insights_access_scope`]), so a scope the
//! account holds always has exactly one content and the create cannot report paths the get
//! would not.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_access_scope_client_tokens`] maps one
//! create's `ClientToken` to the scope it created, so a replay of the same request answers
//! that scope instead of creating a second one.
//!
//! `DeleteNetworkInsightsAccessScope` takes a scope back out through the one removal path
//! ([`ComAmazonawsEc2Amazonec2State::remove_network_insights_access_scope`]), which drops the
//! content and the client token with it: [`NetworkInsightsAccessScope`] carries no state
//! member, so a deleted scope has no terminal state to be reported in and its id simply
//! stops resolving for every reader of this surface.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_access_scope_analyses`] holds the
//! analyses that start has started, keyed by `NetworkInsightsAccessScopeAnalysisId`, as
//! [`NetworkInsightsAccessScopeAnalysisRecord`]s: the modelled
//! [`NetworkInsightsAccessScopeAnalysis`] every operation that reports one answers with,
//! plus the move the clock owes it. An analysis AWS hands back is `running`, a non-terminal
//! `AnalysisStatus`, so its end is owed to the clock on the same [`TRANSITION_DELAY`] as
//! every other transition this module records, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it.
//!
//! That end is `failed`, carrying [`ANALYSIS_UNSUPPORTED_STATUS_MESSAGE`]: a Network Access
//! Scope analysis is a reachability computation over the account's real network, which this
//! emulator does not perform, so the analysis ends in the `AnalysisStatus` catalog's own
//! failure member with a message saying why rather than reporting findings nothing
//! computed.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_access_scope_analysis_client_tokens`]
//! maps one start's `ClientToken` to the analysis it started, so a replay of the same
//! request answers that analysis instead of starting a second one.
//!
//! `DeleteNetworkInsightsAccessScopeAnalysis` takes an analysis back out through the one
//! removal path
//! ([`ComAmazonawsEc2Amazonec2State::remove_network_insights_access_scope_analysis`]), which
//! drops the client token with it and gives the bytes the record held back to the store's
//! budget: `AnalysisStatus` has no member for a deleted analysis, so a deleted one has no
//! terminal state to be reported in and its id simply stops resolving for every reader of
//! this surface.
//!
//! ## Reachability Analyzer paths and the analyses started on them
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_paths`] holds the account's paths as
//! the modelled [`NetworkInsightsPath`] shape itself, keyed by `NetworkInsightsPathId` in a
//! [`BTreeMap`] so every reader of it reports one stable order: it is where
//! `StartNetworkInsightsAnalysis` resolves the path a caller asks to analyze, so a path the
//! account does not hold is refused with [`invalid_network_insights_path_id_not_found`]
//! rather than analyzed. [`ComAmazonawsEc2Amazonec2State::insert_network_insights_path`] is
//! its one writer, so every member a path reports is derived there rather than by each
//! caller, and [`ComAmazonawsEc2Amazonec2State::remove_network_insights_path`] is its one
//! remover, so the bytes a deleted path held go back to the store's budget with it.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_analyses`] holds the analyses started
//! on those paths, keyed by `NetworkInsightsAnalysisId`, as
//! [`NetworkInsightsAnalysisRecord`]s: the modelled [`NetworkInsightsAnalysis`] every
//! operation that reports one answers with, plus the move the clock owes it. An analysis AWS
//! hands back is `running`, a non-terminal `AnalysisStatus`, so its end is owed to the clock
//! on the same [`TRANSITION_DELAY`] as every other transition this module records, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it.
//!
//! That end is `failed`, carrying [`PATH_ANALYSIS_UNSUPPORTED_STATUS_MESSAGE`]: a path
//! analysis is a reachability computation over the account's real network, which this
//! emulator does not perform, so the analysis ends in the `AnalysisStatus` catalog's own
//! failure member with a message saying why, and never reports a `NetworkPathFound`, a path
//! component or an explanation that nothing computed.
//!
//! [`ComAmazonawsEc2Amazonec2State::network_insights_analysis_client_tokens`] maps one
//! start's `ClientToken` to the analysis it started, so a replay of the same request answers
//! that analysis instead of starting a second one.
//!
//! ## VPC Block Public Access exclusions
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_exclusions`] holds the
//! account's VPC BPA exclusions as [`VpcBlockPublicAccessExclusionRecord`]s, keyed by
//! `ExclusionId` in a [`BTreeMap`] so the describe path has one stable order and a
//! pagination token can name the id the next page starts at. The record is the modelled
//! [`VpcBlockPublicAccessExclusion`] every operation that reports one answers with, plus
//! the two facts that shape carries no member for: the VPC or subnet the exclusion is
//! applied to ([`ExclusionTarget`] — "a mode that can be applied to a single VPC or
//! subnet"), and the move the clock owes it ([`ExclusionTransition`]), since
//! `CreateVpcBlockPublicAccessExclusion` answers an exclusion in the non-terminal
//! `create-in-progress` state and the clock owes it `create-complete` one
//! [`TRANSITION_DELAY`] later — the same span, off the same monotonic clock, as an
//! instance's [`PendingTransition`] and a `pending` VPC's `available` move, applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`].
//!
//! ## VPC Block Public Access options
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_options`] holds the account's
//! VPC BPA options for one Region as a [`VpcBlockPublicAccessOptionsRecord`], keyed by that
//! Region: BPA is a SETTING of a Region rather than a resource, so there is no id to key it
//! by and no describe to page over, and a Region with no record at all is one the account has
//! never modified. The modelled shape both operations report is PROJECTED from the record
//! ([`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_options`]), the account id and
//! Region it carries being facts of the request and the emulator rather than of the setting.
//! `ModifyVpcBlockPublicAccessOptions` leaves the options in the non-terminal
//! `update-in-progress` state, so the clock owes them `update-complete` one
//! [`TRANSITION_DELAY`] later — the same span, off the same monotonic clock, as a
//! `create-in-progress` exclusion's own move, applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`].
//!
//! ## NAT gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::nat_gateways`] holds the account's NAT gateways as the
//! modelled [`NatGateway`] shape itself, keyed by `NatGatewayId` in a [`BTreeMap`], for the
//! same reasons the VPC store holds [`Vpc`]s. An address of a NAT gateway is a PART of the
//! gateway rather than a resource of its own — `NatGatewayAddress` has no store of its own
//! and is reached only through the gateway that holds it — so the addresses live on that
//! shape, and the Elastic IP address each one names stays the
//! [`ComAmazonawsEc2Amazonec2State::addresses`] record it already was, now carrying the
//! association: one address, one record, whichever operation reports it.
//! `AssociateNatGatewayAddress` leaves a new address in the non-terminal `associating`
//! status, so the clock owes it `succeeded` one [`TRANSITION_DELAY`] later
//! ([`NatGatewayAddressTransition`], keyed by the address's `AssociationId` beside the
//! gateway store because the modelled shape carries no member for when a transient status
//! resolves).
//!
//! ## AMIs: the published catalog, plus whatever a writer stores
//!
//! [`ComAmazonawsEc2Amazonec2State::images`] holds the AMIs the account can see as the
//! modelled [`Image`] shape itself, keyed by `ImageId`, for the same reason the VPC store
//! holds [`Vpc`]s: every member a writer sets round-trips through `DescribeImages` without
//! a field-by-field copy that can silently drop one. Nothing in the EC2 API creates
//! Amazon's own AMIs — "the images available to you include public images"
//! (`DescribeImages`) — so the store starts with the public images Amazon publishes
//! ([`published_image_specs`]), the same way it starts with the default VPC AWS creates
//! for a fresh account, and an image a later operation registers goes beside them through
//! [`ComAmazonawsEc2Amazonec2State::insert_image`]. Each one's `CreationDate` is the build
//! date its published name carries, so it is the same across reads and across runs and the
//! newest image of a name pattern is always the same image.
//!
//! ## Allowed AMIs is an account-and-Region SETTING, not a resource
//!
//! [`ComAmazonawsEc2Amazonec2State::allowed_images_settings`] holds the Allowed AMIs
//! setting of the account "at the account level in the specified Region"
//! (`GetAllowedImagesSettings`) as an [`AllowedImagesSettings`], keyed by the Region the
//! request that wrote it was signed for ([`request_region`]) — the one thing a request
//! says about the Region it is asking in. It is not a resource: nothing mints or deletes
//! one, so a Region nobody has configured has NO entry and reports
//! [`AllowedImagesSettings::default`] — the `disabled` state with no criteria, which is
//! what an account that has never enabled the feature has. `GetAllowedImagesSettings`
//! therefore writes nothing at all, the way `DescribeAvailabilityZones` does.
//!
//! The two facts the setting carries move independently, which is why one record holds
//! both: `EnableAllowedImagesSettings` / `DisableAllowedImagesSettings` move the state and
//! leave the criteria alone (`GetAllowedImagesSettings` reports "the current state of the
//! Allowed AMIs setting AND the list of Allowed AMIs criteria" whichever state it is in),
//! and `ReplaceImageCriteriaInAllowedImagesSettings` replaces the criteria and leaves the
//! state alone. The criteria are stored as the modelled [`ImageCriterion`] shape the get
//! reports, so the request shapes a replace carries are converted once, where they are
//! validated, rather than at every read.
//!
//! ## The ambient default network of a Region
//!
//! "When you create an AWS account, we create a default VPC for you in each AWS Region"
//! and "when we create a default VPC, we create a default subnet in each Availability Zone
//! in the Region" (Amazon VPC User Guide, "Default VPCs"), so the network an account
//! already holds before it has created anything is a fact OF THE REGION a request is
//! signed for — not of one compile-time Region. A subnet's zone and its ARN name a Region,
//! so a Region's ambient network has to be the one whose zones that Region HAS
//! ([`availability_zones_in_region`]).
//!
//! [`ComAmazonawsEc2Amazonec2State::ensure_default_network`] is the ONE place it is
//! materialised — a default VPC at [`DEFAULT_VPC_CIDR_BLOCK`], one default subnet per zone
//! of that Region, and that VPC's default security group — and it is idempotent per
//! Region: the Region's `VpcId` is recorded, so the second call in a Region answers the
//! same ids instead of seeding a second copy of the same network. The store itself is per
//! ACCOUNT, not per Region (a `Vpc` carries no Region member, so nothing in `DescribeVpcs`
//! could say which Region its VPC belongs to), which is why the Region a resource belongs
//! to is the one its zone and its ARN name — the Region of the request that wrote it.
//!
//! A Region's record is never removed, so a default VPC a caller DELETED is not seeded
//! again behind its back: AWS does not recreate one either, which is what
//! `CreateDefaultVpc` is for —
//! [`ComAmazonawsEc2Amazonec2State::create_default_network`], which writes the SAME network
//! through the same primitive and overwrites the Region's record with the new VPC's id, so
//! an account that deleted its default VPC gets one back only by asking. And only a Region
//! of the Region catalog
//! ([`regions_in_partition_of`]) gets one, the Region in a credential scope being
//! caller-controlled text: an account has a default VPC in the Regions that EXIST, not in
//! every string shaped like a Region code.
//!
//! ## Route servers
//!
//! [`ComAmazonawsEc2Amazonec2State::route_servers`] holds the account's VPC route servers
//! as [`RouteServerRecord`]s: the modelled [`RouteServer`] shape every operation that
//! reports one answers with, plus the instants the clock owes it its `pending` ->
//! `available` move, its `modifying` -> `available` move once a configuration change has
//! been asked for, and the move its route persistence is within (`enabling` -> `enabled`,
//! `disabling` -> `disabled`, or a `resetting` that withdraws the persisted routes and
//! leaves the configuration as it was), and the move a delete owes it (`deleting` ->
//! `deleted`). Those words are facts of the record's age, so
//! [`RouteServerRecord::view_at`] derives them on read instead of letting two writers
//! store two different ones. A deleted route server STAYS in the store, reported in the
//! terminal `deleted` state the model gives it, so a caller waiting for its delete to
//! finish can observe it; what no longer admits a call is asked through
//! [`require_live_route_server`]. Keyed by `RouteServerId` in a [`BTreeMap`], the same stable
//! order the VPC and volume stores are kept in, so a pagination token can name the id the
//! next page starts at.
//!
//! [`ComAmazonawsEc2Amazonec2State::route_server_associations`] holds the connections
//! between those route servers and the account's VPCs as
//! [`RouteServerAssociationRecord`]s, keyed by the (route server, VPC) pair that IS the
//! association — the modelled [`RouteServerAssociation`] shape carries no id of its own — so
//! one pair can hold only one association and `GetRouteServerAssociations` reports one route
//! server's associations in VPC order. The association's `State` is a fact of the record's
//! age, derived by [`RouteServerAssociationRecord::view_at`] like a route server's own two
//! states.
//!
//! [`ComAmazonawsEc2Amazonec2State::route_server_propagations`] holds which route tables
//! each of those route servers may install its learned routes into, keyed by the (route
//! server, route table) pair that IS the propagation — see the
//! [`route_server_propagations`] module, which owns that store's record, its derived states
//! and the association rule a propagation is only valid under.
//!
//! ## Account-level settings
//!
//! A setting is not a resource: it has no id, nothing creates or deletes it, and an
//! account that has never touched it still has a value for it. So
//! [`ComAmazonawsEc2Amazonec2State::snapshot_block_public_access`] is a single field
//! holding the one word the *block public access for snapshots* setting is in —
//! [`SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED`] for an account that has never enabled it —
//! rather than a map keyed by anything. The value stored is always a member of the
//! modelled `SnapshotBlockPublicAccessState` catalog ([`snapshot_block_public_access_mode`]
//! is what turns a request's word into one), never the request's own text, and the field
//! is account-wide because this store is per ACCOUNT and not per Region (see "The default
//! VPC's default subnets" below).
//!
//! The *block public access for AMIs* setting is the same kind of single-word setting, with
//! one difference: it is a setting OF A REGION — "block public access for AMIs at the
//! account level in the specified Amazon Web Services Region"
//! (`GetImageBlockPublicAccessState`) — so
//! [`ComAmazonawsEc2Amazonec2State::image_block_public_access`] is keyed by the Region of
//! the request that set it, the way the EBS encryption-by-default set below is, and a Region
//! with no entry is one the setting has never been enabled in
//! ([`IMAGE_BLOCK_PUBLIC_ACCESS_UNBLOCKED`]), which is also where a disable puts a Region
//! back. Like the snapshot setting it has no clock-owed move: the mode an enable asks for is
//! the mode the next read reports, which is what the pair of operations is observed to do.
//!
//! The account-level instance metadata service (IMDS) defaults are the same kind of thing,
//! with one difference: each of their settings can also be in NO state at all. "To remove a
//! parameter's account-level default setting, specify `no-preference`. If an account-level
//! setting is cleared with `no-preference`, then the instance launch considers the other
//! instance metadata settings" (`ModifyInstanceMetadataDefaults`) — so a setting is either
//! a member of its own catalog or absent, which is what
//! [`ComAmazonawsEc2Amazonec2State::instance_metadata_defaults`] holds as an
//! [`AccountInstanceMetadataDefaults`] of `Option`s. The enum settings hold `&'static str`
//! catalog members for the same reason the snapshot setting does, and
//! [`AccountInstanceMetadataDefaults::account_level`] is the one projection into the
//! modelled `InstanceMetadataDefaultsResponse`, so `ModifyInstanceMetadataDefaults` and
//! `GetInstanceMetadataDefaults` cannot disagree about what the account's defaults are.
//!
//! The DEFAULT CREDIT SPECIFICATION of a burstable instance family is a setting of a Region
//! like the two above, with one difference: it is held per instance family rather than once
//! — "The default credit option is set at the account level per Amazon Web Services Region,
//! and is specified per instance family" (`ModifyDefaultCreditSpecification`). So
//! [`ComAmazonawsEc2Amazonec2State::default_credit_specifications`] is keyed by Region and
//! then by family, and a family with no entry is one the account has never modified there,
//! which is reported at AWS's own default for that family ([`default_cpu_credits`]) rather
//! than stored. [`ComAmazonawsEc2Amazonec2State::default_credit_specification`] is the one
//! projection into the modelled `InstanceFamilyCreditSpecification` both operations answer
//! with, so `ModifyDefaultCreditSpecification` and `GetDefaultCreditSpecification` cannot
//! report one family's default two ways.
//!
//! The MANAGED RESOURCE VISIBILITY default is a single-word setting of the same kind as the
//! snapshot one, and account-wide for the same reason this store is: "visibility settings
//! are account-wide and affect all IAM principals uniformly"
//! (`ModifyManagedResourceVisibility`), with no Region in its documentation. So
//! [`ComAmazonawsEc2Amazonec2State::managed_resource_default_visibility`] is one
//! `&'static str` holding a member of the modelled `ManagedResourceDefaultVisibility`
//! catalog ([`managed_resource_default_visibility_word`] is what turns a request's word into
//! one), [`MANAGED_RESOURCE_VISIBILITY_VISIBLE`] for an account that has never modified it.
//! [`ComAmazonawsEc2Amazonec2State::managed_resource_visibility`] is the one projection into
//! the modelled [`ManagedResourceVisibilitySettings`] shape both operations answer with, so
//! `ModifyManagedResourceVisibility` and `GetManagedResourceVisibility` cannot report one
//! account's setting two ways. Like the snapshot setting it has no clock-owed move: the word
//! a modify asks for is the word the next read reports.
//!
//! ## The Spot Instance data feed
//!
//! [`ComAmazonawsEc2Amazonec2State::spot_datafeed_subscription`] is a single
//! [`SpotDatafeedSubscription`] rather than a map, because the resource is a SINGLETON:
//! "You can create one data feed per Amazon Web Services account"
//! (`CreateSpotDatafeedSubscription`). It is a resource and not a setting, though — an
//! account that has never created one HAS none, which is why the field is an `Option` and
//! why the reporting operation refuses with
//! [`INVALID_SPOT_DATAFEED_NOT_FOUND`] instead of answering a default. The one record is
//! stored as the modelled shape the three operations report, so a create answers exactly
//! what a later describe does.
//!
//! ## Longer-ID (17-character ID) format settings
//!
//! [`ComAmazonawsEc2Amazonec2State::id_format_overrides`] holds the longer-ID preference
//! each principal has EXPLICITLY set, keyed by principal ARN and then by resource type:
//! exactly the set `DescribePrincipalIdFormat` reports beside the account's root user
//! ("all IAM roles and IAM users that have explicitly specified a longer ID
//! (17-character ID) preference"), and what `ModifyIdentityIdFormat` writes. A principal
//! that has set nothing is NOT a row here, because it is not a resource anybody created:
//! "all IAM roles and IAM users default to the same ID settings as the root user, unless
//! they explicitly override the settings", so
//! [`ComAmazonawsEc2Amazonec2State::use_long_ids_for`] derives one principal's setting
//! from its own entry, then the root user's, then [`DEFAULT_USE_LONG_IDS`] — rather than
//! writing a row per principal and per type that two operations could then disagree
//! about. The resource-type key is a `&'static str` out of
//! [`LONGER_ID_RESOURCE_TYPES`], so only a type the model publishes is ever stored.
//! [`ComAmazonawsEc2Amazonec2State::identity_id_formats`] is the one projection of a
//! principal's settings into the modelled `IdFormat` shape, which both
//! `DescribeIdentityIdFormat` and the per-principal statuses of
//! [`ComAmazonawsEc2Amazonec2State::principal_id_formats`] answer with, so no two
//! operations report one principal's setting two ways.
//!
//! ## The default security group of a VPC
//!
//! "Your VPC automatically comes with a default security group. … you can't delete a
//! default security group" (Amazon VPC User Guide, "Default security groups for your
//! VPCs"), so [`ComAmazonawsEc2Amazonec2State::security_groups`] starts with the default
//! VPC's default group rather than empty: the ambient network a fresh account has is a
//! VPC, its per-zone default subnets AND that group, and a client provisioning into the
//! default VPC reads all three before it can name a network. The store holds the modelled
//! [`SecurityGroup`] shape itself, keyed by `GroupId` in a [`BTreeMap`], for the same
//! reasons the VPC and subnet stores do. Its RULES are resources of their own, with ids of
//! their own — see [`ComAmazonawsEc2Amazonec2State::security_group_rules`].
//!
//! ## Transit gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::transit_gateways`] holds the account's transit
//! gateways as the modelled [`TransitGateway`] shape itself, keyed by `TransitGatewayId`,
//! for the same reasons the VPC store does — every member a writer sets round-trips
//! through `DescribeTransitGateways` without a field-by-field copy. The clock-owed
//! `pending` -> `available` move lives beside it in
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_available_at`], on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as a VPC's.
//!
//! The default transit gateway route table a create makes ("When you create a transit
//! gateway, we create a default transit gateway route table and use it as the default
//! association route table and the default propagation route table",
//! `CreateTransitGateway`) is a resource of its own, so it is stored as one in
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_route_tables`] rather than existing
//! only as an id inside the gateway's options: the ids
//! `TransitGatewayOptions$AssociationDefaultRouteTableId` and
//! `$PropagationDefaultRouteTableId` report name that record.
//!
//! The ADDITIONAL route tables of a gateway ("You can use CreateTransitGatewayRouteTable to
//! create additional transit gateway route tables", `CreateTransitGateway`) live in that
//! same map, so one reader reports every route table of a gateway whichever call made it.
//! An additional table holds neither default role, and it comes up on a due instant of its
//! own ([`ComAmazonawsEc2Amazonec2State::transit_gateway_route_table_available_at`]) rather
//! than with its gateway, which may have been `available` before the table was asked for.
//!
//! ## Virtual private gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::vpn_gateways`] holds the account's virtual private
//! gateways as [`VpnGatewayRecord`]s, keyed by `VpnGatewayId` in a [`BTreeMap`]: the
//! modelled [`VpnGateway`] shape every operation that reports one answers with — so a
//! member a writer sets round-trips through `DescribeVpnGateways` without a field-by-field
//! copy — plus the one thing that shape carries no member for, the [`PendingStateMove`] the
//! clock owes the gateway.
//!
//! `CreateVpnGateway` answers a gateway in the non-terminal `pending` state, so the clock
//! owes it `available` one [`TRANSITION_DELAY`] later, and `DeleteVpnGateway` leaves it in
//! the non-terminal `deleting` state with `deleted` owed the same way — `VpnState`'s four
//! members are `pending`, `available`, `deleting` and `deleted`, so a deleted gateway is
//! still a gateway the API reports rather than a record that vanishes.
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies both moves.
//!
//! ## Transit gateway policy tables, their associations and their entries
//!
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_policy_tables`] holds the account's
//! policy tables as [`TransitGatewayPolicyTableRecord`]s, keyed by
//! `TransitGatewayPolicyTableId` in a [`BTreeMap`]: the modelled [`TransitGatewayPolicyTable`]
//! every operation that reports one answers with, plus the two things that shape carries no
//! member for — the attachments associated with the table, keyed by
//! `TransitGatewayAttachmentId`, each as the modelled
//! [`TransitGatewayPolicyTableAssociation`] its own report answers with beside the
//! [`PendingStateMove`] the clock owes it, and the table's ENTRIES, keyed by the rule number
//! that identifies one within it, each as the modelled [`TransitGatewayPolicyTableEntry`]
//! `GetTransitGatewayPolicyTableEntries` reports. An association is per (table, attachment),
//! so that is what the key is, and the order it gives is the order
//! `GetTransitGatewayPolicyTableAssociations` reports and pages over; what an entry is stored
//! as, and the one writer of one, are
//! [`crate::state::transit_gateway_policy_table_entries`].
//!
//! `AssociateTransitGatewayPolicyTable` answers an association in the non-terminal
//! `associating` state, so the clock owes it `associated` one [`TRANSITION_DELAY`] later,
//! on the same monotonic clock as every other move this store owes, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it. An attachment
//! is associated with at most ONE policy table
//! ([`ComAmazonawsEc2Amazonec2State::policy_table_of_attachment`]), so a second association
//! of the same attachment is refused rather than stored beside the first.
//!
//! Which attachment is ASSOCIATED with which of those route tables is a resource of its own
//! — "Associates the specified attachment with the specified transit gateway route table"
//! (`AssociateTransitGatewayRouteTable`) — so
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_associations`] holds one
//! [`TransitGatewayRouteTableAssociationRecord`] per (route table, attachment) pair, keyed
//! and ordered exactly like the propagations below, which is the order
//! `GetTransitGatewayRouteTableAssociations` reports and pages over. An association is
//! answered in the non-terminal `associating` state, so the clock owes it `associated` one
//! [`TRANSITION_DELAY`] later and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies that move. "You can
//! associate only one route table with an attachment" (`AssociateTransitGatewayRouteTable`),
//! so a second association of an attachment that already has one
//! ([`ComAmazonawsEc2Amazonec2State::transit_gateway_route_table_association_of_attachment`])
//! is refused rather than stored beside the first — and that one record is also what a report
//! of the ATTACHMENT names as its association.
//!
//! Which attachment propagates its routes into which of those route tables is a resource of
//! its own too — "Enables the specified attachment to propagate routes to the specified
//! propagation route table" (`EnableTransitGatewayRouteTablePropagation`) — so
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_propagations`] holds one
//! [`TransitGatewayPropagationRecord`] per (route table, attachment) pair, which is exactly
//! as many as there can be: a pair either propagates or does not. The record carries the
//! `TransitGatewayPropagationState` reached so far plus the move the clock owes it
//! ([`PropagationMove`]) — `enabling` -> `enabled` after an enable, and the REMOVAL of the
//! record after a disable, since a pair that no longer propagates is not a propagation the
//! route table has at all. Both fall due on the same monotonic clock and at the same
//! [`TRANSITION_DELAY`] as every other move this store owes, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them.
//!
//! A prefix list reference is a ROUTE of one of those route tables — "Creates a reference
//! (route) to a prefix list in a specified transit gateway route table"
//! (`CreateTransitGatewayPrefixListReference`) — and a route table has at most one route per
//! prefix list, so [`ComAmazonawsEc2Amazonec2State::transit_gateway_prefix_list_references`]
//! holds one [`TransitGatewayPrefixListReferenceRecord`] per (route table, prefix list) pair.
//! Each record keeps the modelled [`TransitGatewayPrefixListReference`] every operation that
//! reports one answers with, so the create and the read cannot describe one reference
//! differently, plus the move the clock owes it: a create answers `pending`
//! ([`PREFIX_LIST_REFERENCE_STATE_PENDING`]) and the clock owes it `available`
//! [`TRANSITION_DELAY`] later, on the same clock as every other move above. A delete is the
//! other end of that life: the reference answers `deleting`
//! ([`PREFIX_LIST_REFERENCE_STATE_DELETING`]) and the clock owes its REMOVAL
//! ([`PrefixListReferenceMove::Removed`]) one span later, because a pair the table no longer
//! routes is not a route of it at all.
//!
//! ## Transit gateway metering policies
//!
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_metering_policies`] holds the account's
//! transit gateway metering policies as the modelled [`TransitGatewayMeteringPolicy`] shape
//! itself rather than a parallel record — the arrangement
//! [`ComAmazonawsEc2Amazonec2State::vpcs`] uses, and for the same reason: every member a
//! writer sets round-trips through `DescribeTransitGatewayMeteringPolicies` without a
//! field-by-field copy that can silently drop one, and a member nothing recorded is reported
//! absent rather than invented by the reader. Keyed by `TransitGatewayMeteringPolicyId` in a
//! [`BTreeMap`], which gives the describe path one stable order and lets a pagination token
//! name the id the next page starts at.
//! [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_metering_policy`] is the one way
//! one is stored, so the id mint and the bytes a stored policy is charged
//! ([`transit_gateway_metering_policy_payload_bytes`]) cannot differ between writers, and
//! [`ComAmazonawsEc2Amazonec2State::insert_pending_transit_gateway_metering_policy`] is what
//! a CREATE goes through: it records the policy in the transient
//! [`TRANSIT_GATEWAY_METERING_POLICY_STATE_PENDING`] state with
//! [`TRANSIT_GATEWAY_METERING_POLICY_STATE_AVAILABLE`] owed one [`TRANSITION_DELAY`] later,
//! held in [`ComAmazonawsEc2Amazonec2State::transit_gateway_metering_policy_moves`] on the
//! same monotonic clock as every other move this store owes.
//!
//! ## Transit gateway Connect peers
//!
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_connect_peers`] holds the Connect peers
//! of the account's Connect attachments as [`TransitGatewayConnectPeerRecord`]s: the modelled
//! [`TransitGatewayConnectPeer`] every operation that reports one answers with, plus the move
//! the clock owes a fresh one, since `CreateTransitGatewayConnectPeer` answers a peer in the
//! non-terminal `pending` state and the clock owes it `available` one [`TRANSITION_DELAY`]
//! later — the same span, off the same monotonic clock, as every other move this store owes.
//! Keyed by `TransitGatewayConnectPeerId` in a [`BTreeMap`], so the describe path has one
//! stable order and a pagination token can name the id the next page starts at.
//!
//! A peer belongs to the Connect attachment it peers over, which lives in the one attachment
//! store as [`TransitGatewayAttachmentView::Connect`] — so the gateway a peer's addresses come
//! from is reached through that attachment rather than copied onto the peer.
//!
//! ## Capacity Blocks
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_blocks`] holds the account's Capacity Blocks
//! as [`CapacityBlockRecord`]s: the modelled [`CapacityBlock`] every operation that reports
//! one answers with, plus the state a writer pinned it to, keyed by `CapacityBlockId` in a
//! [`BTreeMap`] — the same stable order the VPC, subnet and volume stores are kept in, so a
//! pagination token can name the id the next page starts at. A block's `State` is NOT stored
//! on the view: "A Capacity Block expires" at its `EndDate` and its reserved window opens at
//! its `StartDate` (`CapacityBlock$EndDate` / `$StartDate`), so `scheduled` -> `active` ->
//! `expired` is a fact of where the WALL clock stands in that window and
//! [`CapacityBlockRecord::state_at`] derives it on read instead of letting two writers store
//! two different words for it. Only the states no clock drives — a block AWS could not
//! deliver, one that was cancelled, one whose payment is pending or failed — are stored, as
//! [`CapacityBlockRecord::pinned_state`].
//!
//! A block and the Capacity Reservation that holds its capacity are ONE purchase rather than
//! two resources a caller creates separately, so
//! [`ComAmazonawsEc2Amazonec2State::insert_purchased_capacity_block`] writes the pair
//! together and is the one place the two sides of their link are made: the block's
//! `CapacityReservationIds` and the reservation's `CapacityBlockId` therefore always name
//! records this store really holds. The upfront payment of a purchase is not instant — the
//! `payment-pending` member of `CapacityBlockResourceState` is where a block sits while "the
//! upfront payment has not been processed yet" (`CapacityReservation$State`) — so both halves
//! are stored in that state and reach the state their reserved window puts them in one
//! [`TRANSITION_DELAY`] later: the block through [`CapacityBlockRecord::payment_settles_at`],
//! the reservation through the [`CapacityReservationTransition`] on its own record. Neither
//! move is written by whoever next calls; both are derived from the instant they fell due.
//!
//! ## IPAM
//!
//! [`ComAmazonawsEc2Amazonec2State::ipams`] holds the account's IP Address Manager
//! instances as the modelled [`Ipam`] shape itself, keyed by `IpamId` in a [`BTreeMap`],
//! for the same reason the VPC store holds [`Vpc`]s: `CreateIpam` and `DescribeIpams`
//! cannot report one IPAM differently, and the map's order is the one order the describe
//! reports and pages over. Creating an IPAM creates FOUR resources, not one — "an IPAM
//! contains two default scopes" (`IpamScope`) and "the default resource discovery is the
//! resource discovery automatically created when you create an IPAM"
//! ([`IpamResourceDiscovery`]`$IsDefault`), which is then associated with the IPAM
//! ([`IpamResourceDiscoveryAssociation`]`$IsDefault`) — so
//! [`ComAmazonawsEc2Amazonec2State::ipam_scopes`],
//! [`ComAmazonawsEc2Amazonec2State::ipam_resource_discoveries`] and
//! [`ComAmazonawsEc2Amazonec2State::ipam_resource_discovery_associations`] hold those
//! beside it, each as its own modelled shape keyed by its own id, and
//! [`ComAmazonawsEc2Amazonec2State::insert_ipam`] writes all four at once so the ids the
//! `Ipam` reports always name records this store really holds.
//!
//! Each of the four is answered in a non-terminal state (`create-in-progress`,
//! `associate-in-progress`), so the clock owes all four their completed state one
//! [`TRANSITION_DELAY`] later; the due instant is recorded in
//! [`ComAmazonawsEc2Amazonec2State::ipam_create_complete_at`], on the same monotonic clock
//! as an instance's [`PendingTransition`] and a `pending` VPC's
//! [`ComAmazonawsEc2Amazonec2State::vpc_available_at`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it.
//!
//! A resource discovery can also be associated with a FURTHER IPAM — "IPAM aggregates the
//! resource CIDRs discovered by the associated resource discovery"
//! ([`IpamResourceDiscoveryAssociation`]) — which is one more record in that same
//! association store, written by
//! [`ComAmazonawsEc2Amazonec2State::insert_ipam_resource_discovery_association`], the one
//! writer of a caller-made association. It too is answered in a non-terminal state, so its
//! own due instant lives in
//! [`ComAmazonawsEc2Amazonec2State::ipam_resource_discovery_association_complete_at`] — the
//! IPAM's entry above covers only the default association that came up with it.
//!
//! ## IPAM prefix list resolvers and the targets created against them
//!
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolvers`] holds the account's prefix
//! list resolvers as the modelled [`IpamPrefixListResolver`] shape itself, keyed by
//! `IpamPrefixListResolverId`, beside the
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_targets`] created against them:
//! "an IPAM prefix list resolver target is an association between a specific
//! customer-managed prefix list and an IPAM prefix list resolver"
//! ([`IpamPrefixListResolverTarget`]), so `CreateIpamPrefixListResolverTarget` resolves BOTH
//! ends in this store — the resolver here and the prefix list in
//! [`ComAmazonawsEc2Amazonec2State::managed_prefix_lists`] — and refuses an id either store
//! does not hold ([`invalid_ipam_prefix_list_resolver_id_not_found`],
//! [`invalid_prefix_list_id_not_found`]). The association is recorded on the prefix list as
//! well as on the target, because "when set, this prefix list becomes an IPAM managed prefix
//! list" ([`crate::ops::types::ManagedPrefixList`]`$IpamPrefixListResolverTargetId`) is a
//! SINGLE id: one prefix list is the target of at most one resolver, which
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_target_of_prefix_list`] reads
//! back.
//!
//! A target that create answers with is `create-in-progress`, a non-terminal
//! `IpamPrefixListResolverTargetState`, so the clock owes it `create-complete` one
//! [`TRANSITION_DELAY`] later; that due instant is a fact the modelled shape carries no member
//! for, so it lives in
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_target_create_complete_at`], the
//! way an IPAM scope's does, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it — and the
//! resolver a create answers is owed the same move through
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_create_complete_at`].
//!
//! A resolver a `DeleteIpamPrefixListResolver` answers is `delete-in-progress`, another
//! non-terminal member of that catalog, so the clock owes it `delete-complete` through
//! [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_delete_complete_at`] —
//! see [`begin_ipam_prefix_list_resolver_deletion`].
//!
//! ## Capacity Reservations
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_reservations`] holds the account's Capacity
//! Reservations as [`CapacityReservationRecord`]s: the modelled [`CapacityReservation`]
//! every operation that reports one answers with, plus the move the clock owes it
//! ([`CapacityReservationTransition`]), keyed by `CapacityReservationId` in a [`BTreeMap`]
//! — the same stable order every other store is kept in.
//! [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation`] is the one place a record
//! is built, so the id, the ARN, the owning account, the end type and the available count a
//! fresh reservation starts at are decided once however many writers there are. A
//! reservation's expiry is NOT stored: "The Capacity Reservation's state changes to
//! `expired` when it reaches its end date and time" (`CapacityReservation$EndDate`), so
//! [`CapacityReservationRecord::state_at`] derives it from that date on read, the way
//! [`CapacityBlockRecord::state_at`] derives a Capacity Block's window.
//!
//! An INTERRUPTIBLE reservation is one of these records too, not a second kind of resource:
//! `CreateInterruptibleCapacityReservationAllocation` moves unused capacity out of a source
//! reservation into a new one that carries `Interruptible` and an `InterruptionInfo` naming
//! that source, while the source carries the `InterruptibleCapacityAllocation` naming what
//! was allocated and the reservation it went to. The allocation's `pending` -> `active`
//! move, and the new reservation's own `pending` -> `active`, fall due on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as an instance's [`PendingTransition`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them.
//!
//! Capacity also moves BETWEEN two of these records, with no new resource created and no
//! clock-owed move at either end: `MoveCapacityReservationInstances` takes a number of
//! available instances out of one `active` reservation and adds them to another, so each
//! side's `TotalInstanceCount` and `AvailableInstanceCount` change together and the sum of
//! the two is conserved. The move is held against the `ClientToken` it was asked under in
//! [`ComAmazonawsEc2Amazonec2State::capacity_reservation_move_client_tokens`]
//! ([`CapacityReservationInstanceMove`]), so a replay reports it rather than moving the
//! capacity a second time.
//!
//! A FUTURE-DATED reservation is one of these records too: it carries the `StartDate` the
//! request asked for, the [`CapacityReservationCommitment`] it committed to and its
//! `DeliveryPreference`, and it starts in `assessing` — "Amazon EC2 is assessing your
//! request for a future-dated Capacity Reservation" (`CapacityReservation$State`) — with
//! the move to `scheduled` owed one [`TRANSITION_DELAY`] later, the span every other move
//! above is owed. That move and the delivery after it are both reported off the WALL clock
//! rather than from a word a writer stored, because the instants a read compares them
//! against are wall-clock ones ([`ClockTransition::at`] and the reservation's own
//! `StartDate`): [`CapacityReservationRecord::state_at`] reports a reservation whose
//! assessment has fallen due as `scheduled`, and one whose `StartDate` has arrived as
//! `active`, the way it reports one whose `EndDate` has passed as `expired`.
//!
//! ## Capacity Reservation cancellation quotes
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_reservation_cancellation_quotes`] holds the
//! quotes `CreateCapacityReservationCancellationQuote` generated, as
//! [`CapacityReservationCancellationQuoteRecord`]s: the modelled
//! [`CapacityReservationCancellationQuote`] the create and
//! `DescribeCapacityReservationCancellationQuotes` both answer with, plus the move the clock
//! owes it, keyed by `CapacityReservationCancellationQuoteId` in a [`BTreeMap`] — the order
//! the describe reports and pages over.
//! [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation_cancellation_quote`] is the
//! one place a record is built, so the id, the creation instant and the instant the quote
//! stops being usable are decided once however many writers there are.
//!
//! A quote is a DOCUMENT rather than a resource that tracks its reservation: its
//! `CurrentConfiguration` and `CancellationTerms` are the terms as of the instant it was
//! generated, which is what makes them quotable, and nothing a later call does to the
//! reservation rewrites them. Only its `QuoteState` moves, and both moves are the clock's:
//! it is generated `pending` and the clock owes it `active` one [`TRANSITION_DELAY`] later
//! ([`ClockTransition`]), after which it is `expired` from the instant it stops being valid
//! — "Cancellation quotes are valid for 24 hours"
//! (`CreateCapacityReservationCancellationQuote`), carried on the record as its
//! `ExpirationTime`. Neither is stored as a word a reader has to write:
//! [`CapacityReservationCancellationQuoteRecord::quote_state_at`] derives both from those
//! instants, the way [`CapacityReservationRecord::state_at`] derives a reservation's expiry.
//!
//! ## Capacity Reservation Fleets
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_reservation_fleets`] holds the account's
//! Capacity Reservation Fleets as [`CapacityReservationFleetRecord`]s: the modelled
//! [`CapacityReservationFleet`] every operation that reports one answers with — including
//! the individual Capacity Reservations the create reserved its capacity through — plus the
//! move the clock owes it ([`CapacityReservationFleetTransition`]) — `submitted` ->
//! `active` after `CreateCapacityReservationFleet`, and `cancelling` -> `cancelled` after
//! `CancelCapacityReservationFleets`. Both fall due on the same monotonic clock and the same
//! [`TRANSITION_DELAY`] as an instance's [`PendingTransition`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them. The store is
//! keyed by `CapacityReservationFleetId` in a [`BTreeMap`], the order
//! `DescribeCapacityReservationFleets` reports and pages over.
//!
//! ## Secondary subnets
//!
//! [`ComAmazonawsEc2Amazonec2State::secondary_subnets`] holds the account's secondary
//! subnets — the subnets of a secondary network — as the modelled [`SecondarySubnet`]
//! shape itself, keyed by `SecondarySubnetId` in a [`BTreeMap`], exactly the way
//! [`ComAmazonawsEc2Amazonec2State::subnets`] holds ordinary subnets: a member a writer
//! sets round-trips through `DescribeSecondarySubnets` without a field-by-field copy, and
//! the id order is both the order the describe reports in and the position a pagination
//! token names. [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`] is the one
//! place a record is built, so the id, the ARN, the owning account and the id of each
//! IPv4 CIDR block association are decided once however many writers there are.
//!
//! A subnet `DeleteSecondarySubnet` has started taking down stays in that store, because a
//! subnet in either delete state is still reported; the instant its `delete-in-progress` ->
//! `delete-complete` move falls due sits in
//! [`ComAmazonawsEc2Amazonec2State::secondary_subnet_delete_complete_at`] beside it, the way
//! [`ComAmazonawsEc2Amazonec2State::subnet_available_at`] holds an ordinary subnet's own
//! clock-owed move, because the modelled shape this store holds carries no instant.
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it.
//!
//! ## Secondary interfaces
//!
//! [`ComAmazonawsEc2Amazonec2State::secondary_interfaces`] holds the account's secondary
//! interfaces — the interfaces an instance has in a secondary subnet — on exactly the terms
//! the secondary subnets above are held on: the modelled [`SecondaryInterface`] shape
//! itself, keyed by `SecondaryInterfaceId` in a [`BTreeMap`] whose id order is both the
//! order `DescribeSecondaryInterfaces` reports in and the position a pagination token
//! names. [`ComAmazonawsEc2Amazonec2State::insert_secondary_interface`] is the one place a
//! record is built, so the interface's id, its ARN, its MAC address, the owning account, the
//! id of its attachment and the `available`/`in-use` status that follows from that
//! attachment are decided once however many writers there are — an interface can no more
//! claim to be `available` while an instance holds it than a
//! [`ComAmazonawsEc2Amazonec2State::network_interfaces`] record can.
//!
//! ## A Capacity Block OFFERING is a search result, not a stored resource
//!
//! Nothing in the EC2 API creates or deletes a Capacity Block offering:
//! `DescribeCapacityBlockOfferings` "Describes Capacity Block offerings available for
//! purchase in the Amazon Web Services Region that you're currently using", and the search
//! it answers is the reservation duration, instance count and date range the request names.
//! So an offering is derived — from the Region's [`availability_zones_in_region`], the
//! modelled instance-type catalog [`offered_instance_types`] and the wall clock — and
//! NOTHING about one is stored: two searches with the same terms in the same hour report the
//! same offerings, and the search writes nothing a later caller could see.
//!
//! The terms an offering was reported with therefore travel inside its id
//! ([`CapacityBlockOfferingTerms`], [`encode_capacity_block_offering_id`]) rather than in a
//! map keyed by it, which is what lets a purchase resolve the id a caller hands back —
//! window, zone and counts — without the describe having had to write a row for every
//! offering it merely reported. [`capacity_block_upfront_fee`] is the one definition of what
//! such a window costs.
//!
//! ## Capacity Reservation billing requests
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_reservation_billing_requests`] holds the
//! requests to assign the billing of a Capacity Reservation's unused capacity to another
//! account, as the modelled [`CapacityReservationBillingRequest`] shape itself, keyed by
//! `CapacityReservationId` in a [`BTreeMap`] — one reservation's billing is assigned to at
//! most one account, and that key order is the stable order the describe reports and a
//! pagination token can name a position in. Both sides of one assignment are on the record
//! (`RequestedBy`, the Capacity Reservation owner that initiated it, and
//! `UnusedReservationBillingOwnerId`, the consumer account it was sent to), because which of
//! the two the CALLER is is what `DescribeCapacityReservationBillingRequests`'s `Role`
//! selects on. The `Status` is STORED rather than derived: `pending` moves to `accepted` or
//! `rejected` when the consumer account accepts or rejects the request, and to `cancelled`
//! or `revoked` when a party withdraws it — none of which is a fact of where a clock stands,
//! so no read may move it.
//!
//! ## Application status is DERIVED, never stored
//!
//! [`ComAmazonawsEc2Amazonec2State::application_status`] holds, per `InstanceId`, the
//! per-check results an instance's application status checks reported and the suppression
//! in force for it — not the instance-level status itself, which
//! `ApplicationStatus$Status` defines as a function of those results
//! ([`aggregated_application_status`]). So an instance nothing has reported a check for is
//! `not-applicable`, the value the model documents for "no checks with `Aggregation` set
//! to `included` apply to the instance", and no two operations can disagree about one
//! instance's status.
//!
//! ## Instance status FEEDBACK is collected, and reports nothing back
//!
//! [`ComAmazonawsEc2Amazonec2State::instance_status_reports`] holds, per `InstanceId`, the
//! status reports a caller submitted about that instance — see
//! [`instance_status_reports`], the submodule that owns this slice. They are COLLECTED and
//! never projected into any describe: "Amazon EC2 collects this information to improve the
//! accuracy of status checks. Use of this action does not change the value returned by
//! DescribeInstanceStatus" (`ReportInstanceStatus`), so the feedback store is write-only
//! as far as the API surface goes, and submitting a report moves no instance and changes
//! no status any reader reports.
//!
//! ## A QUEUED reboot is a fact of the instance, and moves nothing
//!
//! `RebootInstances` "only queues a request to reboot the specified instances", and a
//! rebooted instance stays in the state it was in — it does not pass through `pending`
//! and no modelled member of [`Instance`] reports a reboot. So the queued reboots of an
//! instance sit on [`InstanceRecord`] itself
//! ([`InstanceRecord::queued_reboots`], [`InstanceRecord::last_reboot_queued_at`]),
//! beside the state they deliberately leave alone, and are written in one place
//! ([`ComAmazonawsEc2Amazonec2State::queue_instance_reboot`]). They are summarised as a
//! count and the latest instant rather than kept as a growing list, because nothing
//! reclaims a queued reboot and an instance may be rebooted any number of times.
//!
//! A DIAGNOSTIC INTERRUPT is the same kind of fact, and is held the same way
//! ([`InstanceRecord::diagnostic_interrupts`],
//! [`InstanceRecord::last_diagnostic_interrupt_at`], written by
//! [`ComAmazonawsEc2Amazonec2State::record_diagnostic_interrupt`]): the interrupt makes the
//! instance's OPERATING SYSTEM crash and reboot, which is no lifecycle move of the
//! instance, and no modelled member of [`Instance`] reports one. It is counted separately
//! from the queued reboots because a caller asking for a reboot and a caller sending an
//! interrupt are asking for different things, and one is not a reading of the other.
//!
//! ## EBS encryption by default: a setting of a Region, not a resource
//!
//! EBS encryption by default has no resource and no id: it is one boolean "for your
//! account in the current Region" (`GetEbsEncryptionByDefault`), which
//! `EnableEbsEncryptionByDefault` turns on and `DisableEbsEncryptionByDefault` turns off.
//! So it is stored as the set of Regions it is ON in (a private field of
//! [`ComAmazonawsEc2Amazonec2State`]), keyed by the
//! Region of the request that set it, written through
//! [`ComAmazonawsEc2Amazonec2State::set_ebs_encryption_by_default`] and read through
//! [`ComAmazonawsEc2Amazonec2State::ebs_encryption_by_default`] — by the reporting
//! operation and by every handler that creates an EBS volume (`CreateVolume`,
//! `CopyVolumes`), which raise a new volume's encryption state to it ("the EBS volumes
//! that you create are always encrypted", `EnableEbsEncryptionByDefault`).
//!
//! ## Capacity Manager: an account-level setting, and the ingestion an enable starts
//!
//! EC2 Capacity Manager has no resource and no id either: it is one status "for your
//! account" (`EnableCapacityManager`) beside the cross-account aggregation flag
//! `OrganizationsAccess` carries. So it is stored as one private
//! [`CapacityManagerSettings`] of [`ComAmazonawsEc2Amazonec2State`] — not keyed by Region,
//! because nothing in its documentation scopes it to one — written through
//! [`ComAmazonawsEc2Amazonec2State::enable_capacity_manager`] and
//! [`ComAmazonawsEc2Amazonec2State::disable_capacity_manager`], and read through
//! [`ComAmazonawsEc2Amazonec2State::capacity_manager_at`], so the operation that reports
//! the settings and the operations that change them cannot disagree.
//!
//! The INGESTION an enable starts is a clock-owed move rather than a stored word: "this
//! starts data ingestion for your EC2 capacity usage" and "initial data processing may
//! take several hours to complete" (`EnableCapacityManager`), so the enable records when
//! the initial ingestion falls complete and [`CapacityManagerSettings::view_at`] derives
//! `initial-ingestion-in-progress` or `ingestion-complete` from that instant — on the same
//! monotonic clock and the same [`TRANSITION_DELAY`] as an instance's
//! [`PendingTransition`] — so a reader never writes and a caller asking straight after the
//! enable still sees the transient status. The DISABLE "stops data ingestion", so it drops
//! that instant rather than moving it: there is then no ingestion to report a status for,
//! and a later enable starts a fresh one.
//!
//! A DATA EXPORT configuration, unlike the setting above, IS a resource with an id of its
//! own, so [`ComAmazonawsEc2Amazonec2State::capacity_manager_data_exports`] holds the
//! account's exports keyed by `CapacityManagerDataExportId`, as the modelled
//! [`CapacityManagerDataExportResponse`] shape itself — the way every other stored resource
//! of this module is held. The operation that deletes one takes it OUT of that map, and the
//! count of configurations `GetCapacityManagerAttributes` reports is the size of that map,
//! so the reader and the delete cannot disagree about what the account holds.
//!
//! The DIMENSIONS Capacity Manager measures capacity along are not a store of their own:
//! they are a reading of the capacity resources the account already holds — its Capacity
//! Reservations and its instances — over a window of time, which is why
//! [`capacity_manager_metrics`] derives them rather than recording them. Which resources one
//! metric measures is read off the metric's own name
//! ([`capacity_metric_subject`]), so nothing has to be written when a
//! reservation is created or an instance is launched for Capacity Manager to have data for
//! it.
//!
//! ## Scheduled event notification tag keys are a fact of a REGION
//!
//! [`ComAmazonawsEc2Amazonec2State::instance_event_tag_registrations`] holds, per Region, the tag
//! keys registered to appear in scheduled event notifications
//! ([`InstanceEventTagRegistration`]) — the registration
//! `DescribeInstanceEventNotificationAttributes` reports "for resources in the current
//! Region", `RegisterInstanceEventNotificationAttributes` writes and
//! `DeregisterInstanceEventNotificationAttributes` takes keys back out of. It is keyed by
//! the Region of the request ([`request_region`]) rather than by a resource id, because no
//! resource owns it, and a Region with no entry has nothing registered — which is also
//! where a registration whose last key is deregistered goes back to
//! ([`ComAmazonawsEc2Amazonec2State::clear_instance_event_tag_registration`]).
//!
//! ## VM import tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::import_image_tasks`] holds the import image tasks
//! `ImportImage` starts as [`ImportImageTaskRecord`]s: the modelled [`ImportImageTask`]
//! `DescribeImportImageTasks` reports, plus the instant the clock owes the task its
//! `active` -> `completed` move. Keyed by `ImportTaskId` in a [`BTreeMap`], the same
//! stable order the other stores are kept in, so a pagination token can name the id the
//! next page starts at. A task AWS hands back is `active`, so the completion — the AMI the
//! import creates, and the snapshot id of each imported disk — is owed by the clock and
//! applied once by
//! [`ComAmazonawsEc2Amazonec2State::complete_due_import_image_tasks`]; only the PERCENTAGE
//! of progress an active task reports is derived per read, being a fact of elapsed time
//! that no store could hold.
//!
//! ## VM export image tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::export_image_tasks`] holds the export image tasks
//! `ExportImage` starts as [`ExportImageTaskRecord`]s: the modelled [`ExportImageTask`]
//! `DescribeExportImageTasks` reports, plus the window the export runs in and the two
//! `ExportImageResult` members that shape carries no member for. Keyed by
//! `ExportImageTaskId` in a [`BTreeMap`], the same stable order the other stores are kept
//! in, so a pagination token can name the id the next page starts at. Its STATUS is derived
//! on every read ([`ExportImageTaskRecord::view_at`]) rather than written, because finishing
//! an export mints nothing for a later read to agree on — the VM file it writes is an object
//! of another service — so `active` -> `completed`, and the `deleting` -> `deleted` of a
//! cancellation, are facts of the clock alone.
//!
//! `CancelImportTask` cancels a task of EITHER import kind, so the state it reads and the
//! state it moves are reached through
//! [`ComAmazonawsEc2Amazonec2State::import_task_state`] and
//! [`ComAmazonawsEc2Amazonec2State::start_import_task_cancellation`], and the instant the
//! clock owes a cancelled task its `deleting` -> `deleted` move sits in the one map
//! [`ComAmazonawsEc2Amazonec2State::import_task_deleted_at`] — neither modelled task shape
//! carrying a member for it.
//!
//! [`ComAmazonawsEc2Amazonec2State::conversion_tasks`] holds the CONVERSION tasks of the
//! older VM import path — "the import of an instance or volume"
//! (`CancelConversionTask`) — as the modelled [`ConversionTask`] shape
//! `DescribeConversionTasks` reports, keyed by `ConversionTaskId`. Which resource a task
//! converts is told by which detail member it carries, and that fixes the prefix of its
//! id ([`IMPORT_INSTANCE_CONVERSION_TASK_ID_PREFIX`],
//! [`IMPORT_VOLUME_CONVERSION_TASK_ID_PREFIX`]). `CancelConversionTask` reads the state it
//! acts on through [`ComAmazonawsEc2Amazonec2State::conversion_task_state`] and moves it
//! through [`ComAmazonawsEc2Amazonec2State::start_conversion_task_cancellation`], with the
//! instant the clock owes a cancelled task its `cancelling` -> `cancelled` move in
//! [`ComAmazonawsEc2Amazonec2State::conversion_task_cancelled_at`].
//!
//! [`ComAmazonawsEc2Amazonec2State::bundle_tasks`] holds the BUNDLE tasks of the
//! instance-store-backed AMI path — "a bundle task" (`BundleTask`) — as the modelled
//! [`BundleTask`] shape `DescribeBundleTasks` reports, keyed by `BundleId`, for the same
//! reason a conversion task is stored as its own modelled shape: every member a writer sets
//! round-trips through the describe without a field-by-field copy that can silently drop
//! one. Nothing WRITES that store: `BundleInstance`, the operation that started a bundle
//! task, and `CancelBundleTask`, the one that cancelled it, are both actions EC2 no longer
//! serves ([`api_not_supported`]), so no call can mint or cancel one and the report is of
//! the tasks the store holds.
//!
//! ## Root volume replacement tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::replace_root_volume_tasks`] holds the root volume
//! replacement tasks `CreateReplaceRootVolumeTask` starts as
//! [`ReplaceRootVolumeTaskRecord`]s: the modelled [`ReplaceRootVolumeTask`]
//! `DescribeReplaceRootVolumeTasks` reports, plus the two moves the clock owes it. A task
//! AWS hands back is `pending` — "the replacement volume is being created" — and walks
//! `pending` -> `in-progress` -> `succeeded` on its own
//! (`smithy_query(query="shape",shape="ReplaceRootVolumeTaskState")`), so each move falls
//! due one [`TRANSITION_DELAY`] after the one before it on the same monotonic clock as an
//! instance's [`PendingTransition`], and [`ReplaceRootVolumeTaskRecord::view_at`] derives
//! the state a reader sees from it rather than writing on a read. `CompleteTime` is "the
//! time the task completed", so it is reported only once the task has reached a terminal
//! state. Keyed by `ReplaceRootVolumeTaskId` in a [`BTreeMap`], the same stable order the
//! other stores are kept in, so a pagination token can name the id the next page starts
//! at.
//!
//! ## Mac modification tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::mac_modification_tasks`] holds the tasks
//! `CreateMacSystemIntegrityProtectionModificationTask` and
//! `CreateDelegateMacVolumeOwnershipTask` start as [`MacModificationTaskRecord`]s: the
//! modelled [`MacModificationTask`] `DescribeMacModificationTasks` reports, plus the two
//! moves the clock owes it. One store for both task types, because both operations create
//! the same resource and the describe reports it without being told which started it; the
//! task's `TaskType` is what tells them apart. A task AWS hands back is `pending` and walks
//! `pending` -> `in-progress` -> `successful` on its own
//! (`smithy_query(query="shape",shape="MacModificationTaskState")`), so each move falls due
//! one [`TRANSITION_DELAY`] after the one before it on the same monotonic clock as an
//! instance's [`PendingTransition`], and [`MacModificationTaskRecord::view_at`] derives the
//! state a reader sees from it rather than writing on a read. Keyed by
//! `MacModificationTaskId` in a [`BTreeMap`], the same stable order the other stores are
//! kept in, so a pagination token can name the id the next page starts at.
//!
//! ## Spot Instance requests
//!
//! [`ComAmazonawsEc2Amazonec2State::spot_instance_requests`] holds the Spot Instance
//! requests `RequestSpotInstances` mints as [`SpotInstanceRequestRecord`]s: the modelled
//! [`SpotInstanceRequest`] `DescribeSpotInstanceRequests` reports, plus the fulfillment the
//! clock owes an `open` request and the instant its own `ValidUntil` expires it. Keyed by
//! `SpotInstanceRequestId` in a [`BTreeMap`], like every other store here, so a pagination
//! token can name the id the next page starts at. A request AWS hands back is `open`, so
//! both moves are owed by the clock and applied once by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_spot_instance_requests`] — and the
//! fulfillment LAUNCHES an instance into
//! [`ComAmazonawsEc2Amazonec2State::instances`], which is why an instance carries the
//! [`InstanceRecord::spot_instance_request_id`] of the request that launched it rather than
//! Spot Instances being kept in a store of their own.
//!
//! ## Spot Fleet requests
//!
//! [`ComAmazonawsEc2Amazonec2State::spot_fleet_requests`] holds the Spot Fleet requests
//! `RequestSpotFleet` mints as [`SpotFleetRequestRecord`]s: the modelled
//! [`SpotFleetRequestConfig`] `DescribeSpotFleetRequests` reports, plus the move the clock
//! owes a `submitted` request and the Spot Instance requests the fleet placed to cover its
//! target capacity. Keyed by `SpotFleetRequestId` in a [`BTreeMap`], like every other store
//! here, so a pagination token can name the id the next page starts at.
//!
//! A Spot Fleet is a COMPOSITE of the Spot Instance requests it places: "the Spot Fleet only
//! places the required requests" (`SpotFleetRequestConfigData$Type`), so the capacity it
//! reports fulfilled is derived from those requests — which are records of
//! [`ComAmazonawsEc2Amazonec2State::spot_instance_requests`], fulfilled into instances by
//! the one clock-owed move every Spot Instance request already has — rather than written on
//! the fleet by whoever asked about it
//! ([`ComAmazonawsEc2Amazonec2State::materialize_due_spot_fleet_requests`]).
//!
//! ## Placement groups
//!
//! [`ComAmazonawsEc2Amazonec2State::placement_groups`] holds the account's placement
//! groups as the modelled [`PlacementGroup`] shape itself, keyed by `GroupId`, for the
//! reason [`ComAmazonawsEc2Amazonec2State::vpcs`] holds VPCs that way: `CreatePlacementGroup`
//! and `DescribePlacementGroups` answer the same shape, so storing it is what stops them
//! reporting one group differently. A placement group is "a logical grouping of instances"
//! (`CreatePlacementGroup`) with nothing to provision, so it is stored `available` and no
//! clock-owed move is recorded for it.
//!
//! ## Internet gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::internet_gateways`] holds the account's internet
//! gateways as the modelled [`InternetGateway`] shape itself, keyed by `InternetGatewayId`,
//! for the reason [`ComAmazonawsEc2Amazonec2State::vpcs`] holds VPCs that way:
//! `CreateInternetGateway` and `DescribeInternetGateways` answer the same shape, so storing
//! it is what stops them reporting one gateway differently. A gateway has nothing to
//! provision and no status member at all, so the clock owes one nothing; it starts life
//! attached to no VPC, an attachment being what `AttachInternetGateway` records on it later.
//! The store's own primitives live in the [`internet_gateways`] module.
//!
//! Nothing here gives the ambient default network of a Region the internet gateway AWS
//! attaches to a default VPC, so an unfiltered `DescribeInternetGateways` on an account that
//! created none reports an empty set where AWS reports that one: the gateway AWS gives a
//! default VPC is an ATTACHED one, and the attachment seam it needs is
//! `AttachInternetGateway`, which is still a scaffold (`src/ops/attach_internet_gateway.rs`
//! carries the stub marker). Seeding it here would write an attachment no implemented
//! operation owns, so it belongs with that operation rather than with this store.
//!
//! ## Infrastructure Performance metric subscriptions
//!
//! [`ComAmazonawsEc2Amazonec2State::network_performance_subscriptions`] holds the
//! subscriptions `EnableAwsNetworkPerformanceMetricSubscription` turns on, as the modelled
//! [`Subscription`] shape itself, keyed by [`subscription_key`] — the four members that
//! name one (`Source`, `Destination`, `Metric`, `Statistic`), because a subscription
//! carries no id of its own. Enabling the same four twice therefore leaves ONE
//! subscription, a disable removes exactly the one it names, and
//! `DescribeAwsNetworkPerformanceMetricSubscriptions` reports what those two have left
//! here — it writes nothing itself, because a subscription is enabled the moment the
//! enable answers and so has no clock-owed move. Every word stored comes from a catalog
//! (a Region or Availability Zone id of [`subscription_endpoint_region`], a `MetricType`
//! and a `StatisticType` member, and the service's own [`SUBSCRIPTION_PERIOD`]), so the
//! store holds no request text.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::net::Ipv6Addr;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Address, AddressAttribute, AddressTransfer, ApplicationStatus, ApplicationStatusDetail,
    AsnAssociation,
    AttachVolumeOutput,
    AttributeValue,
    AvailabilityZone, AvailabilityZoneGeography, AvailabilityZoneMessage,
    AvailabilityZoneSubGeography, BlockDeviceMapping, BlockPublicAccessStates,
    BundleTask,
    Byoasn, ByoipCidr, ConnectionNotification,
    DhcpConfiguration, DhcpOptions,
    CapacityBlock, CapacityReservation, CapacityReservationBillingRequest,
    CapacityManagerDataExportResponse,
    CapacityReservationCancellationQuote, CapacityReservationConfiguration, CancellationTerms,
    CapacityReservationCommitmentInfo, CapacityReservationInfo, CustomerGateway, ExportTask,
    FpgaImage,
    FpgaImageState, ImageUsageReport,
    DestinationOptionsResponse, EncryptionSupport, FlowLog, TagFieldSpecificationResponse,
    LoadPermission,
    CapacityReservationFleet, FleetCapacityReservation,
    StorageLocation, VolumeRecycleBinInfo,
    CreateVolumeOutput, DefaultConnectionTrackingConfiguration,
    DescribeFastLaunchImagesSuccessItem, DiskInfo, DisableFastLaunchOutput,
    VolumeModification,
    EbsBlockDevice,
    EbsInfo, EbsOptimizedInfo, EfaInfo, EnableFastLaunchOutput, GroupIdentifier, Host,
    HostReservation,
    INSTANCE_TYPE_VALUES, Image,
    ImageCriterion, ImportSnapshotTask, Instance, InstanceApplicationStatus, InstanceStorageInfo,
    InstanceTypeInfo,
    ConnectionTrackingSpecificationResponse, InstanceIpv4Prefix, InstanceIpv6Address,
    InstanceIpv6Prefix, InstanceNetworkInterface, InstanceNetworkInterfaceAttachment,
    InstancePrivateIpAddress,
    Ipam, IpamInternetRegistryAssociation, IpamOperatingRegion, IpamPolicy, IpamPool,
    IpamPoolCidr,
    IpamPrefixListResolver, IpamPrefixListResolverRule, IpamPrefixListResolverTarget,
    IpamResourceDiscovery, IpamResourceDiscoveryAssociation, IpamResourceTag,
    IpamRoutingPolicyRegistration, IpamRoutingPolicyRegistrationDelta, IpamScope,
    MemoryInfo, NetworkCardInfo, NetworkInfo, NitroTpmInfo, PlacementGroupInfo, ProcessorInfo,
    VCpuInfo,
    InstanceState, InstanceTypeOffering, InternetGateway, InterruptibleCapacityAllocation,
    InterruptionInfo,
    IpPermission, IpRange, Ipv6Range,
    LOCATION_TYPE_VALUES,
    LaunchTemplate, MacHost, MacModificationTask, MacSystemIntegrityProtectionConfiguration,
    Monitoring, NatGateway, NetworkInsightsAccessScope,
    NetworkInsightsAccessScopeAnalysis, NetworkInsightsAnalysis, NetworkInsightsPath,
    NetworkInterface, NetworkInterfacePermission,
    NetworkInterfacePermissionState, NetworkInterfacePrivateIpAddress,
    OperatorResponse, PathFilter, PeeringAttachmentStatus,
    PlacementGroup, PrefixListId, PtrUpdateStatus,
    PrivateDnsDetails, PrivateDnsNameConfiguration, PrivateDnsNameOptionsOnLaunch,
    ReferencedSecurityGroup,
    SecurityGroupRule, SecurityGroupVpcAssociation,
    DnsEntry, PayerResponsibilityEntry, ProductCode,
    Region, RegionGeography, ReplaceRootVolumeTask, RequestLaunchTemplateData,
    RevokedSecurityGroupRule,
    RouteServer, RouteServerBfdStatus,
    RouteServerBgpOptions, RouteServerBgpStatus, RouteServerEndpoint, RouteServerPeer,
    RouteServerAssociation, RouteServerPropagation, RouteTable,
    RunInstancesOutput, SecondaryInterface, SecondaryInterfaceAttachment,
    SecondaryInterfaceIpv4Address, SecondaryNetwork,
    SecondaryNetworkIpv4CidrBlockAssociation, SecondarySubnet,
    SecondarySubnetIpv4CidrBlockAssociation, SecurityGroup, ServiceConfiguration,
    ServiceDetail, ServiceTypeDetail,

    SnapshotTaskDetail, SpotDatafeedSubscription, SpotPlacementScore, StateReason,
    Subnet, SubnetAssociation, SubnetCidrBlockState, SubnetCidrReservation,
    SubnetIpv6CidrBlockAssociation, Subscription, Tag,
    TagSpecification,
    TransitGateway, TransitGatewayClientVpnAttachment, TransitGatewayConnect,
    TransitGatewayConnectPeer,
    TransitGatewayMeteringPolicy, TransitGatewayMeteringPolicyEntry,
    TransitGatewayMulticastDomainAssociation, TransitGatewayOptions,
    TransitGatewayPeeringAttachment, TransitGatewayPolicyTable,
    TransitGatewayAssociation,
    TransitGatewayPolicyTableAssociation, TransitGatewayPolicyTableEntry,
    TransitGatewayPrefixListAttachment,
    TransitGatewayPrefixListReference, TransitGatewayPropagation,
    TransitGatewayRequestOptions,
    TransitGatewayRoute,
    TransitGatewayRouteTable, TransitGatewayRouteTableAnnouncement,
    TransitGatewayRouteTableAssociation,
    TransitGatewayRouteTablePropagation,
    TransitGatewayVpcAttachment,
    UserIdGroupPair, Vpc, VpcBlockPublicAccessExclusion, VpcCidrBlockAssociation,
    VpcCidrBlockState, VpcEndpoint,
    VpcEndpointAssociation, VpcEndpointConnection, VpnConnection, VpnGateway, VpnStaticRoute,
};
use crate::scaffolded::Scaffolded;

// The Recycle Bin an AMI waits in between its deregistration and its permanent deletion,
// and every rule that moves one in or out of it, in a module of its own.
mod image_recycle_bin;
pub use image_recycle_bin::{RecycledImageRecord, recycled_image, require_recycled_image};

/// The account's KEY PAIRS: the record one is stored as, the bounds a create holds a
/// request to, the quota and name refusals EC2 answers about one, and the one writer that
/// stores one. Re-exported, so a handler reaches them as `crate::state::<item>` like every
/// other primitive of this module.
pub mod key_pairs;
pub use key_pairs::*;

/// The key MATERIAL a key pair is made of — the real RSA or Ed25519 key, the fingerprint its
/// own type is fingerprinted with, and the private-key file format the request asked for.
/// Re-exported, so a handler reaches them as `crate::state::<item>` like every other
/// primitive of this module.
pub mod key_pair_material;
pub use key_pair_material::*;

/// The PUBLIC key material an `ImportKeyPair` carries: what counts as a key EC2 can take,
/// which `KeyType` it is, and the fingerprint an IMPORTED key of that type is published
/// with. Re-exported, so a handler reaches them as `crate::state::<item>` like every other
/// primitive of this module.
pub mod key_pair_import;
pub use key_pair_import::*;

/// A VPC's ROUTE TABLES and the ATTACHMENT an internet gateway of one carries: the store,
/// the ids, the byte charge and the error channel `CreateRouteTable`, `CreateRoute`,
/// `AssociateRouteTable`, `AttachInternetGateway` and `DescribeRouteTables` share. The
/// internet gateway store itself is [`internet_gateways`]'s. Re-exported, so a handler
/// reaches them as `crate::state::<item>` like every other primitive of this module.
pub mod vpc_internet_path;
pub use vpc_internet_path::*;

/// What `CreateNetworkAcl` adds to the network ACL store in the "Network ACLs" section
/// below: the `ResourceType` its tag specification may name, the entries a caller-created
/// ACL comes with, the quota refusal for one ACL too many, and the `ClientToken` map that
/// makes the create idempotent. Re-exported, so a handler reaches them as
/// `crate::state::<item>` like every other primitive of this module.
pub mod network_acls;
pub use network_acls::*;

/// The internet gateway store's own primitives — the id prefix, the `ResourceType` a tag
/// specification may name, the bounds, the byte charge, the one writer and the error channel
/// `CreateInternetGateway` and `DescribeInternetGateways` share. Re-exported, so a handler
/// reaches them as `crate::state::<item>` like every other primitive of this module.
pub mod internet_gateways;
pub use internet_gateways::*;

/// The Traffic Mirror filter store's own primitives — the id prefix, the bounds, the byte
/// charge and the error channel the Traffic Mirror filter operations share, with the one
/// write and the one removal path a filter enters and leaves the store by. Re-exported, so a
/// handler reaches them as `crate::state::<item>` like every other primitive of this module.
pub mod traffic_mirror_filters;
pub use traffic_mirror_filters::*;

/// What `CreatePublicIpv4Pool` adds to the public IPv4 pool store below: the network border
/// group a created pool is advertised from, and the refusal for a create the store has no
/// room for. The pool record, its insert and its projection live in the "Public IPv4 address
/// pools" section of this module, beside the BYOIP provision that also creates one.
/// Re-exported, so a handler reaches them as `crate::state::<item>` like every other
/// primitive here.
pub mod public_ipv4_pools;
pub use public_ipv4_pools::*;

/// The IPv6 address pools the account holds — what one is stored as, the insert that mints
/// its id, the removal, the projection `DescribeIpv6Pools` reports it through and the refusal
/// for a pool id the account holds none for. Re-exported, so a handler reaches them as
/// `crate::state::<item>` like every other primitive here.
pub mod ipv6_pools;
pub use ipv6_pools::*;

/// The Traffic Mirror session store's own primitives — the id prefix, the bounds, the byte
/// charge and the error channel the Traffic Mirror session operations share. Re-exported,
/// so a handler reaches them as `crate::state::<item>` like every other primitive of this
/// module.
pub mod traffic_mirror_sessions;
pub use traffic_mirror_sessions::*;

/// The Traffic Mirror target store's own primitives — the id prefix, the bounds, the byte
/// charge and the error channel the Traffic Mirror target operations share, with the one
/// write and the one removal path a target enters and leaves the store by. Re-exported, so
/// a handler reaches them as `crate::state::<item>` like every other primitive of this
/// module.
pub mod traffic_mirror_targets;
pub use traffic_mirror_targets::*;

/// The VPC peering connections the account holds — what one is stored as, the status words,
/// the `initiating-request` -> `pending-acceptance` move the clock owes a fresh request, the
/// expiry of a request nobody answered, the store's bounds and its refusals — in a module of
/// its own, re-exported so a handler reaches them as `crate::state::<item>` like every other
/// primitive here.
pub mod vpc_peering_connections;
pub use vpc_peering_connections::*;

/// The entries of a transit gateway metering policy — what one is stored as, the state a
/// created entry is in, the refusals a create answers, the store's bounds and its one
/// writer — in a module of its own, re-exported so a handler reaches them as
/// `crate::state::<item>` like every other primitive here. The policies the entries belong
/// to live in the "Transit gateway metering policies" section of this module.
pub mod transit_gateway_metering_policy_entries;
pub use transit_gateway_metering_policy_entries::*;

/// The entries of a transit gateway policy table — what one is stored as, the rule a request
/// may describe, the state a created entry is in, the refusals a create and a reader answer,
/// the store's bounds and its one writer — in a module of its own, re-exported so a handler
/// reaches them as `crate::state::<item>` like every other primitive here. The tables the
/// entries belong to live in the "Transit gateway policy tables" section of this module.
pub mod transit_gateway_policy_table_entries;
pub use transit_gateway_policy_table_entries::*;

/// The static routes of a transit gateway route table — what one is stored as, the states and
/// types its catalogs admit, the refusals a create answers, the store's bounds and its one
/// writer — in a module of its own, re-exported so a handler reaches them as
/// `crate::state::<item>` like every other primitive here. The tables the routes belong to
/// live in the transit gateway route table store beside them.
pub mod transit_gateway_routes;
pub use transit_gateway_routes::*;

/// The `route-search.*` filters an EC2 route search is narrowed by — which CIDR relation each
/// documented name asks for and how that relation is evaluated against a route's destination —
/// in a module of its own beside [`CompiledFilter`] and [`MatchBudget`], re-exported so a
/// handler reaches them as `crate::state::<item>` like every other primitive here. Shared by
/// every operation that searches a route table, so the relations cannot drift between them.
pub mod route_search_filters;
pub use route_search_filters::*;

/// The `Reservation` shape, under the name the scaffold generated for it.
///
/// `com.amazonaws.ec2#Reservation` is both `RunInstances`'s output shape and the
/// member type of `DescribeInstances`'s `Reservations` list, so the generator
/// emitted it once under the output name. This alias lets the code read as the
/// model does.
pub type ReservationView = RunInstancesOutput;

/// The instance event windows this store holds, and the rules a window's schedule must
/// satisfy — one resource's state, records and validators in their own child module, from
/// which this module re-exports them, so a caller still reaches them as
/// `crate::state::…` wherever they are written.
pub mod instance_event_windows;

pub use instance_event_windows::*;

/// The ClassicLink links the account holds — the store `DescribeClassicLinkInstances`
/// reports, its bounds and its projection — in a module of its own, re-exported so a
/// handler reaches them as `crate::state::<item>` like every other primitive here.
mod classic_link;
pub use classic_link::*;

/// The account's access to the EC2 serial console of its instances — the one flag
/// `EnableSerialConsoleAccess` and `DisableSerialConsoleAccess` move and
/// `GetSerialConsoleAccessStatus` reports. Everything it holds is inherent methods of
/// [`ComAmazonawsEc2Amazonec2State`], so there is nothing to re-export: a handler reaches
/// them on the store itself like every other accessor here.
mod serial_console_access;

/// The trust relationship between Reachability Analyzer and Organizations — the one fact
/// `EnableReachabilityAnalyzerOrganizationSharing` establishes. Everything it holds is
/// inherent methods of [`ComAmazonawsEc2Amazonec2State`], so there is nothing to re-export:
/// a handler reaches them on the store itself like every other accessor here.
mod reachability_analyzer_sharing;

/// The Reserved Instances the account holds and the modifications of them — the two stores,
/// the form of their ids, their ceilings, their refusals and the clock-owed
/// `processing` -> `fulfilled` move of a modification — in a module of its own, re-exported
/// so a handler reaches them as `crate::state::<item>` like every other primitive here.
pub mod reserved_instances;
pub use reserved_instances::*;

/// Scheduled Instances: the capacity a Region has set aside for them, the words a schedule is
/// described in and the arithmetic that turns a recurrence into the hours of a term — in a
/// module of its own, re-exported so a handler reaches them as `crate::state::<item>` like
/// every other primitive here.
pub mod scheduled_instances;
pub use scheduled_instances::*;

/// The EBS snapshots the account holds — what one snapshot is stored as, the store's
/// bounds, its byte charge and the clock-owed `pending` -> `completed` move every snapshot
/// starts out owing — in a module of its own, re-exported so a handler reaches them as
/// `crate::state::<item>` like every other primitive here.
pub mod snapshots;
pub use snapshots::*;

/// The snapshots the EBS DIRECT APIs hold, projected onto this service's `Snapshot` shape:
/// the two APIs describe one snapshot resource, so an EC2 read resolves and reports a
/// snapshot minted on that side too. In a module of its own, re-exported so a handler reaches
/// them as `crate::state::<item>` like every other primitive here.
pub mod direct_api_snapshots;
pub use direct_api_snapshots::*;

/// The snapshots the AMI CATALOGUE restores its devices from — the public ones Amazon
/// publishes its images with — derived from the image records rather than copied, in a module
/// of its own, re-exported so a handler reaches them as `crate::state::<item>` like every
/// other primitive here.
pub mod published_snapshots;
pub use published_snapshots::*;

/// The ACM certificates the account holds, as the EC2 operations that take a certificate ARN
/// resolve one: the certificate is ACM's resource, so this ASKS that service over the
/// emulator's in-process callback instead of keeping a second copy of it. In a module of its
/// own, re-exported so a handler reaches it as `crate::state::<item>` like every other
/// primitive here.
pub mod acm_certificates;
pub use acm_certificates::*;

/// The fast snapshot restores the account holds — what one (snapshot, Availability Zone)
/// pair is stored as, the lifecycle the clock owes it and the one reading of the request
/// members an enable and a disable share — in a module of its own, re-exported so a handler
/// reaches them as `crate::state::<item>` like every other primitive here.
pub mod fast_snapshot_restores;
pub use fast_snapshot_restores::*;

/// The EBS root volume a LAUNCH creates for the instance it stores, restored from the AMI's
/// root block device mapping as the launch's own mappings override it — the methods that
/// resolve that device and mint the volume, in a module of its own.
mod launched_root_volumes;
pub use launched_root_volumes::root_block_device_bytes;

/// The PRIMARY network interface a LAUNCH creates for the instance it stores, in the subnet
/// the launch places it in — one method on the state, in a module of its own, beside the
/// root volume the same launch creates.
mod launched_primary_interfaces;

/// The network interfaces a launch was TOLD to give its instance
/// (`RunInstancesRequest$NetworkInterfaces`) — what one of them resolves to and the one
/// writer that creates or attaches each — in a module of its own beside the single primary
/// interface a launch that names none gets. Everything it declares is re-exported below.
mod launched_interfaces;
pub use launched_interfaces::*;

/// The private IPv4 addresses ONE network interface specification asks for, read the same way
/// for `CreateNetworkInterface`'s own request and for each entry of a launch's
/// `NetworkInterfaces`. Everything it declares is re-exported below.
mod interface_private_ips;
pub use interface_private_ips::*;

/// The FIXED detail every instance reports — the machine facts of its AMI, the CPU layout of
/// its instance type, the volumes it holds, where it runs and the option sets its launch
/// chose — in a module of its own, re-exported so a handler reaches its words as
/// `crate::state::<item>` like every other primitive here.
pub mod instance_detail;
pub use instance_detail::*;

/// The associations between the account's instances and the IAM instance profiles they run
/// as — the store, the form of an association id, its ceiling, its refusals and the two
/// clock-owed moves an association makes — in a module of its own, re-exported so a handler
/// reaches them as `crate::state::<item>` like every other primitive here.
pub mod iam_instance_profile_associations;
pub use iam_instance_profile_associations::*;

/// Hand the ONE list of stores whose resources carry the modelled `Tag` list to `consumer`,
/// which is given the bindings in the brackets ahead of it.
///
/// `field` names a store whose record IS the modelled shape; `field.member` one that keeps
/// that shape on a member, beside the facts the shape carries no member for; and `as` names
/// the `ResourceType` the kind is reported under
/// (`smithy_query(query="shape",shape="ResourceType")`). A resource kind is taggable exactly
/// when it is named here, so this list is the one place the tag operations' reach is written
/// down — and every consumer below reads the same list, so a store added here becomes
/// taggable, untaggable and described at once.
macro_rules! taggable_tag_list_stores {
    ($consumer:ident [$($bindings:tt)*]) => {
        $consumer!(
            [$($bindings)*]
            addresses as "elastic-ip",
            capacity_reservation_fleets.view as "capacity-reservation-fleet",
            capacity_reservation_cancellation_quotes.view
                as "capacity-reservation-cancellation-quote",
            capacity_reservations.view as "capacity-reservation",
            carrier_gateways as "carrier-gateway",
            client_vpn_endpoints as "client-vpn-endpoint",
            coip_pools as "coip-pool",
            customer_gateways.view as "customer-gateway",
            dedicated_hosts as "dedicated-host",
            dhcp_options as "dhcp-options",
            export_image_tasks.view as "export-image-task",
            export_tasks.view as "export-instance-task",
            fleets.fleet as "fleet",
            fpga_images.view as "fpga-image",
            images as "image",
            import_image_tasks.view as "import-image-task",
            import_snapshot_tasks as "import-snapshot-task",
            internet_gateways as "internet-gateway",
            ipam_policies.view as "ipam-policy",
            ipam_pools as "ipam-pool",
            ipam_resource_discoveries as "ipam-resource-discovery",
            ipam_resource_discovery_associations as "ipam-resource-discovery-association",
            ipam_scopes as "ipam-scope",
            ipams as "ipam",
            launch_templates.template as "launch-template",
            local_gateway_route_table_virtual_interface_group_associations.view
                as "local-gateway-route-table-virtual-interface-group-association",
            local_gateway_route_table_vpc_associations.view
                as "local-gateway-route-table-vpc-association",
            local_gateway_route_tables.view as "local-gateway-route-table",
            mac_modification_tasks.view as "mac-modification-task",
            managed_prefix_lists.view as "prefix-list",
            nat_gateways as "natgateway",
            network_insights_access_scope_analyses.view
                as "network-insights-access-scope-analysis",
            network_insights_access_scopes as "network-insights-access-scope",
            network_insights_analyses.view as "network-insights-analysis",
            network_insights_paths as "network-insights-path",
            placement_groups as "placement-group",
            replace_root_volume_tasks.view as "replace-root-volume-task",
            reserved_instances as "reserved-instances",
            route_server_endpoints as "route-server-endpoint",
            route_server_peers as "route-server-peer",
            route_servers.view as "route-server",
            route_tables as "route-table",
            secondary_interfaces as "secondary-interface",
            secondary_networks.view as "secondary-network",
            secondary_subnets as "secondary-subnet",
            security_groups as "security-group",
            snapshots.view as "snapshot",
            spot_instance_requests.view as "spot-instances-request",
            spot_fleet_requests.view as "spot-fleet-request",
            subnet_cidr_reservations as "subnet-cidr-reservation",
            subnets as "subnet",
            traffic_mirror_filters as "traffic-mirror-filter",
            traffic_mirror_sessions as "traffic-mirror-session",
            transit_gateway_connect_peers.view as "transit-gateway-connect-peer",
            transit_gateway_metering_policies as "transit-gateway-metering-policy",
            transit_gateway_policy_tables.view as "transit-gateway-policy-table",
            transit_gateway_route_tables as "transit-gateway-route-table",
            transit_gateways as "transit-gateway",
            verified_access_endpoints.view as "verified-access-endpoint",
            verified_access_groups.view as "verified-access-group",
            volumes.view as "volume",
            vpc_block_public_access_exclusions.view as "vpc-block-public-access-exclusion",
            vpc_endpoint_services as "vpc-endpoint-service",
            vpc_endpoints as "vpc-endpoint",
            vpc_peering_connections.view as "vpc-peering-connection",
            vpcs as "vpc",
            vpn_connections.view as "vpn-connection",
            vpn_gateways.view as "vpn-gateway",
        );
    };
}

/// Resolve `$id` against every store [`taggable_tag_list_stores`] names, answering the
/// resource's tags for writing — the consumer
/// [`ComAmazonawsEc2Amazonec2State::resource_tags_mut`] drives.
macro_rules! resource_tag_lists {
    ([$store:ident, $id:ident] $($field:ident $(. $member:ident)? as $kind:literal),+ $(,)?) => {
        $(
            if let Some(record) = $store.$field.get_mut($id) {
                return Some(ResourceTagsMut::List(&mut record $(.$member)?.tags));
            }
        )+
    };
}

/// Report every stored resource of every store [`taggable_tag_list_stores`] names to
/// `$report`, as `(resource id, resource type, tags)` — the consumer
/// [`ComAmazonawsEc2Amazonec2State::tag_descriptions`] drives, so what `DescribeTags`
/// reports is read off the same list the two writers resolve an id against.
macro_rules! resource_tag_descriptions {
    ([$store:ident, $report:ident] $($field:ident $(. $member:ident)? as $kind:literal),+ $(,)?) => {
        $(
            for (resource_id, record) in &$store.$field {
                $report(
                    resource_id,
                    $kind,
                    record $(.$member)?.tags.as_deref().unwrap_or_default(),
                );
            }
        )+
    };
}

// =============================================================================
// Regions
// =============================================================================

// =============================================================================
// EBS volumes
// =============================================================================

// ---------------------------------------------------------------------------
// VM import: the import image tasks `ImportImage` starts
// ---------------------------------------------------------------------------

// The modelled shapes this slice reads, named HERE rather than in the module's shared
// `use crate::ops::types::{..}` list at the top: that list is edited by every store this
// crate gains, so naming them beside the code that uses them keeps this slice off a line
// nothing else of it touches.
use crate::ops::types::{ImportImageOutput, ImportImageTask};

// ---------------------------------------------------------------------------
// VM export: the export image tasks `ExportImage` starts
// ---------------------------------------------------------------------------

// The modelled shapes this slice reads, named here for the reason the ones above are: the
// slice that reads them keeps its own modelled names beside the code that uses them.
use crate::ops::types::{ExportImageOutput, ExportImageTask};

// ---------------------------------------------------------------------------
// VM import: the import snapshot tasks `ImportSnapshot` starts
// ---------------------------------------------------------------------------

// The shape that operation answers with, named here for the reason the two above are: the
// slice that reads it keeps its own modelled names beside the code that uses them.
use crate::ops::types::ImportSnapshotOutput;

// ---------------------------------------------------------------------------
// Application status checks: the configurations `DescribeApplicationStatusChecks` reports
// ---------------------------------------------------------------------------

/// This service's application status CHECK slice: the store's id prefix, the documented
/// defaults and bounds a create fills in and enforces, the quota it asks about, the refusal a
/// request naming an unknown check answers, and what tells an active check from one inside its
/// deletion grace period. A SUBMODULE, so that surface grows without touching the parts
/// of this file the other resource kinds own; `pub use` keeps every name reachable as
/// `crate::state::<name>`, which is where each handler names it.
pub mod application_status_checks;
pub use application_status_checks::*;

// ---------------------------------------------------------------------------
// Instance status reports: the feedback `ReportInstanceStatus` collects
// ---------------------------------------------------------------------------

/// This service's instance status FEEDBACK slice: the record one submitted report is
/// stored as, what it is charged and how many of them this store holds. A SUBMODULE for
/// the same reason [`application_status_checks`] is one; `pub use` keeps every name
/// reachable as `crate::state::<name>`, which is where the handler names it.
pub mod instance_status_reports;
pub use instance_status_reports::*;

// ---------------------------------------------------------------------------
// SQL Server High Availability: what `DescribeInstanceSqlHaStates` reports
// ---------------------------------------------------------------------------

/// This service's SQL Server High Availability slice: the registration one enabled instance
/// is stored as, the modelled status words it is reported with, and the one projection of it
/// into the modelled `RegisteredInstance` shape. A SUBMODULE for the same reason
/// [`application_status_checks`] is one; `pub use` keeps every name reachable as
/// `crate::state::<name>`, which is where each handler names it.
pub mod instance_sql_ha;
pub use instance_sql_ha::*;

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// VM Import/Export conversion tasks
// ---------------------------------------------------------------------------
//
// A conversion task is the record of ONE import of a whole machine or of a bare disk —
// "the task can be the import of an instance or volume" (`CancelConversionTask`) — which
// `ImportInstance` and `ImportVolume` start and `DescribeConversionTasks` reports. It is
// stored as the modelled `ConversionTask` shape
// ([`ComAmazonawsEc2Amazonec2State::conversion_tasks`]) for the same reason an import
// snapshot task is stored as an `ImportSnapshotTask`: every reader of a task reports what
// a writer stored, so no two of them can disagree about one task.
//
// WHICH resource a task converts is told by which detail member it carries
// (`ConversionTask$ImportInstance` or `ConversionTask$ImportVolume`), and that is also
// what fixes the prefix of its id. Its state is its own `ConversionTask$State`, out of the
// closed `ConversionTaskState` catalog, which is the only place any reader takes it from —
// so the cancel reads and writes THAT member rather than a second copy of it.

// The modelled shapes this slice reads, named HERE rather than in the module's shared
// `use crate::ops::types::{..}` list at the top, for the reason the import image task
// store gives where it names its own.
use crate::ops::types::{
    ConversionTask, DiskImageDescription, DiskImageVolumeDescription, ImportInstanceTaskDetails,
    ImportVolumeTaskDetails,
};

// ---------------------------------------------------------------------------
// Network interfaces: the account's own elastic network interfaces, the writer
// seam a create lands on, and what a delete refuses
// ---------------------------------------------------------------------------

// `NetworkInterface` and `NetworkInterfacePrivateIpAddress` are in this module's shared
// import list, beside the store field that holds them; the attachment shape is imported
// here, beside the statuses and the derivation that read it.
use crate::ops::types::{AttachmentEnaSrdSpecification, NetworkInterfaceAttachment};

// ---------------------------------------------------------------------------
// Prefix lists: the managed ones an account creates, and the Amazon Web Services
// service ones a Region publishes
// ---------------------------------------------------------------------------

// =============================================================================
// Capacity Reservation billing requests
// =============================================================================

// =============================================================================
// AMI usage reports
// =============================================================================

/// This service's AMI usage REPORT slice: what one is stored as, the documented words,
/// defaults and bounds its members take, the quota a create asks about, the bytes a stored
/// report is charged, the comparison a replayed `ClientToken` is judged on and the two moves
/// the clock owes a report. A SUBMODULE, so that surface grows without touching the parts of
/// this file the other resource kinds own; `pub use` keeps every name reachable as
/// `crate::state::<name>`, which is where each handler names it. The report ID SPACE and the
/// two refusals a report id can draw stay here, beside this file's other id readers.
pub mod image_usage_reports;
pub use image_usage_reports::*;

/// Declarative-policies account status reports: what one is stored as, the id space and the
/// two refusals that id can draw, the bytes a stored report is charged, and the completion
/// the clock owes one. A SUBMODULE for the same reason [`image_usage_reports`] is one, and
/// `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod declarative_policies_reports;
pub use declarative_policies_reports::*;

/// The CREATE side of the Capacity Manager data export configurations: what one is created
/// from, the quota a create asks about, the bytes a stored configuration is charged, the
/// comparison a replayed `ClientToken` is judged on, and the one writer that stores one. A
/// SUBMODULE for the same reason [`image_usage_reports`] is one, and `pub use` keeps every
/// name reachable as `crate::state::<name>`. The id SPACE of a configuration, the two
/// refusals that id can draw, the count the account reports and the removal path stay beside
/// this file's other id readers.
pub mod capacity_manager_data_exports;
pub use capacity_manager_data_exports::*;

/// What one instance's console shows, which `GetConsoleOutput` serves as text and
/// `GetConsoleScreenshot` draws as an image — one definition, so the two cannot report one
/// instance's console differently.
pub mod instance_console;
pub use instance_console::*;

/// The BOOTS of one instance's guest — every time its operating system started, which is
/// what the console it wrote carries one block of. A SUBMODULE for the same reason
/// [`image_usage_reports`] is one, and `pub use` keeps every name reachable as
/// `crate::state::<name>`.
pub mod instance_boots;
pub use instance_boots::*;

/// The PROGRAM an instance's own user data gives its guest to run at boot — the commands
/// cloud-init runs on every boot and the ones it runs once. A SUBMODULE for the same reason
/// [`image_usage_reports`] is one.
pub mod guest_user_data;
pub use guest_user_data::*;

/// The STATUS CHECKS one instance reports — the system's and the instance's own, each owed to
/// the clock after its guest came up. A SUBMODULE for the same reason [`image_usage_reports`]
/// is one.
pub mod instance_status_checks;
pub use instance_status_checks::*;

/// What ONE boot of one instance's guest writes to that instance's console: the kernel's and
/// the init system's own lines, and the output of the user-data program the boot RUNS. A
/// SUBMODULE for the same reason [`image_usage_reports`] is one.
pub mod guest_console;
pub use guest_console::*;

/// Which capacity resources each of Capacity Manager's metrics measures, and the DIMENSION
/// VALUES each of those resources carries — the one reading every operation that answers
/// along Capacity Manager's dimensions shares, so the report of the available dimensions and
/// the report of the metric data itself cannot disagree about what the account has data for.
/// A SUBMODULE for the same reason [`image_usage_reports`] is one, and `pub use` keeps every
/// name reachable as `crate::state::<name>`.
pub mod capacity_manager_metrics;
pub use capacity_manager_metrics::*;

/// The tag keys EC2 Capacity Manager monitors for the account: what one is stored as, the
/// statuses the clock carries it through, and the one writer and the one reader every
/// operation that answers along them shares. A SUBMODULE for the same reason
/// [`image_usage_reports`] is one, and `pub use` keeps every name reachable as
/// `crate::state::<name>`.
pub mod capacity_manager_monitored_tags;
pub use capacity_manager_monitored_tags::*;

// ---------------------------------------------------------------------------
// Managed resource visibility (the account-wide setting)
// ---------------------------------------------------------------------------

use crate::ops::types::{
    MANAGED_RESOURCE_DEFAULT_VISIBILITY_VALUES, ManagedResourceVisibilitySettings,
};

// ---------------------------------------------------------------------------
// AMI store tasks
// ---------------------------------------------------------------------------

// The modelled shape this slice reports, named HERE rather than in the module's shared
// `use crate::ops::types::{..}` list at the top, the way the Spot Instance slice does it.
use crate::ops::types::StoreImageTaskResult;

// ---------------------------------------------------------------------------
// Spot Instance requests
// ---------------------------------------------------------------------------
//
// A Spot Instance request is a STANDING ORDER for capacity, not a resource that is
// finished when the call returns: "the request becomes active at this date and time and
// remains active until all instances launch, the request expires, or the request is
// canceled" (`RequestSpotInstancesRequest$ValidFrom`). So a stored request carries the
// modelled `SpotInstanceRequest` the describes report plus the two moves the CLOCK owes
// it — the fulfillment that launches its instance, and the expiry its own `ValidUntil`
// makes due — each applied once by
// [`ComAmazonawsEc2Amazonec2State::materialize_due_spot_instance_requests`], which
// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] calls.
//
// Fulfilling a request LAUNCHES AN INSTANCE into this store's own instance lifecycle —
// "the instance ID, if an instance has been launched to fulfill the Spot Instance
// request" (`SpotInstanceRequest$InstanceId`) — so a Spot Instance is one instance of
// this account like any other, reported by `DescribeInstances` with the
// `spot` lifecycle and the id of the request that launched it.

// The modelled shapes this slice reads, named HERE rather than in the module's shared
// `use crate::ops::types::{..}` list at the top, the way the VM import slice does it.
use crate::ops::types::{SpotInstanceRequest, SpotInstanceStatus};

// =============================================================================
// Spot Fleet requests
// =============================================================================

use crate::ops::types::{
    LaunchSpecification, SpotFleetRequestConfig, SpotFleetRequestConfigData,
};

// ---------------------------------------------------------------------------
// Secondary subnets
// ---------------------------------------------------------------------------

// =============================================================================
// VPN concentrators
// =============================================================================

use crate::ops::types::VpnConcentrator;

/// The catalogs of the modelled fleet enums, re-exported from the generated types so this
/// module names the ONE copy of each: `FleetType` (`FleetData$Type`), `FleetStateCode`
/// (`FleetData$FleetState`) and `FleetActivityStatus` (`FleetData$ActivityStatus`). The
/// single-member aliases below name a member of one of them; a second array here could
/// drift from the model.
pub use crate::ops::types::{
    FLEET_ACTIVITY_STATUS_VALUES, FLEET_STATE_CODE_VALUES, FLEET_TYPE_VALUES,
};

/// Client VPN endpoints and their target network associations: the two stores' id forms,
/// refusals, records and writers, in a file of their own rather than as more lines here.
/// Everything it declares is re-exported below, so a handler reaches it as
/// `crate::state::require_known_client_vpn_endpoint` like any helper of this module.
mod client_vpn;

pub use client_vpn::*;

/// The client connections of those endpoints: the store's record, the status codes one moves
/// through, the refusals for a connection or a user an endpoint does not have, and the one
/// writer of a termination — in a file of its own rather than as more lines here, because a
/// connection is a resource of its own beside the endpoint it belongs to. Everything it
/// declares is re-exported below, so a handler reaches it as
/// `crate::state::client_vpn_connection` like any helper of this module.
mod client_vpn_connections;

pub use client_vpn_connections::*;

/// The IAM roles each ACM certificate is associated with for ACM for Nitro Enclaves: the
/// store's record, the form of the two ARNs the surface takes, the refusals for a value that
/// is neither, and the projection every report of an association reads — in a file of their
/// own rather than as more lines here. Everything it declares is re-exported below, so a
/// handler reaches it as `crate::state::require_enclave_certificate_arn` like any helper of
/// this module.
pub mod enclave_certificate_iam_roles;
/// The Amazon S3 destination a flow log publishes to, and the permissions the log delivery
/// service needs on that bucket — read out of the destination ARN and written into the
/// bucket's own policy, where Amazon S3 owns it, in a file of its own rather than as more
/// lines here. Everything it declares is re-exported below, so a handler reaches it as
/// `crate::state::grant_flow_log_delivery` like any helper of this module.
pub mod flow_log_s3_delivery;
/// The attributes of the ACCOUNT itself — the catalog `DescribeAccountAttributes` reports,
/// each attribute's value derived from what already owns it, and the refusal for a name that
/// is no account attribute — in a file of its own rather than as more lines here. Everything
/// it declares is re-exported below, so a handler reaches it as
/// `crate::state::ACCOUNT_ATTRIBUTE_NAMES` like any helper of this module.
mod account_attributes;
mod network_acl_store;
mod fleets;
/// How an EC2 Fleet's capacity becomes instances — the pools it places, the one launch of an
/// instance from a launch specification that a fleet and a Spot Instance request share, and
/// the projection of what a fleet placed — in a file of its own rather than as more lines
/// here. Everything it declares is re-exported below, so a handler reaches it as
/// `crate::state::place_fleet_instances` like any helper of this module.
mod fleet_placements;
/// How an EC2 Fleet is DELETED — the states a deletion moves it through, the states one can
/// be deleted from, the per-fleet codes and sentences a batch delete reports, and the one
/// writer of a deletion — in a file of its own rather than as more lines here. Everything it
/// declares is re-exported below, so a handler reaches it as `crate::state::delete_fleet`
/// like any helper of this module.
mod fleet_deletion;
mod network_insights;
mod nat_gateways_and_vpn_connections;
/// What MAKING a Site-to-Site VPN connection consists of — the two tunnels it is brought up
/// with, the options and defaults the model publishes for one, the values this service mints
/// for a tunnel, the configuration document a connection hands its network administrator, and
/// the one write path that stores the result — in a file of its own rather than as more lines
/// beside the connection record. Everything it declares is re-exported below, so a handler
/// reaches it as `crate::state::VpnConnectionSpec` like any primitive of this module.
mod vpn_connection_creation;
mod spot_fleets;
mod spot_instance_requests;
mod store_image_tasks_and_route_servers;
/// Which route tables each route server may install its learned routes into: the
/// propagation store's record, the states the clock derives for it, and the association
/// that backs one — in a file of its own rather than as more lines here. Everything it
/// declares is re-exported below, so a handler reaches it as
/// `crate::state::route_server_propagations_at` like any helper of this module.
mod route_server_propagations;
mod image_block_public_access_and_metadata_defaults;
mod vpc_block_public_access_options;
mod image_usage_report_store;
mod capacity_billing_and_recycle_bins;
mod ipam;
mod ipam_deletion;
mod ipam_prefix_list_resolver_deletion;
mod ipam_pool_cidrs;
mod vpc_endpoints_and_transit_gateway_attachments;
mod prefix_lists_and_dedicated_hosts;
/// The Dedicated Host RESERVATION store: the states a reservation takes, its id form, the
/// stored record `PurchaseHostReservation` writes and `DescribeHostReservations` reports, and
/// the store's own ceiling — in a file of its own rather than as more lines here. Everything
/// it declares is re-exported below, so a handler reaches it as
/// `crate::state::HostReservationRecord` like any primitive of this module.
mod host_reservations;
mod network_interfaces_and_transit_gateways;
mod elastic_ips_and_transit_gateway_route_tables;
mod elastic_ip_associations;
mod byoip_and_capacity_reservations;
mod resource_dependencies_and_conversion_tasks;
mod security_group_rules_and_page_tokens;
mod images_and_zone_facts;
/// The Amazon Linux images Amazon publishes and the public Systems Manager parameter name
/// each one's id is published under — the POPULATION of the public AMI alias namespace, in a
/// file of its own rather than as more rows of `images_and_zone_facts`. Everything it
/// declares is re-exported below.
mod published_amazon_linux;
pub use published_amazon_linux::*;
/// The image-metadata hierarchies AWS publishes in Parameter Store beside an image's id —
/// the recommended ECS-optimized and Bottlerocket builds — and the ONE projection Parameter
/// Store reads the whole published catalog through. Everything it declares is re-exported
/// below.
mod published_image_metadata;
pub use published_image_metadata::*;
mod import_image_tasks_and_verified_access;
/// The export-image-task store: the status catalog `ExportImageTask$Status` publishes, the
/// record `ExportImage` writes and `DescribeExportImageTasks` reports, and the ceilings that
/// store keeps. Everything it declares is re-exported below, so a handler reaches it as
/// `crate::state::ExportImageTaskRecord` like any primitive of this module.
mod export_image_tasks;
/// Which Verified Access trust providers each Verified Access instance has attached: the one
/// writer of that attachment, the one reading of it, and the tokens the two calls that change
/// it were given — in a file of its own rather than as more lines here. Everything it declares
/// is re-exported below, so a handler reaches it as
/// `crate::state::verified_access_trust_provider_condensed` like any helper of this module.
mod verified_access_trust_provider_attachments;
mod verified_access_endpoint_deletion;
mod volumes;
/// Whether the KMS key an encrypted volume is to be created under will produce that
/// volume's data key, asked of the service that owns the key. Everything it declares is
/// re-exported below, so a handler reaches it as `crate::state::volume_key_refusal` like any
/// other primitive of this module.
mod volume_encryption;
mod local_gateways_and_filter_patterns;
pub mod outpost_lags;
mod service_link_virtual_interfaces;
/// Every tag this store holds, as `DescribeTags` reports one — read off the same list of
/// stores the two tag writers resolve an id against. Everything it declares is re-exported
/// below, so a handler reaches it as `crate::state::tag_description_cursor` like any
/// primitive of this module.
mod tag_descriptions;
mod instance_ids_and_instance_types;
/// The display data AWS publishes for a Region — its long name, its geography and its
/// sub-geography — as the zones of that Region report it. Re-exported below, so a zone's
/// projection reaches it as `crate::state::region_display` like any other primitive here.
mod availability_zone_display;
mod instances_and_filters;
mod vpcs_and_security_groups;
mod store;
/// The bundle-task store's identity and refusals: the prefix a bundle task id carries, the
/// code EC2 answers for one it cannot resolve, and the one reading of a request-named bundle
/// task. Everything it declares is re-exported below, so a handler reaches it as
/// `crate::state::require_bundle_task` like any helper of this module.
mod bundle_tasks;
/// The refusal EC2 answers a call to an API action it no longer serves with: the one
/// wording for an action the model documents as no longer supported. Everything it declares
/// is re-exported below, so a handler reaches it as `crate::state::api_not_supported` like
/// any other refusal of this module.
mod retired_api_actions;
/// The published parameter bounds the handlers enforce by hand, and the sentences EC2
/// refuses a violation of one with.
mod parameter_bounds;
pub use bundle_tasks::*;
pub use retired_api_actions::*;
pub use parameter_bounds::*;
pub use store::*;
pub use vpcs_and_security_groups::*;
pub use instances_and_filters::*;
pub use instance_ids_and_instance_types::*;
pub use availability_zone_display::*;
pub use local_gateways_and_filter_patterns::*;
pub use outpost_lags::*;
pub use service_link_virtual_interfaces::*;
pub use tag_descriptions::*;
pub use volume_encryption::*;
pub use volumes::*;
pub use import_image_tasks_and_verified_access::*;
pub use export_image_tasks::*;
pub use verified_access_trust_provider_attachments::*;
pub use verified_access_endpoint_deletion::*;
pub use images_and_zone_facts::*;
pub use security_group_rules_and_page_tokens::*;
pub use resource_dependencies_and_conversion_tasks::*;
pub use byoip_and_capacity_reservations::*;
pub use elastic_ips_and_transit_gateway_route_tables::*;
pub use elastic_ips_and_transit_gateway_route_tables::INCORRECT_STATE;
pub use elastic_ip_associations::*;
pub use network_interfaces_and_transit_gateways::*;
pub use network_interfaces_and_transit_gateways::INVALID_ROUTE_TABLE_ID_NOT_FOUND;
pub use prefix_lists_and_dedicated_hosts::*;
pub use host_reservations::*;
pub use vpc_endpoints_and_transit_gateway_attachments::*;
pub use ipam::*;
pub use ipam_deletion::*;
pub use ipam_prefix_list_resolver_deletion::*;
pub use ipam_pool_cidrs::*;
pub use capacity_billing_and_recycle_bins::*;
pub use image_usage_report_store::*;
pub use image_block_public_access_and_metadata_defaults::*;
pub use vpc_block_public_access_options::*;
pub use store_image_tasks_and_route_servers::*;
pub use route_server_propagations::*;
pub use spot_instance_requests::*;
pub use spot_fleets::*;
pub use nat_gateways_and_vpn_connections::*;
pub use vpn_connection_creation::*;
pub use network_insights::*;
pub use fleets::*;
pub use fleet_placements::*;
pub use fleet_deletion::*;
pub use network_acl_store::*;
pub use account_attributes::*;

pub use enclave_certificate_iam_roles::*;

pub use flow_log_s3_delivery::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPAM internet registry association fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_ipam_internet_registry_association_capacity(&self) -> bool {
        self.ipam_internet_registry_associations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another routing policy registration delta fits inside that same ceiling.
    pub fn has_ipam_routing_policy_registration_delta_capacity(&self) -> bool {
        self.ipam_routing_policy_registration_deltas.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an IPAM internet registry association and answer the
    /// `IpamInternetRegistryAssociationId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was stored.
    ///
    /// The id and the ARN are minted here rather than taken from `view`, exactly as
    /// [`Self::insert_ipam_pool`] mints a pool's, so no path can store an association
    /// under an id this service could not have issued. An association is stored with no
    /// registrations: every registration is made by a request.
    pub fn insert_ipam_internet_registry_association(
        &mut self,
        mut view: IpamInternetRegistryAssociation,
    ) -> Option<String> {
        if !self.has_ipam_internet_registry_association_capacity() {
            return None;
        }
        let association_id = self.next_id(IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX);
        let region = view
            .ipam_region
            .clone()
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        view.ipam_internet_registry_association_id = Some(association_id.clone());
        // "The Amazon Resource Name (ARN) of the internet registry association"
        // (`IpamInternetRegistryAssociation$IpamInternetRegistryAssociationArn`): an EC2 ARN
        // in the IPAM's own Region, under this resource's type.
        view.ipam_internet_registry_association_arn = Some(::engine::emulator_arn(
            "ec2",
            &region,
            &format!("{RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION}/{association_id}"),
        ));
        // "The ID of the Amazon Web Services account that owns the internet registry
        // association" (`IpamInternetRegistryAssociation$OwnerId`): this emulator serves one
        // account, so the association is owned by it rather than by anything a request says.
        view.owner_id = Some(::engine::emulator_account_id().to_string());
        self.add_stored_bytes(ipam_internet_registry_association_payload_bytes(&view));
        self.ipam_internet_registry_associations.insert(
            association_id.clone(),
            IpamInternetRegistryAssociationRecord {
                view,
                registrations: BTreeMap::new(),
                pending: None,
            },
        );
        Some(association_id)
    }

    /// Store an internet registry association a create has just made, and answer the
    /// `IpamInternetRegistryAssociationId` it was given — or `None` when the store is at its
    /// ceiling and NOTHING was stored.
    ///
    /// The ONE way a CREATED association is stored, the counterpart of
    /// [`Self::insert_creating_ipam_prefix_list_resolver`]: the association is answered in the
    /// non-terminal `create-in-progress` state, so the clock owes it
    /// [`IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE`] one [`TRANSITION_DELAY`]
    /// later ([`PendingStateMove`], applied by [`Self::materialize_due_transitions`]) — the
    /// span is never zero, so a caller reading the association straight after the call that
    /// made it still sees `create-in-progress`. `client_token` names the association from here
    /// on, so a replay of the same request answers it instead of creating a second one.
    pub fn insert_creating_ipam_internet_registry_association(
        &mut self,
        client_token: Option<&str>,
        mut view: IpamInternetRegistryAssociation,
    ) -> Option<String> {
        view.state =
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_CREATE_IN_PROGRESS.to_string());
        view.state_message =
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_CREATE_IN_PROGRESS_MESSAGE.to_string());
        let retained = client_token.unwrap_or_default().len();
        let association_id = self.insert_ipam_internet_registry_association(view)?;
        self.add_stored_bytes(retained);
        if let Some(record) = self
            .ipam_internet_registry_associations
            .get_mut(&association_id)
        {
            record.pending = Some(PendingStateMove::starting_now(
                IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_PENDING_ENABLE,
            ));
        }
        if let Some(token) = client_token {
            self.ipam_internet_registry_association_client_tokens
                .insert(token.to_string(), association_id.clone());
        }
        Some(association_id)
    }

    /// Store `record` as the routing policy registration of `association_id` for the `Cidr`
    /// its view carries, replacing whatever registration that prefix named before, and
    /// answer the canonical prefix it was filed under.
    ///
    /// The key is the prefix in the canonical form the store documents
    /// ([`canonical_byoip_cidr`]), taken here rather than trusted from the caller, so a
    /// writer that stores `192.0.2.5/24` cannot create a registration no lookup can name —
    /// the same contract as [`Self::insert_byoip_cidr`].
    ///
    /// `None`, with NOTHING written, when the view carries no prefix to file it under, when
    /// `association_id` names no association, when a NEW registration would cross the
    /// store's record ceiling, or when the bytes the registration would hold would cross
    /// [`MAX_STORED_BYTES`] — on EVERY path, replacement included, where the question is
    /// about the net change ([`Self::would_exceed_stored_bytes_after_refund`]).
    pub fn insert_ipam_routing_policy_registration(
        &mut self,
        association_id: &str,
        mut record: IpamRoutingPolicyRegistrationRecord,
    ) -> Option<String> {
        let cidr = record
            .view
            .cidr
            .as_deref()
            .and_then(|cidr| canonical_byoip_cidr(cidr).ok())?;
        record.view.cidr = Some(cidr.clone());
        let charge = ipam_routing_policy_registration_payload_bytes(&record.view);
        let association = self.ipam_internet_registry_associations.get(association_id)?;
        // A prefix the association does not already hold a registration for is a NEW record,
        // so it is the only one the record COUNT ceiling can be about; replacing one stores
        // no further record and gives back what the record it replaces was charged.
        let refund = association
            .registrations
            .get(&cidr)
            .map(|held| ipam_routing_policy_registration_payload_bytes(&held.view));
        let registrations_held = association.registrations.len();
        if refund.is_none() && registrations_held >= MAX_STORED_RECORDS_PER_STORE {
            return None;
        }
        // The BYTE budget, by contrast, binds every path, about the NET change: the charge
        // is dominated by caller-chosen text, so re-registering one prefix with a larger
        // record grows the store by the difference, and without this gate repeated
        // replacements would grow this process's memory past the only bound it has.
        if self.would_exceed_stored_bytes_after_refund(charge, refund.unwrap_or(0)) {
            return None;
        }
        if let Some(refund) = refund {
            self.release_stored_bytes(refund);
        }
        self.add_stored_bytes(charge);
        self.ipam_internet_registry_associations
            .get_mut(association_id)?
            .registrations
            .insert(cidr.clone(), record);
        Some(cidr)
    }

    /// The routing policy registration `association_id` holds for `cidr`, if any.
    ///
    /// `cidr` is canonicalised here, so a caller that names the same prefix two ways
    /// ("192.0.2.0/24" and "192.0.2.5/24") reaches the one registration.
    pub fn ipam_routing_policy_registration(
        &self,
        association_id: &str,
        cidr: &str,
    ) -> Option<&IpamRoutingPolicyRegistrationRecord> {
        let cidr = canonical_byoip_cidr(cidr).ok()?;
        self.ipam_internet_registry_associations
            .get(association_id)?
            .registrations
            .get(&cidr)
    }

    /// Mint the delta that records one change to a routing policy registration, and answer
    /// it as the caller is told about it — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] deltas, or the bytes this one would hold would cross
    /// [`MAX_STORED_BYTES`], and NOTHING was stored.
    ///
    /// A delta AWS hands back is not published yet — the state catalog's own `pending`,
    /// before `published` — so it is stored `pending` with the clock owing it `published`
    /// one [`TRANSITION_DELAY`] later, through [`Self::materialize_due_transitions`]: the
    /// span is never zero, so a caller reading the delta straight after the call that minted
    /// it still sees `pending`.
    pub fn insert_ipam_routing_policy_registration_delta(
        &mut self,
        spec: IpamRoutingPolicyRegistrationDeltaSpec,
    ) -> Option<IpamRoutingPolicyRegistrationDelta> {
        let view = IpamRoutingPolicyRegistrationDelta {
            delta_id: None,
            delta_json: Some(spec.delta_json),
            state: Some(IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PENDING.to_string()),
            state_message: Some(
                IPAM_ROUTING_POLICY_REGISTRATION_DELTA_PENDING_MESSAGE.to_string(),
            ),
        };
        let charge = ipam_routing_policy_registration_delta_payload_bytes(&view);
        if !self.has_ipam_routing_policy_registration_delta_capacity()
            || self.would_exceed_stored_bytes(charge)
        {
            return None;
        }
        let delta_id = self.next_id(IPAM_ROUTING_POLICY_REGISTRATION_DELTA_ID_PREFIX);
        let view = IpamRoutingPolicyRegistrationDelta {
            delta_id: Some(delta_id.clone()),
            ..view
        };
        self.add_stored_bytes(charge);
        let stored = view.clone();
        self.ipam_routing_policy_registration_deltas.insert(
            delta_id,
            IpamRoutingPolicyRegistrationDeltaRecord {
                view,
                ipam_internet_registry_association_id: spec.ipam_internet_registry_association_id,
                cidr: spec.cidr,
                pending: Some(PendingStateMove::starting_now(
                    IPAM_ROUTING_POLICY_REGISTRATION_DELTA_STATE_PUBLISHED,
                )),
            },
        );
        Some(stored)
    }

    /// The delta `delta_id` names, as it now stands.
    pub fn ipam_routing_policy_registration_delta(
        &self,
        delta_id: &str,
    ) -> Option<&IpamRoutingPolicyRegistrationDeltaRecord> {
        self.ipam_routing_policy_registration_deltas.get(delta_id)
    }

    /// The IPAM internet registry association `association_id` names, as it now stands.
    pub fn ipam_internet_registry_association(
        &self,
        association_id: &str,
    ) -> Option<&IpamInternetRegistryAssociationRecord> {
        self.ipam_internet_registry_associations.get(association_id)
    }

    /// Start deleting the IPAM internet registry association `association_id` names, and
    /// answer it as the store then holds it — `None` when the account holds no such
    /// association.
    ///
    /// "Deletes an IPAM internet registry association" is work AWS reports as under way: the
    /// `IpamInternetRegistryAssociationState` catalog carries both `delete-in-progress` and
    /// `delete-complete`, so the association is left in the TRANSIENT state with the clock
    /// owing it the terminal one ([`PendingStateMove`], materialised by
    /// [`Self::materialize_due_transitions`]). The span is never zero, so a caller acting
    /// straight after this call still sees `delete-in-progress`.
    ///
    /// The record STAYS in the store, because an association in either delete state still has
    /// a state to report ([`IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE`]) — so
    /// its caller-chosen bytes stay charged to the store's budget, which still holds them, the
    /// same arrangement as [`Self::begin_ipam_policy_deletion`].
    pub fn begin_ipam_internet_registry_association_deletion(
        &mut self,
        association_id: &str,
    ) -> Option<IpamInternetRegistryAssociation> {
        let record = self
            .ipam_internet_registry_associations
            .get_mut(association_id)?;
        let message = IPAM_INTERNET_REGISTRY_ASSOCIATION_DELETE_IN_PROGRESS_MESSAGE.to_string();
        let charged = message.len();
        let released = record.view.state_message.as_deref().map_or(0, str::len);
        record.view.state =
            Some(IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_IN_PROGRESS.to_string());
        record.view.state_message = Some(message);
        record.pending = Some(PendingStateMove::starting_now(
            IPAM_INTERNET_REGISTRY_ASSOCIATION_STATE_DELETE_COMPLETE,
        ));
        let view = record.view.clone();
        // The state message is stored text, charged like every other byte this store retains;
        // the message it replaces goes back to the budget.
        self.release_stored_bytes(released);
        self.add_stored_bytes(charged);
        Some(view)
    }
}

// ---------------------------------------------------------------------------
// Virtual private gateways
// ---------------------------------------------------------------------------
//
// See the module docs for why a gateway is stored as the modelled shape beside the move the
// clock owes it. The words below are members of the model's own enums, named here so the
// operations that create, report and delete a gateway cannot spell one value two ways.

/// The id prefix EC2 gives a virtual private gateway (`vgw-0123456789abcdef0`), which
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints the long form of.
pub const VPN_GATEWAY_ID_PREFIX: &str = "vgw";

/// `VpnState`'s `pending` member: a gateway a create has just minted, which is not usable
/// yet. The generated catalog of the four states is
/// [`crate::ops::types::VPN_STATE_VALUES`].
pub const VPN_GATEWAY_STATE_PENDING: &str = "pending";

/// `VpnState`'s `available` member: a gateway that has finished coming up.
pub const VPN_GATEWAY_STATE_AVAILABLE: &str = "available";

/// `VpnState`'s `deleting` member: a gateway a delete has started on, which is not gone yet.
pub const VPN_GATEWAY_STATE_DELETING: &str = "deleting";

/// `VpnState`'s `deleted` member: a gateway whose deletion has finished. The enum carries
/// this state, so a deleted gateway is still a gateway `DescribeVpnGateways` reports rather
/// than a record that vanished.
pub const VPN_GATEWAY_STATE_DELETED: &str = "deleted";

/// `GatewayType`'s only member: "Currently the only supported type is `ipsec.1`"
/// (`DescribeVpnGatewaysRequest$Filters`). The generated catalog is
/// [`crate::ops::types::GATEWAY_TYPE_VALUES`].
pub const GATEWAY_TYPE_IPSEC_1: &str = "ipsec.1";

/// The `ResourceType` a `CreateVpnGateway` tag specification may name: the virtual private
/// gateway it creates ("The tags to apply to the virtual private gateway",
/// `CreateVpnGatewayRequest$TagSpecifications`).
pub const RESOURCE_TYPE_VPN_GATEWAY: &str = "vpn-gateway";

/// EC2's error code for a request naming a virtual private gateway id this account holds no
/// gateway for.
pub const INVALID_VPN_GATEWAY_ID_NOT_FOUND: &str = "InvalidVpnGatewayID.NotFound";

/// EC2's error code for a request naming something that is not a virtual private gateway id
/// at all, so there is nothing to look up.
pub const INVALID_VPN_GATEWAY_ID_MALFORMED: &str = "InvalidVpnGatewayID.Malformed";

/// EC2's error code for a create that would take the account past its virtual private
/// gateway quota.
pub const VPN_GATEWAY_LIMIT_EXCEEDED: &str = "VpnGatewayLimitExceeded";

/// The largest number of virtual private gateways this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_TRANSIT_GATEWAYS`], not an AWS quota: each
/// gateway is a record in this process's memory, so the number a caller can create has to be
/// finite. [`ComAmazonawsEc2Amazonec2State::has_vpn_gateway_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`vpn_gateway_limit_exceeded`].
pub const MAX_VPN_GATEWAYS: usize = 1_000;

/// One stored virtual private gateway: what every operation that reports one answers with,
/// plus the move the clock owes it.
#[derive(Debug, Clone)]
pub struct VpnGatewayRecord {
    /// The gateway as `CreateVpnGateway` and `DescribeVpnGateways` report it.
    pub view: VpnGateway,
    /// The state this gateway is due to move to, while it is in a non-terminal one:
    /// `available` after a create, `deleted` after a delete. `None` once nothing is owed.
    pub pending: Option<PendingStateMove>,
}

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_vpn_gateway`] needs that it does not
/// derive itself: what the REQUEST said.
#[derive(Debug, Clone)]
pub struct VpnGatewaySpec {
    /// "The type of VPN connection this virtual private gateway supports"
    /// (`CreateVpnGatewayRequest$Type`) — the modelled `GatewayType`, which the boundary
    /// validator has already held to [`GATEWAY_TYPE_IPSEC_1`].
    pub gateway_type: String,
    /// "The Availability Zone for the virtual private gateway"
    /// (`CreateVpnGatewayRequest$AvailabilityZone`), absent when the request named none —
    /// "This field may be empty or not returned" (`VpnGateway$AvailabilityZone`).
    pub availability_zone: Option<String>,
    /// The resolved Amazon-side ASN ([`private_amazon_side_asn`]).
    pub amazon_side_asn: i64,
    /// The tags of a `vpn-gateway` tag specification.
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored virtual private gateway holds: its tags and the
/// Availability Zone name the request named. Everything else on the record — the id, the
/// state, the type, the ASN — is derived by this module or bounded by the model, so it is
/// fixed-size.
///
/// The ONE definition of that charge: `CreateVpnGateway` asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_vpn_gateway`] charges exactly it.
pub fn vpn_gateway_payload_bytes(spec: &VpnGatewaySpec) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.availability_zone.as_deref().map_or(0, str::len))
}

/// Refuse a request that named a virtual private gateway id this account holds no gateway
/// for: HTTP 400 [`INVALID_VPN_GATEWAY_ID_NOT_FOUND`], worded by the one place this module
/// words a missing resource id ([`id_not_found`]).
pub fn invalid_vpn_gateway_id_not_found<E>(vpn_gateway_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPN_GATEWAY_ID_NOT_FOUND,
        "vpnGateway ID",
        vpn_gateway_id,
    )
}

/// Refuse a create that would take the store past [`MAX_VPN_GATEWAYS`]: HTTP 400
/// [`VPN_GATEWAY_LIMIT_EXCEEDED`].
pub fn vpn_gateway_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPN_GATEWAY_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of virtual private gateways has been reached \
             ({MAX_VPN_GATEWAYS})"
        ),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another virtual private gateway fits inside the store's ceiling
    /// ([`MAX_VPN_GATEWAYS`]).
    ///
    /// The same contract as [`Self::has_transit_gateway_capacity`]: a create asks first and
    /// refuses with [`vpn_gateway_limit_exceeded`] when it answers `false`.
    pub fn has_vpn_gateway_capacity(&self) -> bool {
        self.vpn_gateways.len() < MAX_VPN_GATEWAYS
    }

    /// Insert a virtual private gateway in `pending`, and answer the `VpnGatewayId` it was
    /// given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id and the state — so two writers cannot report the same gateway differently.
    /// Like [`Self::insert_transit_gateway`], the caller-chosen bytes of the record are
    /// charged against the store's byte budget here, so a writer cannot store them
    /// uncharged, and the clock-owed `available` move is recorded rather than slept for.
    pub fn insert_vpn_gateway(&mut self, spec: VpnGatewaySpec) -> String {
        let vpn_gateway_id = self.next_id(VPN_GATEWAY_ID_PREFIX);
        self.add_stored_bytes(vpn_gateway_payload_bytes(&spec));
        let view = VpnGateway {
            vpn_gateway_id: Some(vpn_gateway_id.clone()),
            state: Some(VPN_GATEWAY_STATE_PENDING.to_string()),
            r#type: Some(spec.gateway_type),
            availability_zone: spec.availability_zone,
            amazon_side_asn: Some(spec.amazon_side_asn),
            // "Any VPCs attached to the virtual private gateway"
            // (`VpnGateway$VpcAttachments`): a gateway is created detached — "You must
            // first attach the virtual private gateway to the VPC" is `AttachVpnGateway`'s
            // job — so it carries no attachment yet.
            vpc_attachments: Some(Vec::new()),
            tags: Some(spec.tags),
        };
        self.vpn_gateways.insert(
            vpn_gateway_id.clone(),
            VpnGatewayRecord {
                view,
                // What the clock now owes this gateway: `available`, one TRANSITION_DELAY
                // from now — the same span, off the same monotonic clock, as a VPC's
                // pending move.
                pending: Some(PendingStateMove::starting_now(VPN_GATEWAY_STATE_AVAILABLE)),
            },
        );
        vpn_gateway_id
    }

    /// The virtual private gateway `vpn_gateway_id` names, refusing an id that is not shaped
    /// like one with [`INVALID_VPN_GATEWAY_ID_MALFORMED`] and one the account holds no
    /// gateway for with [`INVALID_VPN_GATEWAY_ID_NOT_FOUND`] — in that order, so "not a
    /// virtual private gateway id" is never reported as "no such gateway".
    pub fn require_vpn_gateway<E>(
        &self,
        vpn_gateway_id: &str,
    ) -> Result<&VpnGatewayRecord, Scaffolded<E>> {
        require_well_formed_id(
            VPN_GATEWAY_ID_PREFIX,
            vpn_gateway_id,
            INVALID_VPN_GATEWAY_ID_MALFORMED,
        )?;
        self.vpn_gateways
            .get(vpn_gateway_id)
            .ok_or_else(|| invalid_vpn_gateway_id_not_found(vpn_gateway_id))
    }

    /// [`Self::require_vpn_gateway`], for a writer: the same two refusals in the same order,
    /// handing back the record to change.
    pub fn require_vpn_gateway_mut<E>(
        &mut self,
        vpn_gateway_id: &str,
    ) -> Result<&mut VpnGatewayRecord, Scaffolded<E>> {
        require_well_formed_id(
            VPN_GATEWAY_ID_PREFIX,
            vpn_gateway_id,
            INVALID_VPN_GATEWAY_ID_MALFORMED,
        )?;
        self.vpn_gateways
            .get_mut(vpn_gateway_id)
            .ok_or_else(|| invalid_vpn_gateway_id_not_found(vpn_gateway_id))
    }
}

// =============================================================================
// IPAM policies
// =============================================================================

/// The id prefix this service mints an IPAM policy with.
///
/// NOT `ipam-pol`, and the recorded wire corpus is why. AWS answers
/// `ipam-pol-0000000000000000f` — a seventeen-hexadecimal-digit suffix, the long EC2
/// resource id form — with `InvalidIpamPolicyId.Malformed` ("The ipam-policy ID '…' is
/// malformed"), while the SAME suffix under the prefixes AWS does issue is resolved and only
/// then reported missing: `ipam-pool-0000000000000000f` ("The ipam-pool ID '…' does not
/// exist"), `ipam-scope-0000000000000000f` and `ipam-0000000000000000f` each come back as a
/// NotFound (the probes recorded on [`IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_PREFIX`], from
/// the same corpus). The suffix is therefore well formed and it is the PREFIX that EC2 does
/// not know: `ipam-pol` is not a resource-id prefix of this API, which is what makes an id
/// carrying it malformed rather than missing.
///
/// Which prefix AWS does issue for this resource is published nowhere the model carries
/// (`smithy_query(query="shape",shape="IpamPolicyId")` is a plain `String` with no
/// `@pattern`), so this one is CHOSEN: the resource kind as EC2's own refusal spells it
/// ([`RESOURCE_TYPE_IPAM_POLICY`]), in the style of the IPAM resources whose prefixes the
/// corpus does show (`ipam-pool`, `ipam-scope`).
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the seventeen lowercase hexadecimal
/// digits every long EC2 id carries, so every id this store mints is one
/// [`ipam_policy_id_is_well_formed`] accepts. It is DEFINED as
/// [`RESOURCE_TYPE_IPAM_POLICY`] rather than spelled again, so the prefix the ids carry and
/// the resource type the ARNs name cannot drift apart.
pub const IPAM_POLICY_ID_PREFIX: &str = RESOURCE_TYPE_IPAM_POLICY;

/// The `ResourceType` an IPAM policy's own ARN names, which is also how EC2's refusals spell
/// this resource kind ("The ipam-policy ID '…' is malformed").
pub const RESOURCE_TYPE_IPAM_POLICY: &str = "ipam-policy";

/// An IPAM policy in service: the `IpamPolicyState` catalog's `create-complete` member (the
/// generated catalog is `crate::ops::types::IPAM_POLICY_STATE_VALUES`).
pub const IPAM_POLICY_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A policy whose deletion has been asked for and is not finished: that catalog's
/// `delete-in-progress` member, the TRANSIENT state a delete answers with.
pub const IPAM_POLICY_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A policy whose deletion is finished: that catalog's `delete-complete` member, which the
/// clock owes every policy a delete has started on.
pub const IPAM_POLICY_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The `StateMessage` a policy carries while its deletion is under way — "a message about the
/// state of the IPAM policy" (`IpamPolicy$StateMessage`), in this crate's own words rather
/// than a recording's.
pub const IPAM_POLICY_DELETE_IN_PROGRESS_MESSAGE: &str = "The IPAM policy is being deleted.";

/// The `StateMessage` a policy carries once its deletion is finished, written by the clock
/// that owed it ([`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]).
pub const IPAM_POLICY_DELETE_COMPLETE_MESSAGE: &str = "The IPAM policy has been deleted.";

/// `InvalidIpamPolicyId.Malformed` (HTTP 400): the request named something that is not an
/// IPAM policy id at all, so there is nothing to look up. The code AWS answers, as the
/// recorded wire corpus behind this resource's stub issue shows
/// (`ipam-pol-0000000000000000f` -> `InvalidIpamPolicyId.Malformed`).
pub const INVALID_IPAM_POLICY_ID_MALFORMED: &str = "InvalidIpamPolicyId.Malformed";

/// `InvalidIpamPolicyId.NotFound` (HTTP 400): the request named a well-formed IPAM policy id
/// this account holds no policy for.
///
/// The `.NotFound` counterpart of the `.Malformed` code above, which is how every EC2
/// resource spells the second of those two questions ([`INVALID_IPAM_POOL_ID_NOT_FOUND`],
/// [`INVALID_IPAM_INTERNET_REGISTRY_ASSOCIATION_ID_NOT_FOUND`], …).
pub const INVALID_IPAM_POLICY_ID_NOT_FOUND: &str = "InvalidIpamPolicyId.NotFound";

/// Whether `id` is a well-formed IPAM policy id — one this service could have ISSUED.
///
/// `<prefix>-<8 or 17 lowercase hex digits>`, read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]) under this resource's own prefix
/// ([`IPAM_POLICY_ID_PREFIX`], whose doc carries the recorded evidence for it). Anything else
/// is `.Malformed` BEFORE any lookup, so a caller can tell "not an id of this kind" from "no
/// such policy".
pub fn ipam_policy_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(IPAM_POLICY_ID_PREFIX, id)
}

/// Refuse something that is not an IPAM policy id at all: HTTP 400
/// `InvalidIpamPolicyId.Malformed`, worded the way AWS words a malformed id of this resource
/// and built from the request's own value.
pub fn invalid_ipam_policy_id_malformed<E>(id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPAM_POLICY_ID_MALFORMED,
        400,
        format!("The {RESOURCE_TYPE_IPAM_POLICY} ID '{id}' is malformed"),
    )
}

/// Refuse a well-formed IPAM policy id this account holds no policy for: HTTP 400
/// `InvalidIpamPolicyId.NotFound`, worded by the one writer of that refusal
/// ([`id_not_found`]).
pub fn invalid_ipam_policy_id_not_found<E>(id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_IPAM_POLICY_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_IPAM_POLICY} ID"),
        id,
    )
}

/// Refuse an `IpamPolicyId` whose FORM is not one this service issues: HTTP 400
/// `InvalidIpamPolicyId.Malformed`.
///
/// The first of the two questions an operation taking a policy id asks, and the one that is
/// asked BEFORE `DryRun`: "not an id of this kind" is a fact about the request itself, while
/// "no such policy" is a fact about the account. The two are deliberately separate helpers —
/// this one and [`lookup_ipam_policy`] — so that order is expressed by the helpers an
/// operation calls rather than by a check it has to remember to repeat.
pub fn require_well_formed_ipam_policy_id<E>(id: &str) -> Result<(), Scaffolded<E>> {
    if ipam_policy_id_is_well_formed(id) {
        return Ok(());
    }
    Err(invalid_ipam_policy_id_malformed(id))
}

/// Resolve a well-formed `IpamPolicyId` against the account's policies, refusing one it holds
/// no policy for with HTTP 400 `InvalidIpamPolicyId.NotFound`.
///
/// The second of those two questions, asked after `DryRun` because it is about the account
/// rather than the request. Call [`require_well_formed_ipam_policy_id`] first: an id of no
/// known form must be answered `.Malformed` before any lookup, the order
/// [`require_well_formed_id`] documents for every other resource of this module.
pub fn lookup_ipam_policy<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    id: &str,
) -> Result<&'store IpamPolicyRecord, Scaffolded<E>> {
    store
        .ipam_policies
        .get(id)
        .ok_or_else(|| invalid_ipam_policy_id_not_found(id))
}

/// One stored IPAM policy: the modelled [`IpamPolicy`] every operation that reports one
/// answers with, plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other store
/// here holds one: a member a writer sets round-trips through `DescribeIpamPolicies` without
/// a field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct IpamPolicyRecord {
    /// The policy as the model shapes it, carrying the state reached so far.
    pub view: IpamPolicy,
    /// The terminal state the clock owes this policy while it is still transient
    /// (`delete-in-progress` -> `delete-complete`); `None` for one that owes none.
    pub pending: Option<PendingStateMove>,
}

impl IpamPolicyRecord {
    /// A policy sitting in `state`, with nothing owed to it by the clock.
    pub fn in_state(view: IpamPolicy, state: &str) -> Self {
        Self {
            view: IpamPolicy {
                state: Some(state.to_string()),
                ..view
            },
            pending: None,
        }
    }

    /// The policy's current `IpamPolicyState`.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// Whether a deletion of this policy has already been asked for: it is either still
    /// `delete-in-progress` or already `delete-complete`, and in neither state is there a
    /// policy left to delete.
    pub fn is_deleting(&self) -> bool {
        self.state() == IPAM_POLICY_STATE_DELETE_IN_PROGRESS
            || self.state() == IPAM_POLICY_STATE_DELETE_COMPLETE
    }
}

/// The bytes one stored IPAM policy holds: the tags assigned to it and the message about its
/// state, on top of the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
///
/// The ONE definition of that charge, the way
/// [`ipam_routing_policy_registration_payload_bytes`] is for a registration: a writer charges
/// exactly it.
pub fn ipam_policy_payload_bytes(view: &IpamPolicy) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(view.state_message.as_deref().map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPAM policy fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_ipam_internet_registry_association_capacity`].
    pub fn has_ipam_policy_capacity(&self) -> bool {
        self.ipam_policies.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an IPAM policy and answer the `IpamPolicyId` it was given — or `None` when the
    /// store is at one of its ceilings and NOTHING was stored.
    ///
    /// The id, the ARN and the owning account are minted here rather than taken from the
    /// record, exactly as [`Self::insert_ipam_internet_registry_association`] mints an
    /// association's, so no path can store a policy under an id this service could not have
    /// issued.
    pub fn insert_ipam_policy(&mut self, mut record: IpamPolicyRecord) -> Option<String> {
        let charge = ipam_policy_payload_bytes(&record.view);
        if !self.has_ipam_policy_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let policy_id = self.next_id(IPAM_POLICY_ID_PREFIX);
        let region = record
            .view
            .ipam_policy_region
            .clone()
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        record.view.ipam_policy_id = Some(policy_id.clone());
        // "The Amazon Resource Name (ARN) of the IPAM policy" (`IpamPolicy$IpamPolicyArn`):
        // an EC2 ARN in the policy's own Region, under this resource's type.
        record.view.ipam_policy_arn = Some(::engine::emulator_arn(
            "ec2",
            &region,
            &format!("{RESOURCE_TYPE_IPAM_POLICY}/{policy_id}"),
        ));
        record.view.ipam_policy_region = Some(region);
        // "The account ID that owns the IPAM policy" (`IpamPolicy$OwnerId`): this emulator
        // serves one account, so the policy is owned by it rather than by anything a request
        // says.
        record.view.owner_id = Some(::engine::emulator_account_id().to_string());
        self.add_stored_bytes(charge);
        self.ipam_policies.insert(policy_id.clone(), record);
        Some(policy_id)
    }

    /// The IPAM policy `policy_id` names, as it now stands.
    pub fn ipam_policy(&self, policy_id: &str) -> Option<&IpamPolicyRecord> {
        self.ipam_policies.get(policy_id)
    }

    /// Start deleting the IPAM policy `policy_id` names, and answer it as the store then
    /// holds it — `None` when the account holds no such policy.
    ///
    /// "Deletes an IPAM policy" is work AWS reports as under way: the `IpamPolicyState`
    /// catalog carries both `delete-in-progress` and `delete-complete`, so the policy is left
    /// in the TRANSIENT state with the clock owing it the terminal one
    /// ([`PendingStateMove`], materialised by [`Self::materialize_due_transitions`]). The
    /// span is never zero, so a caller acting straight after this call still sees
    /// `delete-in-progress`.
    ///
    /// The record STAYS in the store, because a policy in either delete state still has a
    /// state to report ([`IPAM_POLICY_STATE_DELETE_COMPLETE`]) — so its caller-chosen bytes
    /// stay charged to the store's budget, which still holds them, the same arrangement as
    /// [`Self::begin_secondary_network_deletion`].
    pub fn begin_ipam_policy_deletion(&mut self, policy_id: &str) -> Option<IpamPolicy> {
        let record = self.ipam_policies.get_mut(policy_id)?;
        let message = IPAM_POLICY_DELETE_IN_PROGRESS_MESSAGE.to_string();
        let charged = message.len();
        let released = record.view.state_message.as_deref().map_or(0, str::len);
        record.view.state = Some(IPAM_POLICY_STATE_DELETE_IN_PROGRESS.to_string());
        record.view.state_message = Some(message);
        record.pending = Some(PendingStateMove::starting_now(
            IPAM_POLICY_STATE_DELETE_COMPLETE,
        ));
        let view = record.view.clone();
        // The state message is stored text, charged like every other byte this store
        // retains; the message it replaces goes back to the budget.
        self.release_stored_bytes(released);
        self.add_stored_bytes(charged);
        Some(view)
    }
}

// =============================================================================
// VPC flow logs
// =============================================================================
//
// A flow log is the record of ONE monitored resource: "Creates one or more flow logs to
// capture information about IP traffic for a specific network interface, subnet, or VPC"
// (`CreateFlowLogs`), so a call naming N resources mints N flow logs. Each is stored as
// the modelled `FlowLog` shape ([`ComAmazonawsEc2Amazonec2State::flow_logs`]) for the
// reason a VPC is stored as a `Vpc`: `DescribeFlowLogs` reports exactly what the create
// stored, so the two cannot disagree about one flow log.
//
// What the three flow-log operations SHARE lives here — the id mint, the documented
// default record format, the status a stored flow log reports, the capacity ceiling and
// the error codes a refusal carries — so `CreateFlowLogs`, `DescribeFlowLogs` and
// `DeleteFlowLogs` cannot answer differently about the same flow log. Each operation's
// own validation stays in its `src/ops/<operation>.rs` file.

/// The record format a flow log created without a `LogFormat` gets: "If you omit this
/// parameter, the flow log is created using the default format"
/// (`CreateFlowLogsRequest$LogFormat`), which is the default set of fields AWS documents
/// for a flow log record, in the order it documents them (Amazon VPC User Guide, "Flow log
/// records" — the default format's field list).
///
/// A CONCRETE value rather than an absent member, because a client reads `LogFormat` to
/// learn the layout of the records it is about to parse.
pub const DEFAULT_FLOW_LOG_FORMAT: &str = "${version} ${account-id} ${interface-id} \
                                           ${srcaddr} ${dstaddr} ${srcport} ${dstport} \
                                           ${protocol} ${packets} ${bytes} ${start} \
                                           ${end} ${action} ${log-status}";

/// The field ids a flow log record may name — the vocabulary a `LogFormat` draws its
/// `${field-id}` tokens from.
///
/// "For more information about the available fields, see Flow log records in the Amazon VPC
/// User Guide or Transit Gateway Flow Log records in the Amazon Web Services Transit Gateway
/// Guide" (`CreateFlowLogsRequest$LogFormat`): the vocabulary is published PROSE, not a
/// model shape — the member is a plain `String` — so it is written out here, as the union of
/// the two published tables. The first block is the Amazon VPC User Guide's available
/// fields, in the order it lists them (the first fourteen are also the default format,
/// [`DEFAULT_FLOW_LOG_FORMAT`]); the second is the fields only a transit gateway record has,
/// from the Transit Gateway Guide's table.
///
/// One catalog for both, because `LogFormat` is validated the same way whatever resource
/// type the create monitors.
pub const FLOW_LOG_RECORD_FIELDS: [&str; 58] = [
    // Amazon VPC User Guide, "Flow log records" — available fields.
    "version",
    "account-id",
    "interface-id",
    "srcaddr",
    "dstaddr",
    "srcport",
    "dstport",
    "protocol",
    "packets",
    "bytes",
    "start",
    "end",
    "action",
    "log-status",
    "vpc-id",
    "subnet-id",
    "instance-id",
    "tcp-flags",
    "type",
    "pkt-srcaddr",
    "pkt-dstaddr",
    "region",
    "az-id",
    "sublocation-type",
    "sublocation-id",
    "pkt-src-aws-service",
    "pkt-dst-aws-service",
    "flow-direction",
    "traffic-path",
    "ecs-cluster-arn",
    "ecs-cluster-name",
    "ecs-container-instance-arn",
    "ecs-container-instance-id",
    "ecs-container-id",
    "ecs-second-container-id",
    "ecs-service-name",
    "ecs-task-definition-arn",
    "ecs-task-arn",
    "ecs-task-id",
    "reject-reason",
    // Amazon Web Services Transit Gateway Guide, "Flow log records" — the fields a transit
    // gateway record adds.
    "resource-type",
    "tgw-id",
    "tgw-attachment-id",
    "tgw-src-vpc-account-id",
    "tgw-dst-vpc-account-id",
    "tgw-src-vpc-id",
    "tgw-dst-vpc-id",
    "tgw-src-subnet-id",
    "tgw-dst-subnet-id",
    "tgw-src-eni",
    "tgw-dst-eni",
    "tgw-src-az-id",
    "tgw-dst-az-id",
    "tgw-pair-details",
    "packets-lost-no-route",
    "packets-lost-blackhole",
    "packets-lost-mtu-exceeded",
    "packets-lost-ttl-expired",
];

/// What separates a field id from the tag key it reads, in a record field that reports a tag
/// of the monitored resource ("The tag configuration associated with the Flow Logs Amazon
/// EC2 Tags feature fields in your custom log format" —
/// `CreateFlowLogsRequest$TagFieldSpecifications`).
///
/// A tag field's token ends in a tag KEY the caller chose, so no published table can
/// enumerate those tokens and [`FLOW_LOG_RECORD_FIELDS`] does not try: a token carrying this
/// separator names a tag rather than a field of the record, and is read as one.
const FLOW_LOG_TAG_FIELD_SEPARATOR: char = ':';

/// The `${field-id}` tokens of `log_format`, in the order the format names them.
///
/// "Specify the fields using the `${field-id}` format, separated by spaces"
/// (`CreateFlowLogsRequest$LogFormat`): a token is what one `${` and the next `}` enclose,
/// and the text between two tokens is separator rather than a field. An unterminated `${`
/// opens no token, so nothing after it is read as one.
pub fn flow_log_format_fields(log_format: &str) -> Vec<&str> {
    let mut fields: Vec<&str> = Vec::new();
    let mut rest = log_format;
    while let Some((_, opened)) = rest.split_once("${") {
        let Some((field, tail)) = opened.split_once('}') else {
            break;
        };
        fields.push(field);
        rest = tail;
    }
    fields
}

/// The `${field-id}` tokens of `log_format` that name no field a flow log record has, each
/// listed once and in the order the format names them.
///
/// A token is recognised when it names an entry of [`FLOW_LOG_RECORD_FIELDS`], or when it
/// reads a tag of the monitored resource ([`FLOW_LOG_TAG_FIELD_SEPARATOR`], whose tag key is
/// the caller's and so is not in any published table). Every other token names a field that
/// does not exist, and the create that carries it is refused with
/// [`unknown_flow_log_fields`].
pub fn unknown_flow_log_format_fields(log_format: &str) -> Vec<&str> {
    let mut unknown: Vec<&str> = Vec::new();
    for field in flow_log_format_fields(log_format) {
        let recognised = FLOW_LOG_RECORD_FIELDS.contains(&field)
            || field.contains(FLOW_LOG_TAG_FIELD_SEPARATOR);
        if !recognised && !unknown.contains(&field) {
            unknown.push(field);
        }
    }
    unknown
}

/// Refuse a `CreateFlowLogs` whose `LogFormat` names fields a flow log record does not have:
/// HTTP 400 [`INVALID_PARAMETER`].
///
/// The format is a layout of PUBLISHED fields — "For more information about the available
/// fields, see Flow log records" (`CreateFlowLogsRequest$LogFormat`) — so a token outside
/// that vocabulary names nothing the service could write, and the whole call is refused
/// before any flow log is stored rather than one being created that could never deliver the
/// records it promises.
///
/// The message names every unrecognised token, each quoted and spelled as the request
/// spelled it, in the order the format named them; `unknown` is never empty, because a
/// format whose every token is a field is not refused at all.
pub fn unknown_flow_log_fields<E>(unknown: &[&str]) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER,
        400,
        format!(
            "Unknown fields provided: {fields}",
            fields = unknown
                .iter()
                .map(|field| format!("'{field}'"))
                .collect::<Vec<String>>()
                .join(", "),
        ),
    )
}

/// The status a stored flow log reports: "The status of the flow log (`ACTIVE`)"
/// (`FlowLog$FlowLogStatus`) — the one value the member documents, and the status AWS
/// answers a flow log in as soon as `CreateFlowLogs` has made it, so this store owes it no
/// later move.
pub const FLOW_LOG_STATUS_ACTIVE: &str = "ACTIVE";

/// The delivery outcome a stored flow log reports: "The status of the logs delivery
/// (`SUCCESS` | `FAILED`)" (`FlowLog$DeliverLogsStatus`).
///
/// The outcome a flow log this store holds is in, for every destination type: a create is
/// served only once the destination it asks for resolves and carries the delivery role that
/// destination requires, so the flow log it stored is delivering — the same basis on which
/// the record reports [`FLOW_LOG_STATUS_ACTIVE`]. `FAILED` is the
/// other value the member documents, and belongs to a flow log whose delivery this store
/// knows to be unusable; nothing here stores one, so no flow log carries a
/// `DeliverLogsErrorMessage` either ("Information about the error that occurred" —
/// `FlowLog$DeliverLogsErrorMessage`, which exists only for a delivery that failed).
pub const DELIVER_LOGS_STATUS_SUCCESS: &str = "SUCCESS";

/// The destination type a `CreateFlowLogs` that names none gets: "Default:
/// `cloud-watch-logs`" (`CreateFlowLogsRequest$LogDestinationType`). One of the
/// `LogDestinationType` enum's members
/// (`smithy_query(query="shape",shape="LogDestinationType")`).
pub const LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS: &str = "cloud-watch-logs";

/// The `LogDestinationType` member naming an S3 bucket.
pub const LOG_DESTINATION_TYPE_S3: &str = "s3";

/// The `LogDestinationType` member naming a Firehose delivery stream.
pub const LOG_DESTINATION_TYPE_KINESIS_DATA_FIREHOSE: &str = "kinesis-data-firehose";

/// The aggregation interval a `CreateFlowLogs` that names none gets: "Default: 600"
/// (`CreateFlowLogsRequest$MaxAggregationInterval`).
pub const DEFAULT_MAX_AGGREGATION_INTERVAL: i32 = 600;

/// The aggregation intervals a flow log may be created with: "The possible values are 60
/// seconds (1 minute) or 600 seconds (10 minutes)"
/// (`CreateFlowLogsRequest$MaxAggregationInterval`), which `FlowLog$MaxAggregationInterval`
/// repeats as "Valid Values: 60 | 600".
pub const FLOW_LOG_AGGREGATION_INTERVALS: [i32; 2] = [60, DEFAULT_MAX_AGGREGATION_INTERVAL];

/// The aggregation interval a transit gateway flow log must use: "This parameter must be 60
/// seconds for transit gateway resource types"
/// (`CreateFlowLogsRequest$MaxAggregationInterval`).
pub const TRANSIT_GATEWAY_MAX_AGGREGATION_INTERVAL: i32 = 60;

/// The id prefix every flow log id carries (`fl-…`, the form EC2 reports a flow log under
/// and `VpcFlowLogId` values take).
pub const FLOW_LOG_ID_PREFIX: &str = "fl";

/// How many bytes a `ClientToken` this service mints for a `CreateFlowLogs` carries before
/// it is written as text: an opaque value, so the only thing its width has to do is make
/// every minted token distinct from every other one.
const GENERATED_CLIENT_TOKEN_BYTES: usize = 32;

/// The idempotency token a `CreateFlowLogs` that carried none is answered with.
///
/// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
/// request" (`CreateFlowLogsRequest$ClientToken`) is answered back on the result
/// (`CreateFlowLogsResult$ClientToken`), so a call that provided no token still learns the
/// token its flow logs are recorded under and can replay itself with it. The value is
/// SERVICE-generated, so it is drawn from the operating system's randomness and never from
/// the request, a counter or a clock: two calls asking for different flow logs must not be
/// handed the same token. Written as base64 text, which is what a token has to be to survive
/// a form-encoded body and an XML answer unchanged.
pub fn generated_client_token() -> String {
    use ::base64::Engine as _;
    use ::ssh_key::rand_core::RngCore;
    let mut bytes = [0u8; GENERATED_CLIENT_TOKEN_BYTES];
    ::ssh_key::rand_core::OsRng.fill_bytes(&mut bytes);
    ::base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The `ResourceType` a `CreateFlowLogs` tag specification may name: the flow log it
/// creates (`ResourceType` enum member `vpc-flow-log`).
pub const RESOURCE_TYPE_VPC_FLOW_LOG: &str = "vpc-flow-log";

/// The `FlowLogsResourceType` members that monitor a transit gateway rather than a network
/// resource — the types `TrafficType` "is not supported for" and whose `ResourceIds` list
/// carries its own smaller maximum (`CreateFlowLogsRequest$TrafficType`,
/// `$ResourceIds`, `$MaxAggregationInterval`).
pub const FLOW_LOG_TRANSIT_GATEWAY_RESOURCE_TYPES: [&str; 2] =
    ["TransitGateway", "TransitGatewayAttachment"];

/// The most resources one `CreateFlowLogs` may name for a transit gateway resource type:
/// "Maximum of 25 for transit gateway resource types"
/// (`CreateFlowLogsRequest$ResourceIds`).
pub const MAX_TRANSIT_GATEWAY_FLOW_LOG_RESOURCE_IDS: usize = 25;

/// The most resources one `CreateFlowLogs` may name for every other resource type:
/// "Maximum of 300 for the other resource types" (`CreateFlowLogsRequest$ResourceIds`).
pub const MAX_FLOW_LOG_RESOURCE_IDS: usize = 300;

/// The most characters a `LogFormat` may carry.
///
/// A CHOSEN retention bound, not a published quota: the member is caller-chosen text the
/// store keeps for as long as the flow log lives, and the model declares no `@length` for
/// it, so the handler is the only place its size can be bounded. Well above the documented
/// default format and any record layout a caller would write. Counted in CHARACTERS, the
/// way every other caller-text bound of this crate is ([`MAX_CLIENT_TOKEN_CHARS`]).
pub const MAX_FLOW_LOG_FORMAT_CHARS: usize = 4096;

/// The file format a flow log's destination options get when the request names none: "The
/// format for the flow log. The default is `plain-text`"
/// (`DestinationOptionsRequest$FileFormat`), a `DestinationFileFormat` enum member.
pub const DEFAULT_FLOW_LOG_FILE_FORMAT: &str = "plain-text";

/// EC2's error code for a flow-log call that named a flow log id this account holds no
/// flow log for (`DeleteFlowLogs`, `InvalidFlowLogId.NotFound`).
pub const INVALID_FLOW_LOG_ID_NOT_FOUND: &str = "InvalidFlowLogId.NotFound";

/// The codes `CreateFlowLogs` refuses an unresolvable `ResourceIds` entry with, one per
/// `FlowLogsResourceType` member that is named by a resource id.
///
/// The id segment is spelled `Id` rather than `ID`, which is how the flow-log API spells
/// its OWN code ([`INVALID_FLOW_LOG_ID_NOT_FOUND`]) and the spelling a recorded
/// `CreateFlowLogs` refusal of an unknown VPC carries. That is a DIVERGENCE from the codes
/// the rest of this crate answers for the same resources — [`INVALID_VPC_ID_NOT_FOUND`] is
/// `InvalidVpcID.NotFound`, [`INVALID_SUBNET_ID_NOT_FOUND`] is `InvalidSubnetID.NotFound`,
/// [`INVALID_NETWORK_INTERFACE_ID_NOT_FOUND`] and
/// [`INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND`] likewise — which is EC2's own documented
/// error-code list for the operations of those resources. Both spellings are therefore
/// deliberate: an operation answers the one its own API family answers, and these four
/// belong to the flow-log family alone. A NAT gateway has no `Invalid…Id.NotFound` code at
/// all, so that resource type keeps [`NAT_GATEWAY_NOT_FOUND`].
pub const INVALID_FLOW_LOG_VPC_ID_NOT_FOUND: &str = "InvalidVpcId.NotFound";

/// The flow-log family's code for a subnet it cannot resolve — see
/// [`INVALID_FLOW_LOG_VPC_ID_NOT_FOUND`] for why the spelling differs from
/// [`INVALID_SUBNET_ID_NOT_FOUND`].
pub const INVALID_FLOW_LOG_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetId.NotFound";

/// The flow-log family's code for a network interface it cannot resolve — see
/// [`INVALID_FLOW_LOG_VPC_ID_NOT_FOUND`].
pub const INVALID_FLOW_LOG_NETWORK_INTERFACE_ID_NOT_FOUND: &str =
    "InvalidNetworkInterfaceId.NotFound";

/// The flow-log family's code for a transit gateway it cannot resolve — see
/// [`INVALID_FLOW_LOG_VPC_ID_NOT_FOUND`].
pub const INVALID_FLOW_LOG_TRANSIT_GATEWAY_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayId.NotFound";

/// The flow-log family's code for a transit gateway attachment it cannot resolve — see
/// [`INVALID_FLOW_LOG_VPC_ID_NOT_FOUND`].
pub const INVALID_FLOW_LOG_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayAttachmentId.NotFound";

/// EC2's error code for a `CreateFlowLogs` that asks for a flow log the account already
/// holds — the same monitoring of the same resource, delivered to the same destination
/// (`FlowLogAlreadyExists`).
pub const FLOW_LOG_ALREADY_EXISTS: &str = "FlowLogAlreadyExists";

/// The error code a create refuses with once the flow-log store is full, in the form EC2
/// names every quota refusal with (`VpcLimitExceeded`, `SubnetLimitExceeded`):
/// `<Resource>LimitExceeded`. The ceiling it reports is this store's
/// ([`MAX_STORED_RECORDS_PER_STORE`]), not a published AWS quota.
pub const FLOW_LOGS_LIMIT_EXCEEDED: &str = "FlowLogsLimitExceeded";

/// Refuse a flow-log call that named flow log ids this account holds no flow log for, the
/// way EC2 refuses the WHOLE batch: HTTP 400 [`INVALID_FLOW_LOG_ID_NOT_FOUND`], with a
/// message built from the request — the count of unknown ids, then the ids themselves, one
/// space before each, which is the form a recorded refusal of a single unknown id shows
/// (`… are not found: [TotalCount: 1] fl-…`).
///
/// Only the UNKNOWN ids are named: an id the account does hold is not at fault and the
/// call leaves it untouched.
pub fn invalid_flow_log_id_not_found<E>(unknown: &[String]) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FLOW_LOG_ID_NOT_FOUND,
        400,
        format!(
            "These flow log ids in the input list are not found: [TotalCount: {count}] {ids}",
            count = unknown.len(),
            ids = unknown.join(" "),
        ),
    )
}

/// Refuse a create that would take the flow-log store past
/// [`MAX_STORED_RECORDS_PER_STORE`]: HTTP 400 [`FLOW_LOGS_LIMIT_EXCEEDED`].
pub fn flow_logs_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(FLOW_LOGS_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a `CreateFlowLogs` that asks for a flow log this account already holds: HTTP 400
/// [`FLOW_LOG_ALREADY_EXISTS`].
///
/// The message carries no variable part — the refusal is about the request as a whole
/// rather than about one member of it, so it names neither the monitored resource nor the
/// flow log already covering it.
pub fn flow_log_already_exists<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        FLOW_LOG_ALREADY_EXISTS,
        400,
        "Error. There is an existing Flow Log with the same configuration and log destination.",
    )
}

/// Refuse a `CreateFlowLogs` whose `ResourceIds` names a resource of `resource_type` this
/// account does not hold — the whole call, because the batch is resolved before anything is
/// stored.
///
/// Each code is the flow-log family's own for that resource (see
/// [`INVALID_FLOW_LOG_VPC_ID_NOT_FOUND`] for the spelling and why it differs from the code
/// the operations of those resources answer). `resource_type` is a `FlowLogsResourceType`
/// member (`smithy_query(query="shape",shape="FlowLogsResourceType")`).
///
/// The MESSAGE is the flow-log family's own too, and does not vary with the resource kind:
/// the monitored resource is named by its id alone (`Unknown resource <id>`, the wording a
/// recorded refusal of an unresolvable `ResourceIds` entry carries), where
/// [`id_not_found`] spells out the kind it was read as. One message for every member of
/// the enum, because the operation resolves all of them through the one lookup
/// ([`ComAmazonawsEc2Amazonec2State::holds_flow_log_resource`]).
pub fn invalid_flow_log_resource_id<E>(resource_type: &str, resource_id: &str) -> Scaffolded<E> {
    let code = match resource_type {
        "VPC" => INVALID_FLOW_LOG_VPC_ID_NOT_FOUND,
        "Subnet" => INVALID_FLOW_LOG_SUBNET_ID_NOT_FOUND,
        "NetworkInterface" => INVALID_FLOW_LOG_NETWORK_INTERFACE_ID_NOT_FOUND,
        "TransitGateway" => INVALID_FLOW_LOG_TRANSIT_GATEWAY_ID_NOT_FOUND,
        "TransitGatewayAttachment" => INVALID_FLOW_LOG_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        // The one remaining `FlowLogsResourceType` member, `RegionalNatGateway`, which has
        // no `Invalid…Id.NotFound` code of its own.
        _ => NAT_GATEWAY_NOT_FOUND,
    };
    Scaffolded::service_error(code, 400, format!("Unknown resource {resource_id}"))
}

/// Refuse a `CreateFlowLogs` that asks for a destination whose delivery needs an IAM role
/// the request did not carry: HTTP 400 [`INVALID_PARAMETER`].
///
/// "The ARN of the IAM role that allows Amazon EC2 to publish flow logs to the log
/// destination. This parameter is required if the destination type is cloud-watch-logs"
/// (`CreateFlowLogsRequest$DeliverLogsPermissionArn`) — so the member is not absent from a
/// request that needed nothing of it, it is absent from one whose `LogDestinationType`
/// makes it mandatory. That is [`INVALID_PARAMETER`] rather than [`MISSING_PARAMETER`],
/// and the message is the service's wording for a member another member makes mandatory:
/// it names `LogDestinationType` and the value `log_destination_type` that required the
/// role, whichever member the request named the destination itself with.
pub fn flow_log_delivery_role_required<E>(log_destination_type: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PARAMETER,
        400,
        format!(
            "DeliverLogsPermissionArn can't be empty if LogDestinationType is \
             {log_destination_type}."
        ),
    )
}

/// The name of the log group a `log-group:` CloudWatch Logs ARN names, or `None` when the
/// ARN names something else — for the caller that gave the destination as an ARN rather
/// than by name ("If the destination type is cloud-watch-logs, specify the ARN of a
/// CloudWatch Logs log group. For example: arn:aws:logs:region:account_id:log-group:my_group
/// Alternatively, use the LogGroupName parameter" —
/// `CreateFlowLogsRequest$LogDestination`). The name is READ OUT of the ARN the request
/// carried; there is no inverse, because an ARN cannot be composed from a group name
/// without a Region and an account the request never named.
///
/// A log-group ARN may carry a further segment after the name — the log-stream wildcard
/// `…:log-group:my_group:*` that a CloudWatch Logs resource ARN conventionally ends with —
/// so the name is the segment up to the next `:`, never the rest of the ARN.
pub fn log_group_name_in_arn(log_destination: &str) -> Option<&str> {
    let (_, resource) = log_destination
        .strip_prefix("arn:")?
        .split_once(":log-group:")?;
    let name = resource.split(':').next().unwrap_or_default();
    (!name.is_empty()).then_some(name)
}

/// The caller-chosen bytes one stored flow log will hold: every member whose length the
/// REQUEST chose, plus the `ClientToken` the token map keeps for it, on top of the fixed
/// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs. The id, the creation time
/// and the status are derived by this module, so they are fixed-size.
///
/// The ONE definition of that charge, the way [`carrier_gateway_payload_bytes`] is for a
/// carrier gateway: `CreateFlowLogs` asks it before the write,
/// [`ComAmazonawsEc2Amazonec2State::insert_flow_log`] charges exactly it and records the
/// number it charged, and [`ComAmazonawsEc2Amazonec2State::remove_flow_log`] gives back
/// exactly that number.
pub fn flow_log_payload_bytes(spec: &FlowLogSpec) -> usize {
    let text = |value: Option<&str>| value.map_or(0, str::len);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(&spec.tags))
        .saturating_add(spec.resource_id.len())
        .saturating_add(text(spec.log_group_name.as_deref()))
        .saturating_add(text(spec.log_destination.as_deref()))
        // A request that named no format is stored with the default one, which is text the
        // record holds just the same.
        .saturating_add(
            spec.log_format
                .as_deref()
                .map_or(DEFAULT_FLOW_LOG_FORMAT.len(), str::len),
        )
        .saturating_add(text(spec.deliver_logs_permission_arn.as_deref()))
        .saturating_add(text(spec.deliver_cross_account_role.as_deref()))
        .saturating_add(
            spec.tag_field_specifications
                .iter()
                .flatten()
                .map(|specification| {
                    text(specification.resource_type.as_deref()).saturating_add(
                        specification
                            .tag_keys
                            .iter()
                            .flatten()
                            .map(String::len)
                            .sum::<usize>(),
                    )
                })
                .sum::<usize>(),
        )
        .saturating_add(text(spec.client_token.as_deref()))
}

/// One stored flow log: the modelled [`FlowLog`] shape `DescribeFlowLogs` reports, beside
/// the two facts that shape carries no member for — the bytes this record was charged
/// against [`MAX_STORED_BYTES`], and the `ClientToken` that minted it.
///
/// The charge is kept ON the record, the way [`SecurityGroupRuleRecord`] keeps its own, so
/// a removal gives back exactly what the insert took however the accounting changes later.
/// The token is kept here for the same reason: a removal then reaches the one token entry
/// that names this flow log directly, instead of walking every entry of
/// [`ComAmazonawsEc2Amazonec2State::flow_log_client_tokens`].
#[derive(Debug, Clone)]
pub struct FlowLogRecord {
    /// The flow log as every reader of it reports it.
    pub view: FlowLog,
    /// What [`flow_log_payload_bytes`] charged for this record when it was stored.
    pub payload_bytes: usize,
    /// The `ClientToken` of the `CreateFlowLogs` that minted this flow log, if it carried
    /// one — the key of its [`ComAmazonawsEc2Amazonec2State::flow_log_client_tokens`] entry.
    pub client_token: Option<String>,
}

/// What ONE flow log is created with, beyond everything the service itself derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_flow_log`] mints the id, stamps the creation
/// time, sets the status and fills in the documented defaults).
///
/// One spec per MONITORED RESOURCE: a `CreateFlowLogs` naming several resources builds one
/// of these for each of them, so every flow log of that call is stored the same way.
#[derive(Debug, Clone)]
pub struct FlowLogSpec {
    /// "The ID of the resource being monitored" (`FlowLog$ResourceId`) — one entry of the
    /// call's `ResourceIds`.
    pub resource_id: String,
    /// "The type of traffic captured for the flow log" (`FlowLog$TrafficType`), absent for
    /// the transit gateway resource types the member is not supported for.
    pub traffic_type: Option<String>,
    /// "The type of destination for the flow log data" (`FlowLog$LogDestinationType`),
    /// already defaulted to [`LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS`] by the caller when
    /// the request named none.
    pub log_destination_type: String,
    /// "The Amazon Resource Name (ARN) of the destination for the flow log data"
    /// (`FlowLog$LogDestination`) — the ARN the REQUEST carried, and `None` for a request
    /// that named a CloudWatch Logs group by name instead, which is the one way the
    /// destination can be given without an ARN.
    pub log_destination: Option<String>,
    /// "The name of the flow log group" (`FlowLog$LogGroupName`) — set only for a
    /// CloudWatch Logs destination, which is the one destination type the member is valid
    /// for.
    pub log_group_name: Option<String>,
    /// "The fields to include in the flow log record" (`CreateFlowLogsRequest$LogFormat`),
    /// or `None` for a request that omitted it — which is what makes the flow log carry
    /// [`DEFAULT_FLOW_LOG_FORMAT`].
    pub log_format: Option<String>,
    /// "The ARN of the IAM role allows the service to publish logs to CloudWatch Logs"
    /// (`FlowLog$DeliverLogsPermissionArn`).
    pub deliver_logs_permission_arn: Option<String>,
    /// "The ARN of the IAM role that allows the service to publish flow logs across
    /// accounts" (`FlowLog$DeliverCrossAccountRole`).
    pub deliver_cross_account_role: Option<String>,
    /// "The maximum interval of time, in seconds, during which a flow of packets is
    /// captured and aggregated into a flow log record"
    /// (`FlowLog$MaxAggregationInterval`), already defaulted by the caller.
    pub max_aggregation_interval: i32,
    /// "The destination options" (`FlowLog$DestinationOptions`), as the request gave them.
    pub destination_options: Option<DestinationOptionsResponse>,
    /// "The tag configuration associated with the Flow Logs Amazon EC2 Tags feature fields
    /// in your custom log format" (`FlowLog$TagFieldSpecifications`).
    pub tag_field_specifications: Option<Vec<TagFieldSpecificationResponse>>,
    /// "The tags for the flow log" (`FlowLog$Tags`).
    pub tags: Vec<Tag>,
    /// The `ClientToken` the request carried, if any: the token map keeps it for as long as
    /// the flow log lives, so the bytes it holds are charged with the flow log's.
    pub client_token: Option<String>,
    /// When the flow log was created ("The date and time the flow log was created" —
    /// `FlowLog$CreationTime`). Taken from the request's instant rather than read here, so
    /// every flow log one call mints carries the same creation time.
    pub created_at: SystemTime,
}

/// What makes two flow logs the SAME flow log: the resource monitored, what is captured of
/// it and how it is recorded, and where the records are delivered.
///
/// Every member a request may leave out is read at the value the stored flow log carries
/// for it, so a create that spelled out a documented default asks for the same thing as one
/// that omitted it. Only what a flow log's own records and their delivery depend on is here:
/// the tags and the delivery roles name no different monitoring, so a request differing in
/// those alone asks for a flow log the account already has.
#[derive(Debug, PartialEq, Eq, Hash)]
struct FlowLogIdentity {
    /// "The ID of the resource being monitored" (`FlowLog$ResourceId`).
    resource_id: Option<String>,
    /// "The type of traffic captured for the flow log" (`FlowLog$TrafficType`).
    traffic_type: Option<String>,
    /// "The type of destination for the flow log data" (`FlowLog$LogDestinationType`).
    log_destination_type: Option<String>,
    /// WHERE the records go ([`flow_log_destination_of`]).
    destination: Option<String>,
    /// "The fields to include in the flow log record" (`FlowLog$LogFormat`), defaulted.
    log_format: String,
    /// "The maximum interval of time … during which a flow of packets is captured and
    /// aggregated into a flow log record" (`FlowLog$MaxAggregationInterval`).
    max_aggregation_interval: Option<i32>,
    /// "The format for the flow log" (`DestinationOptionsResponse$FileFormat`), defaulted.
    file_format: String,
    /// Whether Hive-compatible prefixes are used
    /// (`DestinationOptionsResponse$HiveCompatiblePartitions`), defaulted.
    hive_compatible_partitions: bool,
    /// Whether the flow log is partitioned per hour
    /// (`DestinationOptionsResponse$PerHourPartition`), defaulted.
    per_hour_partition: bool,
}

impl FlowLogIdentity {
    /// The identity of the flow log one [`FlowLogSpec`] would store.
    fn of_spec(spec: &FlowLogSpec) -> Self {
        let (file_format, hive_compatible_partitions, per_hour_partition) =
            flow_log_destination_options_in_force(spec.destination_options.as_ref());
        Self {
            resource_id: Some(spec.resource_id.clone()),
            traffic_type: spec.traffic_type.clone(),
            log_destination_type: Some(spec.log_destination_type.clone()),
            destination: flow_log_destination_of(
                &spec.log_destination_type,
                spec.log_destination.as_deref(),
                spec.log_group_name.as_deref(),
            ),
            log_format: spec
                .log_format
                .clone()
                .unwrap_or_else(|| DEFAULT_FLOW_LOG_FORMAT.to_string()),
            max_aggregation_interval: Some(spec.max_aggregation_interval),
            file_format,
            hive_compatible_partitions,
            per_hour_partition,
        }
    }

    /// The identity of a flow log as this store reports it.
    fn of_reported(view: &FlowLog) -> Self {
        let (file_format, hive_compatible_partitions, per_hour_partition) =
            flow_log_destination_options_in_force(view.destination_options.as_ref());
        Self {
            resource_id: view.resource_id.clone(),
            traffic_type: view.traffic_type.clone(),
            log_destination_type: view.log_destination_type.clone(),
            destination: flow_log_destination_of(
                view.log_destination_type.as_deref().unwrap_or_default(),
                view.log_destination.as_deref(),
                view.log_group_name.as_deref(),
            ),
            log_format: view
                .log_format
                .clone()
                .unwrap_or_else(|| DEFAULT_FLOW_LOG_FORMAT.to_string()),
            max_aggregation_interval: view.max_aggregation_interval,
            file_format,
            hive_compatible_partitions,
            per_hour_partition,
        }
    }
}

/// The destination options at the values they are in force with: "The default is plain-text"
/// (`DestinationOptionsRequest$FileFormat`), "The default is false"
/// (`$HiveCompatiblePartitions`, `$PerHourPartition`) — so options left out and options
/// spelled out at their own defaults are one and the same delivery.
fn flow_log_destination_options_in_force(
    options: Option<&DestinationOptionsResponse>,
) -> (String, bool, bool) {
    (
        options
            .and_then(|options| options.file_format.clone())
            .unwrap_or_else(|| DEFAULT_FLOW_LOG_FILE_FORMAT.to_string()),
        options
            .and_then(|options| options.hive_compatible_partitions)
            .unwrap_or(false),
        options
            .and_then(|options| options.per_hour_partition)
            .unwrap_or(false),
    )
}

/// Where a flow log's records are delivered, as the ONE value that names that destination.
///
/// A CloudWatch Logs destination is a log GROUP, which a request names either way: "specify
/// the ARN of a CloudWatch Logs log group … Alternatively, use the LogGroupName parameter"
/// (`CreateFlowLogsRequest$LogDestination`) — so the group is the destination however it was
/// named. Every other destination type is named by its ARN alone.
fn flow_log_destination_of(
    log_destination_type: &str,
    log_destination: Option<&str>,
    log_group_name: Option<&str>,
) -> Option<String> {
    let named = if log_destination_type == LOG_DESTINATION_TYPE_CLOUD_WATCH_LOGS {
        log_group_name.or_else(|| log_destination.and_then(log_group_name_in_arn))
    } else {
        log_destination
    };
    named.map(str::to_string)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` flow logs fit inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// A COUNT, not one record, because `CreateFlowLogs` stores one flow log per monitored
    /// resource and either stores the whole batch or none of it. The same contract as
    /// [`Self::has_carrier_gateway_capacity`] otherwise: a create asks first and refuses
    /// with [`flow_logs_limit_exceeded`] when it answers `false`.
    pub fn has_flow_log_capacity_for(&self, count: usize) -> bool {
        self.flow_logs.len().saturating_add(count) <= MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether this account holds the resource `resource_id` of the `FlowLogsResourceType`
    /// `resource_type` — the question `CreateFlowLogs` asks of every entry of its
    /// `ResourceIds` before it stores anything.
    ///
    /// Each type is resolved against the ONE store this service keeps that resource in, so
    /// a flow log can never name a resource no describe of that resource would report. A
    /// READ: an id no store holds is not given a resource here.
    pub fn holds_flow_log_resource(&self, resource_type: &str, resource_id: &str) -> bool {
        match resource_type {
            "VPC" => self.vpcs.contains_key(resource_id),
            "Subnet" => self.subnets.contains_key(resource_id),
            "NetworkInterface" => self.network_interfaces.contains_key(resource_id),
            "TransitGateway" => self.transit_gateways.contains_key(resource_id),
            "TransitGatewayAttachment" => {
                self.transit_gateway_attachments.contains_key(resource_id)
            }
            "RegionalNatGateway" => self.nat_gateways.contains_key(resource_id),
            // Unreachable: the model's `FlowLogsResourceType` enum has no other member, and
            // the boundary validator refuses a value outside it before a handler runs. An
            // unknown type resolves NO id, so it can only refuse, never admit.
            _ => false,
        }
    }

    /// Whether this account already holds any of the flow logs `specs` describe — the same
    /// resource monitored the same way and delivered to the same destination
    /// ([`FlowLogIdentity`]) — which is the question `CreateFlowLogs` asks of its WHOLE batch
    /// before it stores anything.
    ///
    /// Each stored flow log's identity is read once and each spec's once, rather than once
    /// per pair: one create may name hundreds of resources
    /// ([`MAX_FLOW_LOG_RESOURCE_IDS`]) and the store holds up to
    /// [`MAX_STORED_RECORDS_PER_STORE`] flow logs.
    ///
    /// A READ: it answers from the flow logs the store holds and writes nothing.
    pub fn holds_any_flow_log_like(&self, specs: &[FlowLogSpec]) -> bool {
        let held: HashSet<FlowLogIdentity> = self
            .flow_logs
            .values()
            .map(|record| FlowLogIdentity::of_reported(&record.view))
            .collect();
        specs
            .iter()
            .any(|spec| held.contains(&FlowLogIdentity::of_spec(spec)))
    }

    /// Insert one flow log, and answer the `FlowLogId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id, the creation time, the flow log status, the delivery status and the record
    /// format a request that named none gets ([`DEFAULT_FLOW_LOG_FORMAT`]) — so no writer
    /// can store a flow log a describe would have to invent a member for. Like
    /// [`Self::insert_carrier_gateway`], the caller-chosen bytes the record retains
    /// ([`flow_log_payload_bytes`]) are charged against the store's byte budget here, so a
    /// writer cannot store them uncharged.
    pub fn insert_flow_log(&mut self, spec: FlowLogSpec) -> String {
        let payload_bytes = flow_log_payload_bytes(&spec);
        let flow_log_id = self.next_id(FLOW_LOG_ID_PREFIX);
        let view = FlowLog {
            creation_time: Some(system_time_to_date_time(spec.created_at)),
            // "The status of the logs delivery (SUCCESS | FAILED)"
            // (`FlowLog$DeliverLogsStatus`) — reported for every flow log this store holds,
            // on the same basis as the `ACTIVE` flow log status below: the create resolved
            // the destination and the delivery role that destination requires before it got
            // here, so this flow log's delivery is the one the member calls
            // `DELIVER_LOGS_STATUS_SUCCESS`. "Information about the error that occurred"
            // (`$DeliverLogsErrorMessage`) describes a delivery that failed, so a successful
            // one carries no message.
            deliver_logs_error_message: None,
            deliver_logs_status: Some(DELIVER_LOGS_STATUS_SUCCESS.to_string()),
            deliver_logs_permission_arn: spec.deliver_logs_permission_arn,
            deliver_cross_account_role: spec.deliver_cross_account_role,
            flow_log_id: Some(flow_log_id.clone()),
            flow_log_status: Some(FLOW_LOG_STATUS_ACTIVE.to_string()),
            log_group_name: spec.log_group_name,
            resource_id: Some(spec.resource_id),
            traffic_type: spec.traffic_type,
            log_destination_type: Some(spec.log_destination_type),
            log_destination: spec.log_destination,
            // "If you omit this parameter, the flow log is created using the default
            // format" (`CreateFlowLogsRequest$LogFormat`) — applied HERE, so no writer can
            // store a flow log whose record layout a client cannot read.
            log_format: Some(
                spec.log_format
                    .unwrap_or_else(|| DEFAULT_FLOW_LOG_FORMAT.to_string()),
            ),
            tags: Some(spec.tags),
            max_aggregation_interval: Some(spec.max_aggregation_interval),
            destination_options: spec.destination_options,
            tag_field_specifications: spec.tag_field_specifications,
        };
        self.add_stored_bytes(payload_bytes);
        let client_token = spec.client_token;
        if let Some(token) = client_token.clone() {
            self.flow_log_client_tokens
                .entry(token)
                .or_default()
                .push(flow_log_id.clone());
        }
        self.flow_logs.insert(
            flow_log_id.clone(),
            FlowLogRecord {
                view,
                payload_bytes,
                client_token,
            },
        );
        flow_log_id
    }

    /// Remove the flow log `flow_log_id` and answer it as it was last reported, or `None`
    /// when the store holds no such flow log. The bytes the record held go back to the
    /// store's budget — exactly the number the insert charged — and the `ClientToken` that
    /// minted it stops naming it, so a replayed create cannot answer with a flow log that is
    /// gone.
    ///
    /// The token entry is reached through the record's own
    /// [`FlowLogRecord::client_token`], so removing a batch of flow logs costs one lookup
    /// each rather than a walk of every stored token.
    pub fn remove_flow_log(&mut self, flow_log_id: &str) -> Option<FlowLog> {
        let record = self.flow_logs.remove(flow_log_id)?;
        if let Some(token) = record.client_token.as_deref() {
            if let Some(minted) = self.flow_log_client_tokens.get_mut(token) {
                minted.retain(|minted_id| minted_id != flow_log_id);
                if minted.is_empty() {
                    self.flow_log_client_tokens.remove(token);
                }
            }
        }
        self.release_stored_bytes(record.payload_bytes);
        Some(record.view)
    }
}

// ---------------------------------------------------------------------------
// Network ACLs
// ---------------------------------------------------------------------------
//
// A network ACL is the packet filter a VPC applies at the boundary of its subnets. It is
// stored as the modelled `NetworkAcl` shape ([`ComAmazonawsEc2Amazonec2State::network_acls`])
// for the reason a VPC is stored as a `Vpc`: `DescribeNetworkAcls` reports exactly what a
// writer stored, so no two operations can disagree about one ACL.
//
// Every VPC has one. "Your VPC automatically comes with a modifiable default network ACL"
// (Amazon VPC User Guide, "Control subnet traffic with network access control lists"), so
// [`ComAmazonawsEc2Amazonec2State::insert_vpc`] writes it as the VPC is stored and
// [`ComAmazonawsEc2Amazonec2State::remove_vpc`] takes it away with the VPC.
//
// Every subnet is associated with one. "Each subnet in your VPC must be associated with a
// network ACL. If you don't explicitly associate a subnet with a network ACL, the subnet is
// automatically associated with the default network ACL" (same guide), so
// [`ComAmazonawsEc2Amazonec2State::insert_subnet`] records that association against the
// VPC's default ACL, and [`ComAmazonawsEc2Amazonec2State::remove_subnet`] drops it again.
// The association lives on the ACL's own `Associations` member — the one place it is held,
// so the describe reports the associations the subnet store implies rather than a second
// copy of them.

use crate::ops::types::{NetworkAcl, NetworkAclAssociation, NetworkAclEntry};
