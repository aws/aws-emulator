// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53resolver-route53resolver service.
//!
//! Besides the state struct this module owns the small pieces the Resolver
//! endpoint, Resolver rule, Resolver-on-Outpost and DNS Firewall domain-list
//! operations share, so no two
//! handlers grow their own copy: id minting, the RFC 3339 clock read, the
//! clock-owed status transition, the opaque pagination cursor the `@paginated`
//! list operations mint and verify, the record -> modelled-shape projections,
//! and the exception bodies the operations report a missing resource with.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

// The account's own Amazon EC2 inventory, which `subnet_placements` and
// `unresolvable_vpc_id` resolve a `SubnetId` and a VPC id against, and which
// `place_endpoint_address` draws an endpoint's addresses out of: a VPC, its
// subnets, and the elastic network interface holding one of a subnet's
// addresses, live in that service's store, so they are read and written through
// its own accessors.
use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;
use ::service_com_amazonaws_ec2_amazonec2::state as ec2;
// The account's own AWS Outposts inventory, which `account_holds_outpost`
// resolves an `OutpostArn` against: the Outposts live in that service's store,
// so they are read through its own accessors.
use ::service_com_amazonaws_outposts_outpostsolafservice::state as outposts;

use crate::ops::types::{
    AccessDeniedException, ConflictException, FirewallConfig, FirewallDomainList,
    FirewallDomainListMetadata,
    FirewallRule, FirewallRuleGroup, FirewallRuleGroupAssociation, FirewallRuleGroupMetadata,
    FirewallRuleType,
    InvalidNextTokenException, InvalidParameterException,
    InvalidPolicyDocument, InvalidRequestException, IpAddressResponse, IpAddressUpdate,
    LimitExceededException, OutpostResolver, ResolverConfig, ResolverDnssecConfig, ResolverEndpoint,
    ResolverQueryLogConfig, ResolverQueryLogConfigAssociation, ResolverRule, ResolverRuleAssociation,
    ResourceExistsException,
    ResourceInUseException,
    ResourceNotFoundException, Tag, TargetAddress, UnknownResourceException, ValidationException,
};
// This service's state is split by concern: each child module holds one
// resource family's records, refusals and the store methods that write them,
// and the root re-exports them all so every handler reaches them from
// `crate::state`.
mod domain_imports;
mod firewall_associations;
mod firewall_rules;
mod query_log;
mod query_log_destinations;
mod resolver_rules;
mod tags;
mod vpc_configs;
pub use domain_imports::*;
pub use firewall_associations::*;
pub use firewall_rules::*;
pub use query_log::*;
pub use query_log_destinations::*;
pub use resolver_rules::*;
pub use tags::*;
pub use vpc_configs::*;

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from the model's `awsService.arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "route53resolver";

/// How this service names ITSELF to another service of the account: the service
/// principal built from its own namespace.
///
/// It is what the account's Amazon EC2 inventory reports as the requester of an
/// elastic network interface this service created to serve a Resolver endpoint
/// ([`place_endpoint_address`]) — "the alias or Amazon Web Services account ID of
/// the principal or service that created the network interface".
pub const SERVICE_PRINCIPAL: &str = "route53resolver.amazonaws.com";

/// `ResolverEndpointStatus` values this slice reports. The model's enum
/// (`smithy_query(query="shape",shape="ResolverEndpointStatus")`) also carries
/// `AUTO_RECOVERING` and `ACTION_NEEDED`, which describe a degraded endpoint the
/// emulator never produces.
pub const ENDPOINT_STATUS_CREATING: &str = "CREATING";
pub const ENDPOINT_STATUS_OPERATIONAL: &str = "OPERATIONAL";
pub const ENDPOINT_STATUS_UPDATING: &str = "UPDATING";
pub const ENDPOINT_STATUS_DELETING: &str = "DELETING";

/// `OutpostResolverStatus` values the Resolver-on-Outpost slice reports. The
/// model's enum (`smithy_query(query="shape",shape="OutpostResolverStatus")`,
/// re-exported as [`crate::ops::types::OUTPOST_RESOLVER_STATUS_VALUES`]) also
/// carries `ACTION_NEEDED`, `FAILED_CREATION` and `FAILED_DELETION`, which
/// describe provisioning failures the emulator never produces.
pub const OUTPOST_RESOLVER_STATUS_CREATING: &str = "CREATING";
pub const OUTPOST_RESOLVER_STATUS_OPERATIONAL: &str = "OPERATIONAL";
pub const OUTPOST_RESOLVER_STATUS_UPDATING: &str = "UPDATING";
pub const OUTPOST_RESOLVER_STATUS_DELETING: &str = "DELETING";

/// The `InstanceCount` a Resolver on an Outpost is created with when the
/// request names none, and the smallest one it may be created or updated with.
///
/// `CreateOutpostResolver`'s `InstanceCount` doc: "Number of Amazon EC2
/// instances for the Resolver on Outpost. The default and minimal value is 4."
/// The model puts no `range` on `InstanceCount`, so the minimum is enforced by
/// the handlers, which refuse a smaller count with the `ValidationException`
/// both operations model.
pub const OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT: i32 = 4;

/// `IpAddressStatus` values this slice reports, out of the model's enum
/// (`smithy_query(query="shape",shape="IpAddressStatus")`); the rest describe
/// failures the emulator never produces.
pub const IP_STATUS_CREATING: &str = "CREATING";
pub const IP_STATUS_ATTACHED: &str = "ATTACHED";
pub const IP_STATUS_UPDATING: &str = "UPDATING";

/// How many IP addresses one Resolver endpoint may hold, from the model's
/// `IpAddressesRequest` length (2..20,
/// `smithy_query(query="shape",shape="IpAddressesRequest")`): the list
/// `CreateResolverEndpoint` takes is the endpoint's address set, so the bounds
/// the model puts on that list are the bounds the set keeps afterwards. The
/// operations that add and remove one address at a time enforce them here,
/// because no modelled constraint covers an endpoint that already exists.
pub const MIN_ENDPOINT_IP_ADDRESSES: usize = 2;

/// See [`MIN_ENDPOINT_IP_ADDRESSES`].
pub const MAX_ENDPOINT_IP_ADDRESSES: usize = 20;

/// `ResolverEndpointDirection` value for an endpoint that "forwards DNS queries
/// from the DNS service for a VPC to your network", from the model's enum
/// (`smithy_query(query="shape",shape="ResolverEndpointDirection")`); it is the
/// direction a Resolver rule routes its queries through.
pub const ENDPOINT_DIRECTION_OUTBOUND: &str = "OUTBOUND";

/// `RuleTypeOption` values, the model's whole enum
/// (`smithy_query(query="shape",shape="RuleTypeOption")`).
pub const RULE_TYPE_FORWARD: &str = "FORWARD";
pub const RULE_TYPE_SYSTEM: &str = "SYSTEM";
pub const RULE_TYPE_RECURSIVE: &str = "RECURSIVE";
pub const RULE_TYPE_DELEGATE: &str = "DELEGATE";

/// The port a rule's target address forwards to when the request names none:
/// the well-known DNS port (RFC 1035 §4.2, "the Internet standard port 53").
/// Inside `TargetAddress$Port`'s modelled range (0..65535).
pub const DNS_PORT: i32 = 53;

/// The protocol an endpoint gets when the request names none.
///
/// `CreateResolverEndpoint`'s `Protocols` doc: "None, which is treated as
/// Do53." `Do53` is a member of the model's `Protocol` enum.
pub const DEFAULT_PROTOCOL: &str = "Do53";

/// How long a resource with a transition in flight (a Resolver endpoint and
/// each IP address being attached, a rule association, a firewall domain list
/// whose domains were just updated) reports its transient status before the
/// clock owes the move to the settled one.
///
/// AWS answers `CreateResolverEndpoint` with `Status: CREATING` and takes
/// minutes to reach `OPERATIONAL`; an emulator must not make a caller wait that
/// long, so the transition is owed after this span instead. It is deliberately
/// not zero: a caller that reads straight after the create still observes the
/// transient status, exactly as it would against AWS.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// Page size applied when a request omits `MaxResults`: the maximum the model's
/// `MaxResults` range (1..100) allows, so an unpaged caller sees everything it
/// can in one page.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The most Resolver endpoints this store holds.
///
/// `CreateResolverEndpoint` admits any new `CreatorRequestId`, so without this
/// bound a caller looping creates would grow one emulator process without
/// limit (each record carries up to 20 IP addresses plus its tags and security
/// groups). A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_endpoint`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record, and a delete frees a slot. Bounding the endpoints bounds
/// [`ComAmazonawsRoute53resolverRoute53resolverState::creator_request_id_index`]
/// too: it only ever holds one entry per stored endpoint.
pub const MAX_STORED_ENDPOINTS: usize = 1_000;

/// The most `Filters` one `ListResolverEndpoints` request may carry, and the
/// most filter values it may carry across all of them.
///
/// The model constrains neither `Filters` nor `Filter$Values`, so without these
/// the only ceiling is the transport body cap — and the scan's cost is the
/// number of stored endpoints MULTIPLIED by the filter terms, so an uncapped
/// request would turn one call into unbounded CPU while the store lock is held.
/// The documented catalog has 7 members, so ten filters is already more than any
/// meaningful query needs. A request past either cap is refused with the
/// `InvalidParameterException` the operation models, before the store is locked.
pub const MAX_FILTERS: usize = 10;

/// See [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 100;

/// The most Resolvers on Outposts this store holds, for the same reason as
/// [`MAX_STORED_ENDPOINTS`]: `CreateOutpostResolver` admits any new
/// `CreatorRequestId`, so without a bound a caller looping creates grows one
/// emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_outpost_resolver`]
/// with the `ServiceQuotaExceededException` that operation models ("Fulfilling
/// the request would cause one or more quotas to be exceeded"); a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record, and a delete frees a slot.
pub const MAX_STORED_OUTPOST_RESOLVERS: usize = 1_000;

/// The most `SecurityGroupIds` one endpoint may be created with.
///
/// The model constrains the list's length no more than it does `Filters`, and a
/// stored endpoint's groups are walked once per filtered scan, so this bounds
/// both what one record can hold and the per-record cost of filtering on
/// `SecurityGroupIds`. A create past it is refused with the
/// `LimitExceededException` the operation models.
pub const MAX_SECURITY_GROUP_IDS: usize = 100;

/// The `MaxResults` bounds the model declares for both list operations.
const MAX_RESULTS_RANGE: (i32, i32) = (1, 100);

/// Domain-separation string mixed into a pagination cursor's checksum. NOT a
/// secret and not a security boundary: it exists so a `NextToken` this emulator
/// never minted is detected and refused, instead of being silently read as
/// "start from the beginning".
const PAGE_TOKEN_DOMAIN: &str = "route53resolver/page-token/v1";

/// The Route 53 Resolver service error code reported when a request names a
/// Resolver endpoint id the service holds no endpoint under.
///
/// Route 53 Resolver carries its own `RSLVR-nnnnn` code inside every exception
/// message, because the HTTP status (400 for every modelled Resolver error) and
/// the exception shape name together still do not say which Resolver fault
/// occurred — the code is what operators and tooling key on. The model states
/// no message text, so this code comes from the recorded responses of the
/// endpoint operations that raise `ResourceNotFoundException`.
pub const RSLVR_CODE_ENDPOINT_NOT_FOUND: &str = "RSLVR-00501";

/// The Route 53 Resolver service error code reported when a request asks a
/// Resolver endpoint to give up an IP address it is not allowed to lose,
/// because the endpoint would be left with fewer than the fewest addresses one
/// may hold.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_ENDPOINT_IP_BELOW_MINIMUM: &str = "RSLVR-00505";

/// The Route 53 Resolver service error code reported when a request asks for a
/// rule of a type Resolver answers itself, yet still names where the rule should
/// forward queries to (a Resolver endpoint, target IP addresses, or both).
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes from
/// the recorded responses of the rule operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET: &str = "RSLVR-00707";

/// The Route 53 Resolver service error code reported when a request asks for a
/// rule that forwards queries onward, yet does not say where to: it names no
/// Resolver endpoint to forward through, or no target IP addresses to forward
/// to, or neither.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`], and it is
/// the counterpart of [`RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes from
/// the recorded responses of the rule operation that raises
/// `InvalidRequestException` for it.
pub const RSLVR_CODE_FORWARD_RULE_FORWARDING_TARGET_REQUIRED: &str = "RSLVR-00706";

/// The Route 53 Resolver service error code reported when a request asks for a
/// Resolver on an Outpost with fewer EC2 instances than `InstanceCount`'s
/// documented minimum.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred — and this operation raises `ValidationException` for several
/// distinct faults — so the code is the only discriminator. The model states no
/// message text, so this code comes from the recorded responses of the
/// Resolver-on-Outpost write operations.
pub const RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM: &str = "RSLVR-03002";

/// The Route 53 Resolver service error code reported when a request asks for a
/// Resolver on an Outpost that the account has not got.
///
/// Carried for the same reason as [`RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM`]:
/// the create operation raises `ValidationException` for several distinct
/// faults, so the code is the only discriminator between them. The model states
/// no message text, so this code comes from the recorded responses of the
/// Resolver-on-Outpost create operation.
pub const RSLVR_CODE_OUTPOST_NOT_FOUND: &str = "RSLVR-03007";

/// The Route 53 Resolver service error code reported when a request names a
/// Resolver rule the service holds no rule under.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the rule operations that resolve a
/// `ResolverRuleId` and raise `ResourceNotFoundException`.
pub const RSLVR_CODE_RULE_NOT_FOUND: &str = "RSLVR-00703";

/// The Route 53 Resolver service error code reported when a rule create carries
/// a `CreatorRequestId` an earlier create of a DIFFERENT rule already spent.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the rule create that raises
/// `ResourceExistsException` for it. See
/// [`resolver_rule_creator_request_id_exists`].
pub const RSLVR_CODE_RULE_CREATOR_REQUEST_ID_EXISTS: &str = "RSLVR-00701";

/// The Route 53 Resolver service error code reported when a rule create carries
/// a domain name, or a delegation record, the service will not read as a DNS
/// name.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`], and it
/// covers BOTH members with one code, as the sentence it goes with does. See
/// [`invalid_domain_or_delegation_record`].
pub const RSLVR_CODE_INVALID_DOMAIN_OR_DELEGATION_RECORD: &str = "RSLVR-00711";

/// The Route 53 Resolver service error code reported when a paginated listing
/// is handed a `NextToken` the service never issued.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the listings that raise
/// `InvalidNextTokenException`. See [`invalid_next_token`].
pub const RSLVR_CODE_INVALID_NEXT_TOKEN: &str = "RSLVR-00302";

/// The Route 53 Resolver service error code reported when a listing is handed a
/// `Filters[].Name` outside the vocabulary that listing selects on.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the
/// listings raise `InvalidParameterException` for several distinct faults, so
/// the code is what tells a bad filter key from the rest. See
/// [`invalid_filter_name`].
pub const RSLVR_CODE_INVALID_FILTER_KEY: &str = "RSLVR-01004";

/// The Route 53 Resolver service error code reported when a delete names a
/// Resolver rule that is still associated with a VPC.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the rule delete operation that raises
/// `ResourceInUseException` for it.
pub const RSLVR_CODE_RULE_IN_USE: &str = "RSLVR-00702";

/// The Route 53 Resolver service error code reported when a request addresses a
/// rule-to-VPC association the service holds no association for.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the operation that addresses an association by
/// its rule/VPC pair and raises `ResourceNotFoundException`.
pub const RSLVR_CODE_RULE_VPC_ASSOCIATION_NOT_FOUND: &str = "RSLVR-00800";

/// The Route 53 Resolver service error code reported when a request names a
/// resource the caller cannot address: one the service holds nothing under, and
/// one it keeps to itself.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the tag operations and of the DNS Firewall
/// domain-list operations, which report this one code for every resource a
/// caller names but cannot address. See [`resource_not_found_message`].
pub const RSLVR_CODE_RESOURCE_NOT_FOUND: &str = "RSLVR-02025";

/// The service error code Route 53 Resolver reports when a request names a VPC
/// the account does not hold, for the per-VPC Resolver behavior configuration.
///
/// The same can't-find sentence ([`coded_resource_not_found_message`]) travels
/// under a code per subsystem: the DNS Firewall operations report
/// [`RSLVR_CODE_RESOURCE_NOT_FOUND`] and the Resolver behavior configuration
/// reports this one, which is what the recorded responses of the two carry.
pub const RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND: &str = "RSLVR-00913";

/// The Route 53 Resolver service error code reported when a request carries a
/// tag the service will not store, because one of the tag's parameters is
/// outside what it accepts.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the tag write. See
/// [`unacceptable_tag_message`].
pub const RSLVR_CODE_TAG_NOT_ACCEPTABLE: &str = "RSLVR-00908";

/// The service error code Route 53 Resolver reports for an IAM policy document
/// it will not attach to a resource, carried for the same reason as
/// [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP status and the error shape alone
/// do not tell this fault apart from the others that share them.
pub const RSLVR_CODE_INVALID_IAM_POLICY: &str = "RSLVR-02035";

/// The Route 53 Resolver service error code reported when a caller asks for a
/// query logging configuration's resource policy and there is no policy to
/// read.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the query-logging policy read. See
/// [`query_log_config_policy_access_denied`].
pub const RSLVR_CODE_QUERY_LOG_CONFIG_POLICY_ACCESS_DENIED: &str = "RSLVR-01600";

/// The Route 53 Resolver service error code reported when the caller is granted
/// nothing on the resource a request names.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes from
/// the recorded denial of the per-VPC configuration read. It is NOT the code of
/// the denial a query logging configuration's policy read answers
/// ([`RSLVR_CODE_QUERY_LOG_CONFIG_POLICY_ACCESS_DENIED`]), which that slice's
/// own recorded response states. See [`resolver_access_denied`].
pub const RSLVR_CODE_ACCESS_DENIED: &str = "RSLVR-01803";

/// The Route 53 Resolver service error code reported when a request names an
/// association between a VPC and a query logging configuration that the service
/// holds nothing under.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded responses of the operation that addresses one of those
/// associations by its own id. See
/// [`resolver_query_log_config_association_not_found`].
pub const RSLVR_CODE_QUERY_LOG_CONFIG_ASSOCIATION_NOT_FOUND: &str = "RSLVR-01602";

/// The Route 53 Resolver service error code reported when a request names a
/// query logging configuration the service holds nothing under.
///
/// Carried for the same reason as
/// [`RSLVR_CODE_QUERY_LOG_CONFIG_ASSOCIATION_NOT_FOUND`], and it is what tells
/// "the configuration is gone" apart from "the association is gone": the two
/// faults share the HTTP status and the exception shape, so the code is the
/// only discriminator. See [`resolver_query_log_config_not_found`].
pub const RSLVR_CODE_QUERY_LOG_CONFIG_NOT_FOUND: &str = "RSLVR-01601";

/// The Route 53 Resolver service error code reported when a DNS Firewall
/// rule-group association is asked for a `Priority` another association of the
/// same VPC already holds.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred — the associate and the update both raise
/// `ValidationException` for several distinct faults — and the model states no
/// message text, so this code comes from the recorded responses of those two
/// operations. See [`association_priority_in_use`].
pub const RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE: &str = "RSLVR-02303";

/// The Route 53 Resolver service error code reported when a DNS Firewall rule
/// is asked for a `Priority` another rule of the same rule group already holds.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`] —
/// the status and the exception shape name do not say which fault occurred, and
/// the model states no message text — and it is the code of the rule's own
/// priority clash, which is a different fault from an association's. See
/// [`firewall_rule_priority_in_use`].
pub const RSLVR_CODE_RULE_PRIORITY_IN_USE: &str = "RSLVR-02203";

/// The Route 53 Resolver service error code reported when a change to, or a
/// removal of, a DNS Firewall rule-group association is refused because the
/// association carries mutation protection.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`],
/// and it comes from the recorded responses of the update and the disassociate.
/// See [`mutation_protection_forbids_change`].
pub const RSLVR_CODE_ASSOCIATION_MUTATION_PROTECTED: &str = "RSLVR-02307";

/// The Route 53 Resolver service error code reported when one batch carries
/// more firewall rules than a single request may.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`]:
/// `LimitExceededException` states every limit of the service, so the HTTP
/// status and the shape name alone do not say which one was exceeded. See
/// [`firewall_rule_update_batch_too_large`].
pub const RSLVR_CODE_FIREWALL_RULE_BATCH_TOO_LARGE: &str = "RSLVR-02052";

/// The Route 53 Resolver service error code reported when no rule of the DNS
/// Firewall rule group the request named matches the source it named the rule
/// by.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`]:
/// `ResourceNotFoundException` states every resource of the service a caller
/// cannot address, so the HTTP status and the shape name alone do not say which
/// one was missing — and a rule inside a group is reported with its own code,
/// and not the one a resource named by its id carries
/// ([`RSLVR_CODE_RESOURCE_NOT_FOUND`]). See [`firewall_rule_not_found`].
pub const RSLVR_CODE_FIREWALL_RULE_NOT_FOUND: &str = "RSLVR-02004";

/// The Route 53 Resolver service error code reported when a DNS Firewall rule
/// write states a block setting for an action that blocks nothing.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`]:
/// `ValidationException` states every malformed rule write, so the HTTP status
/// and the shape name alone do not say which requirement was broken. See
/// [`firewall_rule_block_option_not_for_action`].
pub const RSLVR_CODE_FIREWALL_RULE_BLOCK_OPTION_NOT_FOR_ACTION: &str = "RSLVR-02010";

/// The Route 53 Resolver service error code reported when a DNS Firewall rule
/// group cannot be deleted because it still has a rule in it.
///
/// Carried for the same reason as [`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`]:
/// `ConflictException` states every state transition the service refuses, so
/// the HTTP status and the shape name alone do not say which one. See
/// [`firewall_rule_group_has_rule`].
pub const RSLVR_CODE_FIREWALL_RULE_GROUP_HAS_RULE: &str = "RSLVR-02103";

/// `ResourceNotFoundException$ResourceType`, "the type of resource that doesn't
/// exist": the modelled shape name of a Resolver endpoint.
pub const RESOURCE_TYPE_RESOLVER_ENDPOINT: &str = "ResolverEndpoint";

/// `ResourceNotFoundException$ResourceType` for a Resolver on an Outpost: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_OUTPOST_RESOLVER: &str = "OutpostResolver";

/// The same, for a Resolver rule and for one rule-to-VPC association.
pub const RESOURCE_TYPE_RESOLVER_RULE: &str = "ResolverRule";
pub const RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION: &str = "ResolverRuleAssociation";

/// The `ResolverRuleStatus` values a stored rule reports.
///
/// The model's enum (`smithy_query(query="shape",shape="ResolverRuleStatus")`)
/// is `COMPLETE | DELETING | UPDATING | FAILED`: it has no provisioning value,
/// so a created rule is settled the moment it exists and the clock owes it no
/// transition. `DELETING` is what the rule a delete has just taken out reports,
/// as that call's answer "including the status of the request"; the settings an
/// update writes are likewise in effect when that call answers, so `UPDATING`
/// and `FAILED` describe work this slice never leaves outstanding.
pub const RULE_STATUS_COMPLETE: &str = "COMPLETE";
pub const RULE_STATUS_DELETING: &str = "DELETING";

/// `ResolverRuleAssociationStatus` values this slice reports, out of the
/// model's enum (`smithy_query(query="shape",shape="ResolverRuleAssociationStatus")`);
/// `DELETING` is what the association a disassociate has just taken out
/// reports, while `FAILED` and `OVERRIDDEN` describe states this slice does not
/// produce.
pub const RULE_ASSOCIATION_STATUS_CREATING: &str = "CREATING";
pub const RULE_ASSOCIATION_STATUS_COMPLETE: &str = "COMPLETE";
pub const RULE_ASSOCIATION_STATUS_DELETING: &str = "DELETING";

/// `ShareStatus` for a resource this account owns and has not shared: a member
/// of the model's `ShareStatus` enum
/// (`smithy_query(query="shape",shape="ShareStatus")`). The emulator has one
/// account, so no Resolver rule and no query logging configuration it mints is
/// shared with another.
pub const SHARE_STATUS_NOT_SHARED: &str = "NOT_SHARED";

/// `SortOrder` for the descending order a list operation may be asked for, from
/// the model's enum (`smithy_query(query="shape",shape="SortOrder")`,
/// re-exported as [`crate::ops::types::SORT_ORDER_VALUES`]); the other value,
/// `ASCENDING`, is the order a sorted list reports when the request names none.
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The most Resolver rules, and rule associations, this store holds.
///
/// Both creates admit any new request (`CreateResolverRule` any unseen
/// `CreatorRequestId`, `AssociateResolverRule` any unseen rule/VPC pair), so
/// without these bounds a caller looping creates would grow one emulator
/// process without limit. A create past the bound is REFUSED with the
/// `LimitExceededException` both operations model; a retry of a stored
/// `CreatorRequestId` stays allowed, because it allocates no new record.
pub const MAX_STORED_RULES: usize = 1_000;

/// See [`MAX_STORED_RULES`].
pub const MAX_STORED_RULE_ASSOCIATIONS: usize = 1_000;

/// The `FirewallDomainListStatus` a domain list this account owns reports once
/// no import or update is under way.
///
/// The model's enum (`smithy_query(query="shape",shape="FirewallDomainListStatus")`,
/// re-exported as [`crate::ops::types::FIREWALL_DOMAIN_LIST_STATUS_VALUES`]) is
/// `COMPLETE | COMPLETE_IMPORT_FAILED | IMPORTING | DELETING | UPDATING`: the
/// other values describe an import, an update or a delete in flight, none of
/// which a create performs — `CreateFirewallDomainList` "creates an empty
/// firewall domain list", which is settled the moment it exists, so the clock
/// owes it no transition.
pub const FIREWALL_DOMAIN_LIST_STATUS_COMPLETE: &str = "COMPLETE";

/// The `FirewallDomainListStatus` a domain list reports once a delete has been
/// accepted for it, the model's `DELETING`: `DeleteFirewallDomainList` answers
/// with "the domain list that you just deleted", which is on its way out and so
/// is no longer in the settled status it held until that call.
pub const FIREWALL_DOMAIN_LIST_STATUS_DELETING: &str = "DELETING";

/// The `FirewallDomainListStatus` a domain list reports while an import of a
/// domain file into it is still under way, the model's `IMPORTING`:
/// `ImportFirewallDomains` answers with "the status of the import request",
/// which is in flight until the clock makes the outcome due.
pub const FIREWALL_DOMAIN_LIST_STATUS_IMPORTING: &str = "IMPORTING";

/// The `FirewallDomainListStatus` a domain list reports while a write of its
/// domains is still under way, the model's `UPDATING`.
pub const FIREWALL_DOMAIN_LIST_STATUS_UPDATING: &str = "UPDATING";

/// The `FirewallDomainListStatus` a domain list terminates in when an import
/// into it did not go through, the model's `COMPLETE_IMPORT_FAILED`.
///
/// It is a SETTLED status, like [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`]: the
/// import is over, so the list is not in a transition any more and the next
/// call that writes or deletes it is served
/// ([`firewall_domain_list_status_is_settled`]).
pub const FIREWALL_DOMAIN_LIST_STATUS_COMPLETE_IMPORT_FAILED: &str = "COMPLETE_IMPORT_FAILED";

/// Whether `status` is a step a domain list has REACHED rather than one it is
/// moving through: the two the model declares for a list nothing is doing
/// anything to (`COMPLETE`, `COMPLETE_IMPORT_FAILED`). Every other value
/// (`IMPORTING`, `UPDATING`, `DELETING`) describes a change in flight, which is
/// what the operations that move a list refuse it in.
pub fn firewall_domain_list_status_is_settled(status: &str) -> bool {
    matches!(
        status,
        FIREWALL_DOMAIN_LIST_STATUS_COMPLETE | FIREWALL_DOMAIN_LIST_STATUS_COMPLETE_IMPORT_FAILED
    )
}

/// The `StatusMessage` a domain list reports while it holds
/// [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`].
///
/// `FirewallDomainList$StatusMessage` is "additional information about the
/// status of the list, if available"
/// (`smithy_query(query="doc",shape="FirewallDomainList",member="StatusMessage")`),
/// and what DNS Firewall states there is the lifecycle step the list reached:
/// the sentence is fixed text for that step and carries no part of the request,
/// so one list's message is every list's message while it stands in that step.
pub const FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_COMPLETE: &str = "Created Firewall Domain List";

/// The `StatusMessage` a domain list reports once it holds
/// [`FIREWALL_DOMAIN_LIST_STATUS_DELETING`] — the delete step's own sentence,
/// distinct from [`FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_COMPLETE`] and likewise
/// carrying nothing from the request.
pub const FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_DELETING: &str =
    "Deleting the Firewall Domain List";

/// The `StatusMessage` a domain list reports once a change a caller made to it
/// has landed: a write of its domains, through `UpdateFirewallDomains` or an
/// import that went through.
///
/// What `StatusMessage` states is the step the list LAST reached, so a list
/// someone has written reports the step that write reached and not the one its
/// create did ([`FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_COMPLETE`]). Like every
/// other step's sentence it is fixed text with no part of the request in it.
pub const FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_CHANGED: &str = "Finished domain list update";

/// What last happened to a DNS Firewall resource, which is what the resource's
/// `StatusMessage` states once it stands settled again.
///
/// A domain list and a rule group both report it, so the two slices decide
/// their message the same way: a resource nothing has changed since it was
/// created states its creation step, and one a caller has written states that
/// the change finished ([`firewall_domain_list_status_message`],
/// [`firewall_rule_group_status_message`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FirewallChange {
    /// Nothing has changed the resource since the call that created it.
    #[default]
    Created,
    /// A caller has changed it — the domains of a list, the rules of a group.
    Changed,
}

/// The `StatusMessage` a domain list reports while it holds
/// [`FIREWALL_DOMAIN_LIST_STATUS_IMPORTING`] — the import step's own sentence,
/// stated while the domains of the file the request named are still being read
/// into the list.
///
/// Like every other step's sentence it is fixed text with no part of the
/// request in it: the URL the import reads is not quoted, only that the import
/// is running off the file that was provided.
pub const FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_IMPORTING: &str =
    "Importing domains from provided file.";

/// The `StatusMessage` the lifecycle step `status` leaves on a domain list that
/// last underwent `change`, or `None` for a step DNS Firewall states nothing
/// for.
///
/// It is DERIVED from the status and that change rather than held beside them,
/// so no projection of a list can report one step's status with another step's
/// message. The one step whose sentence the status alone does not decide is the
/// FAILED outcome of an import, which carries the Region of the request that
/// started it ([`firewall_domain_list_import_download_failed`]) and so is held
/// on the record.
pub fn firewall_domain_list_status_message(status: &str, change: FirewallChange) -> Option<String> {
    match status {
        FIREWALL_DOMAIN_LIST_STATUS_COMPLETE => Some(
            match change {
                FirewallChange::Created => FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_COMPLETE,
                FirewallChange::Changed => FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_CHANGED,
            }
            .to_string(),
        ),
        FIREWALL_DOMAIN_LIST_STATUS_DELETING => {
            Some(FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_DELETING.to_string())
        }
        FIREWALL_DOMAIN_LIST_STATUS_IMPORTING => {
            Some(FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_IMPORTING.to_string())
        }
        _ => None,
    }
}

/// The `StatusMessage` a domain list reports once an import into it failed
/// because the domain file could not be read from the URL the request named.
///
/// It is the sentence DNS Firewall states for that outcome: fixed text whose
/// only variable part is the Region the request was signed for, which is the
/// Region the file has to be in ("The file must be in an S3 bucket that's in
/// the same Region as your DNS Firewall" —
/// `ImportFirewallDomains$DomainFileUrl`'s own documentation). It fits
/// `StatusMessage` (length 0..255).
pub fn firewall_domain_list_import_download_failed(region: &str) -> String {
    format!(
        "Failed to download the file from provided url, please try with a smaller file or copy \
         the file into a {region} S3 bucket."
    )
}

/// The most domains one `UpdateFirewallDomains` request may carry.
///
/// `UpdateFirewallDomains$Domains`'s doc states it: "There is a limit of 1000
/// domains per request." A request past it is refused with the
/// `LimitExceededException` the operation models, before the store is locked,
/// so nothing is written.
pub const MAX_DOMAINS_PER_UPDATE: usize = 1_000;

/// The most domains one stored domain list holds.
///
/// DNS Firewall's own quota for a domain list is 100,000 domains, so a list may
/// hold up to that many however they reached it — one
/// [`MAX_DOMAINS_PER_UPDATE`] update at a time, or one imported file. That also
/// bounds what one emulator process holds per list, for the same reason as
/// [`MAX_SECURITY_GROUP_IDS`]: an update that would carry a list past it is
/// refused with the `LimitExceededException` the operation models, and the
/// stored list is left exactly as it was, while a FILE listing more domains
/// than one list holds is a file the import cannot take.
pub const MAX_DOMAINS_PER_LIST: usize = 100_000;

/// `ResourceNotFoundException$ResourceType` for a firewall domain list: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_DOMAIN_LIST: &str = "FirewallDomainList";

/// The most firewall domain lists this store holds, for the same reason as
/// [`MAX_STORED_ENDPOINTS`]: `CreateFirewallDomainList` admits any new
/// `CreatorRequestId`, so without a bound a caller looping creates would grow
/// one emulator process without limit. A create past this count is REFUSED by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_firewall_domain_list`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_FIREWALL_DOMAIN_LISTS: usize = 1_000;

/// The `FirewallRuleGroupStatus` a rule group reports once no change to it is
/// still settling.
///
/// The model's enum (`smithy_query(query="shape",shape="FirewallRuleGroupStatus")`,
/// re-exported as [`crate::ops::types::FIREWALL_RULE_GROUP_STATUS_VALUES`]) is
/// `COMPLETE | DELETING | UPDATING`: the other two describe a delete or an
/// update in flight, neither of which a create performs —
/// `CreateFirewallRuleGroup` "creates an empty DNS Firewall rule group", which
/// is settled the moment it exists, so the clock owes it no transition.
pub const FIREWALL_RULE_GROUP_STATUS_COMPLETE: &str = "COMPLETE";

/// The `StatusMessage` a rule group reports while it holds
/// [`FIREWALL_RULE_GROUP_STATUS_COMPLETE`].
///
/// `FirewallRuleGroup$StatusMessage` is "additional information about the status
/// of the rule group, if available"
/// (`smithy_query(query="doc",shape="FirewallRuleGroup",member="StatusMessage")`):
/// as for a domain list
/// ([`FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_COMPLETE`]), what DNS Firewall states
/// there is the lifecycle step the group reached, as fixed text with no part of
/// the request in it.
pub const FIREWALL_RULE_GROUP_STATUS_MESSAGE_COMPLETE: &str = "Created Firewall Rule Group";

/// The `StatusMessage` a rule group reports once a change a caller made to it
/// has landed: a rule added to it, changed in it or removed from it.
///
/// The rule-group twin of [`FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_CHANGED`], and
/// stated for the same reason: what `StatusMessage` carries is the step the
/// group LAST reached, so a group whose rules someone has written reports that
/// change rather than its creation.
pub const FIREWALL_RULE_GROUP_STATUS_MESSAGE_CHANGED: &str = "Finished rule group update";

/// The `FirewallRuleGroupStatus` a rule group reports once a delete has been
/// accepted for it, the model's `DELETING`: `DeleteFirewallRuleGroup` answers
/// with the rule group it just deleted, which is on its way out and so is no
/// longer in the settled status it held until that call — the rule-group twin of
/// [`FIREWALL_DOMAIN_LIST_STATUS_DELETING`].
pub const FIREWALL_RULE_GROUP_STATUS_DELETING: &str = "DELETING";

/// The `StatusMessage` a rule group reports once it holds
/// [`FIREWALL_RULE_GROUP_STATUS_DELETING`] — the delete step's own sentence,
/// stated for the resource it is about exactly as the domain list's
/// ([`FIREWALL_DOMAIN_LIST_STATUS_MESSAGE_DELETING`]) is, and likewise carrying
/// nothing from the request.
pub const FIREWALL_RULE_GROUP_STATUS_MESSAGE_DELETING: &str = "Deleting the Firewall Rule Group";

/// The `StatusMessage` the lifecycle step `status` leaves on a rule group that
/// last underwent `change`, or `None` for a step DNS Firewall states nothing
/// for — the rule-group twin of [`firewall_domain_list_status_message`], and
/// derived from the status and that change for the same reason.
pub fn firewall_rule_group_status_message(status: &str, change: FirewallChange) -> Option<String> {
    match status {
        FIREWALL_RULE_GROUP_STATUS_COMPLETE => Some(
            match change {
                FirewallChange::Created => FIREWALL_RULE_GROUP_STATUS_MESSAGE_COMPLETE,
                FirewallChange::Changed => FIREWALL_RULE_GROUP_STATUS_MESSAGE_CHANGED,
            }
            .to_string(),
        ),
        FIREWALL_RULE_GROUP_STATUS_DELETING => {
            Some(FIREWALL_RULE_GROUP_STATUS_MESSAGE_DELETING.to_string())
        }
        _ => None,
    }
}

/// `ResourceNotFoundException$ResourceType` for a firewall rule group: the
/// modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_RULE_GROUP: &str = "FirewallRuleGroup";

/// The most firewall rule groups this store holds, for the same reason as
/// [`MAX_STORED_FIREWALL_DOMAIN_LISTS`]: `CreateFirewallRuleGroup` admits any
/// new `CreatorRequestId`, so without a bound a caller looping creates would
/// grow one emulator process without limit. A create past this count is REFUSED
/// by
/// [`ComAmazonawsRoute53resolverRoute53resolverState::can_store_another_firewall_rule_group`]
/// with the `LimitExceededException` the operation models; a retry of a
/// `CreatorRequestId` already stored stays allowed, because it allocates no new
/// record.
pub const MAX_STORED_FIREWALL_RULE_GROUPS: usize = 1_000;

/// The most rules one DNS Firewall rule group holds in this store.
///
/// `CreateFirewallRule` admits any new `CreatorRequestId`, so without a bound a
/// caller looping creates would grow one record without limit — the same
/// reasoning as [`MAX_STORED_FIREWALL_RULE_GROUPS`], and not a published AWS
/// quota. A create past it is refused with the `LimitExceededException` that
/// operation models and the group's rules are left exactly as they were.
pub const MAX_RULES_PER_FIREWALL_RULE_GROUP: usize = 1_000;

/// `ResourceNotFoundException$ResourceType` for one rule of a DNS Firewall rule
/// group: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_RULE: &str = "FirewallRule";

/// The `FirewallRuleGroupAssociationStatus` an association reports once no
/// change to it is in flight, the model's `COMPLETE`
/// (`smithy_query(query="shape",shape="FirewallRuleGroupAssociationStatus")`,
/// re-exported as
/// [`crate::ops::types::FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES`]).
pub const FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE: &str = "COMPLETE";

/// The `FirewallRuleGroupAssociationStatus` an association reports while the
/// service is still putting it in place: the model's enum carries no `CREATING`
/// value at all, so `UPDATING` is the status an association that has just been
/// asked for reports until the move to
/// [`FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE`] the clock owes falls due.
pub const FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_UPDATING: &str = "UPDATING";

/// The `FirewallRuleGroupAssociationStatus` an association reports once a
/// disassociate has been accepted for it, the model's `DELETING`:
/// `DisassociateFirewallRuleGroup` answers with the association it removed, and
/// that association is on its way out.
pub const FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_DELETING: &str = "DELETING";

/// `ResourceNotFoundException$ResourceType` for a rule-group-to-VPC
/// association: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_FIREWALL_RULE_GROUP_ASSOCIATION: &str = "FirewallRuleGroupAssociation";

/// The most rule-group-to-VPC associations this store holds, for the same reason
/// as [`MAX_STORED_FIREWALL_RULE_GROUPS`]: `AssociateFirewallRuleGroup` admits
/// any new `CreatorRequestId`, so without a bound a caller looping associates
/// would grow one emulator process without limit.
pub const MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS: usize = 1_000;

/// The `MutationProtectionStatus` an association carries when the request names
/// none.
///
/// `MutationProtection`'s doc says the setting "disallows modification or
/// removal of the association", which is not in force unless it was asked for,
/// so an association made without it reports the model's `DISABLED`
/// (`smithy_query(query="shape",shape="MutationProtectionStatus")`, re-exported
/// as [`crate::ops::types::MUTATION_PROTECTION_STATUS_VALUES`]).
pub const MUTATION_PROTECTION_DISABLED: &str = "DISABLED";

/// The `MutationProtectionStatus` that holds an association against
/// modification and removal, the model's `ENABLED`.
pub const MUTATION_PROTECTION_ENABLED: &str = "ENABLED";

/// The values a rule-group association's `Priority` may take.
///
/// `AssociateFirewallRuleGroup$Priority`'s doc states them: "The allowed values
/// for Priority are between 100 and 9900." The model puts no `range` on
/// `Priority`, so the bounds are enforced by the handlers, which refuse a
/// priority outside them with the `ValidationException` both the associate and
/// the update model.
pub const ASSOCIATION_PRIORITY_RANGE: ::std::ops::RangeInclusive<i32> = 100..=9_900;

/// The `Action` a DNS Firewall rule takes to "disallow the request", from the
/// model's `Action` enum (`smithy_query(query="shape",shape="Action")`,
/// re-exported as [`crate::ops::types::ACTION_VALUES`]). It is the one action
/// whose handling the rule's `BlockResponse` has to state.
pub const FIREWALL_RULE_ACTION_BLOCK: &str = "BLOCK";

/// The `Action` a DNS Firewall rule takes to "permit the request to go through",
/// from the same enum. Its doc states where it may not be used: "Not available
/// for DNS Firewall Advanced rules."
pub const FIREWALL_RULE_ACTION_ALLOW: &str = "ALLOW";

/// The `Action` a DNS Firewall rule takes to "permit the request to go through
/// but send an alert to the logs", from the same enum. Like
/// [`FIREWALL_RULE_ACTION_ALLOW`] it lets the query through, so neither of the
/// two admits a block setting ([`firewall_rule_block_option_not_for_action`]).
pub const FIREWALL_RULE_ACTION_ALERT: &str = "ALERT";

/// The `BlockResponse` that answers a blocked query with "a custom override in
/// the response", from the model's `BlockResponse` enum
/// (`smithy_query(query="shape",shape="BlockResponse")`, re-exported as
/// [`crate::ops::types::BLOCK_RESPONSE_VALUES`]). It is the one response whose
/// handling the rule's `BlockOverride*` members have to state.
pub const FIREWALL_RULE_BLOCK_RESPONSE_OVERRIDE: &str = "OVERRIDE";

/// The `FirewallDomainRedirectionAction` a rule carries when the request names
/// none.
///
/// `FirewallDomainRedirectionAction`'s doc states it: "`INSPECT_REDIRECTION_DOMAIN`:
/// (Default) inspects all domains in the redirection chain."
pub const FIREWALL_DOMAIN_REDIRECTION_INSPECT: &str = "INSPECT_REDIRECTION_DOMAIN";

/// The `FirewallRuleStatus` of a rule whose provisioning is still running, from
/// that shape's doc (`smithy_query(query="shape",shape="FirewallRuleStatus")`):
/// "`CREATING` — DNS Firewall is provisioning the rule."
pub const FIREWALL_RULE_STATUS_CREATING: &str = "CREATING";

/// The `FirewallRuleStatus` of a provisioned rule, from the same doc:
/// "`COMPLETE` — The rule is provisioned and enforcing matches."
pub const FIREWALL_RULE_STATUS_COMPLETE: &str = "COMPLETE";

/// The `FirewallRuleStatus` of a rule whose provisioning did not succeed, from
/// the same doc: "`CREATION_FAILED` — Provisioning failed", with
/// `StatusMessage` carrying "a human-readable reason".
pub const FIREWALL_RULE_STATUS_CREATION_FAILED: &str = "CREATION_FAILED";

/// The `StatusMessage` of a rule whose match source is a partner product this
/// account holds no subscription to.
///
/// `PartnerThreatProtectionConfig$Partner`'s doc requires one: "The calling
/// account must hold an active Amazon Web Services Marketplace subscription to
/// this product", and `FirewallRuleType`'s doc states the outcome when it does
/// not — "the rule is created with `Status` `CREATION_FAILED` and cannot be
/// modified — only deleted". This emulator holds no Marketplace subscription for
/// any product, so a rule naming one reports that reason, with the product it
/// named.
pub fn partner_subscription_missing(partner: &str) -> String {
    format!(
        "The calling account holds no active Amazon Web Services Marketplace subscription to the \
         partner product \"{partner}\""
    )
}

/// The most VPC firewall configurations this store holds records for.
///
/// `UpdateFirewallConfig` admits any `ResourceId`, so without a bound a caller
/// looping updates over fresh VPC ids would grow one emulator process without
/// limit — the same reasoning as [`MAX_STORED_ENDPOINTS`]. An update that would
/// need a record past this count is refused; one for a VPC the store already
/// holds a record for allocates nothing and stays allowed, and so does a read of
/// any VPC, which stores nothing at all.
pub const MAX_STORED_FIREWALL_CONFIGS: usize = 1_000;

/// The `FirewallFailOpenStatus` a VPC's firewall configuration carries until an
/// update names another value.
///
/// `FirewallConfig$FirewallFailOpen`'s doc states it: "By default, fail open is
/// disabled, which means the failure mode is closed." It is a member of the
/// model's `FirewallFailOpenStatus` enum, re-exported as
/// [`crate::ops::types::FIREWALL_FAIL_OPEN_STATUS_VALUES`].
pub const FIREWALL_FAIL_OPEN_DISABLED: &str = "DISABLED";

/// The most `TargetIps` one Resolver rule may be created with.
///
/// The model's `TargetList` declares a minimum of 1 and NO maximum, so this is
/// the emulator's own bound on what one record can hold — the same reasoning as
/// [`MAX_SECURITY_GROUP_IDS`], not a published AWS quota. A create past it is
/// refused with the `LimitExceededException` the operation models.
pub const MAX_TARGET_IPS: usize = 100;

/// Render one Route 53 Resolver exception message: the service error code in
/// square brackets, a space, the sentence that states the fault, and the
/// terminating period.
///
/// `sentence` is passed WITHOUT its final period, so the shape of the message
/// is decided in exactly one place. A fault the service words with more than
/// one sentence is rendered through [`rslvr_message_sentences`], which this is
/// the one-sentence case of.
pub fn rslvr_message(code: &str, sentence: &str) -> String {
    rslvr_message_sentences(code, &[sentence])
}

/// Render one Route 53 Resolver exception message whose body is `sentences`:
/// the service error code in square brackets, a space, and then every sentence
/// in order, each closed by its own period and separated from the next by a
/// single space.
///
/// A Resolver refusal states the fault it found and, where the service has one,
/// what the caller has to supply instead; both belong in the one message, so
/// the number of sentences is the caller's to decide and not the formatter's.
/// Each sentence is passed WITHOUT its final period, as for [`rslvr_message`].
pub fn rslvr_message_sentences(code: &str, sentences: &[&str]) -> String {
    let mut message = format!("[{code}]");
    for sentence in sentences {
        message.push(' ');
        message.push_str(sentence);
        message.push('.');
    }
    message
}

/// The message Route 53 Resolver answers for a resource the caller named but
/// cannot address, with the id it named inside it.
///
/// ONE sentence for the whole class, because the service reports one
/// ([`RSLVR_CODE_RESOURCE_NOT_FOUND`]) for it: a resource this account never
/// created, and a resource the service publishes and keeps to itself. A caller
/// that cannot address a resource learns nothing else about it — which is what
/// makes a resource it may not write indistinguishable from one that does not
/// exist.
///
/// The sentence names the resource by its ID, so `resource` is reduced to the
/// id it names ([`arn_resource_id`]) whatever the request addressed the resource
/// by: an operation that takes an ARN and one that takes the bare id report the
/// one sentence, with the bare id inside it. An argument that is already an id
/// carries no `/` and so passes through unchanged.
pub fn resource_not_found_message(resource: &str) -> String {
    coded_resource_not_found_message(RSLVR_CODE_RESOURCE_NOT_FOUND, resource)
}

/// The same can't-find sentence under the service error code `code`.
///
/// The sentence is ONE string however the fault is coded, so the subsystem that
/// reports it chooses the code and never the wording: the DNS Firewall
/// operations carry [`RSLVR_CODE_RESOURCE_NOT_FOUND`]
/// ([`resource_not_found_message`]) and the Resolver behavior configuration
/// carries [`RSLVR_CODE_RESOLVER_RESOURCE_NOT_FOUND`].
pub fn coded_resource_not_found_message(code: &str, resource: &str) -> String {
    rslvr_message(
        code,
        &format!(
            "Can’t find the resource with ID \"{}\"",
            arn_resource_id(resource)
        ),
    )
}

/// The `ResourceNotFoundException` every Resolver endpoint operation answers
/// for an id it holds no endpoint under.
///
/// Built here rather than in each handler so the four operations that resolve a
/// `ResolverEndpointId` (`GetResolverEndpoint`,
/// `ListResolverEndpointIpAddresses`, `UpdateResolverEndpoint`,
/// `DeleteResolverEndpoint`) cannot drift apart: all of them report the
/// RSLVR-coded message, with the requested id double-quoted inside it, and the
/// resource type the model asks for.
pub fn resolver_endpoint_not_found(resolver_endpoint_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_ENDPOINT_NOT_FOUND,
            &format!(r#"Resolver endpoint with ID "{resolver_endpoint_id}" does not exist"#),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_ENDPOINT.to_string()),
    }
}

/// `ResourceNotFoundException$ResourceType` and
/// `ResourceExistsException$ResourceType` for one IP address of a Resolver
/// endpoint: the modelled shape name of the resource, as for
/// [`RESOURCE_TYPE_RESOLVER_ENDPOINT`].
pub const RESOURCE_TYPE_ENDPOINT_IP_ADDRESS: &str = "IpAddressResponse";

/// The `ResourceNotFoundException` an endpoint IP-address operation answers for
/// an address the named endpoint does not hold.
///
/// Built here so `AssociateResolverEndpointIpAddress`,
/// `DisassociateResolverEndpointIpAddress` and `UpdateResolverEndpoint` cannot
/// drift apart. `address` is how the message names the address that was asked
/// for, from [`describe_requested_ip_address`].
pub fn endpoint_ip_address_not_found(
    resolver_endpoint_id: &str,
    address: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Resolver endpoint '{resolver_endpoint_id}' has no IP address {address}"
        )),
        resource_type: Some(RESOURCE_TYPE_ENDPOINT_IP_ADDRESS.to_string()),
    }
}

/// How a message names the IP address an `IpAddressUpdate` pointed at: its
/// `IpId` when the request carried one ("Only when removing an IP address from
/// a Resolver endpoint: The ID of the IP address that you want to remove"),
/// otherwise the address literal, otherwise the subnet that holds it.
pub fn describe_requested_ip_address(requested: &IpAddressUpdate) -> String {
    if let Some(ip_id) = &requested.ip_id {
        return format!("with ID '{ip_id}'");
    }
    if let Some(ip) = &requested.ip {
        return format!("'{ip}'");
    }
    if let Some(ipv6) = &requested.ipv6 {
        return format!("'{ipv6}'");
    }
    match &requested.subnet_id {
        Some(subnet_id) => format!("in subnet '{subnet_id}'"),
        None => "to remove".to_string(),
    }
}

/// The `ResourceExistsException` `AssociateResolverEndpointIpAddress` answers
/// for an address the endpoint already holds: "The resource that you tried to
/// create already exists."
pub fn endpoint_ip_address_exists(
    resolver_endpoint_id: &str,
    address: &str,
) -> ResourceExistsException {
    ResourceExistsException {
        message: Some(format!(
            "Resolver endpoint '{resolver_endpoint_id}' already has IP address {address}"
        )),
        resource_type: Some(RESOURCE_TYPE_ENDPOINT_IP_ADDRESS.to_string()),
    }
}

/// The `LimitExceededException` `AssociateResolverEndpointIpAddress` answers for
/// an endpoint that already holds [`MAX_ENDPOINT_IP_ADDRESSES`] addresses.
pub fn endpoint_ip_address_limit_exceeded() -> LimitExceededException {
    LimitExceededException {
        message: Some(format!(
            "A Resolver endpoint can have at most {MAX_ENDPOINT_IP_ADDRESSES} IP addresses"
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_ENDPOINT.to_string()),
    }
}

/// The `InvalidRequestException` `DisassociateResolverEndpointIpAddress`
/// answers for a removal that would leave the endpoint with fewer than
/// [`MIN_ENDPOINT_IP_ADDRESSES`] addresses.
///
/// The sentence states the minimum and nothing from the request, so neither the
/// endpoint nor the address that was asked for is echoed back.
pub fn endpoint_ip_address_below_minimum() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_ENDPOINT_IP_BELOW_MINIMUM,
            &format!(
                "Can only remove IP from resolver endpoints with more than {MIN_ENDPOINT_IP_ADDRESSES} IP addresses"
            ),
        )),
    }
}

/// The `ResourceNotFoundException` every Resolver-on-Outpost operation answers
/// for an id it holds no Resolver under, built here so `GetOutpostResolver`,
/// `UpdateOutpostResolver` and `DeleteOutpostResolver` cannot drift apart.
///
/// Unlike [`resolver_endpoint_not_found`] the sentence carries no `RSLVR-nnnnn`
/// code: that code comes from the recorded responses of the endpoint
/// operations, and no recording supplies one for a missing Resolver on an
/// Outpost, so none is invented here. The wire error code clients match on is
/// the shape name, `ResourceNotFoundException`, and the modelled
/// `ResourceType` says which resource was missing.
pub fn outpost_resolver_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"Resolver on Outpost with ID "{id}" does not exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_OUTPOST_RESOLVER.to_string()),
    }
}

/// The `ValidationException` the Resolver-on-Outpost write operations answer for
/// an `InstanceCount` below the documented minimum.
///
/// Built here, rather than in each handler, so `CreateOutpostResolver` and
/// `UpdateOutpostResolver` report the one RSLVR-coded body for this fault: the
/// code is what tells it apart from the other `ValidationException` faults of
/// those operations. The sentence states the documented minimum and nothing from
/// the request, so the rejected count is not echoed back.
pub fn outpost_resolver_instance_count_below_minimum() -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_INSTANCE_COUNT_BELOW_MINIMUM,
            &format!(
                r#"The instance count must be equal to or greater than "{OUTPOST_RESOLVER_INSTANCE_COUNT_DEFAULT}""#
            ),
        )),
    }
}

/// The `ValidationException` the Resolver-on-Outpost create answers for an
/// `OutpostArn` that names no Outpost of this account.
///
/// The sentence names the Outpost by the ARN the request supplied, verbatim, so
/// the caller is told which value could not be resolved; the code
/// ([`RSLVR_CODE_OUTPOST_NOT_FOUND`]) is what tells this fault apart from the
/// operation's other `ValidationException` faults.
pub fn outpost_not_found(outpost_arn: &str) -> ValidationException {
    ValidationException {
        message: Some(rslvr_message(
            RSLVR_CODE_OUTPOST_NOT_FOUND,
            &format!(r#"Cannot find the Outpost with Arn "{outpost_arn}""#),
        )),
    }
}

/// Whether the account holds, in `region`, the Outpost `outpost_arn` names.
///
/// A Resolver on an Outpost runs ON an Outpost: which Outposts exist is a fact
/// of the account's own AWS Outposts inventory — the one `ListOutposts` answers
/// from — and not a value this service's caller supplies, so this is the one
/// place that resolution happens. The Outposts records live in that service's
/// own store and are resolved through its own accessors, so one Outpost is
/// never described by two services differently.
///
/// An Outpost is identified within the Region it is racked in, so only an ARN
/// naming an Outpost OF `region` (and of this endpoint's partition and account)
/// resolves: an ARN of another Region names nothing this endpoint holds, which
/// is exactly how that service answers a call naming it.
///
/// LOCKING. The Outposts store lock is taken for the single lookup that answers
/// this and is released before returning; handlers call this BEFORE taking this
/// service's own lock, so the two stores are never held at once. AWS Outposts
/// never reaches this service, so no cycle exists.
pub fn account_holds_outpost<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    outpost_arn: &str,
) -> bool {
    let Some(outpost_id) = outposts::resolve_outpost_id(outpost_arn, region)
        .as_ref()
        .and_then(outposts::ResolvedIdentifier::here)
        .map(str::to_string)
    else {
        return false;
    };
    let Some(state) = outposts::service_state(engine) else {
        // No Outposts store on this engine is no Outpost inventory to read, so
        // the account holds no Outpost to put a Resolver on.
        return false;
    };
    let store = state.lock();
    store.outpost(region, &outpost_id).is_some()
}

/// The `InvalidParameterException` an endpoint write answers for a `SubnetId`
/// that names no subnet of the account.
///
/// The sentence names the subnet by the id the request supplied, verbatim, so
/// the caller is told which value could not be resolved. The parameter at fault
/// is the subnet id the sentence already carries, so no `FieldName` is added
/// beside it.
pub fn subnet_not_found(subnet_id: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!("The subnet ID '{subnet_id}' does not exist"),
        field_name: None,
    }
}

/// One subnet of the account, as an endpoint address placed in it reads it: the
/// VPC that OWNS the subnet, and the block an address of that subnet comes out
/// of.
///
/// Every member but the id is optional because the account's record of a subnet
/// is the modelled `Subnet` shape, whose members are all optional: a subnet
/// stating no block is one no address can be drawn from, and one stating no VPC
/// is one no `HostVPCId` can be read off.
#[derive(Debug, Clone)]
pub struct SubnetPlacement {
    pub subnet_id: String,
    pub vpc_id: Option<String>,
    pub cidr_block: Option<String>,
    pub availability_zone: Option<String>,
    pub availability_zone_id: Option<String>,
}

/// Resolve each id in `subnet_ids`, in request order, against the account's own
/// subnet inventory — or `Err` with the FIRST id that names no subnet of the
/// account.
///
/// A Resolver endpoint's IP addresses are placed IN subnets of the caller's VPC:
/// which subnets exist, which VPC owns each one and which addresses each one
/// spans are facts of the account's own Amazon EC2 inventory — the one
/// `DescribeSubnets` answers from — and not values this service's caller
/// supplies, so this is the one place that resolution happens, exactly as
/// [`account_holds_outpost`] resolves an `OutpostArn`. The subnets live in that
/// service's own store and are read through its own accessors, so one subnet is
/// never described by two services differently.
///
/// The ids are tested in the order the request lists them, so the member the
/// caller wrote first is the one it is told about.
///
/// LOCKING. The Amazon EC2 store lock is taken for this one pass and released
/// before returning; a handler calls this with no lock of its own held, so the
/// two stores are never held at once. Amazon EC2 never reaches this service, so
/// no cycle exists.
pub fn subnet_placements<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    subnet_ids: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<SubnetPlacement>, String> {
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is no subnet inventory to resolve
        // against, so no id can be shown to name nothing — and nothing is known
        // about the subnets the request names.
        return Ok(subnet_ids
            .into_iter()
            .map(|subnet_id| SubnetPlacement {
                subnet_id: subnet_id.to_string(),
                vpc_id: None,
                cidr_block: None,
                availability_zone: None,
                availability_zone_id: None,
            })
            .collect());
    };
    let store = state.lock();
    let mut placements: Vec<SubnetPlacement> = Vec::new();
    for subnet_id in subnet_ids {
        let Some(subnet) = store.subnets.get(subnet_id) else {
            return Err(subnet_id.to_string());
        };
        placements.push(SubnetPlacement {
            subnet_id: subnet_id.to_string(),
            vpc_id: subnet.vpc_id.clone(),
            cidr_block: subnet.cidr_block.clone(),
            availability_zone: subnet.availability_zone.clone(),
            availability_zone_id: subnet.availability_zone_id.clone(),
        });
    }
    Ok(placements)
}

/// The `HostVPCId` of an endpoint whose addresses are placed in `placements`.
///
/// `ResolverEndpoint$HostVPCId` is "The ID of the VPC that you want to create
/// the Resolver endpoint in", and the request says which VPC that is only
/// through its addresses' subnets — "The subnet ID uniquely identifies a VPC"
/// (`CreateResolverEndpoint$IpAddresses`). So the id is READ OFF the account's
/// subnet inventory and never derived from the subnet ids themselves: a caller
/// can join it back to a VPC the account really holds, and filtering a listing
/// on that VPC finds the endpoint. `None` when the subnets state no VPC, which
/// is nothing to report rather than an id naming no VPC.
pub fn host_vpc_id(placements: &[SubnetPlacement]) -> Option<String> {
    placements
        .iter()
        .find_map(|placement| placement.vpc_id.clone())
}

/// How the account's Amazon EC2 inventory describes an elastic network interface
/// that carries the DNS queries of the Resolver endpoint `resolver_endpoint_id`,
/// so a caller reading that inventory can tell which endpoint an interface
/// belongs to.
pub fn endpoint_interface_description(resolver_endpoint_id: &str) -> String {
    format!("Route 53 Resolver: {resolver_endpoint_id}")
}

/// One address of a Resolver endpoint as the account's Amazon EC2 inventory
/// holds it: the IPv4 address the endpoint answers on, and the elastic network
/// interface that holds that address.
#[derive(Debug, Clone, Default)]
pub struct PlacedAddress {
    pub ip: Option<String>,
    pub network_interface_id: Option<String>,
}

/// Place one IP address of the Resolver endpoint `resolver_endpoint_id` in the
/// subnet `placement` names, and answer the address it took and the elastic
/// network interface holding it.
///
/// "If you specify a subnet ID, Resolver chooses an IP address for you from the
/// available IPs in the specified subnet"
/// (`IpAddressRequest`/`AssociateResolverEndpointIpAddress`): a request that
/// names an address places THAT address, and one that names only a subnet has an
/// address assigned out of that subnet's block. Either way the address an
/// endpoint answers on is the address of an elastic network interface of the
/// VPC, so the interface is created here, in the one inventory
/// `DescribeNetworkInterfaces` answers from — which is what makes Resolver and
/// Amazon EC2 report the same addresses instead of Resolver reporting an address
/// that exists nowhere. [`release_endpoint_addresses`] is the exact reverse.
///
/// An address is reported only when one could be taken: a subnet that states no
/// block, and one whose block has no free address left, yield none rather than
/// an invented one. Whether an address wants an IPv4 one at all is the caller's
/// decision: an address the request describes by its IPv6 address is one the
/// request has already named.
///
/// LOCKING. The Amazon EC2 store lock is taken for this one placement and
/// released before returning. The caller may hold this service's own store lock,
/// so the order is always this service and then Amazon EC2; Amazon EC2 never
/// reaches this service, so no cycle exists.
pub fn place_endpoint_address<Eng: Send + Sync + 'static>(
    engine: &Eng,
    placement: &SubnetPlacement,
    requested_ip: Option<&str>,
    region: &str,
    resolver_endpoint_id: &str,
    security_group_ids: &[String],
) -> PlacedAddress {
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is no subnet to draw an address out
        // of and no inventory to hold an interface, so the endpoint answers only
        // on an address the request itself named.
        return PlacedAddress {
            ip: requested_ip.map(str::to_string),
            network_interface_id: None,
        };
    };
    let mut store = state.lock();
    let ip = match requested_ip {
        Some(asked) => {
            // Recording the named address in the one in-use pool is what stops
            // Amazon EC2 handing the endpoint's address to something else.
            let _taken = store.reserve_private_ip(asked);
            asked.to_string()
        }
        None => {
            let Some(cidr_block) = placement.cidr_block.as_deref() else {
                return PlacedAddress::default();
            };
            let Some(assigned) = store.mint_private_ip_in_cidr(cidr_block) else {
                return PlacedAddress::default();
            };
            assigned
        }
    };
    let network_interface_id = store.insert_network_interface(ec2::NetworkInterfaceSpec {
        subnet_id: placement.subnet_id.clone(),
        vpc_id: placement.vpc_id.clone().unwrap_or_default(),
        availability_zone: placement.availability_zone.clone().unwrap_or_default(),
        availability_zone_id: placement.availability_zone_id.clone().unwrap_or_default(),
        region: region.to_string(),
        description: endpoint_interface_description(resolver_endpoint_id),
        interface_type: ec2::NETWORK_INTERFACE_TYPE_INTERFACE.to_string(),
        private_ip_addresses: vec![ip.clone()],
        // `CreateResolverEndpoint$SecurityGroupIds` is "The ID of one or more
        // security groups that you want to use to control access to this VPC":
        // the groups the endpoint was created with are the groups of the
        // interfaces its queries pass through.
        groups: security_group_ids
            .iter()
            .map(|group_id| ec2_types::GroupIdentifier {
                group_id: Some(group_id.clone()),
                group_name: None,
            })
            .collect(),
        // A Resolver endpoint's interface is attached to no instance of the
        // account, and the endpoint carries the request's tags; its interfaces
        // are not the resource the request tagged. Route 53 Resolver creates and
        // owns the interface to serve the endpoint, so the inventory reports it
        // as one that service requested and manages, not one the caller made.
        attachment: None,
        requester: Some(SERVICE_PRINCIPAL.to_string()),
        tags: Vec::new(),
    });
    PlacedAddress {
        ip: Some(ip),
        network_interface_id,
    }
}

/// Take the elastic network interfaces of `addresses` back out of the account's
/// Amazon EC2 inventory — the exact reverse of [`place_endpoint_address`].
///
/// The interfaces carry the queries of the endpoint that holds the addresses, so
/// they do not outlive it: the operations that take one address away from an
/// endpoint, and the one that deletes the endpoint outright, give the interfaces
/// and the addresses they held back here.
///
/// LOCKING: as [`place_endpoint_address`].
pub fn release_endpoint_addresses<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    addresses: impl IntoIterator<Item = &'a IpAddressRecord>,
) {
    let Some(state) = ec2::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for address in addresses {
        if let Some(network_interface_id) = address.network_interface_id.as_deref() {
            let _removed = store.remove_network_interface(network_interface_id);
        }
    }
}

/// The `InvalidParameterException` a write answers for a VPC id that names no
/// VPC of the account.
///
/// The sentence is the one Amazon EC2 words for that id
/// ([`vpc_not_found_message`]), so the caller is told which value could not be
/// resolved; the parameter at fault is the VPC id the sentence already carries,
/// so no `FieldName` is added beside it, exactly as for [`subnet_not_found`].
pub fn vpc_not_found(vpc_id: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: vpc_not_found_message(vpc_id),
        field_name: None,
    }
}

/// The same refusal as [`vpc_not_found`] in the `ValidationException` shape, for
/// an operation whose model gives it that error and no
/// `InvalidParameterException` to report a parameter it cannot resolve with.
pub fn vpc_not_found_validation(vpc_id: &str) -> ValidationException {
    ValidationException {
        message: Some(vpc_not_found_message(vpc_id)),
    }
}

/// What a VPC id that names no VPC of the account is refused with, in one place
/// so the operations that resolve one cannot word it two ways.
///
/// Amazon EC2 owns the inventory the id is resolved against and words this
/// sentence itself for its `InvalidVpcID.NotFound` — "The vpc ID '<id>' does not
/// exist" — and a Resolver write that could not resolve the id reports what that
/// inventory said about it, naming the id the REQUEST sent, exactly as the
/// endpoint write path reports an unresolvable subnet.
pub fn vpc_not_found_message(vpc_id: &str) -> String {
    format!("The vpc ID '{vpc_id}' does not exist")
}

/// The first id in `vpc_ids`, in request order, that names no VPC of the
/// account, or `None` when the account holds every one of them.
///
/// A Resolver resource is associated WITH a VPC of the caller's account: which
/// VPCs exist is a fact of the account's own Amazon EC2 inventory — the one
/// `DescribeVpcs` answers from — and not a value this service's caller supplies,
/// so this is the one place that resolution happens, exactly as
/// [`subnet_placements`] resolves a `SubnetId` and
/// [`account_holds_outpost`] an `OutpostArn`.
///
/// LOCKING. As [`subnet_placements`]: the Amazon EC2 store lock is taken
/// for this one pass and released before returning, and a handler calls this
/// with no lock of its own held.
pub fn unresolvable_vpc_id<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    vpc_ids: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    unresolvable_ec2_id(engine, vpc_ids, |store, vpc_id| {
        store.vpcs.contains_key(vpc_id)
    })
}

/// Every VPC the account's own Amazon EC2 inventory holds, in the order that
/// inventory reports and pages over them (by `VpcId`).
///
/// Which VPCs an account HAS is a fact of that inventory — the one
/// `DescribeVpcs` answers from — and not of this service's own records, exactly
/// as in [`unresolvable_vpc_id`]: a VPC the account created carries this
/// service's per-VPC singleton configurations
/// ([`ComAmazonawsRoute53resolverRoute53resolverState::firewall_config`],
/// [`ComAmazonawsRoute53resolverRoute53resolverState::resolver_config`]) whether
/// or not any Resolver call has ever named it, so this is what the operations
/// that LIST those configurations enumerate, beside the VPCs this service's own
/// records name
/// ([`ComAmazonawsRoute53resolverRoute53resolverState::known_vpc_ids`]).
///
/// Amazon EC2 is a REGIONAL service and its `DescribeVpcs` reports the VPCs of
/// the Region it was asked in, where this enumerates the account's whole VPC
/// inventory: these listings are views of what the ACCOUNT holds rather than of
/// one Region's endpoint, which is the reading this service has always taken.
///
/// A pure read: nothing is written to either store.
///
/// LOCKING. As [`unresolvable_vpc_id`]: the Amazon EC2 store lock is taken for
/// this one pass and released before returning, and a handler calls this with no
/// lock of its own held.
pub fn account_vpc_ids<Eng: Send + Sync + 'static>(engine: &Eng) -> Vec<String> {
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is no inventory to enumerate, so
        // the VPCs this service's own records name are all it knows of.
        return Vec::new();
    };
    let store = state.lock();
    store.vpcs.keys().cloned().collect()
}

/// The first of `ids` the account's own Amazon EC2 store does not hold, by the
/// `holds` test naming the inventory to resolve against.
///
/// One pass for every kind of Amazon EC2 id this service resolves, so the
/// lock discipline and the "no store is no inventory" allowance below are
/// stated once rather than per id kind.
fn unresolvable_ec2_id<'a, Eng: Send + Sync + 'static>(
    engine: &Eng,
    ids: impl IntoIterator<Item = &'a str>,
    holds: impl Fn(&ec2::ComAmazonawsEc2Amazonec2State, &str) -> bool,
) -> Option<String> {
    let Some(state) = ec2::service_state(engine) else {
        // No Amazon EC2 store on this engine is no inventory to resolve
        // against, so no id can be shown to name nothing.
        return None;
    };
    let store = state.lock();
    ids.into_iter()
        .find(|id| !holds(&store, id))
        .map(str::to_string)
}

/// The `InvalidRequestException` the rule write path answers for a rule that
/// forwards queries onward but does not say where to.
///
/// One body for the whole precondition — a missing `ResolverEndpointId`, missing
/// `TargetIps`, or both — because a forwarding rule needs the endpoint and the
/// target addresses TOGETHER: either one alone describes no forwarding path, so
/// it is one fault and carries one service error code.
pub fn forward_rule_requires_forwarding_target() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_FORWARD_RULE_FORWARDING_TARGET_REQUIRED,
            &format!(
                "Resolver endpoint ID and target IP are required for {RULE_TYPE_FORWARD} type resolver rule"
            ),
        )),
    }
}

/// The `InvalidRequestException` the rule write path answers for a rule of a
/// type Resolver answers itself that still names where to forward queries to —
/// the mirror image of [`forward_rule_requires_forwarding_target`], and one body
/// for the endpoint, the target addresses or both.
pub fn system_rule_forbids_forwarding_target() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(rslvr_message(
            RSLVR_CODE_SYSTEM_RULE_FORWARDING_TARGET,
            &format!(
                "Cannot specify resolver endpoint ID and target IP for {RULE_TYPE_SYSTEM} type resolver rule"
            ),
        )),
    }
}

/// The `InvalidRequestException` a rule write answers when the rule's TYPE
/// forbids the forwarding members the request names, or `None` when that type
/// admits them.
///
/// `TargetIps` "is available only when the value of Rule type is FORWARD", and
/// `ResolverEndpointId` is the endpoint the queries named by those addresses
/// pass through, so a rule of any other type carries neither. Decided here
/// rather than in each handler so the create and the update of one rule cannot
/// disagree about which member its type admits; the type is the one the rule
/// HAS (the request's on a create, the stored record's on an update).
pub fn forwarding_target_forbidden(
    rule_type: &str,
    names_target_ips: bool,
    names_resolver_endpoint: bool,
) -> Option<InvalidRequestException> {
    // A rule of a type Resolver answers itself names no forwarding target at
    // all, so the endpoint is refused on its own as well as with the addresses.
    if rule_type == RULE_TYPE_SYSTEM && (names_target_ips || names_resolver_endpoint) {
        return Some(system_rule_forbids_forwarding_target());
    }
    // "when creating a DELEGATE rule, you must not provide the TargetIps
    // parameter. If you provide the TargetIps, you may receive an ERROR message
    // similar to 'Delegate resolver rules need to specify a nameserver name'" —
    // the model states that wording, so it is reported.
    if rule_type == RULE_TYPE_DELEGATE && names_target_ips {
        return Some(InvalidRequestException {
            message: Some("Delegate resolver rules need to specify a nameserver name.".to_string()),
        });
    }
    if rule_type != RULE_TYPE_FORWARD && names_target_ips {
        return Some(InvalidRequestException {
            message: Some(format!(
                "TargetIps is available only for a rule with RuleType {RULE_TYPE_FORWARD}, not {rule_type}"
            )),
        });
    }
    None
}

/// What is wrong with the target-address list a rule write carries, when
/// anything is.
///
/// The two faults are reported with DIFFERENT modelled errors, so
/// [`target_address_fault`] names which one it found and the handler lifts it
/// into the error its own operation models.
#[derive(Debug)]
pub enum TargetAddressFault {
    /// More target addresses than one stored rule holds ([`MAX_TARGET_IPS`]).
    TooMany(LimitExceededException),
    /// An address that names no address at all, or a list mixing the two
    /// address families.
    Invalid(InvalidParameterException),
}

/// Whether the target addresses a rule write carries can be stored as they are,
/// and what is wrong with them when they cannot.
///
/// One function for every write of a rule's `TargetIps`, so the bound and the
/// per-address preconditions a create enforces are the ones an update enforces
/// too.
pub fn target_address_fault(targets: &[TargetAddress]) -> Option<TargetAddressFault> {
    // The model puts no maximum on `TargetList`, so the list one record can
    // hold is bounded here.
    if targets.len() > MAX_TARGET_IPS {
        return Some(TargetAddressFault::TooMany(LimitExceededException {
            message: Some(format!(
                "A Resolver rule accepts at most {MAX_TARGET_IPS} target IP addresses in this emulator"
            )),
            resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
        }));
    }

    // Each target address names where to forward to, so one that carries
    // neither `Ip` nor `Ipv6` says nothing; and `TargetIps`: "You can specify
    // either Ipv4 or Ipv6 addresses but not both in the same rule."
    let ipv4_targets = targets.iter().filter(|target| target.ip.is_some()).count();
    let ipv6_targets = targets.iter().filter(|target| target.ipv6.is_some()).count();
    if ipv4_targets + ipv6_targets < targets.len() {
        return Some(TargetAddressFault::Invalid(InvalidParameterException {
            message: "Each target address must specify Ip or Ipv6".to_string(),
            field_name: Some("TargetIps".to_string()),
        }));
    }
    if ipv4_targets > 0 && ipv6_targets > 0 {
        return Some(TargetAddressFault::Invalid(InvalidParameterException {
            message: "A Resolver rule can specify either Ip or Ipv6 target addresses, not both"
                .to_string(),
            field_name: Some("TargetIps".to_string()),
        }));
    }

    None
}

/// The target addresses a rule stores for the list a request carried: the same
/// addresses, in the same order, with the members the request left out filled
/// in from the documented defaults.
///
/// The order is the request's: `TargetIps` is the list of resolvers a query is
/// forwarded to, so the list a read reports is the list the write was given,
/// neither sorted nor de-duplicated.
pub fn normalize_target_addresses(targets: &[TargetAddress]) -> Vec<TargetAddress> {
    targets
        .iter()
        .map(|target| TargetAddress {
            ip: target.ip.clone(),
            // DNS answers on port 53 (RFC 1035 §4.2), which is the port a
            // target address that names none forwards to.
            port: Some(target.port.unwrap_or(DNS_PORT)),
            ipv6: target.ipv6.clone(),
            // The `Protocol` enum's `Do53` is what "None" means for a Resolver
            // protocol, per `CreateResolverEndpoint`'s `Protocols` doc: "None,
            // which is treated as Do53."
            protocol: Some(
                target
                    .protocol
                    .clone()
                    .unwrap_or_else(|| DEFAULT_PROTOCOL.to_string()),
            ),
            // "only used if the Protocol of the TargetAddress is DoH", so it is
            // echoed, never invented.
            server_name_indication: target.server_name_indication.clone(),
        })
        .collect()
}

/// The `ResourceNotFoundException` the rule operations answer for an id they
/// hold no Resolver rule under.
///
/// Built here rather than in each handler so every operation that resolves a
/// `ResolverRuleId` (`GetResolverRule`, `UpdateResolverRule`,
/// `DeleteResolverRule`, `AssociateResolverRule`) reports the one body: the
/// RSLVR-coded message, with the requested id double-quoted inside it, and the
/// resource type the model asks for.
pub fn resolver_rule_not_found(resolver_rule_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_NOT_FOUND,
            &format!(r#"Resolver rule with ID "{resolver_rule_id}" does not exist"#),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
    }
}

/// The `ResourceNotFoundException` `GetResolverRuleAssociation` answers for an
/// id it holds no rule association under. See
/// [`outpost_resolver_not_found`] on the absent service error code: the
/// recordings available here state no code for an association addressed by its
/// OWN id, so none is invented for it.
///
/// The sentence names the missing resource by its MODELLED shape name
/// ([`RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION`], which the body also reports as
/// `ResourceType`) and double-quotes the id the request asked for, which is its
/// one variable part; the capital in "Exist" is the service's own spelling of
/// this sentence.
pub fn resolver_rule_association_not_found(
    resolver_rule_association_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            r#"{RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION} "{resolver_rule_association_id}" does not Exist."#
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION.to_string()),
    }
}

/// The `ResourceExistsException` `CreateResolverRule` answers for a
/// `CreatorRequestId` an earlier create of a DIFFERENT rule already spent.
///
/// `CreatorRequestId` is "a unique string that identifies the request": one
/// token stands for one request, so the token a rule was made with is not
/// available to a second, different request. The sentence states that fault
/// behind the service error code for it
/// ([`RSLVR_CODE_RULE_CREATOR_REQUEST_ID_EXISTS`]) and has one variable part,
/// the token the request carried.
pub fn resolver_rule_creator_request_id_exists(
    creator_request_id: &str,
) -> ResourceExistsException {
    ResourceExistsException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_CREATOR_REQUEST_ID_EXISTS,
            &format!(
                r#"Resolver rule with creator request ID "{creator_request_id}" already exists"#
            ),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
    }
}

/// Whether a domain-shaped member is spelled the way Resolver reads a DNS name:
/// in ASCII.
///
/// A DNS name travels as ASCII octets, so an internationalized label reaches
/// the protocol only in its IDNA/punycode (ACE, `xn--`-prefixed) spelling —
/// RFC 5890 §2.3.2.1. A member carrying the raw-unicode spelling of the same
/// label is therefore not a name the service can resolve on, and is refused
/// ([`invalid_domain_or_delegation_record`]) rather than stored as a selector no
/// query can ever match. Only the SPELLING is checked here: the label syntax of
/// a rule's domain is the service's to decide, and the member's length is the
/// model's own (1..256), already enforced before a handler runs.
pub fn domain_spelling_is_ascii(value: &str) -> bool {
    value.is_ascii()
}

/// The `InvalidParameterException` a Resolver rule write answers for a
/// `DomainName` or a `DelegationRecord` outside the spelling
/// [`domain_spelling_is_ascii`] admits.
///
/// The sentence is the service's own for this fault and is ENTIRELY fixed text:
/// it names neither the member at fault nor the value it was handed — which is
/// why it is not built through [`rslvr_message`], whose terminating period this
/// sentence does not carry — so no `FieldName` is added beside it either.
pub fn invalid_domain_or_delegation_record() -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "[{RSLVR_CODE_INVALID_DOMAIN_OR_DELEGATION_RECORD}] Invalid domain or delegation record"
        ),
        field_name: None,
    }
}

/// The `InvalidNextTokenException` a paginated listing answers for a
/// `NextToken` this service did not mint for that query.
///
/// Built here so every listing that models `InvalidNextTokenException` reports
/// the one body: the sentence the service words this fault with, behind the
/// service error code for it ([`RSLVR_CODE_INVALID_NEXT_TOKEN`]), with the
/// token the request sent double-quoted inside it as its one variable part. A
/// listing whose recorded refusal of an unissued token is the PARAMETER fault
/// instead reports [`invalid_next_token_parameter`].
pub fn invalid_next_token(next_token: &str) -> InvalidNextTokenException {
    InvalidNextTokenException {
        message: Some(rslvr_message(
            RSLVR_CODE_INVALID_NEXT_TOKEN,
            &format!(r#"Invalid next token: "{next_token}", invalid version"#),
        )),
    }
}

/// The `InvalidParameterException` a listing answers for a `Filters[].Name`
/// outside the vocabulary it selects on.
///
/// Built here so every listing that accepts `Filters` reports the one body: the
/// sentence the service words this fault with, behind the service error code
/// for it ([`RSLVR_CODE_INVALID_FILTER_KEY`]), with the filter name the request
/// sent double-quoted inside it as its ONE variable part — the operation is not
/// named, so two listings handed the same unknown key answer the same sentence.
pub fn invalid_filter_name(name: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: rslvr_message(
            RSLVR_CODE_INVALID_FILTER_KEY,
            &format!(r#"The filter key "{name}" is invalid"#),
        ),
        field_name: Some(FILTER_NAME_MEMBER.to_string()),
    }
}

/// The modelled member name a bad filter key is reported against, as the model
/// spells it (`smithy_query(query="shape",shape="Filter")`).
const FILTER_NAME_MEMBER: &str = "Name";

/// The `ResourceNotFoundException` `DisassociateResolverRule` answers when the
/// store holds no association of that RULE with that VPC — the pair that call
/// addresses an association by, rather than the association's own id.
///
/// The missing resource the sentence names is the ASSOCIATION itself, not a
/// property of the rule, and its two variable parts are the rule and the VPC the
/// request named.
pub fn resolver_rule_vpc_association_not_found(
    resolver_rule_id: &str,
    vpc_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_VPC_ASSOCIATION_NOT_FOUND,
            &format!(
                r#"Resolver Rule Association between Resolver Rule "{resolver_rule_id}" and VPC "{vpc_id}" does not exist"#
            ),
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE_ASSOCIATION.to_string()),
    }
}

/// The `ResourceInUseException` `DeleteResolverRule` answers for a rule that is
/// still associated with a VPC.
///
/// The sentence is entirely fixed: it states the remedy the caller has to apply
/// before the delete can succeed, and echoes nothing from the request or the
/// store — neither the rule, nor a VPC, nor how many associations are still
/// held.
pub fn resolver_rule_in_use() -> ResourceInUseException {
    ResourceInUseException {
        message: Some(rslvr_message(
            RSLVR_CODE_RULE_IN_USE,
            "Please disassociate this resolver rule from VPC first before deleting",
        )),
        resource_type: Some(RESOURCE_TYPE_RESOLVER_RULE.to_string()),
    }
}

/// The `UnknownResourceException` the Resolver rule policy operations answer
/// for an `Arn` the store holds no Resolver rule under.
///
/// Both operations address the rule by its ARN rather than by its id, so the
/// sentence names what the request asked for: the ARN it was handed. No
/// recorded response available here states a service error code for this fault
/// — as for [`resolver_rule_association_not_found`] — so none is invented, and
/// the HTTP status (400) and the error shape are what the contract fixes.
pub fn unknown_resolver_rule_arn(arn: &str) -> UnknownResourceException {
    UnknownResourceException {
        message: Some(format!(r#"Resolver rule with ARN "{arn}" does not exist."#)),
    }
}

/// The `UnknownResourceException` the query-logging policy operations answer
/// for an `Arn` the store holds no query logging configuration under.
///
/// Both operations address the configuration by its ARN rather than by its id,
/// so — as for [`unknown_resolver_rule_arn`] — the sentence names what the
/// request asked for: the ARN it was handed.
pub fn unknown_resolver_query_log_config_arn(arn: &str) -> UnknownResourceException {
    UnknownResourceException {
        message: Some(format!(
            r#"Resolver query log config with ARN "{arn}" does not exist."#
        )),
    }
}

/// The `AccessDeniedException` a read of a query logging configuration's
/// resource policy answers while the configuration carries no policy.
///
/// A query logging configuration's resource policy is readable only while one is
/// attached: with nothing attached the caller is granted nothing to read, which
/// the model's `AccessDeniedException` ("the current account doesn't have the
/// IAM permissions required to perform the specified Resolver operation") is the
/// modelled answer for. The message is the one sentence the service words that
/// fault with, carried behind its service error code
/// ([`RSLVR_CODE_QUERY_LOG_CONFIG_POLICY_ACCESS_DENIED`]) and carrying nothing
/// from the request; it is written out here rather than through
/// [`rslvr_message`] because the service ends this sentence with no period.
pub fn query_log_config_policy_access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "[{RSLVR_CODE_QUERY_LOG_CONFIG_POLICY_ACCESS_DENIED}] Access denied"
        )),
    }
}

/// The AWS X-Ray trace id the service reports THIS request under: the `Root` of
/// the tracing header the caller sent, when it sent one naming a trace, and
/// otherwise one minted for this request.
///
/// A Resolver refusal closes with the trace id of the request it refused
/// ([`traced_message`]), so a caller can quote it when asking why. The id is
/// read and minted through the one reading of X-Ray's own form the services of
/// this emulator share, so a chain a caller started is continued rather than
/// replaced.
pub fn request_trace_root(headers: &::http::HeaderMap) -> String {
    headers
        .get("x-amzn-trace-id")
        .and_then(|value| value.to_str().ok())
        .and_then(::service_common::xray_trace::continued_root)
        .map(str::to_string)
        .unwrap_or_else(|| {
            ::service_common::xray_trace::mint_trace_id(::std::time::SystemTime::now())
        })
}

/// `message` closed with the trace clause Route 53 Resolver ends it with: a
/// single space, `Trace Id:`, and the request's trace id in double quotes.
///
/// The recorded refusals of the per-VPC configuration operations carry this
/// clause after the coded sentence, and the id in it is the trace of the
/// REQUEST ([`request_trace_root`]) — so it is a different value on every call
/// and is never derived from anything the caller asked about.
pub fn traced_message(message: String, trace_root: &str) -> String {
    format!("{message} Trace Id: \"{trace_root}\"")
}

/// The `AccessDeniedException` an operation answers for a resource the caller
/// is granted nothing on — one the account does not hold, and one it holds but
/// may not use for what the request asks.
///
/// The model's `AccessDeniedException` is "the current account doesn't have the
/// IAM permissions required to perform the specified Resolver operation", which
/// is the answer for a request the caller is not entitled to have served. The
/// body carries NOTHING from the request, because that is the point of the
/// refusal: the service does not disclose whether the resource exists, so a
/// resource the caller may not reach and one that was never created are
/// indistinguishable.
///
/// The sentence is the one the service's recorded denial words that fault with,
/// behind its service error code ([`RSLVR_CODE_ACCESS_DENIED`]) and closed with
/// the trace clause of the request ([`traced_message`]), which is the only part
/// of it that is not fixed text.
pub fn resolver_access_denied(trace_root: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(traced_message(
            rslvr_message(RSLVR_CODE_ACCESS_DENIED, "Access to resource is denied"),
            trace_root,
        )),
    }
}

/// The `InvalidPolicyDocument` a resource-policy write answers for a document
/// outside the policy grammar, with `fault` stating which part of the grammar it
/// misses (from [`policy_document_fault`]).
///
/// The opening sentence is the fault the model states for the shape ("The
/// specified Resolver rule policy is invalid"); what follows names the element
/// the document is missing, so the caller can fix the document from the message
/// alone.
pub fn invalid_policy_document(fault: &str) -> InvalidPolicyDocument {
    InvalidPolicyDocument {
        message: Some(format!(
            "The specified Resolver rule policy is invalid: {fault}."
        )),
    }
}

/// The `ValidationException` a DNS Firewall rule-group policy write answers for
/// an IAM policy document the service will not attach.
///
/// `PutFirewallRuleGroupPolicy` models no `InvalidPolicyDocument`
/// (`smithy_query(query="operation",operation="PutFirewallRuleGroupPolicy")`
/// lists `ValidationException` instead), so the faults the Resolver rule policy
/// reports through that shape are reported here through the exception this
/// operation does model.
///
/// The body carries the service's code for an invalid IAM policy
/// ([`RSLVR_CODE_INVALID_IAM_POLICY`]) and echoes the document it was handed, so
/// the caller sees exactly which document was refused, and then the remediation
/// sentence that states what a document the service WOULD attach has to do
/// ([`FIREWALL_RULE_GROUP_POLICY_REMEDIATION`]). WHY the document was refused is
/// not part of it, so the one body covers a document outside the policy grammar
/// and one that shares the resource with nobody but its owner.
pub fn invalid_iam_policy(policy: &str) -> ValidationException {
    ValidationException {
        message: Some(rslvr_message_sentences(
            RSLVR_CODE_INVALID_IAM_POLICY,
            &[
                &format!("The specified IAM policy isn't valid: \"{policy}\""),
                FIREWALL_RULE_GROUP_POLICY_REMEDIATION,
            ],
        )),
    }
}

/// The remediation sentence a DNS Firewall rule-group policy refusal closes
/// with: what a document the service would attach has to do.
///
/// It is the only part of the refusal that says what to send instead, and it is
/// fixed text — it echoes nothing from the request or the store. Carried as a
/// constant, WITHOUT its final period (as every sentence handed to
/// [`rslvr_message_sentences`] is), so the refusals of both classes of document
/// the rule-group policy write rejects cannot word it differently. The model
/// states no message text for `ValidationException`
/// (`smithy_query(query="shape",shape="ValidationException")` carries `Message`
/// alone), so this wording is the recorded behaviour of the operation.
pub const FIREWALL_RULE_GROUP_POLICY_REMEDIATION: &str =
    "Provide a valid policy that allows a DNS Firewall rule group to be shared with another AWS \
     account";

/// The policy document a resource with no stored resource policy reports: the
/// empty JSON object, a document that states no statement and so shares the
/// resource with nobody. Inside the modelled length of both members that report
/// one — `ResolverRulePolicy` and `FirewallRuleGroupPolicy`, each 0..30000
/// (`smithy_query(query="shape",shape="ResolverRulePolicy,FirewallRuleGroupPolicy")`).
pub const EMPTY_POLICY_DOCUMENT: &str = "{}";

/// Why `policy` is not a policy document, or `None` when it is one.
///
/// A JSON policy document is an object, and each statement of its `Statement`
/// element — one statement or an array of them — is an object that states an
/// `Effect` and an `Action` (the elements the IAM JSON policy grammar requires
/// of every statement). A document that states no statement at all is still a
/// document: it grants nothing, which is what [`EMPTY_POLICY_DOCUMENT`] is.
///
/// The modelled `ResolverRulePolicy` length (0..30000) admits the empty string
/// and any other text, so the grammar is what a policy write checks here,
/// before it touches the store: a document outside it is refused with
/// `InvalidPolicyDocument` and the resource keeps the policy it had.
pub fn policy_document_fault(policy: &str) -> Option<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return Some("a policy document is JSON".to_string());
    };
    if !document.is_object() {
        return Some("a policy document is a JSON object".to_string());
    }
    for (index, statement) in policy_statements(&document).iter().enumerate() {
        let Some(members) = statement.as_object() else {
            return Some(format!("statement {index} is not a JSON object"));
        };
        for element in ["Effect", "Action"] {
            if !members.contains_key(element) {
                return Some(format!("statement {index} states no {element} element"));
            }
        }
    }
    None
}

/// The statements of a policy document: the elements of its `Statement` array,
/// or the single statement when the element is one statement rather than an
/// array, or none when the document states no `Statement` at all.
fn policy_statements(document: &serde_json::Value) -> Vec<&serde_json::Value> {
    match document.get("Statement") {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    }
}

/// Every principal one statement's `Principal` element names, in the forms the
/// policy grammar writes them: the element is either one string (`*`) or a map
/// from principal type to one value or a list of values.
fn policy_principals(principal: &serde_json::Value) -> Vec<&str> {
    match principal {
        serde_json::Value::String(value) => vec![value.as_str()],
        serde_json::Value::Object(by_type) => by_type
            .values()
            .flat_map(|value| match value {
                serde_json::Value::Array(items) => {
                    items.iter().filter_map(serde_json::Value::as_str).collect()
                }
                single => single.as_str().into_iter().collect::<Vec<&str>>(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The account one principal value names: the account field of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or the value
/// itself when it is a bare account id.
///
/// `None` for anything that names no single account — `*`, or a principal of
/// another kind such as a service — and none of those is the owning account.
fn principal_account(value: &str) -> Option<&str> {
    if let Some(rest) = value.strip_prefix("arn:") {
        return rest.split(':').nth(3).filter(|account| !account.is_empty());
    }
    if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
        return Some(value);
    }
    None
}

/// Whether `policy` grants anything to a principal other than `owner_account`,
/// the account that owns the resource the policy would be attached to.
///
/// A resource policy on a Resolver resource is how it is shared with ANOTHER
/// account, so a document whose principals are all the owning account — or that
/// names no principal at all — shares the resource with nobody and leaves it
/// with no policy. Read with [`policy_principals`] and [`principal_account`], so
/// an account named by its id and the same account named by an ARN carrying it
/// count as the one account.
pub fn policy_shares_with_other_account(policy: &str, owner_account: &str) -> bool {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(policy) else {
        return false;
    };
    policy_statements(&document).iter().any(|statement| {
        statement
            .get("Principal")
            .map(policy_principals)
            .unwrap_or_default()
            .iter()
            .any(|value| principal_account(value) != Some(owner_account))
    })
}

/// Canonicalise the domain name a Resolver rule selects on to a fully-qualified
/// DNS name: the name followed by exactly one root label, written as a trailing
/// dot (RFC 1035 §3.1 — the final zero-length label of a name relative to the
/// root).
///
/// Resolver reports a rule's `DomainName` in that canonical form whatever form
/// the request wrote it in, so `example.com` and `example.com.` name one domain
/// and a read of either rule reports the same member. The modelled `DomainName`
/// length (1..256) leaves room for the root label on a name at the 255-octet DNS
/// limit.
pub fn canonical_domain_name(domain_name: &str) -> String {
    let relative = domain_name.trim_end_matches('.');
    if relative.is_empty() {
        // Nothing but root labels names the root itself.
        ".".to_string()
    } else {
        format!("{relative}.")
    }
}

/// Whether `name` satisfies the model's `Name` pattern,
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`
/// (`smithy_query(query="shape",shape="Name")`): at least one character, every
/// character a letter, a digit, a hyphen, an underscore, an apostrophe or a
/// space, and not digits alone.
///
/// The pattern's negative lookahead is outside the syntax the boundary
/// validator's regex engine accepts, so a name the pattern excludes reaches a
/// handler; the operations that accept a `Name` refuse it here instead, with the
/// client error each of them models.
pub fn name_satisfies_pattern(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '\'' | ' '))
        && !name.chars().all(|c| c.is_ascii_digit())
}

/// The expression a refusal QUOTES for `Name`: the model's pattern
/// (`smithy_query(query="shape",shape="Name")` reports
/// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$`) WITHOUT the `^`/`$` pair the model
/// wraps it in.
///
/// The pair is dropped because the recorded refusal of a digits-only `Name`
/// quotes the expression inside it: those anchors are how a model turns a find
/// into a whole-value match, and what the service publishes is the expression a
/// value has to satisfy — so a client that lifts it out of the message and
/// re-validates locally gets the one the service advertises. The CHECK is made
/// against the whole pattern, by [`name_satisfies_pattern`]; this constant is
/// only what the message states.
pub const NAME_PATTERN_AS_STATED: &str = r"(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)";

/// The message a `Name` the model's pattern excludes is refused with.
///
/// A name the pattern excludes violates a MODELLED constraint, so the sentence
/// is the framework's own for that class rather than one worded by hand: the
/// violation count, the rejected value, the member path in the lowerCamel
/// spelling every path of this emulator uses, and the expression the value had
/// to satisfy. Only that expression is this service's own
/// ([`NAME_PATTERN_AS_STATED`]), so a name refused here differs from one the
/// boundary validator refuses in the clause's requirement half alone.
pub fn name_pattern_violation(name: &str) -> String {
    let violation = ::aws_smithy_schema_server::constraints::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(NAME_MEMBER),
        kind: ::aws_smithy_schema_server::constraints::ViolationKind::Pattern(
            NAME_PATTERN_AS_STATED.to_string(),
        ),
        // `Name`'s modelled length (0..64) is checked before a handler runs, so
        // the value echoed here is already bounded.
        value: Some(name.to_string()),
        sensitive: false,
    };
    ::aws_smithy_schema_server::constraints::envelope(std::slice::from_ref(&violation))
        .unwrap_or_else(|| String::from("Name is outside the pattern the model declares for it"))
}

/// The modelled member name the `Name` refusal states, as the model spells it.
const NAME_MEMBER: &str = "Name";

/// The `ResourceNotFoundException` the firewall domain-list operations answer
/// for a list the caller cannot address, built here so every operation that
/// resolves a `FirewallDomainListId` reports the same body.
///
/// It is the answer both for an id this account created no list under and for
/// one that names a list DNS Firewall publishes itself on a path that would
/// WRITE it ([`managed_firewall_domain_list`]): a published list is the
/// service's own, so nothing this account does changes it and it is not visible
/// as a writable resource at all. Reporting one body for both is what makes
/// those two indistinguishable, as [`resource_not_found_message`] describes.
pub fn firewall_domain_list_not_found(
    firewall_domain_list_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(firewall_domain_list_id)),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_DOMAIN_LIST.to_string()),
    }
}

/// The `ConflictException` a DNS Firewall delete answers for a resource whose
/// last change has not settled yet, built here so the deletes that refuse one
/// report the same body.
///
/// The sentence is the one the model states for this error class
/// (`smithy_query(query="shape",shape="ConflictException")`): "The requested
/// state transition isn't valid. For example, you can't delete a firewall
/// domain list if it is in the process of being deleted". It names no resource
/// and no status, so nothing is interpolated into it.
pub fn state_transition_conflict() -> ConflictException {
    ConflictException {
        message: Some("The requested state transition isn't valid.".to_string()),
    }
}

/// The Route 53 Resolver service error code reported when a call that would
/// change a DNS Firewall domain list arrives while the list's last change is
/// still running.
///
/// Carried for the same reason as [`RSLVR_CODE_ENDPOINT_NOT_FOUND`]: the HTTP
/// status (400) and the exception shape name together do not say which Resolver
/// fault occurred, and the model states no message text, so this code comes
/// from the recorded refusal of a domain-list write issued while an earlier one
/// was still in flight. See [`firewall_domain_list_in_progress_conflict`].
pub const RSLVR_CODE_FIREWALL_DOMAIN_LIST_IN_PROGRESS: &str = "RSLVR-02305";

/// The `ConflictException` a DNS Firewall domain-list operation answers for a
/// list whose last change has not settled yet, built here so every operation
/// that refuses one reports the same body.
///
/// The refusal states the step the list stands in rather than the call that was
/// refused: `status` is the list's own `FirewallDomainListStatus` as of now, so
/// the sentence names the work the caller has to wait out and quotes the list
/// the request named, under the RSLVR code the service states for this fault
/// ([`RSLVR_CODE_FIREWALL_DOMAIN_LIST_IN_PROGRESS`]). The variable parts are the
/// list's step and the id from the request; everything else is fixed text.
///
/// A status a list has REACHED carries no work to wait out
/// ([`firewall_domain_list_status_is_settled`]), and no caller builds this for
/// one, so such a status falls back to the sentence the model states for the
/// error class ([`state_transition_conflict`]).
pub fn firewall_domain_list_in_progress_conflict(
    firewall_domain_list_id: &str,
    status: &str,
) -> ConflictException {
    // The work each in-flight step of a domain list is carrying out, named as
    // the sentence's remediation clause names it.
    let work = match status {
        FIREWALL_DOMAIN_LIST_STATUS_IMPORTING => "import",
        FIREWALL_DOMAIN_LIST_STATUS_UPDATING => "update",
        FIREWALL_DOMAIN_LIST_STATUS_DELETING => "delete",
        _ => return state_transition_conflict(),
    };
    let step = status.to_ascii_lowercase();
    ConflictException {
        message: Some(rslvr_message_sentences(
            RSLVR_CODE_FIREWALL_DOMAIN_LIST_IN_PROGRESS,
            &[
                &format!(
                    "The DNS Firewall domain list is in {step} status: \
                     \"{firewall_domain_list_id}\""
                ),
                &format!("Try again after the {work} completes"),
            ],
        )),
    }
}

/// The `ResourceNotFoundException` the firewall rule-group operations answer for
/// a rule group the caller cannot address, built here so every operation that
/// resolves a `FirewallRuleGroupId` reports the same body.
///
/// It carries the same RSLVR-coded sentence as every other resource of this
/// service a caller names but cannot address
/// ([`resource_not_found_message`]), with the rule group's modelled shape name
/// as the resource type.
///
/// `firewall_rule_group` is how the request addressed the group — its
/// `FirewallRuleGroupId`, or the `Arn` the policy operations take — and the
/// sentence names the bare id either way.
pub fn firewall_rule_group_not_found(firewall_rule_group: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(resource_not_found_message(firewall_rule_group)),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE_GROUP.to_string()),
    }
}

/// The `ResourceNotFoundException` the rule-group association operations answer
/// for an association the caller cannot address, built here so every operation
/// that resolves a `FirewallRuleGroupAssociationId` reports the same body — the
/// same relation [`firewall_rule_group_not_found`] has to the rule-group
/// operations.
///
/// The sentence closes with the trace clause of the request
/// ([`traced_message`]), which is how the service reports this fault.
pub fn firewall_rule_group_association_not_found(
    firewall_rule_group_association_id: &str,
    trace_root: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(traced_message(
            resource_not_found_message(firewall_rule_group_association_id),
            trace_root,
        )),
        resource_type: Some(RESOURCE_TYPE_FIREWALL_RULE_GROUP_ASSOCIATION.to_string()),
    }
}

/// The `ValidationException` a write to a mutation-protected association is
/// refused with.
///
/// `MutationProtection`'s doc states what the setting does: "If enabled, this
/// setting disallows modification or removal of the association, to help prevent
/// against accidentally altering DNS firewall protections." The refusal is the
/// RSLVR-coded sentence the service states for that fault
/// ([`RSLVR_CODE_ASSOCIATION_MUTATION_PROTECTED`]): it names neither the
/// association nor the member that was to change, so nothing is interpolated
/// into it, and the update and the disassociate report the same body — closed,
/// as this slice's refusals are, with the trace clause of the request
/// ([`traced_message`]).
pub fn mutation_protection_forbids_change(trace_root: &str) -> ValidationException {
    ValidationException {
        message: Some(traced_message(
            rslvr_message_sentences(
                RSLVR_CODE_ASSOCIATION_MUTATION_PROTECTED,
                &[
                    "The DNS Firewall rule group association is under mutation protection",
                    "Remove the mutation protection and try again",
                ],
            ),
            trace_root,
        )),
    }
}

/// The `ValidationException` the associate and the update of a rule-group
/// association are refused with when the `Priority` they ask for is one another
/// association of the same VPC already holds.
///
/// `AssociateFirewallRuleGroup$Priority`'s doc states the rule: "You must
/// specify a unique priority for each rule group that you associate with a
/// single VPC." The refusal is the RSLVR-coded sentence the service states for
/// that fault ([`RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE`]), which names the
/// association that already holds the priority — read out of this store, so the
/// caller can address the one it clashed with — and asks for another number,
/// closed with the trace clause of the request ([`traced_message`]).
pub fn association_priority_in_use(
    holding_firewall_rule_group_association_id: &str,
    trace_root: &str,
) -> ValidationException {
    ValidationException {
        message: Some(traced_message(
            rslvr_message(
                RSLVR_CODE_ASSOCIATION_PRIORITY_IN_USE,
                &format!(
                    "This priority is the same with another association: \
                     \"{holding_firewall_rule_group_association_id}\" . \
                     Try again with another priority number"
                ),
            ),
            trace_root,
        )),
    }
}

/// The `ValidationException` a firewall-rule create is refused with when the
/// `Priority` it asks for is one another rule of the same rule group already
/// holds.
///
/// `CreateFirewallRule$Priority`'s doc states the rule: "You must specify a
/// unique priority for each rule in a rule group." The priority is unique over
/// the whole GROUP and over nothing narrower — no other member of the rule (the
/// query type it evaluates, its action, its match source) scopes that
/// uniqueness — so the refusal states the fault with the service's code for it
/// ([`RSLVR_CODE_RULE_PRIORITY_IN_USE`]), names the rule group the clash is in,
/// and asks for another number.
pub fn firewall_rule_priority_in_use(firewall_rule_group_id: &str) -> ValidationException {
    ValidationException {
        message: Some(rslvr_message_sentences(
            RSLVR_CODE_RULE_PRIORITY_IN_USE,
            &[
                &format!(
                    "The DNS Firewall rule group already has a rule associated with the same \
                     priority: \"{firewall_rule_group_id}\""
                ),
                "Provide another priority",
            ],
        )),
    }
}

/// The `ConflictException` `DeleteFirewallRuleGroup` is refused with while the
/// group it names still has a rule in it.
///
/// A rule group that still holds rules still filters DNS traffic, so deleting
/// it is not a state transition the service makes: the rules go first, which is
/// the teardown order `DeleteFirewallRule`/`DeleteFirewallRuleGroup` are meant
/// to be called in, and the model's `ConflictException` ("the requested state
/// transition isn't valid") is the answer for a transition it will not make.
/// The refusal is the RSLVR-coded sentence the service states for that fault
/// ([`RSLVR_CODE_FIREWALL_RULE_GROUP_HAS_RULE`]): it names the rule group from
/// the request and tells the caller to delete the rule first. The second
/// sentence carries the extra space the service writes between the two, as
/// [`association_priority_in_use`]'s does.
pub fn firewall_rule_group_has_rule(firewall_rule_group_id: &str) -> ConflictException {
    ConflictException {
        message: Some(rslvr_message_sentences(
            RSLVR_CODE_FIREWALL_RULE_GROUP_HAS_RULE,
            &[
                &format!(
                    "The DNS Firewall rule group has an associated rule: \
                     \"{firewall_rule_group_id}\""
                ),
                " Delete the rule and try again later",
            ],
        )),
    }
}

/// The canonical spelling of a DNS name a DNS Firewall resource HOLDS: the
/// fully qualified name, rooted with exactly one trailing dot and case folded.
///
/// Rooting is the ONE canonicalization this service already applies to the
/// domain name a Resolver rule selects on ([`canonical_domain_name`]) — a domain
/// name is written relative to the root, and the root is written as a trailing
/// dot (RFC 1035 §3.1). Case folding is the other half of the same reduction:
/// DNS labels are compared without regard to case (RFC 4343 §1), and the ASCII
/// letters are the only ones that case folding applies to — a punycode/ACE
/// label is already in the form it is compared in.
///
/// So a name written `Foo.test` and a name written `foo.test.` are ONE name:
/// whichever a request spells, what is stored and what every reader projects is
/// `foo.test.`, and writing the canonical form again changes nothing because
/// this reduction is IDEMPOTENT. That is what makes two spellings of one domain
/// one entry of a domain list rather than two, and what makes the name a rule
/// reports for its block override the name the caller can compare against.
pub fn firewall_domain_canonical(domain: &str) -> String {
    canonical_domain_name(domain).to_ascii_lowercase()
}

/// Whether `domain` satisfies the requirements
/// `UpdateFirewallDomains$Domains`'s doc states for "each domain specification
/// in your domain list": "It can optionally start with * (asterisk). With the
/// exception of the optional starting asterisk, it must only contain the
/// following characters: A-Z, a-z, 0-9, - (hyphen)."
///
/// The doc's character list omits the two separators a domain name is written
/// with, which its own wildcard form (`*.example.com`) requires, so a name is
/// read as DNS labels: the optional leading asterisk, then labels of the
/// documented characters (plus the underscore of an underscored service name,
/// RFC 8552 §1) separated by dots, each label non-empty and within the 63
/// octets RFC 1035 §2.3.4 allows, optionally ending in the root label written
/// as a trailing dot (RFC 1035 §3.1). The length is the model's own
/// (`FirewallDomainName`, 1..255), already enforced before a handler runs.
pub fn firewall_domain_is_wellformed(domain: &str) -> bool {
    let name = match domain.strip_prefix('*') {
        Some(rest) => rest.strip_prefix('.').unwrap_or(rest),
        None => domain,
    };
    let labels = name.strip_suffix('.').unwrap_or(name);
    !labels.is_empty()
        && labels.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
}

/// The `ManagedOwnerName` of every domain list DNS Firewall itself publishes.
///
/// `FirewallDomainList$ManagedOwnerName`'s doc states it: "the managed domain
/// list `AWSManagedDomainsMalwareDomainList` has the managed owner name
/// `Route 53 Resolver DNS Firewall`". It fits `ServicePrinciple` (length
/// 1..512).
pub const MANAGED_FIREWALL_DOMAIN_LIST_OWNER: &str = "Route 53 Resolver DNS Firewall";

/// `DomainListType` values, the model's whole enum
/// (`smithy_query(query="shape",shape="DomainListType")`).
pub const DOMAIN_LIST_TYPE_THREAT: &str = "THREAT";
pub const DOMAIN_LIST_TYPE_CONTENT: &str = "CONTENT";

/// One domain list DNS Firewall publishes itself, rather than one this account
/// created.
///
/// A managed list belongs to the service, not to the account that reads it: it
/// is visible to every account in every region, no request created it, and
/// nothing an account does changes it. So it is NOT a stored record — it is a
/// published catalog entry, and every member a response reports for it is
/// computed from this entry plus the region the request was signed for.
#[derive(Debug, Clone, Copy)]
pub struct ManagedFirewallDomainList {
    /// The list's `Name`, which is also how AWS publishes it.
    pub name: &'static str,
    /// The `ManagedListType` this list reports: "the type of the managed domain
    /// list, for example `THREAT`".
    pub managed_list_type: &'static str,
    /// The `Category` this list reports: "the category of the domain list" —
    /// what the threat intelligence inside it is about, which is published with
    /// the list and is the same for every account that reads it.
    pub category: &'static str,
}

/// The domain lists DNS Firewall publishes to every account, in the order
/// `ListFirewallDomainLists` reports them.
///
/// The names are the ones AWS publishes for DNS Firewall's managed domain
/// lists, the first of which the model itself names in
/// `FirewallDomainList$ManagedOwnerName`'s doc, each with the category AWS
/// publishes beside it. All four are threat lists, so each reports
/// [`DOMAIN_LIST_TYPE_THREAT`]; `CONTENT` ([`DOMAIN_LIST_TYPE_CONTENT`]) is the
/// other value the model's enum carries, which no published list of this
/// service uses.
pub const MANAGED_FIREWALL_DOMAIN_LISTS: &[ManagedFirewallDomainList] = &[
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsMalwareDomainList",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
        category: "Malware",
    },
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsBotnetCommandandControl",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
        category: "Botnet/Command and Control",
    },
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsAggregateThreatList",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
        category: "Aggregate Threat List",
    },
    ManagedFirewallDomainList {
        name: "AWSManagedDomainsAmazonGuardDutyThreatList",
        managed_list_type: DOMAIN_LIST_TYPE_THREAT,
        category: "Amazon GuardDuty Threat List",
    },
];

impl ManagedFirewallDomainList {
    /// This list's `Id`, derived from its name so that every read of it — in any
    /// process, in any region — reports the same id, exactly as a published
    /// resource must. The form is [`mint_firewall_domain_list_id`]'s
    /// (`rslvr-fdl-<hex>`), so an id is recognisable as a domain list's and fits
    /// `ResourceId` (length 1..64).
    pub fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"managed-firewall-domain-list/v1");
        feed_field(&mut hasher, self.name.as_bytes());
        format!("rslvr-fdl-{}", hex::encode(&hasher.finalize()[..8]))
    }

    /// This list's `Arn`, rooted in the region the request was signed for, as
    /// every ARN this service reports is.
    pub fn arn(&self, region: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("firewall-domain-list/{}", self.id()),
        )
    }

    /// Project the catalog entry onto the modelled `FirewallDomainListMetadata`
    /// `ListFirewallDomainLists` reports.
    pub fn to_metadata(&self, region: &str) -> FirewallDomainListMetadata {
        FirewallDomainListMetadata {
            id: Some(self.id()),
            arn: Some(self.arn(region)),
            name: Some(self.name.to_string()),
            // `CreatorRequestId` is "a unique string defined by you to identify
            // the request": the request that created a published list is the
            // service's own, and what DNS Firewall identifies it by is the
            // list's published name, which is what every account is served for
            // it.
            creator_request_id: Some(self.name.to_string()),
            managed_owner_name: Some(MANAGED_FIREWALL_DOMAIN_LIST_OWNER.to_string()),
            managed_list_type: Some(self.managed_list_type.to_string()),
            category: Some(self.category.to_string()),
        }
    }

    /// Project the catalog entry onto the modelled `FirewallDomainList`
    /// `GetFirewallDomainList` reports, so a read of an id
    /// `ListFirewallDomainLists` reported resolves to the same list.
    pub fn to_domain_list(&self, region: &str) -> FirewallDomainList {
        FirewallDomainList {
            id: Some(self.id()),
            arn: Some(self.arn(region)),
            name: Some(self.name.to_string()),
            // `DomainCount` is "the number of domain names that are specified
            // in the domain list": the domains of a published list are the
            // service's own and are held nowhere here
            // ([`managed_firewall_domain_list_access_denied`] is what a request
            // for them is answered with), so the count this reports is the
            // count this store holds for the list — none — exactly as a stored
            // record's count is derived from its own domains
            // ([`FirewallDomainListRecord::domain_count`]) rather than claimed.
            domain_count: Some(0),
            status: Some(FIREWALL_DOMAIN_LIST_STATUS_COMPLETE.to_string()),
            // The published list stands in the same settled step a list of this
            // account stands in, and `StatusMessage` states that step
            // ([`firewall_domain_list_status_message`]).
            status_message: firewall_domain_list_status_message(
                FIREWALL_DOMAIN_LIST_STATUS_COMPLETE,
                // Nothing this account did changed a published list, so the
                // step it stands in is the one it was published in.
                FirewallChange::Created,
            ),
            managed_owner_name: Some(MANAGED_FIREWALL_DOMAIN_LIST_OWNER.to_string()),
            // The published list is identified by its own name, exactly as
            // [`Self::to_metadata`] reports it; `CreationTime`/`ModificationTime`
            // are unknown — no call of this account created or last changed the
            // list — so neither is invented.
            creator_request_id: Some(self.name.to_string()),
            creation_time: None,
            modification_time: None,
            category: Some(self.category.to_string()),
            managed_list_type: Some(self.managed_list_type.to_string()),
        }
    }
}

/// The published domain list this `FirewallDomainListId` names, or `None` for an
/// id that names none.
///
/// Every operation that resolves a `FirewallDomainListId` consults this after
/// the store, so one id means one list throughout the service rather than
/// resolving in the operation that lists them and nowhere else.
pub fn managed_firewall_domain_list(id: &str) -> Option<&'static ManagedFirewallDomainList> {
    MANAGED_FIREWALL_DOMAIN_LISTS
        .iter()
        .find(|managed| managed.id() == id)
}

/// The `AccessDeniedException` `ListFirewallDomains` answers for a list DNS
/// Firewall publishes itself.
///
/// The domains of a published list are the service's own threat intelligence,
/// not this account's data: the list is readable as a resource
/// ([`ManagedFirewallDomainList::to_domain_list`]) while the domains inside it
/// are not readable at all, which the model's `AccessDeniedException` ("the
/// current account doesn't have the IAM permissions required to perform the
/// specified Resolver operation") is the modelled answer for. The sentence
/// states the fault and names the kind of list it is about — the owner every
/// published list reports ([`MANAGED_FIREWALL_DOMAIN_LIST_OWNER`]) — and
/// carries nothing from the request.
pub fn managed_firewall_domain_list_access_denied() -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "You don't have a permission to list the domains inside a \
             {MANAGED_FIREWALL_DOMAIN_LIST_OWNER} managed list."
        )),
    }
}

/// A firewall domain list, as stored.
///
/// `DomainCount` is DERIVED from [`Self::domains`] and never held separately, so
/// the count and the domains cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct FirewallDomainListRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub creator_request_id: String,
    /// The domain names this list holds, in the order they were added. A list
    /// is "created empty" and is populated by `ImportFirewallDomains` or
    /// `UpdateFirewallDomains`.
    pub domains: Vec<String>,
    /// The `FirewallDomainListStatus` this list reports; see
    /// [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] on why a created list is
    /// already settled. The `StatusMessage` that goes with it is DERIVED from
    /// it ([`Self::status_message_at`]), so it is never held beside it.
    pub status: String,
    /// `CreateFirewallDomainList$Tags`, kept with the record so they are not
    /// silently discarded. `FirewallDomainList` models no `Tags` member, so no
    /// response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`Self::settled_status`] is owed, or `None` for a list with no
    /// transition in flight (one nobody has changed since it was created
    /// empty).
    pub settled_at: Option<Instant>,
    /// The `FirewallDomainListStatus` the clock owes this list, reported from
    /// [`Self::settled_at`] onwards, or `None` when that step is the settled
    /// [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] every change but a failed
    /// import ends in.
    pub settled_status: Option<String>,
    /// The `StatusMessage` the settled step states, for the one step whose
    /// sentence is not derived from the status alone because it carries the
    /// Region of the request that started it
    /// ([`firewall_domain_list_import_download_failed`]); `None` leaves the
    /// message derived from the status, as every other step's is.
    pub settled_status_message: Option<String>,
    /// What last changed this list, which is the step its `StatusMessage`
    /// states once it stands settled again ([`Self::status_message_at`]): a
    /// list nobody has written since the create states its creation, and one
    /// whose domains a caller wrote states that the change finished.
    pub change: FirewallChange,
}

impl FirewallDomainListRecord {
    /// `DomainCount`: "The number of domain names that are specified in the
    /// domain list."
    pub fn domain_count(&self) -> i32 {
        i32::try_from(self.domains.len()).unwrap_or(i32::MAX)
    }

    /// This list's `FirewallDomainListStatus` as of `now`: the status it holds
    /// until the transition the clock owes falls due, [`Self::settled_status`]
    /// — [`FIREWALL_DOMAIN_LIST_STATUS_COMPLETE`] unless the change in flight
    /// owes another step — after it. The same shape as
    /// [`FirewallRuleGroupRecord::status_at`], so a list the clock owes nothing
    /// reports the step it stands in.
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => self
                .settled_status
                .as_deref()
                .unwrap_or(FIREWALL_DOMAIN_LIST_STATUS_COMPLETE),
            None => &self.status,
        }
    }

    /// This list's `StatusMessage` as of `now`: the sentence the lifecycle step
    /// it reports then ([`Self::status_at`]) leaves on it — the one the step
    /// that settled it stated, where that sentence carries more than the status
    /// ([`Self::settled_status_message`]).
    pub fn status_message_at(&self, now: Instant) -> Option<String> {
        let settled = self.settled_at.is_some_and(|due| now >= due);
        if settled {
            if let Some(message) = &self.settled_status_message {
                return Some(message.clone());
            }
        }
        firewall_domain_list_status_message(self.status_at(now), self.change)
    }

    /// Project the record onto the modelled `FirewallDomainList`.
    pub fn to_domain_list(&self, now: Instant) -> FirewallDomainList {
        FirewallDomainList {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            domain_count: Some(self.domain_count()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message_at(now),
            // `ManagedOwnerName`, `Category` and `ManagedListType` describe "the
            // managed domain list" kinds AWS itself publishes; a list this
            // account created is not one, so all three stay unset.
            managed_owner_name: None,
            creator_request_id: Some(self.creator_request_id.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            category: None,
            managed_list_type: None,
        }
    }

    /// Project the record onto the modelled `FirewallDomainListMetadata`, the
    /// "minimal high-level information for a firewall domain list"
    /// `ListFirewallDomainLists` reports.
    ///
    /// It is the same record [`Self::to_domain_list`] projects, narrowed to the
    /// members `FirewallDomainListMetadata` declares — the metadata shape
    /// carries no `DomainCount`, `Status`, `StatusMessage` or timestamps, which
    /// is why the doc says to "call GetFirewallDomainList and
    /// ListFirewallDomains" for the full information.
    pub fn to_metadata(&self) -> FirewallDomainListMetadata {
        FirewallDomainListMetadata {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            // `ManagedOwnerName` is "used only for lists that are NOT managed by
            // you", and `Category`/`ManagedListType` describe such a list, so
            // all three stay unset for a list this account created — exactly as
            // in [`Self::to_domain_list`].
            managed_owner_name: None,
            managed_list_type: None,
            category: None,
        }
    }
}

/// A DNS Firewall rule group, as stored.
///
/// `RuleCount` is DERIVED from [`Self::rules`] and never held separately — the
/// same way [`FirewallDomainListRecord::domain_count`] derives from its domains
/// — so the count and the rules cannot disagree.
#[derive(Debug, Clone, Default)]
pub struct FirewallRuleGroupRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    /// "The Amazon Web Services account ID for the account that created the rule
    /// group."
    pub owner_id: String,
    pub creator_request_id: String,
    /// The `ShareStatus` this rule group reports; the emulator has one account,
    /// so it is [`SHARE_STATUS_NOT_SHARED`] until a resource policy shares it.
    pub share_status: String,
    /// The rules this group filters DNS traffic with, in the order they were
    /// added — a group is created empty ("you can add rules to the new rule
    /// group by calling CreateFirewallRule"), so the rule operations are what
    /// populate it.
    pub rules: Vec<FirewallRuleRecord>,
    /// The `FirewallRuleGroupStatus` this group reports, as of
    /// [`Self::settled_at`]. The `StatusMessage` that goes with it is DERIVED
    /// from it ([`Self::status_message_at`]), so it is never held beside it.
    pub status: String,
    /// `CreateFirewallRuleGroup$Tags`, kept with the record so they are not
    /// silently discarded. `FirewallRuleGroup` models no `Tags` member, so no
    /// response reports them: `ListTagsForResource` is a separate operation.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`FIREWALL_RULE_GROUP_STATUS_COMPLETE`] is owed, or `None` for a group
    /// with no transition in flight (one nobody has changed since it was
    /// created empty).
    pub settled_at: Option<Instant>,
    /// The resource policy attached to this rule group, as the last
    /// `PutFirewallRuleGroupPolicy` for it wrote it, and `None` while no policy
    /// has been written — the same relation [`ResolverRuleRecord`] keeps with
    /// its own policy.
    ///
    /// It is held ON the record, and not in a map beside it, because a resource
    /// policy belongs to the resource it shares: a read of the group's policy
    /// resolves through the same record every other rule-group operation reads,
    /// and a group the store no longer holds has no policy to report.
    pub firewall_rule_group_policy: Option<String>,
    /// What last changed this group, which is the step its `StatusMessage`
    /// states once it stands settled again ([`Self::status_message_at`]) — the
    /// rule-group twin of [`FirewallDomainListRecord::change`]: a group no rule
    /// write has touched since the create states its creation, and one whose
    /// rules a caller wrote states that the change finished.
    pub change: FirewallChange,
}

impl FirewallRuleGroupRecord {
    /// `RuleCount`: "The number of rules in the rule group."
    pub fn rule_count(&self) -> i32 {
        i32::try_from(self.rules.len()).unwrap_or(i32::MAX)
    }

    /// This group's `FirewallRuleGroupStatus` as of `now`: the transient status
    /// until the transition the clock owes falls due,
    /// [`FIREWALL_RULE_GROUP_STATUS_COMPLETE`] after it — the same shape as
    /// [`FirewallDomainListRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => FIREWALL_RULE_GROUP_STATUS_COMPLETE,
            None => &self.status,
        }
    }

    /// This group's `StatusMessage` as of `now`: the sentence the lifecycle
    /// step it reports then ([`Self::status_at`]) leaves on it — the
    /// rule-group twin of [`FirewallDomainListRecord::status_message_at`].
    pub fn status_message_at(&self, now: Instant) -> Option<String> {
        firewall_rule_group_status_message(self.status_at(now), self.change)
    }

    /// Project the record onto the modelled `FirewallRuleGroup`.
    pub fn to_rule_group(&self, now: Instant) -> FirewallRuleGroup {
        FirewallRuleGroup {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            rule_count: Some(self.rule_count()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message_at(now),
            owner_id: Some(self.owner_id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            share_status: Some(self.share_status.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }

    /// Project the record onto the modelled `FirewallRuleGroupMetadata`, the
    /// "minimal high-level information for a firewall rule group"
    /// `ListFirewallRuleGroups` reports.
    ///
    /// It is the same record [`Self::to_rule_group`] projects, narrowed to the
    /// members `FirewallRuleGroupMetadata` declares
    /// (`smithy_query(query="shape",shape="FirewallRuleGroupMetadata")`): the
    /// metadata shape carries no `RuleCount`, `Status`, `StatusMessage` or
    /// timestamps, which is why its doc says to "call GetFirewallRuleGroup and
    /// ListFirewallRules" for the full information.
    pub fn to_metadata(&self) -> FirewallRuleGroupMetadata {
        FirewallRuleGroupMetadata {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            owner_id: Some(self.owner_id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            share_status: Some(self.share_status.clone()),
        }
    }
}

/// An association between a DNS Firewall rule group and a VPC, as stored.
///
/// The association is a resource of its own — `AssociateFirewallRuleGroup` mints
/// an id for it and `GetFirewallRuleGroupAssociation` resolves that id — so it is
/// held in its own collection rather than on either of the two resources it
/// joins, exactly as [`ResolverRuleAssociationRecord`] is for a Resolver rule and
/// a VPC.
#[derive(Debug, Clone, Default)]
pub struct FirewallRuleGroupAssociationRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub firewall_rule_group_id: String,
    pub vpc_id: String,
    pub name: String,
    /// "The setting that determines the processing order of the rule group among
    /// the rule groups that are associated with a single VPC."
    pub priority: i32,
    /// One of [`crate::ops::types::MUTATION_PROTECTION_STATUS_VALUES`].
    pub mutation_protection: String,
    pub creator_request_id: String,
    /// The `FirewallRuleGroupAssociationStatus` this association reports, as of
    /// [`Self::settled_at`].
    pub status: String,
    pub status_message: Option<String>,
    /// `AssociateFirewallRuleGroup$Tags`, kept with the record so they are not
    /// silently discarded; `FirewallRuleGroupAssociation` models no `Tags`
    /// member, so they are reported by `ListTagsForResource` alone.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move from the transient [`Self::status`] to
    /// [`FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE`] is owed, or `None` for
    /// an association with no transition in flight.
    pub settled_at: Option<Instant>,
}

impl FirewallRuleGroupAssociationRecord {
    /// This association's `FirewallRuleGroupAssociationStatus` as of `now`: the
    /// transient status until the transition the clock owes falls due,
    /// [`FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE`] after it — the same
    /// shape as [`FirewallRuleGroupRecord::status_at`].
    pub fn status_at(&self, now: Instant) -> &str {
        match self.settled_at {
            Some(due) if now < due => &self.status,
            Some(_) => FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE,
            None => &self.status,
        }
    }

    /// Project the record onto the modelled `FirewallRuleGroupAssociation`.
    pub fn to_association(&self, now: Instant) -> FirewallRuleGroupAssociation {
        FirewallRuleGroupAssociation {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            firewall_rule_group_id: Some(self.firewall_rule_group_id.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            name: Some(self.name.clone()),
            priority: Some(self.priority),
            mutation_protection: Some(self.mutation_protection.clone()),
            // `ManagedOwnerName` is "the owner of the association, used only for
            // associations that are NOT managed by you"; an association this
            // account made is its own, so it stays unset.
            managed_owner_name: None,
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creator_request_id: Some(self.creator_request_id.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }
}

/// The DNS Firewall configuration of ONE VPC, as stored.
///
/// A VPC's firewall configuration is a singleton the service maintains itself —
/// there is no operation that creates or deletes one — so a record exists only
/// for a VPC whose configuration was moved off its default; every other VPC
/// reports the default one [`ComAmazonawsRoute53resolverRoute53resolverState::firewall_config`]
/// derives. `Id` and `OwnerId` are therefore the same whether the record exists
/// or not, so a VPC's configuration does not change identity when it is updated.
#[derive(Debug, Clone)]
pub struct FirewallConfigRecord {
    /// Creation order, for the list operation's pages (the same role as
    /// [`FirewallDomainListRecord::seq`]).
    ///
    /// `0` on the default configuration
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::firewall_config`]
    /// derives for a VPC the store holds no record for: the id sequence starts
    /// at 1, so a stored record never carries it.
    pub seq: u64,
    /// The `Id` of this configuration, [`firewall_config_id`] of its VPC.
    pub id: String,
    /// The `ResourceId`: "The ID of the VPC that this firewall configuration
    /// applies to."
    pub resource_id: String,
    /// "The Amazon Web Services account ID of the owner of the VPC."
    pub owner_id: String,
    /// One of [`crate::ops::types::FIREWALL_FAIL_OPEN_STATUS_VALUES`].
    pub firewall_fail_open: String,
}

impl FirewallConfigRecord {
    /// The record as the modelled `FirewallConfig` shape the firewall
    /// configuration operations report.
    pub fn to_firewall_config(&self) -> FirewallConfig {
        FirewallConfig {
            id: Some(self.id.clone()),
            resource_id: Some(self.resource_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            firewall_fail_open: Some(self.firewall_fail_open.clone()),
        }
    }
}

/// One IP address of a Resolver endpoint, as stored.
#[derive(Debug, Clone)]
pub struct IpAddressRecord {
    /// The emulator-minted `IpId` (`rni-<16 hex>`).
    pub ip_id: String,
    pub subnet_id: String,
    pub ip: Option<String>,
    pub ipv6: Option<String>,
    /// The elastic network interface of the account's Amazon EC2 inventory that
    /// holds [`Self::ip`] ([`place_endpoint_address`]), so the interface can be
    /// given back when the endpoint gives the address up
    /// ([`release_endpoint_addresses`]). `None` for an address no interface
    /// could be created for.
    pub network_interface_id: Option<String>,
    /// The transient `IpAddressStatus` this IP reports until [`Self::attached_at`]
    /// falls due.
    pub status: String,
    pub status_message: Option<String>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `ATTACHED` is owed.
    pub attached_at: Instant,
}

impl IpAddressRecord {
    /// This IP's `IpAddressStatus` as of `now`: the transient status until the
    /// attach the clock owes falls due, `ATTACHED` after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.attached_at {
            &self.status
        } else {
            IP_STATUS_ATTACHED
        }
    }

    /// Whether this is the address an `IpAddressUpdate` NAMES, as
    /// `DisassociateResolverEndpointIpAddress` reads one: its `IpId` identifies
    /// an address on its own, and otherwise the address literal does, narrowed
    /// by `SubnetId` when the request carries one. A request that carries only a
    /// subnet names the address stored in that subnet.
    pub fn is_named_by(&self, requested: &IpAddressUpdate) -> bool {
        if let Some(ip_id) = &requested.ip_id {
            return &self.ip_id == ip_id;
        }
        let subnet_matches = requested
            .subnet_id
            .as_ref()
            .is_none_or(|subnet_id| &self.subnet_id == subnet_id);
        if let Some(ip) = &requested.ip {
            return subnet_matches && self.ip.as_ref() == Some(ip);
        }
        if let Some(ipv6) = &requested.ipv6 {
            return subnet_matches && self.ipv6.as_ref() == Some(ipv6);
        }
        requested.subnet_id.is_some() && subnet_matches
    }

    /// Whether this stored address IS the address an `IpAddressUpdate` carries,
    /// which is what `AssociateResolverEndpointIpAddress` consults before adding
    /// one: an IP address is unique within the VPC the endpoint is in, so one
    /// the endpoint already holds cannot be added again. Unlike
    /// [`Self::is_named_by`] the subnet alone decides nothing — an endpoint may
    /// hold several addresses in one subnet.
    pub fn holds_same_address(&self, requested: &IpAddressUpdate) -> bool {
        (requested.ip.is_some() && self.ip == requested.ip)
            || (requested.ipv6.is_some() && self.ipv6 == requested.ipv6)
    }

    /// Project the record onto the modelled `IpAddressResponse`.
    pub fn to_response(&self, now: Instant) -> IpAddressResponse {
        IpAddressResponse {
            ip_id: Some(self.ip_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            ip: self.ip.clone(),
            ipv6: self.ipv6.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
        }
    }
}

/// A Resolver endpoint, as stored. `IpAddressCount` is DERIVED from
/// [`Self::ip_addresses`] and never held separately, so the count and the
/// addresses cannot disagree.
#[derive(Debug, Clone)]
pub struct ResolverEndpointRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It
    /// is the list order and the source of the minted ids.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub creator_request_id: String,
    pub security_group_ids: Vec<String>,
    pub direction: String,
    pub host_vpc_id: Option<String>,
    /// The transient `ResolverEndpointStatus` reported until [`Self::ready_at`]
    /// falls due (`CREATING` after a create, `UPDATING` after an update).
    pub status: String,
    pub status_message: Option<String>,
    pub endpoint_type: Option<String>,
    pub protocols: Vec<String>,
    pub outpost_arn: Option<String>,
    pub preferred_instance_type: Option<String>,
    pub rni_enhanced_metrics_enabled: Option<bool>,
    pub target_name_server_metrics_enabled: Option<bool>,
    pub dns64_enabled: Option<bool>,
    pub ipv6_internet_access_enabled: Option<bool>,
    pub ip_addresses: Vec<IpAddressRecord>,
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `OPERATIONAL` is owed.
    pub ready_at: Instant,
}

impl ResolverEndpointRecord {
    /// This endpoint's `ResolverEndpointStatus` as of `now`: the transient
    /// status until the transition the clock owes falls due, `OPERATIONAL`
    /// after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.ready_at {
            &self.status
        } else {
            ENDPOINT_STATUS_OPERATIONAL
        }
    }

    /// `IpAddressCount`: the number of IP addresses stored for this endpoint.
    pub fn ip_address_count(&self) -> i32 {
        i32::try_from(self.ip_addresses.len()).unwrap_or(i32::MAX)
    }

    /// Project the record onto the modelled `ResolverEndpoint`.
    pub fn to_endpoint(&self, now: Instant) -> ResolverEndpoint {
        ResolverEndpoint {
            id: Some(self.id.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            arn: Some(self.arn.clone()),
            name: self.name.clone(),
            security_group_ids: Some(self.security_group_ids.clone()),
            direction: Some(self.direction.clone()),
            ip_address_count: Some(self.ip_address_count()),
            host_vpc_id: self.host_vpc_id.clone(),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            outpost_arn: self.outpost_arn.clone(),
            preferred_instance_type: self.preferred_instance_type.clone(),
            resolver_endpoint_type: self.endpoint_type.clone(),
            protocols: Some(self.protocols.clone()),
            rni_enhanced_metrics_enabled: self.rni_enhanced_metrics_enabled,
            target_name_server_metrics_enabled: self.target_name_server_metrics_enabled,
            dns64_enabled: self.dns64_enabled,
            ipv6_internet_access_enabled: self.ipv6_internet_access_enabled,
        }
    }

    /// Whether this endpoint satisfies one `ListResolverEndpoints` filter: does
    /// any value it presents for `key` appear in `values`?
    ///
    /// A member that holds several values (`SecurityGroupIds`) matches when ANY
    /// of them is among the filter values. The test allocates nothing per record
    /// (bar the fixed-size decimal rendering of `IpAddressCount`) and is a set
    /// lookup rather than a scan of the filter values, so the cost of a filtered
    /// page is O(stored endpoints x filter keys) — not multiplied by how many
    /// values the request carried.
    pub fn matches_filter(&self, key: FilterKey, values: &HashSet<String>, now: Instant) -> bool {
        match key {
            FilterKey::CreatorRequestId => values.contains(self.creator_request_id.as_str()),
            FilterKey::Direction => values.contains(self.direction.as_str()),
            FilterKey::HostVpcId => self
                .host_vpc_id
                .as_deref()
                .is_some_and(|id| values.contains(id)),
            FilterKey::IpAddressCount => values.contains(&self.ip_address_count().to_string()),
            FilterKey::Name => self.name.as_deref().is_some_and(|name| values.contains(name)),
            FilterKey::SecurityGroupIds => self
                .security_group_ids
                .iter()
                .any(|group| values.contains(group.as_str())),
            FilterKey::Status => values.contains(self.status_at(now)),
        }
    }
}

/// Fold a wire filter name for matching against a catalog.
///
/// `Filter$Name`'s doc says the early, upper-snake spellings
/// (`CREATOR_REQUEST_ID` for `CreatorRequestId`) "are still supported", so the
/// name is upper-cased with `_` removed before it is matched — which accepts
/// both spellings and nothing else. Written once here because the rule is the
/// same for every list operation's catalog, while the catalogs themselves
/// differ.
fn fold_filter_name(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_uppercase)
        .collect()
}

/// The member a `ListResolverEndpoints` filter selects on.
///
/// The catalog is exactly the one the model documents for this operation
/// (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverEndpoints**): `CreatorRequestId`, `Direction`, `HostVPCId`,
/// `IpAddressCount`, `Name`, `SecurityGroupIds`, `Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKey {
    CreatorRequestId,
    Direction,
    HostVpcId,
    IpAddressCount,
    Name,
    SecurityGroupIds,
    Status,
}

impl FilterKey {
    /// The canonical name of this key, used to describe a filter set in a
    /// pagination cursor's scope.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CreatorRequestId => "CreatorRequestId",
            Self::Direction => "Direction",
            Self::HostVpcId => "HostVPCId",
            Self::IpAddressCount => "IpAddressCount",
            Self::Name => "Name",
            Self::SecurityGroupIds => "SecurityGroupIds",
            Self::Status => "Status",
        }
    }

    /// Resolve a wire filter name, or `None` when it is outside the documented
    /// catalog. The name is folded by [`fold_filter_name`], which is what
    /// accepts the early upper-snake spelling as well as the documented one.
    pub fn parse(name: &str) -> Option<Self> {
        match fold_filter_name(name).as_str() {
            "CREATORREQUESTID" => Some(Self::CreatorRequestId),
            "DIRECTION" => Some(Self::Direction),
            "HOSTVPCID" => Some(Self::HostVpcId),
            "IPADDRESSCOUNT" => Some(Self::IpAddressCount),
            "NAME" => Some(Self::Name),
            "SECURITYGROUPIDS" => Some(Self::SecurityGroupIds),
            "STATUS" => Some(Self::Status),
            _ => None,
        }
    }
}

/// A Resolver on an Outpost, as stored.
#[derive(Debug, Clone)]
pub struct OutpostResolverRecord {
    /// Position in the store's creation order, from
    /// [`ComAmazonawsRoute53resolverRoute53resolverState::id_seq`]. It is the
    /// list order and the source of the minted id.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub creator_request_id: String,
    pub instance_count: i32,
    pub preferred_instance_type: String,
    pub outpost_arn: String,
    /// The transient `OutpostResolverStatus` reported until [`Self::ready_at`]
    /// falls due (`CREATING` after a create, `UPDATING` after an update).
    pub status: String,
    pub status_message: Option<String>,
    /// `CreateOutpostResolver$Tags`, kept with the record so it is not silently
    /// discarded. `OutpostResolver` models no `Tags` member, so no response
    /// reports them yet: `ListTagsForResource` is still a scaffold stub.
    pub tags: Vec<Tag>,
    pub creation_time: String,
    pub modification_time: String,
    /// The instant at which the move to `OPERATIONAL` is owed.
    pub ready_at: Instant,
}

impl OutpostResolverRecord {
    /// This Resolver's `OutpostResolverStatus` as of `now`: the transient
    /// status until the transition the clock owes falls due, `OPERATIONAL`
    /// after it.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.ready_at {
            &self.status
        } else {
            OUTPOST_RESOLVER_STATUS_OPERATIONAL
        }
    }

    /// Project the record onto the modelled `OutpostResolver`.
    pub fn to_outpost_resolver(&self, now: Instant) -> OutpostResolver {
        OutpostResolver {
            arn: Some(self.arn.clone()),
            creation_time: Some(self.creation_time.clone()),
            modification_time: Some(self.modification_time.clone()),
            creator_request_id: Some(self.creator_request_id.clone()),
            id: Some(self.id.clone()),
            instance_count: Some(self.instance_count),
            preferred_instance_type: Some(self.preferred_instance_type.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status_at(now).to_string()),
            status_message: self.status_message.clone(),
            outpost_arn: Some(self.outpost_arn.clone()),
        }
    }
}

/// A stored `Rfc3339TimeString` as nanoseconds since the epoch, so two
/// timestamps are ordered by the instants they name rather than by their text.
///
/// A string this service did not render (there is none: every stored timestamp
/// comes from [`now_rfc3339`]) orders as the epoch.
fn rfc3339_epoch_nanos(raw: &str) -> i128 {
    aws_smithy_types::DateTime::from_str(raw, aws_smithy_types::date_time::Format::DateTime)
        .map(|at| i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos()))
        .unwrap_or_default()
}

/// A stored `Rfc3339TimeString` as the whole Unix seconds it names, the spelling
/// `Filter$Name` documents for a `CreationTime` filter's values. Rendered from
/// the same parse as [`rfc3339_epoch_nanos`], so the two readings of one stored
/// timestamp cannot disagree.
fn rfc3339_unix_seconds(raw: &str) -> String {
    (rfc3339_epoch_nanos(raw) / 1_000_000_000).to_string()
}

/// Mint the `Id` of a query logging configuration from the id sequence. AWS names
/// one `rqlc-<hex>`. Every result fits `ResourceId` (length 1..64).
pub fn mint_query_log_config_id(seq: u64) -> String {
    format!("rqlc-{:016x}", scramble(seq))
}

/// Mint the `Id` of one VPC-to-configuration association from the id sequence.
/// AWS names one `rqlca-<hex>`, the association form of the configuration's own
/// `rqlc-` prefix. Every result fits `ResourceId` (length 1..64).
pub fn mint_query_log_config_association_id(seq: u64) -> String {
    format!("rqlca-{:016x}", scramble(seq))
}

/// Per-service state for com-amazonaws-route53resolver-route53resolver. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53resolverRoute53resolverState {
    /// Every stored Resolver endpoint, keyed by its `Id`.
    pub resolver_endpoints: HashMap<String, ResolverEndpointRecord>,
    /// `CreatorRequestId` -> endpoint id, so a retried create with the same
    /// `CreatorRequestId` resolves to the endpoint it already made instead of a
    /// duplicate.
    pub creator_request_id_index: HashMap<String, String>,
    /// Every stored Resolver on an Outpost, keyed by its `Id`.
    pub outpost_resolvers: HashMap<String, OutpostResolverRecord>,
    /// `CreatorRequestId` -> Resolver-on-Outpost id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resource. Kept separate
    /// because a `CreatorRequestId` identifies a request to ONE operation: the
    /// same string used for an endpoint and for a Resolver on an Outpost names
    /// two different resources.
    pub outpost_resolver_creator_request_ids: HashMap<String, String>,
    /// Every stored Resolver rule, keyed by its `Id`.
    pub resolver_rules: HashMap<String, ResolverRuleRecord>,
    /// `CreatorRequestId` -> rule id, so a retried create with the same
    /// `CreatorRequestId` resolves to the rule it already made instead of a
    /// duplicate.
    pub rule_creator_request_id_index: HashMap<String, String>,
    /// Every stored rule-to-VPC association, keyed by its `Id`.
    pub resolver_rule_associations: HashMap<String, ResolverRuleAssociationRecord>,
    /// Every stored firewall domain list, keyed by its `Id`.
    pub firewall_domain_lists: HashMap<String, FirewallDomainListRecord>,
    /// `CreatorRequestId` -> firewall domain list id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub firewall_domain_list_creator_request_ids: HashMap<String, String>,
    /// Every stored DNS Firewall rule group, keyed by its `Id`.
    pub firewall_rule_groups: HashMap<String, FirewallRuleGroupRecord>,
    /// `CreatorRequestId` -> firewall rule group id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub firewall_rule_group_creator_request_ids: HashMap<String, String>,
    /// Every stored association between a DNS Firewall rule group and a VPC,
    /// keyed by its `Id`.
    pub firewall_rule_group_associations: HashMap<String, FirewallRuleGroupAssociationRecord>,
    /// `CreatorRequestId` -> rule-group association id, the same retry index as
    /// [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub firewall_rule_group_association_creator_request_ids: HashMap<String, String>,
    /// Every stored Resolver query logging configuration, keyed by its `Id`.
    pub resolver_query_log_configs: HashMap<String, ResolverQueryLogConfigRecord>,
    /// `CreatorRequestId` -> query logging configuration id, the same retry
    /// index as [`Self::creator_request_id_index`] for the other resources: a
    /// `CreatorRequestId` identifies a request to ONE operation, so this map is
    /// kept apart from theirs.
    pub query_log_config_creator_request_ids: HashMap<String, String>,
    /// The configuration for DNSSEC validation of every VPC whose validation
    /// setting was moved off its default, keyed by that VPC's id
    /// (`ResolverDnssecConfig$ResourceId`).
    ///
    /// A configuration for DNSSEC validation is a per-VPC singleton the service
    /// maintains itself, so this map is an override table and not an inventory:
    /// a VPC it holds no entry for still HAS a configuration, the default one
    /// [`Self::resolver_dnssec_config`] derives.
    pub resolver_dnssec_configs: HashMap<String, ResolverDnssecConfigRecord>,
    /// The Resolver behavior configuration of every VPC whose configuration was
    /// moved off its default, keyed by that VPC's id
    /// (`ResolverConfig$ResourceId`).
    ///
    /// A Resolver configuration is a per-VPC singleton the service maintains
    /// itself, so this map is an override table and not an inventory: a VPC it
    /// holds no entry for still HAS a configuration, the default one
    /// [`Self::resolver_config`] derives.
    pub resolver_configs: HashMap<String, ResolverConfigRecord>,
    /// The DNS Firewall configuration of every VPC whose configuration was
    /// moved off its default, keyed by that VPC's id (`FirewallConfig$ResourceId`).
    ///
    /// A firewall configuration is a per-VPC singleton the service maintains
    /// itself, so this map is an override table and not an inventory: a VPC it
    /// holds no entry for still HAS a configuration, the default one
    /// [`Self::firewall_config`] derives.
    pub firewall_configs: HashMap<String, FirewallConfigRecord>,
    /// Monotonic counter behind every minted id (endpoint, IP, Resolver on an
    /// Outpost, rule, association, DNSSEC configuration), so ids are
    /// reproducible within a run and never collide — across resources as well
    /// as within one.
    pub id_seq: u64,
}

impl ComAmazonawsRoute53resolverRoute53resolverState {
    /// Whether one more endpoint may be stored, i.e. the store is still below
    /// [`MAX_STORED_ENDPOINTS`]. A create that this refuses answers
    /// `LimitExceededException` and writes nothing.
    pub fn can_store_another_endpoint(&self) -> bool {
        self.resolver_endpoints.len() < MAX_STORED_ENDPOINTS
    }

    /// Take the next value of the id sequence.
    pub fn next_seq(&mut self) -> u64 {
        self.id_seq = self.id_seq.saturating_add(1);
        self.id_seq
    }

    /// Whether one more Resolver rule may be stored, i.e. the store is still
    /// below [`MAX_STORED_RULES`].
    pub fn can_store_another_rule(&self) -> bool {
        self.resolver_rules.len() < MAX_STORED_RULES
    }

    /// Whether one more rule association may be stored, i.e. the store is still
    /// below [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn can_store_another_rule_association(&self) -> bool {
        self.resolver_rule_associations.len() < MAX_STORED_RULE_ASSOCIATIONS
    }

    /// The association between `resolver_rule_id` and `vpc_id`, if the store
    /// holds one.
    ///
    /// A rule and a VPC are associated at most once, so this is what
    /// `AssociateResolverRule` consults before writing a second record for the
    /// same pair. The scan is over a store bounded by
    /// [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn find_rule_association(
        &self,
        resolver_rule_id: &str,
        vpc_id: &str,
    ) -> Option<&ResolverRuleAssociationRecord> {
        self.resolver_rule_associations
            .values()
            .find(|record| record.resolver_rule_id == resolver_rule_id && record.vpc_id == vpc_id)
    }

    /// The stored Resolver rule `arn` names, if the store holds one.
    ///
    /// The rule policy operations address a rule by its `Arn` and not by its
    /// `ResolverRuleId`, so they resolve it through this. The whole ARN is
    /// compared, so an ARN naming another account, another region or another
    /// resource type matches no rule. The scan is over a store bounded by
    /// [`MAX_STORED_RULES`].
    pub fn rule_by_arn(&self, arn: &str) -> Option<&ResolverRuleRecord> {
        self.resolver_rules.values().find(|record| record.arn == arn)
    }

    /// The same lookup as [`Self::rule_by_arn`], for the write side.
    pub fn rule_by_arn_mut(&mut self, arn: &str) -> Option<&mut ResolverRuleRecord> {
        self.resolver_rules
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// The stored query logging configuration `arn` names, if the store holds
    /// one.
    ///
    /// The query-logging policy operations address a configuration by its `Arn`
    /// and not by its `ResolverQueryLogConfigId`, so — as for
    /// [`Self::rule_by_arn`] — they resolve it through this. The whole ARN is
    /// compared, so an ARN naming another account, another region or another
    /// resource type matches no configuration. The scan is over a store bounded
    /// by [`MAX_STORED_QUERY_LOG_CONFIGS`].
    pub fn query_log_config_by_arn(&self, arn: &str) -> Option<&ResolverQueryLogConfigRecord> {
        self.resolver_query_log_configs
            .values()
            .find(|record| record.arn == arn)
    }

    /// The same lookup as [`Self::query_log_config_by_arn`], for the write side.
    pub fn query_log_config_by_arn_mut(
        &mut self,
        arn: &str,
    ) -> Option<&mut ResolverQueryLogConfigRecord> {
        self.resolver_query_log_configs
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// How many VPCs a rule is still associated with.
    ///
    /// `DeleteResolverRule` is what reads it: "Before you can delete a Resolver
    /// rule, you must disassociate it from all the VPCs that you associated the
    /// Resolver rule with", so a rule with any association left cannot be
    /// deleted. The scan is over a store bounded by
    /// [`MAX_STORED_RULE_ASSOCIATIONS`].
    pub fn rule_association_count(&self, resolver_rule_id: &str) -> usize {
        self.resolver_rule_associations
            .values()
            .filter(|record| record.resolver_rule_id == resolver_rule_id)
            .count()
    }

    /// Every stored endpoint in creation order — the order both the list
    /// operation's pages and its cursor are defined over.
    pub fn endpoints_in_creation_order(&self) -> Vec<&ResolverEndpointRecord> {
        let mut all: Vec<&ResolverEndpointRecord> = self.resolver_endpoints.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored Resolver rule in creation order — the order
    /// `ListResolverRules`' pages and its cursor are defined over, for the same
    /// reason as [`Self::endpoints_in_creation_order`]: a page is cut at an
    /// offset into this sequence, so the sequence must not depend on the hash
    /// order of the map that holds the records.
    pub fn rules_in_creation_order(&self) -> Vec<&ResolverRuleRecord> {
        let mut all: Vec<&ResolverRuleRecord> = self.resolver_rules.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored rule association in creation order — the order
    /// `ListResolverRuleAssociations`' pages and its cursor are defined over,
    /// for the same reason as [`Self::rules_in_creation_order`].
    pub fn rule_associations_in_creation_order(&self) -> Vec<&ResolverRuleAssociationRecord> {
        let mut all: Vec<&ResolverRuleAssociationRecord> =
            self.resolver_rule_associations.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Whether one more Resolver on an Outpost may be stored, i.e. the store is
    /// still below [`MAX_STORED_OUTPOST_RESOLVERS`]. A create that this refuses
    /// answers `ServiceQuotaExceededException` and writes nothing.
    pub fn can_store_another_outpost_resolver(&self) -> bool {
        self.outpost_resolvers.len() < MAX_STORED_OUTPOST_RESOLVERS
    }

    /// Whether one more firewall domain list may be stored, i.e. the store is
    /// still below [`MAX_STORED_FIREWALL_DOMAIN_LISTS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_firewall_domain_list(&self) -> bool {
        self.firewall_domain_lists.len() < MAX_STORED_FIREWALL_DOMAIN_LISTS
    }

    /// Whether one more firewall rule group may be stored, i.e. the store is
    /// still below [`MAX_STORED_FIREWALL_RULE_GROUPS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_firewall_rule_group(&self) -> bool {
        self.firewall_rule_groups.len() < MAX_STORED_FIREWALL_RULE_GROUPS
    }

    /// Whether one more rule-group-to-VPC association may be stored, i.e. the
    /// store is still below [`MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS`]. An
    /// associate that this refuses answers `LimitExceededException` and writes
    /// nothing.
    pub fn can_store_another_firewall_rule_group_association(&self) -> bool {
        self.firewall_rule_group_associations.len() < MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS
    }

    /// The association between `firewall_rule_group_id` and `vpc_id`, if the
    /// store holds one.
    ///
    /// One rule group is associated with one VPC at most once, so this is what
    /// `AssociateFirewallRuleGroup` consults before writing a second record for
    /// the same pair — the same lookup [`Self::find_rule_association`] is for a
    /// Resolver rule. The scan is over a store bounded by
    /// [`MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS`].
    pub fn find_firewall_rule_group_association(
        &self,
        firewall_rule_group_id: &str,
        vpc_id: &str,
    ) -> Option<&FirewallRuleGroupAssociationRecord> {
        self.firewall_rule_group_associations
            .values()
            .find(|record| {
                record.firewall_rule_group_id == firewall_rule_group_id && record.vpc_id == vpc_id
            })
    }

    /// The association of `vpc_id` that already holds `priority`, if the store
    /// holds one, skipping the association `excluding` names.
    ///
    /// "You must specify a unique priority for each rule group that you
    /// associate with a single VPC": this is the lookup both
    /// `AssociateFirewallRuleGroup` and `UpdateFirewallRuleGroupAssociation`
    /// consult before writing a priority, so the two cannot drift apart, and it
    /// answers the record itself because the refusal names the association that
    /// holds the priority ([`association_priority_in_use`]). `excluding` is the
    /// association being updated, which never clashes with itself. The scan is
    /// over a store bounded by
    /// [`MAX_STORED_FIREWALL_RULE_GROUP_ASSOCIATIONS`].
    pub fn firewall_rule_group_association_holding_priority(
        &self,
        vpc_id: &str,
        priority: i32,
        excluding: Option<&str>,
    ) -> Option<&FirewallRuleGroupAssociationRecord> {
        self.firewall_rule_group_associations.values().find(|record| {
            record.vpc_id == vpc_id
                && record.priority == priority
                && excluding.is_none_or(|id| record.id != id)
        })
    }

    /// Every stored rule-group-to-VPC association in creation order — the order
    /// `ListFirewallRuleGroupAssociations`' pages and its cursor are defined
    /// over, for the same reason as
    /// [`Self::firewall_rule_groups_in_creation_order`].
    pub fn firewall_rule_group_associations_in_creation_order(
        &self,
    ) -> Vec<&FirewallRuleGroupAssociationRecord> {
        let mut all: Vec<&FirewallRuleGroupAssociationRecord> =
            self.firewall_rule_group_associations.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// The stored firewall rule group `arn` names, if the store holds one.
    ///
    /// The rule-group policy operations address a group by its `Arn` and not by
    /// its `FirewallRuleGroupId`, so they resolve it through this — the same
    /// lookup [`Self::rule_by_arn`] is for a Resolver rule. The whole ARN is
    /// compared, so an ARN naming another account, another region or another
    /// resource type matches no group. The scan is over a store bounded by
    /// [`MAX_STORED_FIREWALL_RULE_GROUPS`].
    pub fn firewall_rule_group_by_arn(&self, arn: &str) -> Option<&FirewallRuleGroupRecord> {
        self.firewall_rule_groups
            .values()
            .find(|record| record.arn == arn)
    }

    /// The same lookup as [`Self::firewall_rule_group_by_arn`], for the write
    /// side.
    pub fn firewall_rule_group_by_arn_mut(
        &mut self,
        arn: &str,
    ) -> Option<&mut FirewallRuleGroupRecord> {
        self.firewall_rule_groups
            .values_mut()
            .find(|record| record.arn == arn)
    }

    /// Whether one more query logging configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_QUERY_LOG_CONFIGS`]. A create that this
    /// refuses answers `LimitExceededException` and writes nothing.
    pub fn can_store_another_query_log_config(&self) -> bool {
        self.resolver_query_log_configs.len() < MAX_STORED_QUERY_LOG_CONFIGS
    }

    /// The VPC-to-configuration association the store holds under
    /// `association_id`, if any.
    ///
    /// An association lives on the configuration it belongs to, so resolving one
    /// by its own id walks the configurations; both collections are bounded
    /// ([`MAX_STORED_QUERY_LOG_CONFIGS`],
    /// [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`]).
    pub fn query_log_config_association(
        &self,
        association_id: &str,
    ) -> Option<&ResolverQueryLogConfigAssociationRecord> {
        self.resolver_query_log_configs
            .values()
            .flat_map(|config| config.associations.iter())
            .find(|record| record.id == association_id)
    }

    /// Every stored query logging configuration in creation order — the order
    /// `ListResolverQueryLogConfigs`' unsorted pages and its cursor are defined
    /// over, for the same reason as [`Self::endpoints_in_creation_order`]: a
    /// page is cut at an offset into this sequence, so the sequence must not
    /// depend on the hash order of the map that holds the records.
    pub fn query_log_configs_in_creation_order(&self) -> Vec<&ResolverQueryLogConfigRecord> {
        let mut all: Vec<&ResolverQueryLogConfigRecord> =
            self.resolver_query_log_configs.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every VPC-to-configuration association the store holds, in association
    /// order — the order `ListResolverQueryLogConfigAssociations`' unsorted
    /// pages and its cursor are defined over, for the same reason as
    /// [`Self::query_log_configs_in_creation_order`]: a page is cut at an offset
    /// into this sequence, so the sequence must not depend on the hash order of
    /// the map that holds the configurations the associations live on.
    ///
    /// Both collections are bounded ([`MAX_STORED_QUERY_LOG_CONFIGS`],
    /// [`MAX_QUERY_LOG_CONFIG_ASSOCIATIONS`]).
    pub fn query_log_config_associations_in_creation_order(
        &self,
    ) -> Vec<&ResolverQueryLogConfigAssociationRecord> {
        let mut all: Vec<&ResolverQueryLogConfigAssociationRecord> = self
            .resolver_query_log_configs
            .values()
            .flat_map(|config| config.associations.iter())
            .collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored Resolver on an Outpost in creation order — the order
    /// `ListOutpostResolvers`' pages and its cursor are defined over.
    pub fn outpost_resolvers_in_creation_order(&self) -> Vec<&OutpostResolverRecord> {
        let mut all: Vec<&OutpostResolverRecord> = self.outpost_resolvers.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Whether one more configuration for DNSSEC validation may be stored,
    /// i.e. the store is still below [`MAX_STORED_DNSSEC_CONFIGS`]. An update
    /// that this refuses writes nothing.
    pub fn can_store_another_dnssec_config(&self) -> bool {
        self.resolver_dnssec_configs.len() < MAX_STORED_DNSSEC_CONFIGS
    }

    /// The configuration for DNSSEC validation of the VPC `resource_id`, which
    /// every VPC has: the stored record when one was written for it, and
    /// otherwise the default configuration that VPC reports — identical
    /// identity ([`dnssec_config_id`] of the VPC, owned by this account) with
    /// validation off, settled.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn resolver_dnssec_config(&self, resource_id: &str) -> ResolverDnssecConfigRecord {
        self.resolver_dnssec_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| ResolverDnssecConfigRecord {
                seq: 0,
                id: dnssec_config_id(resource_id),
                owner_id: ::engine::emulator_account_id().to_string(),
                resource_id: resource_id.to_string(),
                status: DNSSEC_STATUS_DISABLED.to_string(),
                settled_status: DNSSEC_STATUS_DISABLED.to_string(),
                settled_at: Instant::now(),
            })
    }

    /// Every stored configuration for DNSSEC validation in creation order — the
    /// order `ListResolverDnssecConfigs`' pages and its cursor are defined over,
    /// for the same reason as [`Self::endpoints_in_creation_order`]: a page is
    /// cut at an offset into this sequence, so the sequence must not depend on
    /// the hash order of the map that holds the records.
    pub fn dnssec_configs_in_creation_order(&self) -> Vec<&ResolverDnssecConfigRecord> {
        let mut all: Vec<&ResolverDnssecConfigRecord> =
            self.resolver_dnssec_configs.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// The DNS Firewall configuration of the VPC `resource_id`, which every VPC
    /// has: the stored record when one was written for it, and otherwise the
    /// default configuration that VPC reports — identical identity
    /// ([`firewall_config_id`] of the VPC, owned by this account) with
    /// `FirewallFailOpen` at its documented default.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn firewall_config(&self, resource_id: &str) -> FirewallConfigRecord {
        self.firewall_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| FirewallConfigRecord {
                seq: 0,
                id: firewall_config_id(resource_id),
                resource_id: resource_id.to_string(),
                owner_id: ::engine::emulator_account_id().to_string(),
                firewall_fail_open: FIREWALL_FAIL_OPEN_DISABLED.to_string(),
            })
    }

    /// Whether one more VPC's firewall configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_FIREWALL_CONFIGS`]. Asked only for a
    /// VPC [`Self::firewall_configs`] holds no record for yet.
    pub fn can_store_another_firewall_config(&self) -> bool {
        self.firewall_configs.len() < MAX_STORED_FIREWALL_CONFIGS
    }

    /// Every VPC the account has that this service can know of: the account's
    /// own VPC inventory first, in the order that inventory reports them, and
    /// then the VPCs only this service's records name, in the order it came to
    /// know of each one.
    ///
    /// A per-VPC singleton configuration ([`FirewallConfigRecord`],
    /// [`ResolverConfigRecord`]) exists for every VPC of the account, not only
    /// for those a caller moved off the default, so the operation that LISTS
    /// such configurations enumerates VPCs and not override records. Which VPCs
    /// the account HAS is a fact of its own Amazon EC2 inventory, which
    /// [`account_vpc_ids`] reads and the caller passes in as `inventory` — a VPC
    /// the account created carries its configurations with no Resolver call ever
    /// naming it. Beside the inventory come the VPCs this service's OWN records
    /// name, which an account can hold records for without that store holding
    /// the VPC: a VPC a Resolver rule is associated with, a VPC a Resolver
    /// endpoint is hosted in, a VPC a DNS Firewall rule group is associated
    /// with, and a VPC one of the per-VPC configurations was written for.
    ///
    /// The inventory leads, because every VPC of an account is in it and a VPC
    /// it reports later is one minted later: a list operation cuts a page at an
    /// offset into this sequence, so a VPC that appears while a caller is paging
    /// has to append past the page already read. The records that name a VPC the
    /// inventory does not hold follow, ordered by the position in the store's
    /// creation order ([`Self::id_seq`]) of the FIRST record that named it, so
    /// that tail never depends on the hash order of the maps holding those
    /// records and it, too, only appends. Each VPC appears ONCE, however many
    /// records name it and however often the inventory repeats it. A pure read:
    /// a VPC is not given any record here.
    pub fn known_vpc_ids(&self, inventory: &[String]) -> Vec<String> {
        let named = self
            .resolver_rule_associations
            .values()
            .map(|record| (record.vpc_id.as_str(), record.seq))
            .chain(self.resolver_endpoints.values().filter_map(|record| {
                record
                    .host_vpc_id
                    .as_deref()
                    .map(|vpc_id| (vpc_id, record.seq))
            }))
            .chain(
                self.resolver_dnssec_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            )
            .chain(
                self.resolver_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            )
            .chain(
                self.firewall_configs
                    .values()
                    .map(|record| (record.resource_id.as_str(), record.seq)),
            )
            .chain(
                self.firewall_rule_group_associations
                    .values()
                    .map(|record| (record.vpc_id.as_str(), record.seq)),
            );

        let mut first_seen: HashMap<&str, u64> = HashMap::new();
        for (vpc_id, seq) in named {
            let earliest = first_seen.entry(vpc_id).or_insert(seq);
            *earliest = (*earliest).min(seq);
        }

        // The account's inventory leads, each id once however often it repeats.
        let mut listed: HashSet<&str> = HashSet::new();
        let mut ids: Vec<String> = Vec::new();
        for vpc_id in inventory {
            if listed.insert(vpc_id.as_str()) {
                ids.push(vpc_id.clone());
            }
        }

        // Then the VPCs only this service's records name, by the id and the
        // sequence number together, so VPCs that became known in the same call
        // still have ONE order rather than the map's.
        let mut record_only: Vec<(u64, &str)> = first_seen
            .iter()
            .filter(|(vpc_id, _)| !listed.contains(**vpc_id))
            .map(|(vpc_id, seq)| (*seq, *vpc_id))
            .collect();
        record_only.sort_unstable();
        ids.extend(
            record_only
                .into_iter()
                .map(|(_, vpc_id)| vpc_id.to_string()),
        );
        ids
    }

    /// The Resolver behavior configuration of the VPC `resource_id`, which every
    /// VPC has: the stored record when one was written for it, and otherwise the
    /// default configuration that VPC reports — identical identity
    /// ([`resolver_config_id`] of the VPC, owned by this account) with
    /// `AutodefinedReverse` at its documented default, settled.
    ///
    /// A pure read: a VPC with no record is NOT given one here, so reporting a
    /// configuration never changes what a later caller sees.
    pub fn resolver_config(&self, resource_id: &str) -> ResolverConfigRecord {
        self.resolver_configs
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| ResolverConfigRecord {
                seq: 0,
                id: resolver_config_id(resource_id),
                resource_id: resource_id.to_string(),
                owner_id: ::engine::emulator_account_id().to_string(),
                status: AUTODEFINED_REVERSE_ENABLED.to_string(),
                settled_status: AUTODEFINED_REVERSE_ENABLED.to_string(),
                settled_at: Instant::now(),
            })
    }

    /// Whether one more VPC's Resolver configuration may be stored, i.e. the
    /// store is still below [`MAX_STORED_RESOLVER_CONFIGS`]. Asked only for a
    /// VPC [`Self::resolver_configs`] holds no record for yet.
    pub fn can_store_another_resolver_config(&self) -> bool {
        self.resolver_configs.len() < MAX_STORED_RESOLVER_CONFIGS
    }

    /// Every stored firewall domain list in creation order — the order
    /// `ListFirewallDomainLists`' pages and its cursor are defined over, for the
    /// same reason as [`Self::endpoints_in_creation_order`]: a page is cut at an
    /// offset into this sequence, so the sequence must not depend on the hash
    /// order of the map that holds the records.
    pub fn firewall_domain_lists_in_creation_order(&self) -> Vec<&FirewallDomainListRecord> {
        let mut all: Vec<&FirewallDomainListRecord> = self.firewall_domain_lists.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// Every stored firewall rule group in creation order — the order
    /// `ListFirewallRuleGroups`' pages and its cursor are defined over, for the
    /// same reason as [`Self::firewall_domain_lists_in_creation_order`]: a page
    /// is cut at an offset into this sequence, so the sequence must not depend
    /// on the hash order of the map that holds the records.
    pub fn firewall_rule_groups_in_creation_order(&self) -> Vec<&FirewallRuleGroupRecord> {
        let mut all: Vec<&FirewallRuleGroupRecord> = self.firewall_rule_groups.values().collect();
        all.sort_by_key(|record| record.seq);
        all
    }

    /// The resource an ARN names, when this store holds one under it.
    ///
    /// Matched against the ARN each record was minted with, so an ARN resolves
    /// only to the resource it was issued for — in the region and account that
    /// created it — and no ARN text is parsed to find the record. Each scan is
    /// over a store bounded by the `MAX_STORED_*` cap of its resource.
    pub fn find_tagged_resource(&self, resource_arn: &str) -> Option<TaggedResource> {
        if let Some(record) = self
            .resolver_endpoints
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverEndpoint(record.id.clone()));
        }
        if let Some(record) = self
            .resolver_rules
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverRule(record.id.clone()));
        }
        if let Some(record) = self
            .outpost_resolvers
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::OutpostResolver(record.id.clone()));
        }
        if let Some(record) = self
            .firewall_domain_lists
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::FirewallDomainList(record.id.clone()));
        }
        if let Some(record) = self
            .firewall_rule_groups
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::FirewallRuleGroup(record.id.clone()));
        }
        if let Some(record) = self
            .firewall_rule_group_associations
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::FirewallRuleGroupAssociation(
                record.id.clone(),
            ));
        }
        if let Some(record) = self
            .resolver_query_log_configs
            .values()
            .find(|record| record.arn == resource_arn)
        {
            return Some(TaggedResource::ResolverQueryLogConfig(record.id.clone()));
        }
        None
    }

    /// The tags of the resource `resource_arn` names, for reading, or `None`
    /// when this store holds no resource under that ARN.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&[Tag]> {
        match self.find_tagged_resource(resource_arn)? {
            TaggedResource::ResolverEndpoint(id) => self
                .resolver_endpoints
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::ResolverRule(id) => self
                .resolver_rules
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::OutpostResolver(id) => self
                .outpost_resolvers
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::FirewallDomainList(id) => self
                .firewall_domain_lists
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::FirewallRuleGroup(id) => self
                .firewall_rule_groups
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::FirewallRuleGroupAssociation(id) => self
                .firewall_rule_group_associations
                .get(&id)
                .map(|record| record.tags.as_slice()),
            TaggedResource::ResolverQueryLogConfig(id) => self
                .resolver_query_log_configs
                .get(&id)
                .map(|record| record.tags.as_slice()),
        }
    }

    /// The same tag set, for writing: the tags live ON the resource's record, so
    /// a tag write is a write to that record and every operation that reports
    /// the resource sees it.
    pub fn resource_tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        match self.find_tagged_resource(resource_arn)? {
            TaggedResource::ResolverEndpoint(id) => self
                .resolver_endpoints
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::ResolverRule(id) => self
                .resolver_rules
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::OutpostResolver(id) => self
                .outpost_resolvers
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::FirewallDomainList(id) => self
                .firewall_domain_lists
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::FirewallRuleGroup(id) => self
                .firewall_rule_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::FirewallRuleGroupAssociation(id) => self
                .firewall_rule_group_associations
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::ResolverQueryLogConfig(id) => self
                .resolver_query_log_configs
                .get_mut(&id)
                .map(|record| &mut record.tags),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53resolverRoute53resolverState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53resolverRoute53resolverState>)
}

/// The wall clock, as the `Rfc3339TimeString` the model's time members declare
/// (length 20..40).
pub fn now_rfc3339() -> String {
    aws_smithy_types::DateTime::from(SystemTime::now())
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The region the request was signed for, which is the region of every ARN the
/// request mints. Falls back to the emulator's default region when the request
/// carries no SigV4 credential scope.
pub fn request_region(headers: &http::HeaderMap) -> String {
    emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Spread a counter value over 64 bits, so a minted id reads like the opaque hex
/// AWS returns while staying a pure function of the counter. splitmix64's
/// finaliser is a bijection on `u64`, so distinct counter values never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Mint the `Id` of a Resolver endpoint from the id sequence.
///
/// AWS names an inbound endpoint `rslvr-in-<hex>` and an outbound one
/// `rslvr-out-<hex>`; `INBOUND_DELEGATION` is an inbound direction, so it takes
/// the inbound infix. Every result fits `ResourceId` (length 1..64).
pub fn mint_endpoint_id(direction: &str, seq: u64) -> String {
    let infix = if direction == ENDPOINT_DIRECTION_OUTBOUND {
        "out"
    } else {
        "in"
    };
    format!("rslvr-{infix}-{:016x}", scramble(seq))
}

/// Mint the `IpId` of one endpoint IP address from the id sequence. AWS names a
/// Resolver network interface `rni-<hex>`.
pub fn mint_ip_id(seq: u64) -> String {
    format!("rni-{:016x}", scramble(seq))
}

/// Mint the `Id` of a Resolver on an Outpost from the id sequence.
///
/// The model constrains `ResourceId` only to length 1..64, so the shape is
/// taken from [`mint_endpoint_id`]'s `rslvr-<infix>-<hex>` with the Outpost
/// infix: an id is therefore recognisable as this resource's and fits
/// `ResourceId`.
pub fn mint_outpost_resolver_id(seq: u64) -> String {
    format!("rslvr-op-{:016x}", scramble(seq))
}

/// Mint the `Id` of a Resolver rule from the id sequence. AWS names a Resolver
/// rule `rslvr-rr-<hex>`. Every result fits `ResourceId` (length 1..64).
pub fn mint_rule_id(seq: u64) -> String {
    format!("rslvr-rr-{:016x}", scramble(seq))
}

/// Mint the `Id` of a rule-to-VPC association from the id sequence. AWS names a
/// Resolver rule association `rslvr-rrassoc-<hex>`.
pub fn mint_rule_association_id(seq: u64) -> String {
    format!("rslvr-rrassoc-{:016x}", scramble(seq))
}

/// Mint the `Id` of a firewall domain list from the id sequence. AWS names a
/// DNS Firewall domain list `rslvr-fdl-<hex>`. Every result fits `ResourceId`
/// (length 1..64).
pub fn mint_firewall_domain_list_id(seq: u64) -> String {
    format!("rslvr-fdl-{:016x}", scramble(seq))
}

/// Mint the `Id` of a firewall rule group from the id sequence. AWS names a DNS
/// Firewall rule group `rslvr-frg-<hex>`. Every result fits `ResourceId`
/// (length 1..64).
pub fn mint_firewall_rule_group_id(seq: u64) -> String {
    format!("rslvr-frg-{:016x}", scramble(seq))
}

/// Mint the `Id` of a rule-group-to-VPC association from the id sequence. AWS
/// names a DNS Firewall rule group association `rslvr-frgassoc-<hex>`. Every
/// result fits `ResourceId` (length 1..64).
pub fn mint_firewall_rule_group_association_id(seq: u64) -> String {
    format!("rslvr-frgassoc-{:016x}", scramble(seq))
}

/// Mint the `FirewallThreatProtectionId` of a DNS Firewall Advanced rule from
/// the id sequence.
///
/// The model constrains the member only to `ResourceId`'s length (1..64), so the
/// shape is taken from [`mint_firewall_rule_group_id`]'s `rslvr-<infix>-<hex>`
/// with this resource's own infix — the same reasoning as
/// [`mint_outpost_resolver_id`]: an id is recognisable as this resource's and
/// collides with no other, since every infix is distinct and the counter is
/// shared.
pub fn mint_firewall_threat_protection_id(seq: u64) -> String {
    format!("rslvr-ftp-{:016x}", scramble(seq))
}

/// The `Id` of a VPC's DNS Firewall configuration. AWS names one
/// `rslvr-fc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, because no
/// operation creates a firewall configuration: one exists for a VPC whether or
/// not anything has ever written to it, so its id has to be a function of the
/// VPC it belongs to. The same VPC therefore reports the same `Id` from every
/// call, before and after an update.
pub fn firewall_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/firewall-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rslvr-fc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// The `Id` of a VPC's Resolver behavior configuration. AWS names one
/// `rslvr-rc-<hex>`; every result fits `ResourceId` (length 1..64).
///
/// Derived from the VPC id rather than taken from the id sequence, for the same
/// reason as [`firewall_config_id`]: no operation creates a Resolver
/// configuration, so one exists for a VPC whether or not anything has ever
/// written to it, and its id has to be a function of the VPC it belongs to. The
/// same VPC therefore reports the same `Id` from every call, before and after an
/// update.
pub fn resolver_config_id(resource_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/resolver-config/v1");
    feed_field(&mut hasher, resource_id.as_bytes());
    format!("rslvr-rc-{}", hex::encode(&hasher.finalize()[..8]))
}

/// Feed a length-prefixed field into a digest, so two different sequences of
/// fields cannot hash to the same byte stream whatever the fields contain.
fn feed_field(hasher: &mut Sha256, field: &[u8]) {
    hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(field);
}

/// The cursor scope of one `ListResolverEndpoints` query.
///
/// The offset a cursor carries is an index into the FILTERED collection, so a
/// cursor only means anything for the filter set it was minted under. Folding
/// that set into the scope (canonically, so the order the filters and their
/// values arrive in does not matter) means resuming with a different `Filters`
/// is answered with `InvalidNextTokenException` rather than with a page cut at
/// the wrong position.
pub fn endpoint_list_scope(filters: &[(FilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverEndpoints",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one `ListResolverRules` query, for the same reason and in
/// the same shape as [`endpoint_list_scope`]'s.
pub fn rule_list_scope(filters: &[(RuleFilterKey, HashSet<String>)]) -> String {
    filter_list_scope(
        "ListResolverRules",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one `ListResolverRuleAssociations` query, for the same
/// reason and in the same shape as [`endpoint_list_scope`]'s.
pub fn rule_association_list_scope(
    filters: &[(RuleAssociationFilterKey, HashSet<String>)],
) -> String {
    filter_list_scope(
        "ListResolverRuleAssociations",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    )
}

/// The cursor scope of one filtered list query: the operation being paged, plus
/// a digest of its filter set folded canonically, so neither the order the
/// filters arrived in nor the order of one filter's values changes the scope.
fn filter_list_scope<'a>(
    operation: &str,
    filters: impl Iterator<Item = (&'static str, &'a HashSet<String>)>,
) -> String {
    let mut described: Vec<(&'static str, Vec<&str>)> = filters
        .map(|(name, values)| {
            let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
            values.sort_unstable();
            (name, values)
        })
        .collect();
    described.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"route53resolver/list-filters/v1");
    feed_field(&mut hasher, operation.as_bytes());
    for (name, values) in &described {
        feed_field(&mut hasher, name.as_bytes());
        for value in values {
            feed_field(&mut hasher, value.as_bytes());
        }
    }
    format!("{operation}/{}", hex::encode(&hasher.finalize()[..8]))
}

/// The cursor scope of one `ListResolverQueryLogConfigs` query.
///
/// The offset a cursor carries is an index into the filtered, sorted collection,
/// so — as for [`endpoint_list_scope`] — the filter set belongs in the scope;
/// the ordering does too, because "if you submit a second or subsequent
/// ListResolverQueryLogConfigs request and specify the NextToken parameter, you
/// must use the same value for SortBy / SortOrder, if any, as in the previous
/// request": a resume that changes either is answered as a cursor this query
/// does not accept, rather than with a page cut at the wrong position.
pub fn query_log_config_list_scope(
    filters: &[(QueryLogConfigFilterKey, HashSet<String>)],
    sort_by: Option<QueryLogConfigSortKey>,
    sort_order: Option<&str>,
) -> String {
    let base = filter_list_scope(
        "ListResolverQueryLogConfigs",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    );
    let sort_by = sort_by.map_or("", QueryLogConfigSortKey::canonical_name);
    format!("{base}/{sort_by}/{}", sort_order.unwrap_or_default())
}

/// The cursor scope of one `ListResolverQueryLogConfigAssociations` query, for
/// the same reason and in the same shape as [`query_log_config_list_scope`]'s:
/// the offset a cursor carries is an index into the filtered, ordered
/// collection, and "you must use the same value for `SortBy` / `SortOrder`, if
/// any, as in the previous request", so a resume that changes the filters or the
/// ordering is answered as a cursor this query does not accept rather than with
/// a page cut at the wrong position.
pub fn query_log_config_association_list_scope(
    filters: &[(QueryLogConfigAssociationFilterKey, HashSet<String>)],
    sort_by: Option<QueryLogConfigAssociationSortKey>,
    sort_order: Option<&str>,
) -> String {
    let base = filter_list_scope(
        "ListResolverQueryLogConfigAssociations",
        filters
            .iter()
            .map(|(key, values)| (key.canonical_name(), values)),
    );
    let sort_by = sort_by.map_or("", QueryLogConfigAssociationSortKey::canonical_name);
    format!("{base}/{sort_by}/{}", sort_order.unwrap_or_default())
}

/// The cursor scope of one `ListOutpostResolvers` query.
///
/// The offset a cursor carries is an index into the collection the query
/// selects, which `OutpostArn` narrows, so — exactly as for
/// [`endpoint_list_scope`] — resuming with a different `OutpostArn` (or with
/// none) is answered as an unknown cursor rather than with a page cut at the
/// wrong position.
pub fn outpost_resolver_list_scope(outpost_arn: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListOutpostResolvers/outpost-arn/v1");
    if let Some(outpost_arn) = outpost_arn {
        feed_field(&mut hasher, outpost_arn.as_bytes());
    }
    format!(
        "ListOutpostResolvers/{}",
        hex::encode(&hasher.finalize()[..8])
    )
}

/// The cursor scope of one `ListFirewallDomainLists` query.
///
/// `ListFirewallDomainLists` models no filter at all — only `MaxResults` and
/// `NextToken` — so every query of it pages ONE collection and the scope is a
/// constant. It still names the operation, so a cursor minted by another list
/// operation is not read as an offset into this one (see
/// [`endpoint_list_scope`]).
pub const FIREWALL_DOMAIN_LIST_LIST_SCOPE: &str = "ListFirewallDomainLists/all/v1";

/// The cursor scope of one `ListFirewallConfigs` query.
///
/// `ListFirewallConfigs` models no filter either, so — for the same reason as
/// [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`] — the scope is a constant that names the
/// operation.
pub const FIREWALL_CONFIG_LIST_SCOPE: &str = "ListFirewallConfigs/all/v1";

/// The cursor scope of one `ListFirewallRuleGroups` query.
///
/// `ListFirewallRuleGroups` models no filter either — only `MaxResults` and
/// `NextToken` (`smithy_query(query="operation",operation="ListFirewallRuleGroups")`)
/// — so, for the same reason as [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`], the scope
/// is a constant that names the operation.
pub const FIREWALL_RULE_GROUP_LIST_SCOPE: &str = "ListFirewallRuleGroups/all/v1";

/// The cursor scope of one `ListFirewallRules` query.
///
/// The offset a cursor carries is an index into the collection the query
/// selects, which the rule group and the `Priority` and `Action` filters narrow
/// (`smithy_query(query="operation",operation="ListFirewallRules")`), so — for
/// the same reason as [`endpoint_list_scope`] — resuming with a different group
/// or filter is answered as an unknown cursor rather than with a page cut at the
/// wrong position.
pub fn firewall_rule_list_scope(
    firewall_rule_group_id: &str,
    priority: Option<i32>,
    action: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListFirewallRules/group-priority-action/v1");
    feed_field(&mut hasher, firewall_rule_group_id.as_bytes());
    if let Some(priority) = priority {
        feed_field(&mut hasher, priority.to_string().as_bytes());
    }
    if let Some(action) = action {
        feed_field(&mut hasher, action.as_bytes());
    }
    format!("ListFirewallRules/{}", hex::encode(&hasher.finalize()[..8]))
}

/// The cursor scope of one `ListFirewallRuleGroupAssociations` query.
///
/// Its four filters (`FirewallRuleGroupId`, `VpcId`, `Priority`, `Status`) each
/// narrow the collection a page is cut out of
/// (`smithy_query(query="operation",operation="ListFirewallRuleGroupAssociations")`),
/// so they belong in the scope for the same reason as in
/// [`firewall_rule_list_scope`].
pub fn firewall_rule_group_association_list_scope(
    firewall_rule_group_id: Option<&str>,
    vpc_id: Option<&str>,
    priority: Option<i32>,
    status: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"ListFirewallRuleGroupAssociations/group-vpc-priority-status/v1");
    if let Some(firewall_rule_group_id) = firewall_rule_group_id {
        feed_field(&mut hasher, firewall_rule_group_id.as_bytes());
    }
    if let Some(vpc_id) = vpc_id {
        feed_field(&mut hasher, vpc_id.as_bytes());
    }
    if let Some(priority) = priority {
        feed_field(&mut hasher, priority.to_string().as_bytes());
    }
    if let Some(status) = status {
        feed_field(&mut hasher, status.as_bytes());
    }
    format!(
        "ListFirewallRuleGroupAssociations/{}",
        hex::encode(&hasher.finalize()[..8])
    )
}

/// The largest page `ListFirewallConfigs` may be asked for, from the range the
/// model puts on its `MaxResults` (5..10,
/// `smithy_query(query="operation",operation="ListFirewallConfigs")`), which is
/// narrower than the usual 1..100 the other list operations carry.
pub const MAX_FIREWALL_CONFIGS_PER_PAGE: i32 = 10;

/// The cursor scope of one `ListResolverConfigs` query.
///
/// `ListResolverConfigs` models no filter either — only `MaxResults` and
/// `NextToken` (`smithy_query(query="operation",operation="ListResolverConfigs")`)
/// — so, for the same reason as [`FIREWALL_DOMAIN_LIST_LIST_SCOPE`], the scope
/// is a constant that names the operation.
pub const RESOLVER_CONFIG_LIST_SCOPE: &str = "ListResolverConfigs/all/v1";

/// The `InvalidParameterException` `ListResolverConfigs` answers for a
/// `NextToken` this service did not mint for that query.
///
/// `ListResolverConfigs` models both `InvalidNextTokenException` and
/// `InvalidParameterException`, and the one it answers for a token the service
/// never issued is the PARAMETER fault: the recorded responses of that operation
/// carry `InvalidParameterException` at HTTP 400, with the token the request sent
/// quoted inside the message. The sentence is therefore built from the request —
/// two different tokens give two different messages — and the parameter at fault
/// is named in the sentence itself, so no `FieldName` is added beside it, as for
/// [`subnet_not_found`].
pub fn invalid_next_token_parameter(next_token: &str) -> InvalidParameterException {
    InvalidParameterException {
        message: format!(
            "Value ( {next_token} ) for parameter NextToken is invalid. The token is invalid."
        ),
        field_name: None,
    }
}

/// One page of a `@paginated` list operation.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The cursor that resumes where this page stopped, or `None` on the last
    /// page.
    pub next_token: Option<String>,
    /// The page size this page was actually cut to: the `MaxResults` the request
    /// asked for, or the documented default when it named none.
    ///
    /// It comes from the ONE place the page size is decided ([`paginate_up_to`]),
    /// so an operation whose response models `MaxResults` echoes the size it
    /// really applied rather than a second reading of the request.
    pub max_results: i32,
}

/// The checksum carried inside a pagination cursor.
fn page_token_digest(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN.as_bytes());
    hasher.update(b":");
    hasher.update(payload.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Mint the opaque cursor that resumes `scope`'s collection at `offset`.
///
/// `scope` names the collection being paged (it carries the endpoint id for a
/// per-endpoint collection, and the filter set for a filtered one), so a cursor
/// minted for one collection is not accepted for another.
pub fn mint_page_token(scope: &str, offset: usize) -> String {
    let payload = format!("{scope}:{offset}");
    let digest = page_token_digest(&payload);
    hex::encode(format!("{payload}:{digest}"))
}

/// Read a cursor this emulator minted for `scope`, or `None` for one it did not
/// — which a list operation answers with `InvalidNextTokenException`.
pub fn parse_page_token(scope: &str, token: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (payload, digest) = decoded.rsplit_once(':')?;
    if digest != page_token_digest(payload) {
        return None;
    }
    let (token_scope, offset) = payload.rsplit_once(':')?;
    if token_scope != scope {
        return None;
    }
    offset.parse().ok()
}

/// Cut one page out of `all`, honouring the `@paginated` contract: resume at the
/// cursor, truncate at `MaxResults`, and mint a cursor only when items remain.
///
/// `None` means `next_token` is not a cursor this emulator minted for `scope`.
pub fn paginate<T>(
    all: Vec<T>,
    scope: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page<T>> {
    paginate_up_to(all, scope, max_results, next_token, MAX_RESULTS_RANGE.1)
}

/// [`paginate`] for a list operation whose `MaxResults` range is not the usual
/// 1..100: `largest_page` is the maximum THAT operation's range declares, so a
/// page is never cut shorter than a caller the model lets ask for more.
pub fn paginate_up_to<T>(
    all: Vec<T>,
    scope: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
    largest_page: i32,
) -> Option<Page<T>> {
    let offset = match next_token {
        Some(token) => parse_page_token(scope, token)?,
        None => 0,
    };
    let page_size = max_results
        .map(|requested| requested.clamp(MAX_RESULTS_RANGE.0, largest_page.max(MAX_RESULTS_RANGE.0)))
        .and_then(|bounded| usize::try_from(bounded).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE);

    // One item past the page is enough to know whether a cursor is owed, so the
    // allocation is bounded by the page size rather than by the collection.
    let mut items: Vec<T> = all
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
    let next_token = if items.len() > page_size {
        items.truncate(page_size);
        Some(mint_page_token(scope, offset.saturating_add(page_size)))
    } else {
        None
    };
    Some(Page {
        items,
        next_token,
        max_results: i32::try_from(page_size).unwrap_or(largest_page),
    })
}
