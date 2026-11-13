// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/describe_vpcs.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
// Declared FIRST rather than in sorted position: a new ec2 suite lands every few minutes
// and inserts its two lines in sorted order, so the sorted neighbourhood of this suite is
// the contended region of this file — this fix lost several serialized squash merges to
// exactly that. The lines above are the file header, which no landing edits.
#[path = "all/prefix_lists.rs"]
mod prefix_lists;
// What `CreateClientVpnEndpoint` persists — the endpoint it stores from the request's members,
// the certificates it resolves against Certificate Manager, and what it refuses. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the
// tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/create_client_vpn_endpoint.rs"]
mod create_client_vpn_endpoint;
// The WORDING of EC2's duplicate-resource and unreadable-id refusals: the fixed sentence of
// each error class, with only the variable parts EC2 interpolates. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail
// of this file are where every new ec2 suite inserts its lines.
#[path = "all/conflict_and_unreadable_id_wording.rs"]
mod conflict_and_unreadable_id_wording;
// The `locale` an IPAM pool reports: every reader of a pool carries the modelled member, with
// the default spelled out for a pool created without one, and a `locale` filter selects by the
// value the body shows. Declared beside `prefix_lists` for the reason the note above gives:
// the sorted neighbourhood and the tail of this file are where every new ec2 suite inserts its
// lines.
#[path = "all/ipam_pool_locale.rs"]
mod ipam_pool_locale;
// The per-instance-type hardware facts the catalog publishes — the EBS, instance store,
// network, accelerator, processor and host-feature members that are facts of one type rather
// than functions of its name's size multiplier. Declared beside `prefix_lists` for the reason
// the note above gives: the sorted neighbourhood and the tail of this file are where every new
// ec2 suite inserts its lines.
#[path = "all/published_instance_type_hardware_facts.rs"]
mod published_instance_type_hardware_facts;
// The RECORDED response elements a reported shape carries that this checkout's model
// declares no member for — a VPC's `dhcpOptionsSource` and a Verified Access instance's
// transit-gateway attachment set — beside the state those same reads depend on: a created
// VPC reaching `available`, and the description every public IPv4 pool holds. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood and
// the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/recorded_response_members.rs"]
mod recorded_response_members;
// The ONE projection of a network ACL's rule table: every reader reports the ACL's stored
// rules egress-first by ascending rule number, each carrying the port range and the ICMP type
// and code as present-but-empty members where the protocol has no use for them. Declared
#[path = "all/network_acl_entry_set_projection.rs"]
mod network_acl_entry_set_projection;
// The MODELLED MEMBERS a successful response carries: the Region display data a zone
// reports, a volume's snapshot/zone id/operator, a launch's reservation groups and the
// member set of every instance, a launch template's version counters and operator, and
// every tag an address carries. Declared beside `prefix_lists` for the reason the note
// above gives: the sorted neighbourhood and the tail of this file are where every new ec2
// suite inserts its lines.
#[path = "all/modeled_response_members.rs"]
mod modeled_response_members;
// `RegisterImage`: the AMI a register mints from the request's own description — its members
// and block device mappings read back through `DescribeImages`, what it refuses, and the state
// it writes (a snapshot its AMI uses cannot be deleted). Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood and the tail of this file are where
// every new ec2 suite inserts its lines.
#[path = "all/register_image.rs"]
mod register_image;
// The FILTERS a describe serves for the members of the records it reports — the reported
// record's own id, and the tag pair for a record that carries tags — whether or not that
// operation's own documentation enumerates them. Declared beside `prefix_lists` for the reason
#[path = "all/describe_filters_on_reported_members.rs"]
mod describe_filters_on_reported_members;
// An application status check's `Protocol`, spelled in another ASCII case: the create is served
// and the declared spelling is stored. Declared beside `prefix_lists` for the same reason.
#[path = "all/application_status_check_protocol_case.rs"]
mod application_status_check_protocol_case;
// An EMPTY LIST member as a query client renders it — the member's own key with an empty value
// and no index — read as the list of zero elements it names. Declared beside `prefix_lists` for
// the same reason.
#[path = "all/query_empty_list_members.rs"]
mod query_empty_list_members;
// The LOCAL GATEWAYS an account's Outposts put it behind, as `DescribeLocalGateways`
// reports them: every stored gateway with each member of the modelled shape, the narrowing
// its documented filters and `LocalGatewayIds` do, the pages it cuts the store into, the
// bounds on one request's match, and what it refuses. Declared beside `prefix_lists` for
// the reason the note above gives: the sorted neighbourhood and the tail of this file are
// where every new ec2 suite inserts its lines.
#[path = "all/local_gateways.rs"]
mod local_gateways;
// `RunInstances` driven from a LAUNCH TEMPLATE: the version's parameters overlaid with the
// request's own, which version a request names, and what a template the account does not hold
// refuses. Declared beside `prefix_lists` for the reason the note above gives: the sorted
// neighbourhood and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/run_instances_from_launch_template.rs"]
mod run_instances_from_launch_template;
// `RunInstances` with a NETWORK INTERFACE SPECIFICATION: the interfaces a launch names are
// created in the subnets, with the groups, addresses and options they name, or an existing one
// is attached — and what a launch asks for that an interface cannot be given. Declared beside
// `prefix_lists` for the reason the note above gives: the sorted neighbourhood and the tail of
// this file are where every new ec2 suite inserts its lines.
#[path = "all/run_instances_network_interfaces.rs"]
mod run_instances_network_interfaces;
// The SQL Server High Availability REGISTRATIONS the account holds: what an enable writes and
// a describe reads back, what a disable takes out and reports, and the order both refuse in.
// Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/instance_sql_ha_registrations.rs"]
mod instance_sql_ha_registrations;
// Where `DryRun` is answered relative to an operation's own checks, for the operations whose
// recorded contract puts the flag before the resource the request names is looked up.
#[path = "all/dry_run_before_resource_lookup.rs"]
mod dry_run_before_resource_lookup;
// Setting the reverse DNS (PTR) record of an Elastic IP address: what the modify writes and
// reports, the one outstanding request per record, and what it refuses. Declared beside
// `prefix_lists` for the reason the note above gives.
#[path = "all/address_attribute_modify.rs"]
mod address_attribute_modify;
// The account's KEY PAIRS: the key `CreateKeyPair` generates for each type and format, the
// public half the store keeps and `DescribeKeyPairs` reports, the fingerprint each type is
// fingerprinted with, what each call refuses, and what the published filters select.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted
#[path = "all/key_pairs.rs"]
mod key_pairs;
// The key pair as a resource the rest of the service uses: what a created key pair is good for
// while it is held (reported by the read, usable as a launch's `KeyName`) and what its delete
// does and does not change (the name stops resolving and is free again; an instance already
// launched with it keeps it). Declared beside `key_pairs`, the suite it shares a subject with.
#[path = "all/key_pair_launch_lifecycle.rs"]
mod key_pair_launch_lifecycle;
// The ONE snapshot resource the EBS direct APIs and EC2 both describe: a snapshot
// `ebs:StartSnapshot` minted reported and resolved on the EC2 side through its whole
// lifecycle, the narrowings that select it, and the refusal that stays for an id nothing
// holds. Declared beside `prefix_lists` for the reason the note above gives: the sorted
#[path = "all/ebs_direct_api_snapshots.rs"]
mod ebs_direct_api_snapshots;
// The IPv6 address pools the account holds: the pool a BYOIP IPv6 provision creates under
// the id the range reports, the blocks it holds in address order, the narrowing its
// documented filters and `PoolIds` do, the pages `DescribeIpv6Pools` cuts the store into,
// and what that read refuses. Declared beside `prefix_lists` for the reason the note above
// gives: the sorted neighbourhood and the tail of this file are where every new ec2 suite
// inserts its lines.
#[path = "all/ipv6_pools.rs"]
mod ipv6_pools;
// What `DescribeAccountAttributes` reports about the ACCOUNT: every attribute the operation
// documents with the value the account holds for it, `default-vpc` asserted against the VPC
// the same Region's `DescribeVpcs` reports, the narrowing `AttributeNames` does, and the
// refusals for a name that is no account attribute and for a dry run. Declared beside
// What `DescribeBundleTasks` reports out of the bundle-task store: every member of a stored
// task, the ids and the published filters that narrow the report, the bounds on a request's
// match, the dry-run answer, and that describing writes nothing. Declared beside
#[path = "all/bundle_tasks.rs"]
mod bundle_tasks;
// The VPC Block Public Access OPTIONS of one Region: the mode
// `ModifyVpcBlockPublicAccessOptions` puts it into, the default a Region no modify was made
// for reports, the clock-owed `update-in-progress` -> `update-complete` move, and what each
// of the two operations refuses without writing. Declared beside `prefix_lists` for the
#[path = "all/vpc_block_public_access_options.rs"]
mod vpc_block_public_access_options;
// How this service refuses a request that violates a PUBLISHED parameter limit: the
// page-size bounds of a paged describe, the instance-count range of a Capacity Reservation,
// the order of a launch's two counts, the one attribute type a modify names, the attributes
// a reset acts on, and the hardware a Dedicated Host can be configured for — each in EC2's
// own sentence for that violation class, with the bound the operation publishes in it.
#[path = "all/published_parameter_limits.rs"]
mod published_parameter_limits;
// EC2 as a regional service: a stored resource is reported only in the Region it was created
// in, an account has one default VPC per Region, a Region it has not opted into refuses every
// request, an instance type is offered in some of a Region's zones and not others, and a
// terminated instance has released its network. Declared beside `prefix_lists` for the reason
#[path = "all/region_scoped_state.rs"]
mod region_scoped_state;
// What an Elastic IP association binds to and what both sides of it report: the interface
// the request names or the instance's primary one, the address record `DescribeAddresses`
// reports, the interface's own `association`, and the `DisassociateAddress` that ends it so
// `ReleaseAddress` can give the allocation up. Declared beside `prefix_lists` for the reason
// the note above gives: the sorted neighbourhood and the tail of this file are where every
// new ec2 suite inserts its lines.
#[path = "all/elastic_ip_interface_associations.rs"]
mod elastic_ip_interface_associations;
// How EC2 words the `IncorrectInstanceState` refusal across the family of operations that
// act on an instance only from certain lifecycle states: the instance id the request named,
// the action its state does not admit, and no disclosure of the state itself. Declared
#[path = "all/incorrect_instance_state_refusals.rs"]
mod incorrect_instance_state_refusals;
// Resolving a launch AMI the documented way out of the published catalog: every variant
// Amazon publishes has a CURRENT build — available and not deprecated — so `Owners=amazon`
// plus that variant's published name pattern and `state=available` reports an image, and the
// newest match launches. Declared beside `prefix_lists` for the reason the note above gives:
// the sorted neighbourhood and the tail of this file are where every new ec2 suite inserts
// its lines.
#[path = "all/published_amazon_linux_catalog.rs"]
mod published_amazon_linux_catalog;
// The account's EC2 serial console access: the one persisted account-level boolean
// `EnableSerialConsoleAccess` and `DisableSerialConsoleAccess` move and
// `GetSerialConsoleAccessStatus` reports, with the `ManagedBy` every value carries and the
// `DryRun` each of the three refuses without writing. Declared beside `prefix_lists` for the
#[path = "all/serial_console_access.rs"]
mod serial_console_access;
// What `ModifyNetworkInterfaceAttribute` writes onto one stored network interface and what
// `ResetNetworkInterfaceAttribute` puts back to its default, each read back through the
// describes that report the same record. Declared beside `prefix_lists` for the reason the
// note above gives: the sorted neighbourhood and the tail of this file are where every new
#[path = "all/modify_network_interface_attribute.rs"]
mod modify_network_interface_attribute;
// Which network ACL a subnet is associated with, changed by `ReplaceNetworkAclAssociation`
// and read back through `DescribeNetworkAcls`: the one association the moved subnet is left
// with, the new association id the call answers and the next move names, the cross-VPC
// pairing it refuses, and the ids and dry run that move nothing. Declared beside
#[path = "all/replace_network_acl_association.rs"]
mod replace_network_acl_association;
// What `CreateDefaultVpc` creates and what it refuses: the default VPC of the Region the
// request is signed for — the /16, a default subnet in every Availability Zone of that Region
// and the VPC's default security group — and the `DefaultVpcAlreadyExists` refusal a Region
// that already has one answers. Declared beside `prefix_lists` for the reason the note above
#[path = "all/create_default_vpc.rs"]
mod create_default_vpc;
// What `ReplaceNetworkAclEntry` overwrites: the one rule a network ACL holds at a rule number
// in a direction, replaced in place and read back through `DescribeNetworkAcls`, beside the
// rule a replacement may not write and the refusals it answers. Declared beside
#[path = "all/replace_network_acl_entry.rs"]
mod replace_network_acl_entry;
// The per-VPC VPC Encryption Control slice: what `CreateVpcEncryptionControl` records for one
// VPC, what `ModifyVpcEncryptionControl` writes on it, what `DeleteVpcEncryptionControl`
// retires, and what `DescribeVpcEncryptionControls` reports back out of that one store — with
// the clock-owed moves each starts, the narrowing and paging of the report, and the refusals
// each answers. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted neighbourhood and the tail of this file are where every new ec2 suite inserts its
#[path = "all/vpc_encryption_controls.rs"]
mod vpc_encryption_controls;
// The route path out of a VPC: an internet gateway attached to exactly one VPC, a route
// table created for one VPC with the local route it comes with, one route per destination
// against a target of that VPC, and a subnet associated with one table of its own VPC — each
// read back through its own describe. Declared beside `prefix_lists` for the reason the note
#[path = "all/vpc_internet_path.rs"]
mod vpc_internet_path;
// The create side of the route table slice: the one table `CreateRouteTable` stores for the
// VPC a request names, with the local route it comes with for each block that VPC has
// associated, the tags it asked for, the client token that makes the create idempotent, and
// the refusals and dry run that store nothing — each read back through `DescribeRouteTables`.
#[path = "all/create_route_table.rs"]
mod create_route_table;
// An account's internet gateways: what `CreateInternetGateway` stores and
// `DescribeInternetGateways` reports back out of that store — the id, the owning account, the
// empty attachment set, the tags, and how the id list, the documented filters and the page
// size narrow the report. Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/internet_gateways.rs"]
mod internet_gateways;
// The other half of that report: how `InternetGatewayIds` and `Filters` narrow the internet
// gateway store and how `MaxResults`/`NextToken` then walk exactly the narrowed set, at every
// page size the operation publishes — and that the walk leaves the store as it was. Declared
// beside `internet_gateways` because it reads the same store through the same describe.
#[path = "all/describe_internet_gateways_selection.rs"]
mod describe_internet_gateways_selection;
// What `DescribeNatGateways` reports out of the NAT gateway store: every member a stored
// gateway holds, how the id list, the documented filters and the page size narrow it, what it
// refuses, and the address status the clock owes a gateway it is read through. Declared beside
#[path = "all/describe_nat_gateways.rs"]
mod describe_nat_gateways;
// The FIXED TEXT of a refusal's message, per class: the sentence the service words that error
// class with, with only the variable parts it interpolates from the request and the store —
// no restatement of the rule the value missed, no identifier out of the account's own state,
// and the holder set of a `VolumeInUse` rendered as the set it is. Declared beside
#[path = "all/refusal_fixed_text.rs"]
mod refusal_fixed_text;
// What `GetManagedPrefixListEntries` reports: the entry set of the one prefix list a request
// names — a list the account created or one the Region publishes — in CIDR order, how it
// pages, which version it reports and what it refuses. Declared beside `prefix_lists` for the
#[path = "all/get_managed_prefix_list_entries.rs"]
mod get_managed_prefix_list_entries;
// What `GetManagedPrefixListAssociations` reports: the resources that reference the one prefix
// list a request names — a security group whose rule grants it, a route table routing to it, a
// transit gateway route table referencing it — each with its own owner, how it pages and what
// it refuses. Declared beside `get_managed_prefix_list_entries` because the two report two
// projections of one prefix list.
#[path = "all/get_managed_prefix_list_associations.rs"]
mod get_managed_prefix_list_associations;
// What `DescribeIpamPools` reports out of the IPAM pool store: every member the create stored,
// how the id list, the documented filters and the page size narrow it, what it refuses, and the
// state move the clock owes a pool it is read through. Declared after the suite above rather
// than directly beside `prefix_lists`: that anchor is where every new ec2 suite inserts its
// lines, so these two sit one block further down with context between them.
#[path = "all/describe_ipam_pools.rs"]
mod describe_ipam_pools;
// What `DescribeIpamResourceDiscoveries` reports out of the store: the default resource
// discovery every `CreateIpam` comes up with, narrowed by the ids and filters the request
// carries, paged in id order, and the `create-complete` move the clock owes one it is read
// through.
#[path = "all/describe_ipam_resource_discoveries.rs"]
mod describe_ipam_resource_discoveries;
// What `DescribeIpamResourceDiscoveryAssociations` reports: the associations the store holds,
// narrowed to the ids and filters the request carries, paged in association-id order, and the
// `associate-complete` move the clock owes one it is read through.
#[path = "all/describe_ipam_resource_discovery_associations.rs"]
mod describe_ipam_resource_discovery_associations;
// The entries (rules) of a network ACL: what `CreateNetworkAclEntry` stores on a stored ACL
// and in which order, what `DeleteNetworkAclEntry` takes off it, and the not-found answer
// every read of a `DeleteNetworkAcl`-ed ACL gets. Declared beside `prefix_lists` for the
#[path = "all/network_acl_entries.rs"]
mod network_acl_entries;
// What `CancelCapacityReservation` releases: the reservation's own `cancelled` state in every
// reader of it, and the terminal status cancelling either side gives an interruptible
// allocation. Declared beside `prefix_lists` for the reason the note above gives.
#[path = "all/capacity_reservation_cancellation.rs"]
mod capacity_reservation_cancellation;
// `DescribeAddressesAttribute` as the projection of an Elastic IP address's `domain-name`
// attribute state it is: which addresses carry a row, how `AllocationIds` filters them, what
// it refuses, and how it pages. Declared beside `prefix_lists` for the reason the note above
#[path = "all/describe_addresses_attribute.rs"]
mod describe_addresses_attribute;
// What `CancelConversionTask` cancels — the form it judges a conversion task id by, the
// states that admit a cancel, the `cancelling` -> `cancelled` move the clock owes an
// admitted one, and what `DryRun` answers. Declared beside `prefix_lists` for the reason the
#[path = "all/conversion_task_cancellation.rs"]
mod conversion_task_cancellation;
// The two forms an instance id is read in before any lookup, and the two answers a value
// inside and outside them gets, across a describe, a password read and a mutation. Declared
#[path = "all/instance_id_forms.rs"]
mod instance_id_forms;
// What `CreateImageUsageReport` computes one AMI usage report from — the image, resource
// types, scope options, accounts and tags the request names — the `pending` -> `available`
// move the clock owes it, the 30-day deletion it carries, its `ClientToken` idempotency and
// what it refuses. Declared beside `prefix_lists` for the reason the note above gives: the
#[path = "all/create_image_usage_report.rs"]
mod create_image_usage_report;
// What `EnableFastSnapshotRestores` stores for each (snapshot, Availability Zone) pair, the
// lifecycle the clock carries that pair through, what `DescribeFastSnapshotRestores` reports,
// narrows, pages and refuses, and what `DisableFastSnapshotRestores` leaves behind. Declared
// HERE rather than in sorted position or beside `prefix_lists`: both of those are the
// contended regions the note at the top of this file describes, and this fix lost three
// serialized squash merges to the lines just below `prefix_lists`.
#[path = "all/fast_snapshot_restore_lifecycle.rs"]
mod fast_snapshot_restore_lifecycle;
// What `CopySnapshot` mints from a stored snapshot, which members it reads off the source and
// which off the request, and what it refuses. Declared beside `prefix_lists` for the reason
#[path = "all/copy_snapshot.rs"]
mod copy_snapshot;
// What `ModifyVerifiedAccessInstanceLoggingConfiguration` sets on one Verified Access
// instance, what an instance nothing has configured carries, and what
// `DescribeVerifiedAccessInstanceLoggingConfigurations` reports, narrows, pages and refuses.
// Declared HERE rather than in sorted position or beside `prefix_lists`: both of those are the
// contended regions the note at the top of this file describes, and this fix lost two
#[path = "all/verified_access_instance_logging.rs"]
mod verified_access_instance_logging;
// How the interruptible-allocation pair screens a Capacity Reservation id: the shape it reads
// one by, the not-found answer every value of that shape gets and the malformed answer every
// value outside it gets, across both operations. Declared HERE rather than in sorted position
// or beside `prefix_lists`, for the reason the note at the top of this file gives: both of
// those are the contended regions every new ec2 suite inserts its lines into.
#[path = "all/interruptible_allocation_capacity_reservation_ids.rs"]
mod interruptible_allocation_capacity_reservation_ids;
// What `CreateTransitGatewayConnect` computes a Connect attachment from — the transport
// attachment it names, that attachment's own transit gateway, the requested tunnel protocol
// and tags — the `pending` -> `available` move the clock owes it, and what it refuses.
#[path = "all/create_transit_gateway_connect.rs"]
mod create_transit_gateway_connect;
// What `DeleteVpnConcentrator` takes away, what it refuses and what it leaves alone.
#[path = "all/delete_vpn_concentrator.rs"]
mod delete_vpn_concentrator;
// What `DisassociateSubnetCidrBlock` lets go of: the association it names, wherever in the
// account's subnets it is held, the transient state it answers, the clock-owed terminal
// state, and what it refuses. Declared beside `prefix_lists` for the reason the note above
#[path = "all/disassociate_subnet_cidr_block.rs"]
mod disassociate_subnet_cidr_block;
// The sentence this service refuses a request with, per refusal CLASS: the resource named in
// that class's own words, the request's value interpolated in that class's own quoting and
// position, and nothing repeated back by a class whose sentence interpolates nothing.
#[path = "all/per_resource_refusal_wording.rs"]
mod per_resource_refusal_wording;
// What `ModifyTransitGateway` applies to a stored transit gateway, what it leaves alone,
// what it refuses, and the `modifying` -> `available` move the clock owes the gateway.
#[path = "all/modify_transit_gateway.rs"]
mod modify_transit_gateway;
// The primary network interface a launch gives its instance, the subnet and VPC it puts the
// instance in, and the filters that select on them. Declared beside `prefix_lists` for the
#[path = "all/instance_network_interfaces.rs"]
mod instance_network_interfaces;
// How `DescribeInstances` pages: the instances `MaxResults` caps and the token that walks to
// the full set, over stores whose instances are spread differently over reservations.
#[path = "all/describe_instances_pages.rs"]
mod describe_instances_pages;
// The EBS-optimized performance, the NVMe-for-EBS answer and the hibernation support
// `DescribeInstanceTypes` reports, across families, generations and sizes. Declared beside
#[path = "all/instance_type_ebs_and_hibernation.rs"]
mod instance_type_ebs_and_hibernation;
// The deprecated images of the published AMI catalog: their age, when they were deprecated,
// and what a name-filtered `DescribeImages` does with them. Declared beside `prefix_lists`
// for the reason the note above gives.
#[path = "all/deprecated_catalog_images.rs"]
mod deprecated_catalog_images;
// The code and the request id a refusal of the instance read and write paths carries.
#[path = "all/instance_read_path_refusals.rs"]
mod instance_read_path_refusals;
// What EC2 refuses about a REQUEST before it reads any state: a page size beside an id
// selection, a Capacity Block search that names both dimensions or neither, an idempotency
// token replayed with different parameters, and an AMI id a launch could not have been
// issued. Declared beside `prefix_lists` for the reason the note above gives: the sorted
#[path = "all/request_validation_before_state.rs"]
mod request_validation_before_state;
// What `DeleteCapacityManagerDataExport` takes out of the account's Capacity Manager data
// export configurations, what the reader then counts, and what the delete refuses. Declared
#[path = "all/capacity_manager_data_exports.rs"]
mod capacity_manager_data_exports;
// The form an Amazon FPGA Image id is read in before any lookup, and the two answers an id
// outside and inside it gets. Declared beside `prefix_lists` for the reason the note above
#[path = "all/fpga_image_id_form.rs"]
mod fpga_image_id_form;
// The buckets the rules-per-security-group quota is counted in — one per group, direction
// and address family — and what a filled bucket refuses. Declared beside `prefix_lists` for
#[path = "all/security_group_rule_quota_buckets.rs"]
mod security_group_rule_quota_buckets;
// What `AcceptReservedInstancesExchangeQuote` exchanges: which Convertible Reserved Instances
// it retires, the purchases of the target offerings it mints in their place, and what it
#[path = "all/reserved_instances_exchanges.rs"]
mod reserved_instances_exchanges;
// What `ModifyCapacityReservationFleet` changes about a stored Capacity Reservation Fleet,
// what a describe then reports of it, and what the modification refuses. Declared beside
#[path = "all/modify_capacity_reservation_fleet.rs"]
mod modify_capacity_reservation_fleet;
// What `RevokeSecurityGroupEgress` removes from a group, which requests it admits and
#[path = "all/revoke_security_group_egress.rs"]
mod revoke_security_group_egress;
// The one sentence every `Invalid<Kind>.Malformed` of this service words an unreadable
// resource id with, across the id kinds and the operations that resolve them. Declared
#[path = "all/malformed_id_message_template.rs"]
mod malformed_id_message_template;
// What `AttachVolume` records on a volume, which requests it admits and refuses, and the
// `attaching` -> `attached` move the clock owes the attachment. Declared beside
#[path = "all/attach_volume.rs"]
mod attach_volume;
// The ancestry chain `GetImageAncestry` walks over the parent link of the stored AMIs, and
#[path = "all/image_ancestry.rs"]
mod image_ancestry;
// What `AssociateIpamResourceDiscovery` links, what the IPAM then counts, and what the
// associate refuses. Declared beside `prefix_lists` for the reason the note above gives: the
#[path = "all/ipam_resource_discovery_associations.rs"]
mod ipam_resource_discovery_associations;
// What `SendDiagnosticInterrupt` delivers to one instance, which states it can be sent
// from, and what it refuses. Declared beside `prefix_lists` for the reason the note above
#[path = "all/send_diagnostic_interrupt.rs"]
mod send_diagnostic_interrupt;
// Which suppression `DisableApplicationStatusCheckSuppression` lifts, what a reader then
// reports for that instance, and what the call refuses. Declared beside `prefix_lists` for
#[path = "all/disable_application_status_check_suppression.rs"]
mod disable_application_status_check_suppression;
// What `CreateLocalGatewayRoute` writes into one local gateway route table, what the clock
// then owes the route, and what the create refuses. Declared beside `prefix_lists` for the
#[path = "all/create_local_gateway_route.rs"]
mod create_local_gateway_route;
// Where `ModifyLocalGatewayRoute` re-points one stored local gateway route, what the clock
// then owes it, and what the modify refuses. Declared beside `prefix_lists` for the
#[path = "all/modify_local_gateway_route.rs"]
mod modify_local_gateway_route;
// The IAM instance profile an instance runs as: what the associate records, what the two
// describes report of it, and what the disassociate takes away. Declared beside
#[path = "all/iam_instance_profile_associations.rs"]
mod iam_instance_profile_associations;
// WHICH instance profile an association is over: the ARN and the id the account's IAM
// records hold it under, on every report of one. Declared beside `prefix_lists` for the
// reason the note above gives: the sorted neighbourhood is where every new ec2 suite
// inserts its two lines.
#[path = "all/iam_instance_profile_identity.rs"]
mod iam_instance_profile_identity;
// What `DeleteVolume` removes, what it refuses, and what a dry run of it performs. Declared
// beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood is where
// every new ec2 suite inserts its two lines.
#[path = "all/delete_volume.rs"]
mod delete_volume;
// The endpoint services a caller can create a VPC endpoint to: the Region's catalogue and the
// account's own service configurations. Declared beside `prefix_lists` for the reason the note
// above gives — the sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/vpc_endpoint_service_name_families.rs"]
mod vpc_endpoint_service_name_families;
#[path = "all/vpc_endpoint_services.rs"]
mod vpc_endpoint_services;
// The `domain-name` attribute of an Elastic IP address: what `ResetAddressAttribute` starts
// on one address's reverse DNS record, what it refuses, and what the report reads back.
#[path = "all/address_attribute_reset.rs"]
mod address_attribute_reset;
// What `AllocateHosts` writes: how many hosts one call mints, the configuration each of them
// carries, and that an earlier call's hosts are left as stored. Declared here rather than in
// sorted position, for the reason the note above gives — the sorted neighbourhood is the
// contended region of this file.
#[path = "all/allocate_hosts.rs"]
mod allocate_hosts;
// What a delete does to the state behind it: the dependency refusals and the reads of a
// resource that is gone. Declared here rather than in sorted position, for the reason the
// note above gives — the sorted neighbourhood is the contended region of this file.
#[path = "all/deleted_resources_and_dependencies.rs"]
mod deleted_resources_and_dependencies;
// The pre-conditions a mutating call is held to — a referenced resource has to exist and be
// compatible with the others named, and a resource something still occupies cannot be deleted
// — and what a delete leaves readable. Declared beside the suite above rather than in sorted
// position, for the reason the note above gives: the sorted neighbourhood and the tail are
#[path = "all/dependency_and_not_found_refusals.rs"]
mod dependency_and_not_found_refusals;
// What `CreateVpcEndpoint` stores for each endpoint type, and what it refuses. Declared here
// rather than in sorted position, for the reason the note above gives: the sorted
// neighbourhood is where every new ec2 suite inserts its lines.
#[path = "all/create_vpc_endpoint.rs"]
mod create_vpc_endpoint;
// What `CreateVpcEndpointServiceConfiguration` stores for an endpoint service the account offers
// — the load balancers behind it, the acceptance, address types, Regions, private DNS name and
// tags it was configured with, and the name, base DNS name and verification record derived from
// the `ServiceId` it mints — what it refuses, and what `DescribeVpcEndpointServiceConfigurations`
// and `DescribeVpcEndpointServices` then report out of that store. Declared beside the endpoint
// suite above rather than at the tail or in sorted position, for the reason the header note
// gives: both of those are where every new ec2 suite inserts its lines.
#[path = "all/create_vpc_endpoint_service_configuration.rs"]
mod create_vpc_endpoint_service_configuration;
// Associating a virtual interface group with a local gateway route table, and reading the
// association back. Declared beside `prefix_lists` for the reason the note above gives: the
// sorted region AND the tail of this file are where every new ec2 suite inserts its lines,
// and these two lines lost several serialized squash merges to the tail.
#[path = "all/create_local_gateway_route_table_virtual_interface_group_association.rs"]
mod create_local_gateway_route_table_virtual_interface_group_association;
// What `CreateClientVpnRoute` adds to one Client VPN endpoint's route table, what it refuses,
// and what `DescribeClientVpnRoutes` reports back. Declared BELOW the two suites above rather
// than straight after `prefix_lists`: that first position is where every new ec2 suite
// inserts its lines, and these two lost three serialized squash merges to exactly that.
#[path = "all/client_vpn_routes.rs"]
mod client_vpn_routes;
// The field vocabulary a `CreateFlowLogs` `LogFormat` draws from: a format naming a field a
// flow log record does not have is refused and stores nothing. Declared beside the suites
// above rather than in sorted position, for the reason this file's header note gives: the
// sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/flow_log_format_fields.rs"]
mod flow_log_format_fields;
// The message an authorize is refused with once a security group holds as many rules in one
// direction as the account's quota allows. Declared beside the suites above for the same
// reason.
#[path = "all/security_group_rule_quota_refusal.rs"]
mod security_group_rule_quota_refusal;
// What `ModifyReservedInstances` records for the Reserved Instances it names, what the
// clock's fulfilment of it replaces them with, and what it refuses. Declared beside the
// suites above rather than in sorted position, for the reason this file's header note
// gives: the sorted neighbourhood is where every new ec2 suite inserts its two lines.
#[path = "all/reserved_instances_modifications.rs"]
mod reserved_instances_modifications;
// The Reserved Instance Marketplace listings of those purchases: what
// `CreateReservedInstancesListing` puts up for sale, what `CancelReservedInstancesListing`
// withdraws, what `DescribeReservedInstancesListings` reports back and narrows by, and what
// each refuses. Declared beside the modifications suite it shares a subject with, rather
// than in sorted position, for the reason this file's header note gives.
#[path = "all/reserved_instances_listings.rs"]
mod reserved_instances_listings;
// Where a paged describe bounds its page size and where it does not: a `MaxResults` the
// model leaves unbounded above is served, not refused. Declared beside the suites above
// rather than in sorted position, for the reason this file's header note gives.
#[path = "all/unranged_max_results.rs"]
mod unranged_max_results;
// What `RunInstances` does with a `TagSpecifications` entry, per resource type. Declared
// beside the suites above for the same reason.
#[path = "all/run_instances_tag_specifications.rs"]
mod run_instances_tag_specifications;
// What `RunInstances` does with an `IamInstanceProfile`, and the sentence the association
// describe answers for a filter name it does not document. Declared beside the launch suite
// above rather than in sorted position, for the reason this file's header note gives.
#[path = "all/run_instances_iam_instance_profile.rs"]
mod run_instances_iam_instance_profile;
// What a LAUNCH does with an `ImageId` that names a Systems Manager parameter instead of an
// AMI, and the EBS attachment lifecycle such a launch feeds: attach, the describe that reports
// it, and the detach. Declared beside the two launch suites above rather than at the end of
// this file: the end is where every new ec2 suite now appends, so a line added there loses
// serialized squash merges.
#[path = "all/launch_from_ssm_image_alias.rs"]
mod launch_from_ssm_image_alias;
// The virtual private gateway slice: create, read back, delete. Declared here rather than in
// sorted position for the reason given above — the sorted neighbourhood is the contended
// region of this file, and this suite lost several serialized squash merges to it.
#[path = "all/vpn_gateways.rs"]
mod vpn_gateways;
// What `DescribeClientVpnTargetNetworks` reports for the target networks associated with one
// Client VPN endpoint, and how the ids, the filters and the page size narrow it. Declared
// here rather than in sorted position, for the reason the note above gives.
#[path = "all/client_vpn_target_networks.rs"]
mod client_vpn_target_networks;
// What `ResetInstanceAttribute` resets, to what, from which instance states, and what it
// refuses. Declared beside `prefix_lists` for the same reason: the sorted region is where
#[path = "all/instance_attribute_reset.rs"]
mod instance_attribute_reset;
// The root volume replacement slice: create a task, then read it back. Declared beside the
// two above for the same reason — the sorted region is where every new ec2 suite inserts
// its lines, and this fix lost three serialized squash merges to exactly that.
#[path = "all/replace_root_volume_tasks.rs"]
mod replace_root_volume_tasks;
// What `ReportInstanceStatus` collects, which instances it accepts feedback about, and what
// it refuses. Declared beside the suites above rather than in sorted position, for the reason
// this file's header note gives: the sorted neighbourhood is where every new ec2 suite
// inserts its two lines, and that is where a serialized squash merge is lost.
#[path = "all/report_instance_status.rs"]
mod report_instance_status;
// Declared here rather than in sorted position, for the reason above: the sorted
// neighbourhood of this suite is the contended region of this file.
#[path = "all/local_gateway_routes.rs"]
mod local_gateway_routes;
// The Reachability Analyzer path analysis slice: what `StartNetworkInsightsAnalysis` stores
// for one path, what the clock then does to it, and how `DescribeNetworkInsightsAnalyses`
// narrows, pages and refuses. Declared here rather than in sorted position, for the reason
// this file's header note gives: the sorted neighbourhood is the contended region.
#[path = "all/network_insights_analyses.rs"]
mod network_insights_analyses;
// What `DeleteNetworkInsightsPath` takes out of the path store, what it answers, what it
// refuses and what a dry run of it performs. Declared beside the analyses suite above —
// topically its neighbour — rather than in sorted position or at the tail, for the reason
// this file's header note gives: both are the contended regions where a serialized squash
// merge is lost.
#[path = "all/delete_network_insights_path.rs"]
mod delete_network_insights_path;
// What `DescribeNetworkInsightsPaths` reports out of the path store, how the id list, the
// documented filters and the page size narrow it, and what it refuses. Declared beside the
// two path suites above — topically its neighbours — rather than in sorted position or at the
// tail, for the reason this file's header note gives: both are contended regions where a
// serialized squash merge is lost.
#[path = "all/describe_network_insights_paths.rs"]
mod describe_network_insights_paths;
// What `CreateNetworkInsightsPath` stores for one Reachability Analyzer path, what it answers,
// which parameter pairings it refuses and what a dry run of it performs. Declared beside the
// three path suites above — topically its neighbours — rather than in sorted position or at
// the tail, for the reason this file's header note gives: both are contended regions where a
#[path = "all/create_network_insights_path.rs"]
mod create_network_insights_path;
// The transit gateway prefix list reference slice (create, then read it back) and the
// error envelope's request id. Declared beside the suites above for the reason they
// give: the sorted region is where every new ec2 suite inserts its lines, and these two
// lines have already lost several serialized squash merges there.
#[path = "all/transit_gateway_prefix_list_references.rs"]
mod transit_gateway_prefix_list_references;
// What `DeleteTransitGatewayPrefixListReference` takes out of a transit gateway route table:
// the one route the request names, answered in `deleting` with the clock owing its removal,
// and the ids, unrouted pairs and dry run it refuses without writing anything. Declared beside
// the create-and-read suite above, its topical neighbour, for the reason those lines give.
#[path = "all/delete_transit_gateway_prefix_list_reference.rs"]
mod delete_transit_gateway_prefix_list_reference;
// What `DescribeTransitGatewayRouteTableAnnouncements` reports out of the announcement store,
// and how the id list, the filters and the page size narrow it. Declared beside the transit
// gateway suite above rather than in sorted position or at the tail, for the reason this
// file's header note gives: BOTH the sorted neighbourhood and the tail are where every new
// ec2 suite inserts its two lines, and these two lines lost four serialized squash merges at
// the tail.
#[path = "all/transit_gateway_route_table_announcements.rs"]
mod transit_gateway_route_table_announcements;
// What `CreateTransitGatewayRouteTableAnnouncement` advertises, and what it refuses.
#[path = "all/create_transit_gateway_route_table_announcement.rs"]
mod create_transit_gateway_route_table_announcement;
// What `RebootInstances` queues, which instances it can be queued for, and what it
// refuses. Declared beside the suites above rather than in sorted position or at the tail,
// for the reason this file's header note gives: both are contended regions where a
#[path = "all/reboot_instances.rs"]
mod reboot_instances;
// The modelled members of the reference read paths: a Region's published geography, the
// members an instance type's name yields, the addressing a created application status check
// reports, what a published prefix list carries, and selecting an endpoint service by name.
#[path = "all/read_path_modeled_members.rs"]
mod read_path_modeled_members;
#[path = "all/ec2_query_error_envelope.rs"]
mod ec2_query_error_envelope;
// The same envelope rule, driven over seven further operations and the four refusal kinds
// they answer. Declared beside the suite that owns the rule rather than in sorted position,
// for the reason this file's header note gives: the sorted neighbourhood is where every new
// ec2 suite inserts its two lines.
#[path = "all/ec2_query_error_envelope_operations.rs"]
mod ec2_query_error_envelope_operations;
// What `CreatePublicIpv4Pool` mints and persists, read back through the describe. Declared
// here rather than in sorted position, for the reason the suites above give: the sorted
// neighbourhood is where every new ec2 suite inserts its lines, and these two lines have
// already lost several serialized squash merges there.
#[path = "all/create_public_ipv4_pool.rs"]
mod create_public_ipv4_pool;
#[path = "all/address_transfers.rs"]
mod address_transfers;
// The account's VPC Encryption Control configuration: the mode and the per-resource
// exclusions `ModifyAccountVpcEncryptionControl` records, and what
// `DescribeAccountVpcEncryptionControl` reports back out of that one store — including the
// transitional states the moves it starts pass through.
#[path = "all/account_vpc_encryption_control.rs"]
mod account_vpc_encryption_control;
#[path = "all/allowed_images_settings.rs"]
mod allowed_images_settings;
#[path = "all/application_status.rs"]
mod application_status;
#[path = "all/application_status_checks.rs"]
mod application_status_checks;
// What `CreateApplicationStatusCheck` stores, the documented defaults it fills in, the
// published bounds it refuses past, and the check it answers read back through the describe.
#[path = "all/create_application_status_check.rs"]
mod create_application_status_check;
// What `AssociateApplicationStatusCheck` associates one check with — tag pairs on the check
// itself and the monitoring that begins for each named instance — read back through
// `DescribeApplicationStatusChecks` and `DescribeApplicationStatus`.
#[path = "all/associate_application_status_check.rs"]
mod associate_application_status_check;
// What `DeleteApplicationStatusCheck` does to the check it names: its deletion instant, the
// associations it takes with it, the quota slot it gives back and the bounded grace period the
// deleted record spends still readable.
#[path = "all/delete_application_status_check.rs"]
mod delete_application_status_check;
// What `AssignIpv6Addresses` takes out of the IPv6 block of the subnet a network interface
// sits in, and records on that interface — named addresses and prefixes against a count of
// each, read back through `DescribeNetworkInterfaces`.
#[path = "all/assign_ipv6_addresses.rs"]
mod assign_ipv6_addresses;
#[path = "all/associate_address.rs"]
mod associate_address;
#[path = "all/associate_client_vpn_target_network.rs"]
mod associate_client_vpn_target_network;
#[path = "all/associate_subnet_cidr_block.rs"]
mod associate_subnet_cidr_block;
// What `AssociateTransitGatewayMulticastDomain` persists: one association per subnet the
// request names, through an `available` attachment, in the transient state the clock carries
// to `associated` — and the refusal each resource it cannot resolve gets.
#[path = "all/associate_transit_gateway_multicast_domain.rs"]
mod associate_transit_gateway_multicast_domain;
#[path = "all/authorize_security_group_egress.rs"]
mod authorize_security_group_egress;
#[path = "all/availability_zones.rs"]
mod availability_zones;
#[path = "all/byoip_cidr_lifecycle.rs"]
mod byoip_cidr_lifecycle;
#[path = "all/byoip_cidrs.rs"]
mod byoip_cidrs;
#[path = "all/capacity_block_offerings.rs"]
mod capacity_block_offerings;
// The Capacity Block extension offered for one stored Capacity Block reservation: the window
// that continues it, the capacity and tenancy it already holds, and what the search refuses.
#[path = "all/capacity_block_extension_offerings.rs"]
mod capacity_block_extension_offerings;
// The availability of the capacity of the account's Capacity Blocks, summed over the
// Capacity Reservations each block holds its capacity as.
#[path = "all/capacity_block_status.rs"]
mod capacity_block_status;
#[path = "all/capacity_blocks.rs"]
mod capacity_blocks;
// The account's EC2 Capacity Manager settings: what `EnableCapacityManager` records and
// what `GetCapacityManagerAttributes` reports back, including the ingestion the enable
// starts.
#[path = "all/capacity_manager.rs"]
mod capacity_manager;
// What `DisableCapacityManager` records: the account's status and Organizations access
// going off, the ingestion it stops, and what an enable after one starts.
#[path = "all/capacity_manager_disable.rs"]
mod capacity_manager_disable;
// What `GetCapacityManagerAttributes` reports about those settings: every member of the
// result, over the account histories behind it and the Regions it is asked for in.
#[path = "all/capacity_manager_attributes.rs"]
mod capacity_manager_attributes;
// What `AcceptCapacityReservationBillingOwnership` makes of the billing assignment request
// this account was sent for a shared Capacity Reservation, and what it refuses. Declared in
// the MIDDLE of this list rather than at either end: the head and the tail are both where
// concurrent ec2 landings insert their lines.
#[path = "all/accept_capacity_reservation_billing_ownership.rs"]
mod accept_capacity_reservation_billing_ownership;
#[path = "all/capacity_reservation_billing_requests.rs"]
mod capacity_reservation_billing_requests;
// What `CreateCapacityReservationCancellationQuote` quotes for cancelling a future-dated
// Capacity Reservation inside its commitment, and what
// `DescribeCapacityReservationCancellationQuotes` reports back out of that store.
#[path = "all/capacity_reservation_cancellation_quotes.rs"]
mod capacity_reservation_cancellation_quotes;
#[path = "all/capacity_reservation_fleets.rs"]
mod capacity_reservation_fleets;
#[path = "all/carrier_gateways.rs"]
mod carrier_gateways;
// What `DescribeClassicLinkInstances` reports out of the ClassicLink link store, and how the
// instance ids, the documented filters and the page size narrow it. Declared in the MIDDLE
// of the list rather than at either end: the head and the tail are both where concurrent ec2
// landings insert their two lines, and these two lost repeated serialized squash merges at
#[path = "all/classic_link_instances.rs"]
mod classic_link_instances;
// What `DeleteVpnConnection` starts on the Site-to-Site VPN connection it names, what it
// refuses, and what the clock then owes it. Declared in the MIDDLE of the list for the
// reason the suite above gives: the head and the tail are both where concurrent ec2
// landings insert their two lines, and these two already lost a serialized squash merge at
#[path = "all/delete_vpn_connection.rs"]
mod delete_vpn_connection;
// What `CreateSnapshots` makes of the volumes one instance holds — one snapshot each, the
// two exclusions, the tags, and the refusals. Declared in the MIDDLE of the list for the
// reason the suite above gives: the head and the tail are both where concurrent ec2 landings
// insert their two lines, and these two lost three serialized squash merges at the tail.
#[path = "all/create_snapshots.rs"]
mod create_snapshots;
// What `CreateSnapshot` makes of one volume, and the refusals that store nothing. Declared
// here for the reason the suite above gives: the head and the tail of this list are where
#[path = "all/create_snapshot.rs"]
mod create_snapshot;
// What `DescribeSnapshots` reports of the snapshots the account holds — the members, the
// narrowings, the paging, the refusals and the clock-owed completion. Declared in the MIDDLE
// of the list for the reason the suite above gives: the head and the tail are both where
// concurrent ec2 landings insert their two lines.
#[path = "all/describe_snapshots.rs"]
mod describe_snapshots;
// What `DescribeSnapshots` reports of the PUBLIC half of that inventory — the snapshots the
// published AMI catalogue restores its root devices from — and the full size, transfer type,
// page token and parameter-layer id refusal every snapshot of it carries. Declared in the
// MIDDLE of the list for the reason the suite above gives.
#[path = "all/describe_snapshots_public_catalogue.rs"]
mod describe_snapshots_public_catalogue;
// What a DELETE of a snapshot leaves of the fast snapshot restores enabled on it — the pairs
// live in THIS service's store for a snapshot of either side of the one snapshot resource, so
// the rule is one rule. Declared in the MIDDLE of the list for the reason the suites above
// give: the head and the tail are both where concurrent ec2 landings insert their two lines.
#[path = "all/delete_snapshot_fast_snapshot_restores.rs"]
mod delete_snapshot_fast_snapshot_restores;
// Which other VPCs `DescribeSecurityGroupReferences` reports as referencing one of the
// account's security groups, and what it refuses. Declared in the MIDDLE of the list for the
// reason the suites above give: the head and the tail are both where concurrent ec2 landings
// insert their two lines, and these two lost two serialized squash merges at the head.
#[path = "all/security_group_references.rs"]
mod security_group_references;
// What `DeleteCoipCidr` takes out of one customer-owned address pool's ranges, what it
// reports back, and the requests that resolve to no range at all. Declared HERE, beside the
// `coip_pools` fixtures its rows are written against, rather than in the `delete_*` block
// below: that block invites every concurrent `delete_*` suite to insert its two lines in the
// same few lines of context, and this one lost repeated serialized squash merges there.
#[path = "all/delete_coip_cidr.rs"]
mod delete_coip_cidr;
#[path = "all/coip_pools.rs"]
mod coip_pools;
#[path = "all/confirm_product_instance.rs"]
mod confirm_product_instance;
#[path = "all/copy_volumes.rs"]
mod copy_volumes;
// What `DescribeVpcEndpointAssociations` reports out of the VPC endpoint association store,
// and how the endpoint ids, the documented filters and the page size narrow it. Declared in
// the MIDDLE of the list for the reason the two suites above give: the head and the tail are
// both where concurrent ec2 landings insert their two lines, and these two lost two
// serialized squash merges at the head.
#[path = "all/vpc_endpoint_associations.rs"]
mod vpc_endpoint_associations;
// What `CreateDhcpOptions` stores: the options a request names, the ones it refuses, and
// the set a caller created being one `AssociateDhcpOptions` resolves.
#[path = "all/create_dhcp_options.rs"]
mod create_dhcp_options;
#[path = "all/create_security_group.rs"]
mod create_security_group;
#[path = "all/dedicated_hosts.rs"]
mod dedicated_hosts;
// The Dedicated Host RESERVATION plane: what `PurchaseHostReservation` buys and stores, what
// `DescribeHostReservations` reports, selects and pages, and what each of them refuses.
#[path = "all/host_reservations.rs"]
mod host_reservations;
#[path = "all/default_network.rs"]
mod default_network;
// The account's default credit option for CPU usage of a burstable instance family, per
// Region and per family: `ModifyDefaultCreditSpecification` and
// `GetDefaultCreditSpecification`.
#[path = "all/default_credit_specification.rs"]
mod default_credit_specification;
#[path = "all/default_network_per_region.rs"]
mod default_network_per_region;
// What `DeleteCarrierGateway` removes from the carrier gateway store, what it reports back,
// what it refuses and what it releases. Declared beside the other `delete_*` suites rather
// than at the tail of this file: the tail is where every concurrent ec2 landing appends its
// two lines, and these two lost repeated serialized squash merges there.
#[path = "all/delete_carrier_gateway.rs"]
mod delete_carrier_gateway;
#[path = "all/delete_coip_pool.rs"]
mod delete_coip_pool;
// What `DeleteNetworkAcl` takes out of the store: the one ACL its id names, the blockers the
// operation documents, and the store a refused or dry-run delete leaves untouched.
#[path = "all/delete_network_acl.rs"]
mod delete_network_acl;
#[path = "all/delete_network_interface.rs"]
mod delete_network_interface;
// What `DeleteRoute` takes out of a route table: the ONE route the destination it names
// matches, the table that route is in, and the refusals for what names no route of a table.
#[path = "all/delete_route.rs"]
mod delete_route;
#[path = "all/delete_security_group.rs"]
mod delete_security_group;
#[path = "all/delete_vpc.rs"]
mod delete_vpc;
#[path = "all/describe_images.rs"]
mod describe_images;
// What `DescribeLaunchTemplates` reports out of the launch template store: the templates a
// create stored, the narrowing its ids, names, filters and pages do, and the refusal an id
// the account holds no template under gets. Declared HERE, in sorted position inside the
// long-settled block rather than at the head of the file, where every new ec2 suite inserts
// its lines and merges contend.
#[path = "all/describe_launch_templates.rs"]
mod describe_launch_templates;
// What `DescribeNetworkAcls` reports out of the network ACL store: the default ACL every VPC
// comes with, the subnets associated with it, and the narrowing its ids, filters and pages
// do. Declared HERE, in sorted position inside the long-settled block rather than beside
// `prefix_lists`: the head of this file is where every new ec2 suite inserts its lines, and
// this fix lost two serialized squash merges to exactly that contention.
#[path = "all/describe_network_acls.rs"]
mod describe_network_acls;
#[path = "all/describe_page_truncation.rs"]
mod describe_page_truncation;
#[path = "all/describe_security_groups.rs"]
mod describe_security_groups;
#[path = "all/describe_subnets.rs"]
mod describe_subnets;
#[path = "all/describe_vpc_attribute.rs"]
mod describe_vpc_attribute;
#[path = "all/describe_vpcs.rs"]
mod describe_vpcs;
// What `DetachNetworkInterface` takes down and what it refuses: the `detaching` state and the
// completion the clock owes it, the malformed/not-found split on an `AttachmentId`, and the
// attachments that cannot be detached at all. Declared HERE, in sorted position inside the
#[path = "all/detach_network_interface.rs"]
mod detach_network_interface;
// What `DetachVolume` refuses and what it writes beyond the detach path in
// `fpga_image_tasks_and_volume_io`: the `IncorrectState` wording, the owed `detached`
// instant, and an attachment to an Amazon Web Services-managed resource.
#[path = "all/detach_volume.rs"]
mod detach_volume;
// The DHCP option set a VPC inherits: the Region's one default set, and what
// `DescribeDhcpOptions` reports for it.
#[path = "all/dhcp_options.rs"]
mod dhcp_options;
#[path = "all/dhcp_options_association.rs"]
mod dhcp_options_association;
#[path = "all/dry_run_and_unknown_ids.rs"]
mod dry_run_and_unknown_ids;
// The account-level EBS encryption-by-default setting and the CreateVolume default it
// decides.
#[path = "all/ebs_encryption_by_default.rs"]
mod ebs_encryption_by_default;
// The shared ec2Query plumbing every suite drives the service through.
#[path = "all/ec2_query.rs"]
mod ec2_query;
// The ec2Query response envelope: the success flag and the media type every response
// carries, whichever operation answered.
#[path = "all/ec2_query_envelope.rs"]
mod ec2_query_envelope;
// Which request-id element each side of one operation's envelope reports: the pair on
// `CreateVpnConnectionRoute`, on top of the across-operations rule
// `ec2_query_error_envelope` owns.
#[path = "all/ec2_query_error_request_id.rs"]
mod ec2_query_error_request_id;
// Why `DescribeElasticGpus` serves no report at all: the Elastic Graphics accelerator it
// reports on reached end of life, so EC2 refuses the call for the absent functionality.
#[path = "all/elastic_gpus.rs"]
mod elastic_gpus;
#[path = "all/elastic_ip_addresses.rs"]
mod elastic_ip_addresses;
// What `DescribeExportImageTasks` reports out of the export image task store `ExportImage`
// writes, how the clock and a cancellation move a task through the published statuses, and how
// the ids, the filters and the pages narrow the report.
#[path = "all/export_image_tasks.rs"]
mod export_image_tasks;
// What `DescribeExportTasks` reports out of the export task store, and how `ExportTaskIds`
// and the filters narrow it.
#[path = "all/export_tasks.rs"]
mod export_tasks;
#[path = "all/fast_launch_images.rs"]
mod fast_launch_images;
// What `DisableFastSnapshotRestores` answers per (snapshot, Availability Zone) pair.
#[path = "all/fast_snapshot_restores.rs"]
mod fast_snapshot_restores;
#[path = "all/filter_matcher_budget.rs"]
mod filter_matcher_budget;
#[path = "all/fleet_instances.rs"]
mod fleet_instances;
#[path = "all/fpga_image_tasks_and_volume_io.rs"]
mod fpga_image_tasks_and_volume_io;
// What `CreateRestoreImageTask` refuses — a bucket name that reaches no bucket, read as a
// request parameter, and the stored AMI object this emulator holds none of — and what it
// answers ahead of that read. Declared HERE, beside the suite it took those tests from,
// rather than at the tail, for the reason the header note gives: the tail is where every
// concurrent ec2 landing inserts its two lines, and these two have already lost two
// serialized squash merges there.
#[path = "all/create_restore_image_task.rs"]
mod create_restore_image_task;
#[path = "all/get_password_data.rs"]
mod get_password_data;
// The security groups a network interface in one VPC can be given: the groups created in it,
// the groups another VPC's association makes usable there, the filters that narrow the report
// and the refusals.
#[path = "all/get_security_groups_for_vpc.rs"]
mod get_security_groups_for_vpc;
// The JPG-format console screenshot `GetConsoleScreenshot` reports for one instance, what it
// refuses, and what `DryRun` and `WakeUp` do to it. Declared beside the suite above rather
// than in sorted position, for the reason this file's header note gives: the sorted
// neighbourhood and the tail of this list are where every new ec2 suite inserts its lines.
#[path = "all/get_console_screenshot.rs"]
mod get_console_screenshot;
// Which ONE attribute of a stored AMI `DescribeImageAttribute` reports, and what it
// refuses. Declared HERE rather than in sorted position or at the tail, for the reason the
// header note gives: both the sorted `image_*` neighbourhood and the tail of this list are
// where concurrent ec2 landings insert their two lines, and these two have already lost
// several serialized squash merges there.
#[path = "all/image_attributes.rs"]
mod image_attributes;
#[path = "all/host_reservation_offerings.rs"]
mod host_reservation_offerings;
#[path = "all/image_references.rs"]
mod image_references;
#[path = "all/image_usage_reports.rs"]
mod image_usage_reports;
#[path = "all/images.rs"]
mod images;
#[path = "all/import_image_tasks.rs"]
mod import_image_tasks;
#[path = "all/import_snapshot_tasks.rs"]
mod import_snapshot_tasks;
// Importing a disk into an EBS snapshot: what `ImportSnapshot` starts, and the snapshot the
// clock makes from it.
#[path = "all/snapshot_imports.rs"]
mod snapshot_imports;
// Cancelling an import task of either kind: what `CancelImportTask` moves and what it
// refuses.
#[path = "all/import_task_cancellation.rs"]
mod import_task_cancellation;
#[path = "all/instance_connect_endpoints.rs"]
mod instance_connect_endpoints;
// The fixed detail every instance reports, and the monitoring state a caller moves.
#[path = "all/instance_detail.rs"]
mod instance_detail;
#[path = "all/instance_event_notification_attributes.rs"]
mod instance_event_notification_attributes;
#[path = "all/instance_event_window_associations.rs"]
mod instance_event_window_associations;
#[path = "all/instance_event_windows.rs"]
mod instance_event_windows;
// Every start of an instance's guest — its launch, a start of a stopped instance and a
// queued reboot taking effect — as a BOOT of its own, appended to the console it writes.
#[path = "all/instance_guest_boots.rs"]
mod instance_guest_boots;
#[path = "all/instance_lifecycle.rs"]
mod instance_lifecycle;
#[path = "all/instance_metadata_defaults.rs"]
mod instance_metadata_defaults;
// The SQL Server High Availability state every enabled instance is reported in.
#[path = "all/instance_sql_ha_states.rs"]
mod instance_sql_ha_states;
// Every SQL Server High Availability state an enabled instance HAS HELD, over the period the
// request asks about.
#[path = "all/instance_sql_ha_history_states.rs"]
mod instance_sql_ha_history_states;
#[path = "all/instance_type_offerings.rs"]
mod instance_type_offerings;
#[path = "all/instance_types.rs"]
mod instance_types;
// Associating an ASN brought to IPAM with a BYOIP address range IPAM manages.
#[path = "all/ipam_byoasn_associations.rs"]
mod ipam_byoasn_associations;
// Allocating unused capacity out of a Capacity Reservation into an interruptible one.
#[path = "all/interruptible_capacity_reservation_allocations.rs"]
mod interruptible_capacity_reservation_allocations;
#[path = "all/ipam_lifecycle.rs"]
mod ipam_lifecycle;
// What an IPAM resource discovery samples as a Region's public addresses: which Elastic IP
// addresses of the account are in the sample, what the resource holding one contributes to it,
// how `Filters` narrow it and how `MaxResults` pages it.
#[path = "all/ipam_discovered_public_addresses.rs"]
mod ipam_discovered_public_addresses;
// Which Amazon Web Services accounts an IPAM resource discovery reports as monitored in a
// Region, what each discovered account carries, how `Filters` narrow the report and how
// `MaxResults` pages it.
#[path = "all/ipam_discovered_accounts.rs"]
mod ipam_discovered_accounts;
// Which BGP routes an IPAM resource discovery reports for a Region: one per announcement of the
// account's own BYOIP address ranges advertised from a border group of that Region, what each
// discovered route carries, how `Filters` narrow the report and how `MaxResults` pages it.
#[path = "all/ipam_discovered_routes.rs"]
mod ipam_discovered_routes;
// Creating the association between an IPAM and a Regional Internet Registry.
#[path = "all/ipam_internet_registry_association_creation.rs"]
mod ipam_internet_registry_association_creation;
// Deleting the association between an IPAM and a Regional Internet Registry.
#[path = "all/ipam_internet_registry_associations.rs"]
mod ipam_internet_registry_associations;
// Reporting the associations between an IPAM and a Regional Internet Registry.
#[path = "all/ipam_internet_registry_association_reports.rs"]
mod ipam_internet_registry_association_reports;
#[path = "all/ipam_policies.rs"]
mod ipam_policies;
#[path = "all/ipam_pools.rs"]
mod ipam_pools;
// Deleting an IPAM pool, what it refuses while the pool still holds resources, and what
// `Cascade` takes with it.
#[path = "all/ipam_pool_deletion.rs"]
mod ipam_pool_deletion;
// Creating one further IPAM scope in an IPAM, and reading the account's scopes back.
#[path = "all/ipam_scopes.rs"]
mod ipam_scopes;
// Creating an IPAM prefix list resolver and reading the account's resolvers back.
#[path = "all/ipam_prefix_list_resolvers.rs"]
mod ipam_prefix_list_resolvers;
// Associating a customer-managed prefix list with an IPAM prefix list resolver, and reading
// the account's resolver targets back.
#[path = "all/ipam_prefix_list_resolver_targets.rs"]
mod ipam_prefix_list_resolver_targets;
#[path = "all/ipam_routing_policy_registrations.rs"]
mod ipam_routing_policy_registrations;
// The RPKI validation `GetIpamRouteProtectionFindings` reports for the BYOIP routes an IPAM
// manages.
#[path = "all/ipam_route_protection_findings.rs"]
mod ipam_route_protection_findings;
// What `ModifyHosts` applies to the Dedicated Hosts it names, what it leaves as it was, which
// hosts it reports unsuccessful, and what the request itself is refused for. Declared here
// rather than in sorted position for the reason the header gives: the `modify_*` neighbourhood
// and the tail of this file are where other suites insert their lines, and this declaration
// lost several serialized squash merges to exactly that.
#[path = "all/modify_hosts.rs"]
mod modify_hosts;
#[path = "all/launch_templates.rs"]
mod launch_templates;
#[path = "all/local_gateway_route_table_virtual_interface_group_associations.rs"]
mod local_gateway_route_table_virtual_interface_group_associations;
#[path = "all/local_gateway_route_tables.rs"]
mod local_gateway_route_tables;
#[path = "all/local_gateway_virtual_interface_groups.rs"]
mod local_gateway_virtual_interface_groups;
#[path = "all/local_gateway_virtual_interfaces.rs"]
mod local_gateway_virtual_interfaces;
#[path = "all/service_link_virtual_interfaces.rs"]
mod service_link_virtual_interfaces;
#[path = "all/outpost_lags.rs"]
mod outpost_lags;
#[path = "all/mac_dedicated_hosts.rs"]
mod mac_dedicated_hosts;
#[path = "all/mac_modification_tasks.rs"]
mod mac_modification_tasks;
// The order in which an operation reads a caller-supplied identifier or format-constrained
// parameter — its form first, the store second — read across the cancel of an export task, the
// deletes of an AFI, an AMI usage report and a secondary network, and the restore task's bucket
// name. Declared here, in sorted position inside the long-settled block.
#[path = "all/malformed_before_lookup.rs"]
mod malformed_before_lookup;
#[path = "all/modify_route_server.rs"]
mod modify_route_server;
#[path = "all/modify_subnet_attribute.rs"]
mod modify_subnet_attribute;
#[path = "all/nat_gateway_address_settling.rs"]
mod nat_gateway_address_settling;
#[path = "all/nat_gateway_addresses.rs"]
mod nat_gateway_addresses;
#[path = "all/nat_gateway_lifecycle.rs"]
mod nat_gateway_lifecycle;
#[path = "all/network_insights_access_scope_analyses.rs"]
mod network_insights_access_scope_analyses;
#[path = "all/network_insights_access_scopes.rs"]
mod network_insights_access_scopes;
#[path = "all/network_interface_attributes.rs"]
mod network_interface_attributes;
#[path = "all/network_interfaces.rs"]
mod network_interfaces;
#[path = "all/network_interface_permissions.rs"]
mod network_interface_permissions;
#[path = "all/network_interface_permission_grants.rs"]
mod network_interface_permission_grants;
#[path = "all/network_performance_data.rs"]
mod network_performance_data;
#[path = "all/network_performance_subscriptions.rs"]
mod network_performance_subscriptions;
#[path = "all/placement_groups.rs"]
mod placement_groups;
#[path = "all/principal_id_format.rs"]
mod principal_id_format;
// The Region-wide audit of the same longer-ID settings `principal_id_format` writes and
// reports per principal. Declared beside its sibling suite rather than in sorted position,
// for the reason the note at the head of this file gives: the sorted neighbourhood and the
// head of the list are where every new ec2 suite inserts its two lines.
#[path = "all/aggregate_id_format.rs"]
mod aggregate_id_format;
// What `CreateRouteServerEndpoint` resolves before it writes, what it replays for a repeated
// idempotency token, and what a dry run of it performs. Declared here rather than in sorted
// position, for the reason the note at the head of this file gives: the sorted neighbourhood
// and the head of the list are where every new ec2 suite inserts its two lines.
#[path = "all/create_route_server_endpoint.rs"]
mod create_route_server_endpoint;
// The public IPv4 pool slice: create a pool, then report it back out of the store.
#[path = "all/public_ipv4_pools.rs"]
mod public_ipv4_pools;
// What `PurchaseCapacityBlock` buys out of a Capacity Block offering, and the pair it stores:
// the block, and the Capacity Reservation that holds that block's capacity.
#[path = "all/purchase_capacity_block.rs"]
mod purchase_capacity_block;
// What `DeleteQueuedReservedInstances` deletes and what it reports per id.
#[path = "all/queued_reserved_instances.rs"]
mod queued_reserved_instances;
// What `DescribeTransitGatewayMulticastDomains` reports out of the domain store, and how the
// id list, the documented filters and the page size narrow it. Declared HERE rather than in
// sorted position, for the reason this file's header note and the relocations at its head and
// tail record: a suite's own sorted neighbourhood is where concurrent ec2 landings insert
// their two lines, and the `transit_gateway_*` cluster is the busiest of them.
#[path = "all/transit_gateway_multicast_domains.rs"]
mod transit_gateway_multicast_domains;
#[path = "all/regions.rs"]
mod regions;
// Declared in the MIDDLE of this list rather than at either end, for the reason the header
// note gives: the media type every ec2Query response declares, across the operations, the
// statuses and the layers that render one.
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/resource_specific_error_codes.rs"]
mod resource_specific_error_codes;
#[path = "all/revoke_security_group_ingress.rs"]
mod revoke_security_group_ingress;
#[path = "all/route_server_endpoints.rs"]
mod route_server_endpoints;
#[path = "all/route_server_peers.rs"]
mod route_server_peers;
#[path = "all/route_server_routing_database.rs"]
mod route_server_routing_database;
#[path = "all/route_servers.rs"]
mod route_servers;
// What `DescribeScheduledInstanceAvailability` computes a schedule search from — the Scheduled
// Instance capacity the signed Region has set aside, the documented filters that narrow it, the
// recurrence and slot bounds that fix each reported slot and the term's hours, how it pages, and
// what it refuses. Declared in its SORTED position rather than beside `prefix_lists`: the head of
// this file is where every concurrently landing ec2 suite inserts, and this fix lost several
// serialized squash merges to exactly that.
#[path = "all/scheduled_instance_availability.rs"]
mod scheduled_instance_availability;
// What `DescribeScheduledInstances` reports: the Scheduled Instances the signed Region holds,
// the ids, documented filters and slot-start window that narrow them, the schedule each report
// derives from the stored recurrence, how it pages, and what it refuses. Declared in its SORTED
// position, for the reason the suite above gives.
#[path = "all/scheduled_instances.rs"]
mod scheduled_instances;
#[path = "all/search_local_gateway_routes.rs"]
mod search_local_gateway_routes;
#[path = "all/secondary_interfaces.rs"]
mod secondary_interfaces;
#[path = "all/secondary_networks.rs"]
mod secondary_networks;
// What `DeleteSecondarySubnet` does to the secondary subnet the request names: the form of the
// id it reads before any lookup, the secondary interfaces that block the delete, and the
// clock-owed move to `delete-complete`.
#[path = "all/secondary_subnet_deletion.rs"]
mod secondary_subnet_deletion;
#[path = "all/secondary_subnets.rs"]
mod secondary_subnets;
#[path = "all/security_group_filter_limits.rs"]
mod security_group_filter_limits;
#[path = "all/security_groups.rs"]
mod security_groups;
#[path = "all/snapshot_block_public_access.rs"]
mod snapshot_block_public_access;
#[path = "all/spot_datafeed_subscription.rs"]
mod spot_datafeed_subscription;
// The running instances `DescribeSpotFleetInstances` reports for one Spot Fleet request.
#[path = "all/spot_fleet_instances.rs"]
mod spot_fleet_instances;
// What `CancelSpotFleetRequests` does to one Spot Fleet request and the Spot Instance requests
// it placed, and what it reports per fleet.
#[path = "all/spot_fleet_request_cancellation.rs"]
mod spot_fleet_request_cancellation;
#[path = "all/spot_fleet_requests.rs"]
mod spot_fleet_requests;
#[path = "all/spot_instance_requests.rs"]
mod spot_instance_requests;
#[path = "all/spot_placement_scores.rs"]
mod spot_placement_scores;
#[path = "all/spot_price_history.rs"]
mod spot_price_history;
#[path = "all/stale_security_groups.rs"]
mod stale_security_groups;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_image_tasks.rs"]
mod store_image_tasks;
// Reserving part of a subnet's CIDR range, and reading the reservations of one subnet back.
#[path = "all/subnet_cidr_reservations.rs"]
mod subnet_cidr_reservations;
#[path = "all/transit_gateway_acceptance.rs"]
mod transit_gateway_acceptance;
#[path = "all/transit_gateway_connect_peers.rs"]
mod transit_gateway_connect_peers;
#[path = "all/transit_gateway_lifecycle.rs"]
mod transit_gateway_lifecycle;
#[path = "all/transit_gateway_policy_table_associations.rs"]
mod transit_gateway_policy_table_associations;
#[path = "all/transit_gateway_policy_table_entries.rs"]
mod transit_gateway_policy_table_entries;
#[path = "all/transit_gateway_rejection.rs"]
mod transit_gateway_rejection;
#[path = "all/transit_gateway_route_table_propagation.rs"]
mod transit_gateway_route_table_propagation;
// The Verified Access ENDPOINT slice: create an endpoint on a stored group, then read it
// back through the describe.
#[path = "all/verified_access_endpoints.rs"]
mod verified_access_endpoints;
// The Verified Access GROUP slice: create a group on a stored instance, then reach it
// through the operations that resolve a group.
#[path = "all/verified_access_groups.rs"]
mod verified_access_groups;
// The Verified Access trust provider slice: create, then read it back.
#[path = "all/verified_access_trust_providers.rs"]
mod verified_access_trust_providers;
// The volume attribute slice: modify one attribute of a volume, then describe it back.
#[path = "all/volume_attributes.rs"]
mod volume_attributes;
#[path = "all/volume_lifecycle.rs"]
mod volume_lifecycle;
#[path = "all/volume_modifications.rs"]
mod volume_modifications;
// The volume status check slice: what `DescribeVolumeStatus` reports for the stored
// volumes, and how `VolumeIds`, the filters and the page size narrow it.
#[path = "all/volume_status.rs"]
mod volume_status;
// `RestoreVolumeFromRecycleBin`: what the bin gives back, and what it refuses.
#[path = "all/volumes_in_recycle_bin.rs"]
mod volumes_in_recycle_bin;
#[path = "all/vpc_attributes.rs"]
mod vpc_attributes;
#[path = "all/vpc_block_public_access_exclusions.rs"]
mod vpc_block_public_access_exclusions;
// `DisableVpcClassicLink` and the reader it is proved through, `DescribeVpcClassicLink`: the
// ClassicLink enablement the store keeps per VPC, what a disable changes and what it refuses.
#[path = "all/vpc_classic_link.rs"]
mod vpc_classic_link;
// `DescribeVpcClassicLinkDnsSupport`: the ClassicLink DNS support the store keeps per VPC,
// what the describe reports for each VPC it resolves, how it pages, and what it refuses.
#[path = "all/vpc_classic_link_dns_support.rs"]
mod vpc_classic_link_dns_support;
#[path = "all/vpc_endpoint_connection_notifications.rs"]
mod vpc_endpoint_connection_notifications;
#[path = "all/vpc_endpoint_connections.rs"]
mod vpc_endpoint_connections;
// The readiness poll a client makes on a created VPC endpoint: the loop on the non-terminal
// state terminates, at the terminal one.
#[path = "all/vpc_endpoint_readiness_poll.rs"]
mod vpc_endpoint_readiness_poll;
#[path = "all/vpc_endpoint_state.rs"]
mod vpc_endpoint_state;
// The vocabulary of VPC endpoint states: every value the filter publishes, against the
// members of the `State` enum the model generates.
#[path = "all/vpc_endpoint_states.rs"]
mod vpc_endpoint_states;
#[path = "all/vpc_endpoints_and_byoip.rs"]
mod vpc_endpoints_and_byoip;
#[path = "all/vpc_lifecycle.rs"]
mod vpc_lifecycle;
#[path = "all/vpc_network_lifecycle.rs"]
mod vpc_network_lifecycle;
// What `CreateVpcPeeringConnection` records between two VPCs and how
// `DescribeVpcPeeringConnections` reports it: the status the clock moves, the expiry of an
// unanswered request, the overlapping-range failure, and the ids, filters and pages.
#[path = "all/vpc_peering_connections.rs"]
mod vpc_peering_connections;
#[path = "all/vpc_resources_blocking_encryption_enforcement.rs"]
mod vpc_resources_blocking_encryption_enforcement;
#[path = "all/vpn_connection_device_types.rs"]
mod vpn_connection_device_types;
#[path = "all/vpn_connection_routes.rs"]
mod vpn_connection_routes;
// Reading one VPC back out of an ec2Query response, on top of `ec2_query`'s plumbing.
#[path = "all/wire.rs"]
mod wire;
// Declared HERE, between two long-settled suites, rather than in sorted position or at the
// end of this file: the header records that both the sorted neighbourhood and the tail are
// where every new ec2 suite inserts its lines, and these two lines lost repeated serialized
// squash merges to the tail. What `DescribeCapacityReservationTopology` reports for the
// Capacity Reservations of one Region: the path through the topology tree each one's capacity
// hangs off, and how the ids, the two documented filters and the page size narrow it.
#[path = "all/capacity_reservation_topology.rs"]
mod capacity_reservation_topology;
// Stopping the advertisement of a BYOIP address range, and what that refuses.
#[path = "all/withdraw_byoip_cidr.rs"]
mod withdraw_byoip_cidr;
// Declared LAST rather than in sorted position, for the reason the header gives: the
// sorted neighbourhood of these two suites is where other ec2 landings insert their lines,
// and the head of the list has become contended too. What `DescribeSecurityGroupRules`
// reports out of the rule store, and the byte and record ceilings every store of this
// crate is held to.
#[path = "all/security_group_rules.rs"]
mod security_group_rules;
#[path = "all/store_ceilings.rs"]
mod store_ceilings;
// Declared LAST rather than in sorted position, for the reason the note at the top of this
// file records for `prefix_lists`: a new ec2 suite lands every few minutes and inserts its
// two lines in sorted order, so a suite's own sorted neighbourhood is the contended region
// of this file — and this one lost repeated serialized squash merges to exactly that.
#[path = "all/traffic_mirror_filters.rs"]
mod traffic_mirror_filters;
// What `CreateCapacityReservation` reserves and what every reader of a Capacity
// Reservation then reports. Declared LAST rather than in sorted position, for the reason
// the note at the top of this file records: a new ec2 suite lands every few minutes and
// inserts its two lines in sorted order, so the sorted neighbourhood of this suite is the
// contended region of this file — and these two lines have already lost repeated
#[path = "all/capacity_reservations.rs"]
mod capacity_reservations;
// What `CancelExportTask` cancels, what it refuses and what it leaves alone. Declared LAST
// rather than in sorted position, for the reason the header records: a suite's own sorted
#[path = "all/export_task_cancellation.rs"]
mod export_task_cancellation;
// Declared LAST for the same reason as the two suites above: the sorted region of this file
// is where every new ec2 suite inserts its lines, and a landing that loses the squash merge
// has to redo it.
// Associating a VPC with a local gateway route table, and reading the association back.
#[path = "all/local_gateway_route_table_vpc_associations.rs"]
mod local_gateway_route_table_vpc_associations;
// Declared LAST for the reason the header gives: the sorted region of this file is where
// every new ec2 suite inserts its lines. What `DeleteTags` removes from a resource of any
// kind, and what it refuses.
#[path = "all/resource_tags.rs"]
mod resource_tags;
// Giving Dedicated Hosts back: which of the named hosts `ReleaseHosts` releases, what it
// reports per id, and what a released host still looks like through `DescribeHosts`.
// Declared LAST for the reason the header gives.
#[path = "all/release_hosts.rs"]
mod release_hosts;
// Declared LAST for the reason the header gives: taking one AMI back out of the Recycle
// Bin, and what that refuses.
#[path = "all/image_recycle_bin.rs"]
mod image_recycle_bin;
// Declared LAST for the reason the header gives: copying one Amazon FPGA Image, what the
// copy computes from the stored source, and what it refuses.
#[path = "all/copy_fpga_image.rs"]
mod copy_fpga_image;
// Declared LAST for the reason the header gives: what a stored network interface blocks
// (the group, subnet and VPC deletes it is a dependency of), and the public Systems Manager
// parameter catalog a `resolve:ssm:` launch-template `ImageId` resolves through.
#[path = "all/network_interface_dependencies.rs"]
mod network_interface_dependencies;
#[path = "all/launch_template_ssm_alias_catalog.rs"]
mod launch_template_ssm_alias_catalog;
// Declared LAST for the reason the header gives: what `DescribeIpamByoasn` reports out of
// the store of ASNs brought to IPAM, and how the page size and the token narrow it.
#[path = "all/ipam_byoasns.rs"]
mod ipam_byoasns;
// Declared LAST for the reason the header gives: the route server ASSOCIATION slice —
// associating a route server with a VPC, and reading one route server's associations back.
#[path = "all/route_server_associations.rs"]
mod route_server_associations;
// Declared LAST for the same reason: the route server PROPAGATION slice — which route tables
// a route server may install its learned routes into, and reading one route server's
// propagations back.
#[path = "all/route_server_propagations.rs"]
mod route_server_propagations;
// every new ec2 suite inserts its lines. What `DeleteTrafficMirrorSession` deletes out of
// the Traffic Mirror session store, and what it refuses.
#[path = "all/traffic_mirror_sessions.rs"]
mod traffic_mirror_sessions;
// every new ec2 suite inserts its lines. What `DescribeClientVpnEndpoints` reports out of the
// Client VPN endpoint store, and how the ids, the documented filters and the page size narrow
// it.
#[path = "all/client_vpn_endpoints.rs"]
mod client_vpn_endpoints;
// Declared LAST for the reason the header gives: the VPC flow log slice — turning flow
// logging on for a resource, reading the flow logs back, and deleting them in batches.
#[path = "all/flow_logs.rs"]
mod flow_logs;
// every new ec2 suite inserts its lines. What `DescribeTransitGatewayPolicyTables` reports
// out of the policy table store, and how the id list, the filters and the page size narrow
#[path = "all/transit_gateway_policy_tables.rs"]
mod transit_gateway_policy_tables;
// Declared LAST for the same reason: what `DescribeTransitGatewayPeeringAttachments` reports
// out of the attachment store, and how the id list, the documented filters and the page size
// narrow it.
#[path = "all/transit_gateway_peering_attachments.rs"]
mod transit_gateway_peering_attachments;
// every new ec2 suite inserts its lines. What `CreateNetworkInsightsAccessScope` persists —
// the scope and the content it was given — and what `GetNetworkInsightsAccessScopeContent`
// reports back out of that store.
#[path = "all/network_insights_access_scope_content.rs"]
mod network_insights_access_scope_content;
// every new ec2 suite inserts its lines. What `CreateFpgaImage` refuses — the design
// checkpoint it reads, which this emulator holds no Amazon S3 object for — and what it
// answers ahead of that read.
#[path = "all/create_fpga_image.rs"]
mod create_fpga_image;
// every new ec2 suite inserts its lines. Which ONE member of `FpgaImageAttribute`
// `DescribeFpgaImageAttribute` reports out of the stored AFI, and what it refuses.
#[path = "all/describe_fpga_image_attribute.rs"]
mod describe_fpga_image_attribute;
// every new ec2 suite inserts its lines. The Verified Access INSTANCE slice: what
// `CreateVerifiedAccessInstance` mints and persists, and the group create that resolves it.
#[path = "all/verified_access_instances.rs"]
mod verified_access_instances;
// every new ec2 suite inserts its lines. What `ValidateSecurityGroupQuotasForInterface`
// answers every caller of this emulator, and what it leaves the store holding.
// every new ec2 suite inserts its lines. The managed prefix list slice: what
// `CreateManagedPrefixList` stores and refuses, and what `DescribeManagedPrefixLists`
// reports back — the account's own lists and the ones a Region publishes.
#[path = "all/managed_prefix_lists.rs"]
mod managed_prefix_lists;
#[path = "all/validate_security_group_quotas_for_interface.rs"]
mod validate_security_group_quotas_for_interface;
// every new ec2 suite inserts its lines. The account's *block public access for AMIs*
// setting: what the enable puts in effect, what the disable takes back out, and what
// `GetImageBlockPublicAccessState` reports out of each Region's own setting.
#[path = "all/image_block_public_access.rs"]
mod image_block_public_access;
// Appended at the END of this file, like the suites above it and for the reason the header
// gives: the sorted region of this file is where every new ec2 suite inserts its lines, so a
// line added there loses serialized squash merges. What `CreateTransitGatewayRouteTable`
// mints and persists for the gateway it names, and what it refuses.
#[path = "all/create_transit_gateway_route_table.rs"]
mod create_transit_gateway_route_table;
// Appended at the END for the same reason. What `DescribeTransitGatewayMeteringPolicies`
// reports out of the metering policy store, and how the id list, the filters and the page
// size narrow it.
#[path = "all/transit_gateway_metering_policies.rs"]
mod transit_gateway_metering_policies;
// Appended at the END for the same reason. What `MoveCapacityReservationInstances` shifts
// between two Capacity Reservations, what it conserves, and what it refuses.
#[path = "all/move_capacity_reservation_instances.rs"]
mod move_capacity_reservation_instances;
// Appended at the END of this file, for the reason the header gives. The security group VPC
// association slice: what `AssociateSecurityGroupVpc` writes and refuses, and what
// `DescribeSecurityGroupVpcAssociations` reports out of that store.
#[path = "all/security_group_vpc_associations.rs"]
mod security_group_vpc_associations;
// Appended at the END of this file, for the reason the header gives. What
// `DescribeCustomerGateways` reports out of the customer gateway store, and how the id list,
// the documented filters and the clock narrow it.
#[path = "all/customer_gateways.rs"]
mod customer_gateways;
// Appended at the END of this file, for the reason the header gives. The enclave certificate
// slice: which values `AssociateEnclaveCertificateIamRole` takes for the certificate and the
// role it associates, what it stores for the pair, and what
// `GetAssociatedEnclaveCertificateIamRoles` reports back out of that store.
#[path = "all/enclave_certificate_iam_roles.rs"]
mod enclave_certificate_iam_roles;
// Appended at the END of this file, for the reason the header gives. The CREATE side of the
// network ACL slice: what `CreateNetworkAcl` mints and persists for the VPC it names, what it
// refuses, and what a created ACL reads back as (the describe's own cover is
// `describe_network_acls`).
#[path = "all/network_acls.rs"]
mod network_acls;
// Appended at the END of this file, for the reason the header gives. Giving one ASN brought
// to IPAM back out of the IPAM that holds it: what `DeprovisionIpamByoasn` leaves the store
// holding, what the clock then decides the release ended as, and what it refuses.
#[path = "all/ipam_byoasn_deprovisioning.rs"]
mod ipam_byoasn_deprovisioning;
// `DescribeTransitGatewayVpcAttachments` reports out of the attachment store, how the id list,
// the documented filters and the page size narrow it, and that an acceptance's write and the
// move the clock owes it are both visible through it.
#[path = "all/transit_gateway_vpc_attachments.rs"]
mod transit_gateway_vpc_attachments;
// `CreateLocalGatewayRouteTable` mints and persists for the local gateway it names, the move
// the clock then owes the new table, and what it refuses.
#[path = "all/create_local_gateway_route_table.rs"]
mod create_local_gateway_route_table;
// Appended at the END of this file, for the reason the header gives. What `GetCoipPoolUsage`
// reports out of the addresses allocated from one customer-owned address pool, and how the
// documented filters and the page size narrow it.
#[path = "all/get_coip_pool_usage.rs"]
mod get_coip_pool_usage;
// `DescribeTransitGatewayConnects` reports out of the attachment store, and how the id list,
// the documented filters and the page size narrow it.
#[path = "all/describe_transit_gateway_connects.rs"]
mod describe_transit_gateway_connects;
// `DescribeTransitGatewayRouteTables` reports out of the transit gateway route table store,
// how the id list, the documented filters and the page size narrow it, and that the tables
// the implemented creates store — with the moves the clock owes them — read back through it.
#[path = "all/transit_gateway_route_tables.rs"]
mod transit_gateway_route_tables;
// `DescribeTransitGatewayAttachments` reports out of the attachment store — every kind of
// attachment, as the kind-independent shape — how the id list, the documented filters and the
// page size narrow it, and that the association a writer stored reads back through it.
#[path = "all/describe_transit_gateway_attachments.rs"]
mod describe_transit_gateway_attachments;
// `ModifyVerifiedAccessInstance` applies to a stored Verified Access instance, what it leaves
// as it was, what it refuses, and how one `ClientToken` bounds it to one modification.
#[path = "all/modify_verified_access_instance.rs"]
mod modify_verified_access_instance;
// What the interface-trunking operations — `AssociateTrunkInterface`,
// `DescribeTrunkInterfaceAssociations` and `DisassociateTrunkInterface` — answer a caller
// whose account may not use interface trunking: one refusal for every shape of request, in
// any Region, with nothing stored.
#[path = "all/trunk_interface_associations.rs"]
mod trunk_interface_associations;
// Appended at the END of this file, for the reason the header gives. The account's *managed
// resource visibility* setting: the word `ModifyManagedResourceVisibility` puts in effect and
// the word `GetManagedResourceVisibility` reports out of the account's own setting.
#[path = "all/managed_resource_visibility.rs"]
mod managed_resource_visibility;
// `CreateTransitGatewayMeteringPolicy` mints and persists for the transit gateway it names,
// the move the clock then owes the new policy, and what it refuses.
#[path = "all/create_transit_gateway_metering_policy.rs"]
mod create_transit_gateway_metering_policy;
// `AssociateTransitGatewayRouteTable` stores for the (route table, attachment) pair it names,
// the move the clock then owes the new association, what it refuses, and how both reports of
// an association — the route table's own and the attachment's — answer with that one record.
#[path = "all/transit_gateway_route_table_associations.rs"]
mod transit_gateway_route_table_associations;
// `CreateTransitGatewayVpcAttachment` computes a VPC attachment from — the gateway, the VPC and
// the subnets the request names, the VPC's stored owner, and the options it asked for or
// inherited from the gateway — the `pending` -> `available` move the clock owes it, and what it
#[path = "all/create_transit_gateway_vpc_attachment.rs"]
mod create_transit_gateway_vpc_attachment;
// Appended at the END of this file, for the reason the header gives. The entries (rules) of a
// transit gateway metering policy: what `CreateTransitGatewayMeteringPolicyEntry` stores on a
// stored policy, how one rule number names one entry of one policy, and what
// `GetTransitGatewayMeteringPolicyEntries` reports, filters and pages out of that store.
#[path = "all/transit_gateway_metering_policy_entries.rs"]
mod transit_gateway_metering_policy_entries;
// Appended at the END of this file, for the reason the header gives: the sorted region of this
// file is where every new ec2 suite inserts its lines. What `DescribeReservedInstances` reports
// out of the Reserved Instances store — every member of a stored purchase, and how the id list,
// the offering class and type, and the documented filters narrow it — and that what a writer of
// a purchase stored is what the report answers with.
#[path = "all/describe_reserved_instances.rs"]
mod describe_reserved_instances;
// file is where every new ec2 suite inserts its lines. What `DeleteVerifiedAccessInstance`
// removes — the Verified Access instance the request names and the logging configuration kept
// beside it — what it answers, what still naming the instance refuses it, and how one
// `ClientToken` bounds it to one delete.
#[path = "all/delete_verified_access_instance.rs"]
mod delete_verified_access_instance;
// file is where every new ec2 suite inserts its lines. What `DeleteVerifiedAccessEndpoint`
// leaves behind — the endpoint the request names in the non-terminal `deleting` status with its
// removal owed to the clock — what it answers, what it refuses for an id that names no
// endpoint, and how one `ClientToken` bounds it to one delete.
#[path = "all/delete_verified_access_endpoint.rs"]
mod delete_verified_access_endpoint;
// file is where every new ec2 suite inserts its lines. The accepting half of the Elastic IP
// address transfer handshake: what `AcceptAddressTransfer` takes out of a standing offer, what
// it refuses for an address nothing has on offer, and where the tags it carries land.
#[path = "all/accept_address_transfer.rs"]
mod accept_address_transfer;
// file is where every new ec2 suite inserts its lines. What `UnmonitorInstances` turns off: the
// `disabling` -> `disabled` move it starts for every distinct instance it names, read back
// through `DescribeInstances`, the instance it leaves where it was, and the states, ids, batch
// sizes and dry run it refuses without changing any instance's monitoring.
#[path = "all/unmonitor_instances.rs"]
mod unmonitor_instances;
// What `DescribeFpgaImages` reports out of the Amazon FPGA Image store: every AFI available to
// the caller (its own, the public ones and those shared with it) as the store holds it, how the
// id list, the `Owners` terms, the documented filters and the page size narrow the report, what
// it refuses, and the minting outcome the clock owes an AFI it is read through. Declared beside
#[path = "all/describe_fpga_images.rs"]
mod describe_fpga_images;
// What `DeleteInternetGateway` takes out of the internet gateway store: the one gateway the
// request names, the gateway it leaves where it was, and the attachment, the unresolvable id
// and the dry run it refuses without removing anything — beside the `DetachInternetGateway`
// its doc requires first, which frees the VPC and blackholes the routes that pointed at the
// gateway.
#[path = "all/delete_internet_gateway.rs"]
mod delete_internet_gateway;
// What `ModifyManagedPrefixList` revises on one managed prefix list of the account: the entry
// set it computes from the one the list holds, the version a revision of that set mints while a
// rename leaves it alone, the entries the superseded version keeps, the state the clock settles
// and what the operation refuses. Declared beside the suite above rather than in sorted
// position, for the reason the note at the top of this file gives.
#[path = "all/modify_managed_prefix_list.rs"]
mod modify_managed_prefix_list;
// file is where every new ec2 suite inserts its lines. What `DeleteTransitGatewayRouteTable`
// removes — the transit gateway route table the request names with the propagations and prefix
// list references stored in it — the state it answers that table with, and the ids, default
// roles, standing associations and dry run it refuses without removing anything.
#[path = "all/delete_transit_gateway_route_table.rs"]
mod delete_transit_gateway_route_table;
// The REFUSAL paths of the nine operation families whose negative contract used to answer the
// scaffold's 501: the code and status each family publishes for a resource this account holds
// none of, a value the operation cannot act on, a dependency the store holds and a `DryRun` of
// a request that would otherwise have succeeded. Declared at the tail of this file, which is
#[path = "all/refusal_paths_answer_modelled_codes.rs"]
mod refusal_paths_answer_modelled_codes;
// The state a GATEWAY VPC endpoint is created in: one that stands up no endpoint network
// interface is in service from the moment it exists, whatever route tables it was associated
// with. Declared beside `prefix_lists` for the reason the note above gives: the sorted
#[path = "all/gateway_endpoint_availability.rs"]
mod gateway_endpoint_availability;
// What `DescribeVpcs` reports for a VPC's IPv6 associations: the association set of a VPC that
// has one, and no such member at all for a VPC that has none. Declared beside `prefix_lists`
#[path = "all/describe_vpcs_ipv6_associations.rs"]
mod describe_vpcs_ipv6_associations;
// The prefix list a security group rule grants: resolved against the lists the account can
// see, on both sides of the group, and refused with the prefix list's own not-found class
// when it names none of them. Declared beside `prefix_lists` for the reason the note above
// gives.
#[path = "all/security_group_rule_prefix_lists.rs"]
mod security_group_rule_prefix_lists;
// What `ModifyIpam` applies to the IPAM it names — its description, tier, metered account,
// private-GUA setting and the operating Regions it adds and removes — what it leaves as it
// was, the transient state it answers, and what it refuses without changing anything.
// Declared at the tail of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/modify_ipam.rs"]
mod modify_ipam;
// How this service classifies a refusal: a long-form resource id whose identifier it issued
// to another kind of resource is no id of the kind a member declares (that kind's malformed
// class, before any lookup), a pair of members that cannot hold together is a parameter
// COMBINATION, a launch template's Systems Manager alias must resolve when the version is
// created, and a modelled enum value EC2 reads without regard to case is served rather than
// refused. Declared at the tail of this file, which is where every new ec2 suite inserts its
#[path = "all/refusal_classes_and_unreadable_ids.rs"]
mod refusal_classes_and_unreadable_ids;
// What `MonitorInstances` turns on: the `pending` -> `enabled` move it starts for every
// distinct instance it names, read back through `DescribeInstances`, and the PARAMETER layer
// both halves of the monitoring move read their `InstanceIds` list in — a value outside the
// instance-id form refused for the parameter, one inside it looked up. Declared at the tail
// of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/monitor_instances.rs"]
mod monitor_instances;
// How both authorize operations WORD the two refusals a described rule earns: a rule the group
// would hold twice named by its peer, protocol, port range and action rather than by any rule
// id, and an address range that is no CIDR block named as the request wrote it. Declared at the
// tail of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/security_group_rule_refusal_wording.rs"]
mod security_group_rule_refusal_wording;
// What a SUCCESSFUL delete makes observable on the Network Access Scope, Traffic Mirror filter
// and route server surfaces: an id that stops resolving for every read of a resource whose
// shape carries no state, and the `deleting` -> `deleted` states a route server is described
// in. Declared at the tail of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/deleted_resources_stop_resolving.rs"]
mod deleted_resources_stop_resolving;
// What `DeletePublicIpv4Pool` removes: the pool id that stops resolving for the describe, the
// network border group that narrows which pool the delete finds, and the address ranges that
// must leave the pool first. Declared at the tail of this file, which is where every new ec2
#[path = "all/delete_public_ipv4_pool.rs"]
mod delete_public_ipv4_pool;
// What `GetConsoleOutput` serves for one instance: that instance's console as text,
// base64-encoded, beside the instant it was last written — and what it refuses. Declared at
// the tail of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/get_console_output.rs"]
mod get_console_output;
// The declarative-policies account status report surface: what
// `StartDeclarativePoliciesReport` records, what `GetDeclarativePoliciesReportSummary` reads
// back, what `CancelDeclarativePoliciesReport` stops, and what each refuses.
#[path = "all/declarative_policies_reports.rs"]
mod declarative_policies_reports;
// What `GetReservedInstancesExchangeQuote` quotes for an exchange of Convertible Reserved
// Instances: what each side is worth, what the exchange would cost, and why one that cannot
// be performed cannot.
#[path = "all/reserved_instances_exchange_quotes.rs"]
mod reserved_instances_exchange_quotes;
// What `GetHostReservationPurchasePreview` prices for a Dedicated Host reservation: the
// published offering applied to the hosts the request names, and the host ids and offerings it
// refuses. Declared at the tail of this file, which is where every new ec2 suite inserts its
#[path = "all/host_reservation_purchase_preview.rs"]
mod host_reservation_purchase_preview;
// The two reports of ONE Capacity Reservation — `GetGroupsForCapacityReservation` and
// `GetCapacityReservationUsage`: the permission probe they answer before resolving the
// reservation, the one half of `InvalidCapacityReservationId` they refuse every id they cannot
// resolve with, and the empty group set a reservation no Resource Groups call has added to a
// group is in. Declared at the tail of this file, which is where every new ec2 suite inserts
#[path = "all/capacity_reservation_reports.rs"]
mod capacity_reservation_reports;
// What `DescribeInstanceAttribute` reports for ONE attribute of one instance: the member
// `Attribute` names and no other, out of the state the launch wrote and the AMI, instance
// type, volume store and interface store it is derived from — and the ids, attributes and
// dry run it refuses. Declared at the tail of this file, which is where every new ec2 suite
#[path = "all/instance_attribute_describe.rs"]
mod instance_attribute_describe;
// What `GetCapacityManagerMetricDimensions` reports: the dimension combinations the account
// has capacity data for inside the window a request names, the capacity each metric family is
// measured over, the conditions that narrow them, the pages they are reported in, and the
// `CapacityManager.Disabled` refusal an account that has not enabled Capacity Manager gets.
#[path = "all/capacity_manager_metric_dimensions.rs"]
mod capacity_manager_metric_dimensions;
// What `DeleteNetworkInsightsAccessScopeAnalysis` takes out of the Network Access Scope
// analysis store: the one analysis the request names, answered by its id, with the client
// token that named it and the bytes it held released — and the ids and dry run it refuses
// without removing anything. Declared at the tail of this file, which is where every new ec2
#[path = "all/delete_network_insights_access_scope_analysis.rs"]
mod delete_network_insights_access_scope_analysis;
// What `DeprovisionIpamPoolCidr` takes back out of an IPAM pool — the transient state it
// leaves a range in, the removal the clock then owes it — and the `IncorrectState` refusal a
// deprovision of a range the pool holds no provisioned CIDR for gets. Declared at the tail of
// this file, which is where every new ec2 suite inserts its lines.
#[path = "all/deprovision_ipam_pool_cidr.rs"]
mod deprovision_ipam_pool_cidr;
// What `ModifyInstanceMetadataOptions` writes onto ONE instance's metadata options: exactly
// the options the request names, with every one it does not named left as it stands, the
// `pending` the change is in until the clock carries it onto the instance, and the ids, hop
// limits, instance states and dry run it refuses without writing. Declared at the tail of
#[path = "all/modify_instance_metadata_options.rs"]
mod modify_instance_metadata_options;
// How a Capacity Reservation Fleet's floating-point members reach the ec2Query wire — the
// decimal form AWS writes a double in, which is the response envelope's rule rather than
// this shape's — and the priority every Capacity Reservation of a Fleet reports.
#[path = "all/capacity_reservation_fleet_wire_values.rs"]
mod capacity_reservation_fleet_wire_values;
// What `ModifyCapacityReservation` applies to a stored Capacity Reservation — its capacity,
// the condition it is released under and the instances it accepts — which states admit a
// modification at all, and what it refuses. Declared at the tail of this file, which is
#[path = "all/modify_capacity_reservation.rs"]
mod modify_capacity_reservation;
// What `AttachVerifiedAccessTrustProvider` and `DetachVerifiedAccessTrustProvider` change: the
// trust providers one Verified Access instance reports as attached, read back through
// `DescribeVerifiedAccessInstances`, the pairs and ids they refuse, how one `ClientToken`
// bounds each of them to one change, and the attachments a deleted trust provider leaves
// behind. Declared at the tail of this file, which is where every new ec2 suite inserts its
#[path = "all/verified_access_trust_provider_attachment.rs"]
mod verified_access_trust_provider_attachment;
// What `ModifySecurityGroupRules` replaces on one stored security group rule: the protocol,
// the port range, the one source it names and its description, while the rule keeps its id,
// ARN, group, owner, direction, tags and rule TYPE — read back through
// `DescribeSecurityGroupRules` and the group's aggregated permissions. Declared beside
#[path = "all/modify_security_group_rules.rs"]
mod modify_security_group_rules;
// WHO may drive EC2's account-wide `Describe` reads: the identity-based IAM policies in
// force on the principal a request's credential speaks for, decided over every resource —
// the gate the CDK context providers' lookup role is served and refused through. Declared
// at the tail of this file, which is where every new ec2 suite inserts its lines.
#[path = "all/account_wide_read_authorization.rs"]
mod account_wide_read_authorization;
// Which launch template a `DeleteLaunchTemplate` resolves out of a store holding several of
// them: one key, exactly the template that carries it, the template echoed as the readers
// reported it, and every sibling left as resolvable as it was. Declared beside
#[path = "all/delete_launch_template_resolution.rs"]
mod delete_launch_template_resolution;
// Which Reserved Instance offerings the service publishes for a Region — the product of the
// places one can be used, the product descriptions, the offering classes, the payment options
// and the terms — how each is priced from its own terms, how the narrowing members, the
// documented filters, the offering ids and the pages select from it, and that the describe is a
// read whose ids the exchange operations resolve. Declared at the tail of this file, which is
#[path = "all/describe_reserved_instances_offerings.rs"]
mod describe_reserved_instances_offerings;
// The tag filter family on the describes of a taggable resource: `tag:<key>`, `tag-key` and
// `tag-value` over the tags a Capacity Reservation and a Capacity Reservation Fleet really
// hold, and the names outside the documented set and the family that are still refused.
#[path = "all/capacity_reservation_tag_filters.rs"]
mod capacity_reservation_tag_filters;
// `CreateLaunchTemplateVersion`: the versions after the first, what a `SourceVersion`
// inherits, and the versions one `DescribeLaunchTemplateVersions` then reports, newest
// first.
#[path = "all/launch_template_versions.rs"]
mod launch_template_versions;
#[path = "all/account_attributes.rs"]
mod account_attributes;
// The published instance-type catalog every call that PLACES an instance is held to: a launch,
// a Spot Instance request, one pool of a Spot Fleet request and a launch template's own data
// each refuse a type the service does not publish, naming the value. Declared at the tail of
#[path = "all/launch_instance_type_catalog.rs"]
mod launch_instance_type_catalog;
// The PUBLIC key an `ImportKeyPair` carries: the `KeyType` and `KeyFingerprint` that
// material decides for each algorithm and length, both public-key forms the import
// requirements name, the pair read back through `DescribeKeyPairs`, and what the import
#[path = "all/import_key_pair.rs"]
mod import_key_pair;
// Three suites that stood here — `ebs_direct_api_snapshots`,
// `instance_sql_ha_history_states` and `describe_reserved_instances_offerings` — are each
// ALREADY declared above, where their own entry records why they were placed there: two
// `mod` items for one name is a redefinition that stops this target compiling, so one
// declaration of each stands and the duplicates are gone.
// How an idempotency conflict is WORDED: the Network Access Analyzer / Reachability Analyzer
// actions answer that surface's own sentence beside `IdempotentParameterMismatch`, while the
// older resource creates of this service answer theirs. Declared at the tail of this file,
// which is where every new ec2 suite inserts its lines.
#[path = "all/network_insights_idempotency_wording.rs"]
mod network_insights_idempotency_wording;
// What `DescribeConversionTasks` reports out of the conversion-task store: every member of a
// stored task of either kind of VM import, the narrowing `ConversionTaskIds` does, the
// refusals for an id outside the minted form and for one no task answers to, the clock-owed
// end of a cancellation observed rather than caused, and the dry-run answer. Declared at the
#[path = "all/conversion_task_reporting.rs"]
mod conversion_task_reporting;
// `ModifyLaunchTemplate`: the default version it moves to the version a request names — by
// number or by one of the aliases the launch template API documents — what both readers of
// the template then report, and the refusals that move nothing.
#[path = "all/modify_launch_template_default_version.rs"]
mod modify_launch_template_default_version;
// What `DeleteSnapshot` removes — a snapshot of either side of the ONE snapshot resource this
// account has, the EC2 one and the EBS direct-API one — what it refuses, and what the rest of
// the lifecycle then sees. Declared at the tail of this file, which is where every new ec2
#[path = "all/delete_snapshot.rs"]
mod delete_snapshot;
// What `CreateFleet` creates: the EC2 Fleet the store holds for a request, the instances its
// target capacity places (Spot capacity through Spot Instance requests of the account,
// On-Demand capacity launched directly), the clock-owed `submitted` -> `active` move of an
// asynchronous fleet, the instances an `instant` fleet reports, and what it refuses without
// placing anything. Declared beside `prefix_lists` for the reason the note above gives: the
#[path = "all/create_fleet.rs"]
mod create_fleet;
// What `DescribeFleets` reports out of the EC2 Fleet store: each fleet as `CreateFleet`
// stored it, the instances an `instant` fleet launched, the instant fleets a report naming no
// ids leaves out, the narrowing its documented filters and `FleetIds` do, the pages it cuts
// the store into, and what it refuses. Declared beside `prefix_lists` for the reason the note
#[path = "all/describe_fleets.rs"]
mod describe_fleets;
// `CreateTransitGatewayPeeringAttachment` records between two DISTINCT transit gateways, the
// refusal a request that names one gateway as both ends of the peering earns before either end
// is resolved, and what `DescribeTransitGatewayPeeringAttachments` then reads back.
#[path = "all/create_transit_gateway_peering_attachment.rs"]
mod create_transit_gateway_peering_attachment;
// What `CancelSpotInstanceRequests` writes: the `cancelled` state and the Spot request status
// code for the way each named request ended, the instances it leaves running, the state it
// reports for a request that had already left `open` and `active`, and what it refuses
// without cancelling anything. Declared beside `prefix_lists` for the reason the note above
#[path = "all/spot_instance_request_cancellation.rs"]
mod spot_instance_request_cancellation;
// What `CreateLaunchTemplate` accepts as an `ImageId` that names an AWS-published public
// Systems Manager parameter: every hierarchy the published catalog carries an AMI id under,
// the value stored as submitted, and the resolution deferred to a launch from the template.
// Declared at the tail of this file for the reason the note above gives.
#[path = "all/launch_template_published_ssm_alias.rs"]
mod launch_template_published_ssm_alias;
// What `DeleteTrafficMirrorTarget` removes out of the Traffic Mirror target store, what it
// refuses (an id naming no target, a target a session still mirrors to, a request naming no
// id) and what it leaves alone. Declared at the tail of this file, which is where every new
// ec2 suite lands, for the reason the note at the top records.
#[path = "all/traffic_mirror_targets.rs"]
mod traffic_mirror_targets;
// Deleting LAUNCH TEMPLATE VERSIONS: the per-version outcome a request gets for the versions
// it names, the default version no request may delete, the numbering a delete does not give
// back, and what the call refuses without deleting anything. Declared beside `prefix_lists`
// for the reason the note above gives: the sorted neighbourhood and the tail of this file are
#[path = "all/delete_launch_template_versions.rs"]
mod delete_launch_template_versions;
// What `DeleteVpnConnectionRoute` takes out of the Site-to-Site VPN connection it names,
// what it refuses, and what the clock then owes the route it left `deleting`. Declared HERE,
// beside the connection-delete suite it shares its fixtures with, for the reason that suite
// gives: the head and the tail of this list are where concurrent ec2 landings insert lines.
#[path = "all/delete_vpn_connection_route.rs"]
mod delete_vpn_connection_route;
// How every operation that names an import task answers an id it cannot resolve to one.
#[path = "all/import_task_id_resolution.rs"]
mod import_task_id_resolution;
// The settings of ONE principal, out of the same store `principal_id_format` writes and
// reports for every principal. Declared beside its sibling suites rather than in sorted
// position, for the reason the note at the head of this file gives: the sorted
// neighbourhood and the head of the list are where every new ec2 suite inserts its two
#[path = "all/identity_id_format.rs"]
mod identity_id_format;
// `DeleteKeyPair`: the key pair a delete removes, the id it reports for it, what it refuses
// and what a dry run of it leaves behind. Declared beside `prefix_lists` for the reason the
#[path = "all/delete_key_pair.rs"]
mod delete_key_pair;
// What a `DeleteKeyPair` gives BACK to the store, read through the engine: the name, the id
// and the share of the caller-supplied byte budget the record charged, over a create/delete
// cycle repeated many times. Declared beside `prefix_lists` for the reason the note above
#[path = "all/key_pair_deletion.rs"]
mod key_pair_deletion;
// The ONE set of instances a Spot Fleet reports: that the instances
// `DescribeSpotFleetInstances` reports are the ones whose capacity the fleet reports as
// fulfilled, over the launch specifications and the launch-template overrides a fleet
// describes its pools with. Declared beside `prefix_lists` for the reason the note above
#[path = "all/spot_fleet_fulfilled_capacity.rs"]
mod spot_fleet_fulfilled_capacity;
// How a LAUNCH TEMPLATE OVERRIDE says which instance type its pool launches: the instance
// type and the instance requirements of one override cannot both be given, a requirement set
// this service resolves no instance type from is refused rather than quietly dropped, and an
// instance type alone is the pool it names — over both fleet APIs that take overrides.
#[path = "all/launch_template_override_instance_selection.rs"]
mod launch_template_override_instance_selection;
// The ONE attachment record an `AttachNetworkInterface` writes: answered out of the store in
// the transient state the clock owes `attached`, reported member for member by both
// `DescribeNetworkInterfaces` and `DescribeInstances`, and not given to a second instance
// while one holds it. Declared at the tail for the reason the note at the head of this file
// gives: the sorted neighbourhood is where concurrent ec2 landings insert their lines.
#[path = "all/attach_network_interface.rs"]
mod attach_network_interface;
// `ReleaseAddress`: the one Elastic IP allocation a release gives back to the address pool,
// the `<return>` envelope it reports, the second release and the border group that name no
// address of the account, the association that makes an address unreleasable, and the dry run
// that gives up nothing. Declared at the tail of this list, where every new ec2 suite inserts
#[path = "all/release_address.rs"]
mod release_address;
// The Traffic Mirror TARGET slice: what `CreateTrafficMirrorTarget` mints from the one
// destination member a request names, what it refuses, and what
// `DescribeTrafficMirrorTargets` then reports, narrows and pages over. Declared at the tail
// of this file, which is where every new ec2 suite lands, for the reason the note at the top
// records.
#[path = "all/traffic_mirror_target_creation.rs"]
mod traffic_mirror_target_creation;
// What `DescribeDeclarativePoliciesReports` reports for the declarative-policies account
// status reports an organization holds: the stored set in `ReportId` order, narrowed by the
// report ids the request names, paged by `MaxResults` / `NextToken`, with the status the clock
// has left each generation in — and what it refuses.
#[path = "all/describe_declarative_policies_reports.rs"]
mod describe_declarative_policies_reports;
// The CloudFormation template `GetFlowLogsIntegrationTemplate` generates for one stored flow
// log: the Athena table its own record format implies, the named queries that table can
// answer, the partition loader its frequency asks for, the partitions between the two
// timestamps, and what it refuses. Declared at the tail of this file, where every new ec2
// suite lands.
#[path = "all/get_flow_logs_integration_template.rs"]
mod get_flow_logs_integration_template;
// `DeleteFleets`: the state a deletion moves an EC2 Fleet to, the instances and Spot Instance
// requests it ends, the `deleted` it reaches once no instance it placed is left, and the
// per-fleet errors and request-level refusals it answers. Declared beside `prefix_lists` for
#[path = "all/delete_fleets.rs"]
mod delete_fleets;
// `GetAssociatedIpv6PoolCidrs`: the blocks of one IPv6 address pool that a resource of the
// request's Region holds, paired with that resource, in address order behind a pool-scoped
// `NextToken` — and what the read refuses. Declared at the tail of this file, which is where
// every new ec2 suite lands, for the reason the note at the top records.
#[path = "all/get_associated_ipv6_pool_cidrs.rs"]
mod get_associated_ipv6_pool_cidrs;
// `DeleteManagedPrefixList`: the stored list a delete answers in `delete-in-progress`, the
// removal the clock then owes it, the references that refuse it and what else it refuses.
// Declared beside `prefix_lists` for the reason the note above gives: the sorted neighbourhood
// and the tail of this file are where every new ec2 suite inserts its lines.
#[path = "all/delete_managed_prefix_list.rs"]
mod delete_managed_prefix_list;
// `CreateLocalGatewayVirtualInterfaceGroup`: the group it mints and persists on the local
// gateway it names, the move the clock then owes it, the interface a later create puts in it,
// and what it refuses. Declared at the tail of this file, which is where every new ec2 suite
// lands, for the reason the note at the top records.
#[path = "all/create_local_gateway_virtual_interface_group.rs"]
mod create_local_gateway_virtual_interface_group;
// `CreateCoipPool`: the one customer-owned address pool a create writes onto the local gateway
// route table it names, the members it mints for that pool, and what it refuses — declared at
// the tail of this file, which is where every new ec2 suite lands.
#[path = "all/create_coip_pool.rs"]
mod create_coip_pool;
// `CreateCapacityReservationBySplitting`: the reading it gives a source reservation id it
// cannot resolve, and how much capacity a source has to give. Declared beside `prefix_lists`
#[path = "all/capacity_reservation_split_refusals.rs"]
mod capacity_reservation_split_refusals;
// WHO may drive an EC2 action that supports resource-level permissions: the
// `UnauthorizedOperation` a caller whose documents allow the action nowhere earns, the
// `DryRunOperation` one that holds it earns under `DryRun`, the ceiling a permissions
// boundary sets, and the state a refused create does not write. Declared at the tail of this
// file, which is where every new ec2 suite lands, for the reason the note at the top records.
#[path = "all/resource_level_action_authorization.rs"]
mod resource_level_action_authorization;
// `DeleteLocalGatewayRouteTable`: the stored local gateway route table a delete answers in
// `deleting`, the terminal `deleted` the clock then owes it, and the ids, absent parameters,
// standing associations, dry runs and already-deleted tables it refuses without touching a
// stored record. Declared at the tail of this file, which is where every new ec2 suite lands.
#[path = "all/delete_local_gateway_route_table.rs"]
mod delete_local_gateway_route_table;
// The tag keys EC2 Capacity Manager monitors for the account: what
// `UpdateCapacityManagerMonitoredTagKeys` activates and deactivates, the statuses the clock
// carries a key through, and what `GetCapacityManagerMonitoredTagKeys` reports and pages over
// — declared at the tail of this file, which is where every new ec2 suite lands.
#[path = "all/capacity_manager_monitored_tag_keys.rs"]
mod capacity_manager_monitored_tag_keys;
// `DescribeLaunchTemplates` for a template that is GONE: a key this service itself resolved
// once — an id it minted, or a name a template carried — is reported absent with EC2's one
// fixed sentence, and every template the read does report carries its operator and the version
// counters its versions gave it — declared at the tail of this file, which is where every new
// ec2 suite lands.
#[path = "all/describe_launch_templates_after_delete.rs"]
mod describe_launch_templates_after_delete;
// The endpoint-type rule `CreateVpcEndpoint` holds a request to: the `VpcEndpointType` asked
// for has to be one of the types the endpoint service it names is available through, whichever
// source that service came from — declared at the tail of this file, which is where every new
#[path = "all/vpc_endpoint_type_matches_service.rs"]
mod vpc_endpoint_type_matches_service;
// `DescribeTags`: the tags of every taggable resource the store holds, the filters that
// narrow that report, the pages it is walked in and what it refuses — declared at the tail of
// this file, which is where every new ec2 suite lands.
#[path = "all/describe_tags.rs"]
mod describe_tags;
// `ModifyVpcTenancy`: the tenancy attribute the call writes onto the VPC the request names,
// read back through `DescribeVpcs`, beside what it refuses and what it leaves alone. Declared
#[path = "all/modify_vpc_tenancy.rs"]
mod modify_vpc_tenancy;
// `TerminateClientVpnConnections`: the client connections of a Client VPN endpoint a
// terminate moves out of `active` — the one its `ConnectionId` names, or every active
// connection its `Username` names — the terminal status the clock then owes each, and the
// endpoints, selectors, connections and users it refuses without touching a stored record.
// Declared at the tail of this file, which is where every new ec2 suite lands.
#[path = "all/terminate_client_vpn_connections.rs"]
mod terminate_client_vpn_connections;
// `CreateVpnConnection`: the Site-to-Site VPN connection a create makes between a customer
// gateway and one Amazon-side gateway — its two tunnels, their addresses, inside blocks and
// pre-shared keys, the options at their published defaults, the configuration document, and
// what it refuses — declared at the tail of this file, which is where every new ec2 suite
// lands.
#[path = "all/create_vpn_connection.rs"]
mod create_vpn_connection;
// The status checks every instance reports, the states they apply in, and the filters and
// pages `DescribeInstanceStatus` answers them through.
#[path = "all/describe_instance_status.rs"]
mod describe_instance_status;
// What the guest DOES with the instance's own user data: the init system runs it when the
// guest comes up, and what it writes to the console device is on that instance's console.
#[path = "all/instance_guest_console_user_data.rs"]
mod instance_guest_console_user_data;
// `UpdateSecurityGroupRuleDescriptionsEgress`: the description it writes onto each outbound
// rule a request names — by rule id or by the rule's own properties — read back through
// `DescribeSecurityGroupRules` and `DescribeSecurityGroups`, beside what it refuses and what
// it leaves alone. Declared at the tail of this file, which is where every new ec2 suite
#[path = "all/update_security_group_rule_descriptions_egress.rs"]
mod update_security_group_rule_descriptions_egress;
// How an instance, a network interface and a volume attachment are projected: the empty
// reservation groupSet, the `attaching` attachment a launch makes and the volumes it hands
// the instance as it completes, an interface's public hostname and requester, the zone a
// reported volume attachment carries, and the boot mode an instance's AMI and its own type
// give it. Declared at the tail of this file, which is where every new ec2 suite lands.
#[path = "all/instance_eni_and_volume_projections.rs"]
mod instance_eni_and_volume_projections;
// The log-delivery grants a flow log with an Amazon S3 destination leaves on that bucket:
// written into the bucket's own policy, where Amazon S3 holds it, merged into whatever the
// bucket already carried. Declared at the tail of this file, which is where every new ec2
#[path = "all/flow_log_s3_delivery_grants.rs"]
mod flow_log_s3_delivery_grants;
// `DeleteIpamScope`: the scope a delete moves to `delete-in-progress` and the clock then
// removes, the default scopes it refuses, the pools that block it, and the ids it turns down.
#[path = "all/ipam_scope_deletion.rs"]
mod ipam_scope_deletion;
// `ModifyVpcEndpoint`: the subnets, security groups, route tables, policy and private hosted
// zone the call applies to the stored endpoint it names, read back through
// `DescribeVpcEndpoints` and `DescribeRouteTables`, beside what it refuses and what it leaves
// alone. Declared at the tail of this file, which is where every new ec2 suite lands.
#[path = "all/modify_vpc_endpoint.rs"]
mod modify_vpc_endpoint;
// How a launch template's KEY and its VERSION resolve across every operation that takes one: an
// id no mint of this service reached is unreadable where one it issued whose template is gone is
// absent, a launch pins a version the template holds or is refused naming it, and a version's
// data reads back as the data its writer submitted. Declared at the tail of this file, which is
// where every new ec2 suite lands.
#[path = "all/launch_template_key_and_version_resolution.rs"]
mod launch_template_key_and_version_resolution;
// Every call this service SERVES in the account's management-event history — the refusals
// with the fault class, the code and the message their caller saw and no response elements,
// the successes with none of those. Declared beside `prefix_lists` for the reason the note
#[path = "all/served_call_audit_records.rs"]
mod served_call_audit_records;
// What a SUCCESSFUL body carries: the association members an Elastic IP reports through every
// read that reports it (and the filters that select on those same stored values), and the
// envelope's single `<return>true</return>` beside an operation's modelled output. Declared
#[path = "all/associated_address_reads_and_return_flag.rs"]
mod associated_address_reads_and_return_flag;
// The published per-type rows of the instance-type catalog the previous generation compute
// optimized, the inference accelerated and the memory optimized with local NVMe families
// carry, and the member set every reported instance type carries. Declared at the tail of
// this file, which is where every new ec2 suite lands.
#[path = "all/published_catalog_rows.rs"]
mod published_catalog_rows;
