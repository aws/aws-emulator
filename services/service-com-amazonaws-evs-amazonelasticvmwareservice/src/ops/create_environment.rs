// SPDX-License-Identifier: CC0-1.0

//! @op CreateEnvironment
//! Mints an environment and persists it in [`crate::state`]: a new `environmentId`,
//! its ARN, `environmentState` `CREATING` (the clock owes it `CREATED`), and the ten
//! VLAN subnets materialised from `initialVlans`. Every other operation of this
//! service reads what this one stored.
//!
//! Those ten VLAN subnets are PROVISIONED, not just recorded: "Amazon EVS provisions
//! only the VLAN subnets" in `SELF_DEPLOYED` mode and provisions them plus VCF
//! otherwise, and they are subnets of the caller's own VPC — so each is created in the
//! account's Amazon EC2 store through EC2's own writer
//! ([`crate::state::ec2_resources::provision_vlan_subnets`]), and the `Vlan` reports
//! the `subnetId` and `availabilityZone` of the subnet EC2 holds.
//!
//! The `hosts` the request carries are MATERIALISED the same way, not merely recorded:
//! they are "The ESX hosts to add to the environment" (`hosts` doc) and "When you
//! specify any other VCF version, Amazon EVS installs and configures VCF for you" (this
//! operation's doc), so each entry becomes a host of the environment exactly as
//! `CreateEnvironmentHost` adds one — a `Host` record `ListEnvironmentHosts` reports,
//! `HostState` `CREATING` with the move to `CREATED` the clock owes it, a management
//! address out of the environment's own `vmkManagement` VLAN subnet, the EC2 bare metal
//! instance an ESX host IS ([`crate::state::ec2_resources::launch_host_instance`]), and
//! the default ESX version of the environment's VCF version ("When Amazon EVS installs
//! VCF, the default ESX version for the selected VCF version will be used"), since no
//! member of this request names one. `SELF_DEPLOYED` is the one mode that adds none:
//! "no hosts are added and no VCF installation is performed. After the environment is
//! created, you can add hosts with CreateEnvironmentHost".
//!
//! The request-level rules it enforces beyond the modelled constraints, each from the
//! documentation quoted under "Modeled contract" below:
//!
//! 1. `dedicatedHostId` and `placementGroupId` cannot be used together in one
//!    `HostInfoForCreate`.
//! 2. A VLAN subnet CIDR block is between /24 and /28, and no two of an
//!    environment's VLAN subnets may overlap. A VLAN subnet Amazon EVS provisions
//!    PUBLIC is narrowed further, to exactly a /28: "If you plan to use a public HCX
//!    VLAN subnet, the following requirements must be met: Must have a /28 netmask"
//!    (`InitialVlans$hcx` doc) — the extra requirement the public case carries, which
//!    the general range alone would admit a larger block for. The other requirements
//!    of that same list (the block allocated from the account's public IPAM pool, added
//!    to the VPC as a secondary CIDR block, and two Elastic IP addresses allocated from
//!    that pool) name no member of this request, so this operation has nothing to
//!    decide them from and refuses nothing for them.
//! 3. `hcxNetworkAclId` is required when `isHcxPublic` is true.
//! 4. An environment name is unique within the account and Region.
//! 5. `termsAccepted` must be true: it is the caller's confirmation that it holds the
//!    VCF licenses the environment needs, so an environment cannot be created without
//!    it.
//! 6. The `vpcId` is not already used by another deployed environment of this Region.
//! 7. The `vpcId` names a VPC the account holds, and the `serviceAccessSubnetId` a
//!    subnet of THAT VPC: an environment is deployed into the caller's existing
//!    network, so a request naming a VPC or subnet that does not exist is refused with
//!    a clause naming the id it gave ([`crate::state::ec2_resources`], which reads
//!    Amazon EC2's own store).
//! 8. Each VLAN subnet CIDR block is a block of that VPC's own address space and
//!    overlaps no subnet the VPC already holds: "Amazon EVS VLAN subnet CIDR blocks
//!    must not overlap with other subnets in the VPC" (`InitialVlans` doc) — rule 2 at
//!    the scope the doc states it, which is what lets each block become a real subnet
//!    of the VPC.
//! 9. A `placementGroupId` or `dedicatedHostId` an entry of `hosts` names resolves in
//!    that same EC2 store: a host is PLACED on those resources and Amazon EVS creates
//!    neither, so they are the caller's own, judged exactly as rule 7 judges the
//!    network ([`crate::state::ec2_resources::host_placement_failures`]). An entry
//!    refused by rule 1 names both mechanisms, so neither of its references is resolved.
//!    The `keyName` of every entry resolves there too: it is the "EC2 SSH keypair name"
//!    the member doc below asks each host for, so it names an Amazon EC2 key pair of the
//!    caller's own account — minted there by `CreateKeyPair` or `ImportKeyPair`, never by
//!    Amazon EVS — and it is resolved whatever an entry's placement members say, a key
//!    pair being no placement reference
//!    ([`crate::state::ec2_resources::host_key_pair_failures`]). Every reference an entry
//!    carries, placement and key pair alike, is resolved only where Amazon EVS INSTALLS
//!    VCF, exactly as rule 10 over the same member is: that is the mode in which the hosts
//!    are added at all, and a mode that adds none places nothing and reaches nothing, so
//!    an entry's ids and names name no resource that has to resolve.
//! 10. A `hostName` an entry of `hosts` names is free: "DNS hostnames for hosts must be
//!    unique across Amazon EVS environments and within VCF"
//!    (`HostInfoForCreate$hostName` doc), and the hosts this create adds TAKE their
//!    names — so a name a host of some environment already carries, or a name the
//!    request gives twice, names a host that cannot be added. The rule
//!    `CreateEnvironmentHost` applies to its single `host`, at the scope the doc states
//!    it, and judged only where the request's VCF version adds the hosts at all, because
//!    a mode that adds none takes no name. Two names are one name when they differ only
//!    in case, because a DNS name is case-insensitive
//!    ([`crate::state::same_dns_name`]).
//! 11. No `tags` key claims the reserved `aws:` namespace
//!    ([`crate::state::RESERVED_TAG_KEY_PREFIX`]): that namespace is Amazon Web
//!    Services' own, so a caller cannot create a tag in it — the same reservation
//!    `TagResource` and `UntagResource` enforce, applied here because this operation
//!    writes the request's tags onto the environment it mints.
//! 12. No two members of `vcfHostnames` carry the same DNS hostname: "Each hostname
//!    must be unique" (`VcfHostnames` doc), because the nine members name nine
//!    separate VCF management appliances and a hostname resolves to one of them — so a
//!    hostname a member before it already claimed names an appliance that cannot be
//!    installed, and that member is refused. Judged only where Amazon EVS INSTALLS
//!    VCF, exactly as rule 10 is: a mode that installs no appliance claims no hostname.
//!    "The same" is read as rule 10 reads it, ignoring case.
//! 13. Each `serviceAccessSecurityGroups.securityGroups` entry names a security group
//!    the account holds IN that same VPC: those groups "allow traffic between the Amazon
//!    EVS control plane and your VPC for Amazon EVS service access"
//!    (`ServiceAccessSecurityGroups` doc) and Amazon EVS creates none of them, and a
//!    security group belongs to one VPC — rule 7 for the one other member of this
//!    request that names an existing EC2 resource of the caller's network
//!    ([`crate::state::ec2_resources::security_group_failures`]).
//! 14. Each `connectivityInfo.privateRouteServerPeerings` entry names a route server peer
//!    the account holds: they are "The unique IDs for private route server peers" and
//!    "during environment creation, the route server endpoints peer with the NSX uplink
//!    VLAN" (`ConnectivityInfo` docs), so each id names an Amazon EC2 resource of the
//!    caller's own account that Amazon EVS does not create — rule 7 for one more member
//!    naming an existing EC2 resource, asked of the same store
//!    ([`crate::state::ec2_resources::route_server_peer_failures`]). Judged only where
//!    Amazon EVS INSTALLS VCF, exactly as rules 10 and 12 are, because that peering is
//!    part of the installation and the member is "Not supported when `vcfVersion` is
//!    `SELF_DEPLOYED`".
//!
//! 15. The `initialVlans.hcxNetworkAclId` a request carries names a network access
//!    control list the account holds IN that same VPC: it is "A unique ID for a network
//!    access control list that the HCX VLAN uses" (`InitialVlans$hcxNetworkAclId` doc) —
//!    an Amazon EC2 resource of the caller's own account, which Amazon EVS does not
//!    create — and a network ACL belongs to one VPC, so it is judged exactly as rule 13
//!    judges the service access security groups, from the same store
//!    ([`crate::state::ec2_resources::network_acl_failures`]). Judged wherever the member
//!    is SUPPLIED, public HCX VLAN or not, because the id is stored on the HCX VLAN and
//!    reported back as `Vlan$networkAclId` either way.
//!
//! 16. The ACCOUNT holds an Amazon Web Services Premium Support subscription: an
//!    environment runs licensed VCF software under one, so an account holding none
//!    creates no environment, however well formed its request is. It is a fact about the
//!    account rather than about any member of the request, so it is read from the Support
//!    emulator's own store ([`crate::state::premium_support`]) — where an operator grants
//!    a plan out of band — and its clause is collected as the FIRST of the request's
//!    failures, ahead of every member-level check above it.
//!
//! 17. The `instanceType` of each `hosts` entry is one of the EC2 instance types Amazon
//!    EVS provides for the `vcfVersion` the request names: "EC2 instance types provided by
//!    Amazon EVS for this VCF version for creating environments"
//!    (`VcfVersionInfo$instanceTypes`) is a PER-VERSION list of this service's own version
//!    catalogue — the one `GetVersions` reports — so an entry asking for a type outside it
//!    asks for hardware Amazon EVS does not offer that version on, and is refused
//!    ([`crate::state::unprovided_instance_type_failure`]), the rule
//!    `CreateEnvironmentHost` applies to its single `host`. Judged only where Amazon EVS
//!    INSTALLS VCF, exactly as rules 9, 10 and 12 over this same member are: a mode that
//!    adds no host builds none, so an inert entry's instance type names no hardware
//!    anything runs on.
//!
//! Supplying a parameter the chosen VCF mode does not support is NOT one of them.
//! `SELF_DEPLOYED` "does not support" `licenseInfo`, `hosts`, `vcfHostnames`, `siteId`
//! and `connectivityInfo` in the sense that it does nothing with them — no hosts are
//! added and no VCF is installed in that mode — and the recorded wire confirms that a
//! request carrying all five of them together with `vcfVersion` `SELF_DEPLOYED` is NOT
//! refused for carrying them: the refusal it drew named the VPC and nothing else. The
//! members are therefore accepted and stored as supplied, exactly as `clientToken` is
//! ("If you supply input for this parameter, it will have no effect").
//!
//! "No effect" is what the rest of the service then owes them: a member this mode does
//! not support is reported back and nothing else reads a fact out of it — no host is
//! added for `hosts` ([`add_requested_hosts`]) and no license key is carried for
//! `licenseInfo` ([`crate::state::StoredEnvironment::license_keys`]), so neither this
//! environment's health checks nor another environment's see anything the mode does
//! nothing with. The preconditions above owe them the same: a check that derives a fact
//! out of an unsupported member — a name it claims, a resource it references — is asked
//! only in the mode that uses it (rules 9, 10, 12 and 14), so only the shape's own
//! structural constraints (rule 1) still judge it.
//!
//! Every one of those checks runs before anything is minted, and they are all
//! evaluated: a request that breaks several is refused ONCE, with one
//! `ValidationException` whose `fieldList` carries an entry per failed check and whose
//! `message` joins those entries ([`crate::state::aggregate_validation_error`]). A
//! refused request creates nothing.
//!
//! The store itself is bounded by [`crate::state::MAX_ENVIRONMENTS`]: this is the only
//! operation that grows it, and it refuses with the service's
//! `ServiceQuotaExceededException` once the cap is reached and no finished (`DELETED`)
//! record can be reclaimed.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEnvironment::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateEnvironment`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")
//! ```
//!
//! Creates an Amazon EVS environment that runs VCF software, such as SDDC Manager, NSX
//! Manager, and vCenter Server. When you specify SELF_DEPLOYED for vcfVersion, Amazon EVS
//! provisions only the VLAN subnets; no hosts are added and no VCF installation is
//! performed. After the environment is created, you can add hosts with
//! CreateEnvironmentHost and install VCF yourself. The licenseInfo, hosts, vcfHostnames,
//! siteId, and connectivityInfo parameters are not supported in this mode. When you specify
//! any other VCF version, Amazon EVS installs and configures VCF for you. For more
//! information, see Self-deployed mode in the Amazon EVS User Guide. When Amazon EVS
//! installs VCF, the default ESX version for the selected VCF version will be used. After a
//! host is added with a specific ESX version, it can only be upgraded using vCenter
//! Lifecycle Manager. You cannot use the dedicatedHostId and placementGroupId parameters
//! together in the same CreateEnvironment action. This results in a ValidationException
//! response.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..100, pattern ^[!-~]+$, @idempotencyToken) — This
//!   parameter is not used in Amazon EVS currently. If you supply input for this parameter,
//!   it will have no effect. A unique, case-sensitive identifier that you provide to ensure
//!   the idempotency of the environment creation request. If you do not specify a client
//!   token, a randomly generated token is used for the request to ensure idempotency.
//! * `connectivityInfo` (ConnectivityInfo) — The connectivity configuration for the
//!   environment. Amazon EVS requires that you specify two route server peer IDs. During
//!   environment creation, the route server endpoints peer with the NSX edges over the NSX
//!   uplink subnet, providing BGP-based dynamic routing for overlay networks. Not supported
//!   when vcfVersion is SELF_DEPLOYED.
//! * `environmentName` (EnvironmentName, len 1..100, pattern ^[a-zA-Z0-9_-]+$) — The name to
//!   give to your environment. The name can contain only alphanumeric characters
//!   (case-sensitive), hyphens, and underscores. It must start with an alphanumeric
//!   character, and can't be longer than 100 characters. The name must be unique within the
//!   Amazon Web Services Region and Amazon Web Services account that you're creating the
//!   environment in.
//! * `hosts` (HostInfoForCreateList, len 4..4) — The ESX hosts to add to the environment. For
//!   each host, provide the desired hostname, EC2 SSH keypair name, and EC2 instance type.
//!   Optionally, provide a partition or cluster placement group, or use Amazon EC2 Dedicated
//!   Hosts. Not supported when vcfVersion is SELF_DEPLOYED. In that case, you can add hosts
//!   using CreateEnvironmentHost after the environment is created.
//! * `initialVlans` (InitialVlans) **required** — The initial VLAN subnets for the Amazon EVS
//!   environment. For each Amazon EVS VLAN subnet, you must specify a non-overlapping CIDR
//!   block. Amazon EVS VLAN subnets have a minimum CIDR block size of /28 and a maximum size
//!   of /24.
//! * `kmsKeyId` (String) — A unique ID for the customer-managed KMS key that is used to
//!   encrypt the VCF credential pairs for SDDC Manager, NSX Manager, and vCenter appliances.
//!   These credentials are stored in Amazon Web Services Secrets Manager.
//! * `licenseInfo` (LicenseInfoList, len 1..1) — The license information that Amazon EVS
//!   requires to create an environment. Amazon EVS requires two license keys: a VCF solution
//!   key and a vSAN license key. The VCF solution key must meet minimum core requirements,
//!   and the vSAN license key must meet minimum capacity requirements for your selected
//!   instance type. For information about minimum license requirements, see the VCF
//!   subscriptions section in the Amazon EVS User Guide. VCF licenses can be used for only
//!   one Amazon EVS environment. Amazon EVS does not support reuse of VCF licenses for
//!   multiple environments. VCF license information can be retrieved from the Broadcom
//!   portal. Not supported when vcfVersion is SELF_DEPLOYED.
//! * `serviceAccessSecurityGroups` (ServiceAccessSecurityGroups) — The security group that
//!   controls communication between the Amazon EVS control plane and VPC. The default
//!   security group is used if a custom security group isn't specified. The security group
//!   should allow access to the following. TCP/UDP access to the DNS servers HTTPS/SSH access
//!   to the host management VLAN subnet HTTPS/SSH access to the Management VM VLAN subnet You
//!   should avoid modifying the security group rules after deployment, as this can break the
//!   persistent connection between the Amazon EVS control plane and VPC. This can cause
//!   future environment actions like adding or removing hosts to fail.
//! * `serviceAccessSubnetId` (SubnetId, len 15..24, pattern
//!   ^subnet-[a-f0-9]{8}([a-f0-9]{9})?$) **required** — The subnet that is used to establish
//!   connectivity between the Amazon EVS control plane and VPC. The Amazon EVS control plane
//!   uses this subnet to interface with your environment. This includes validating DNS
//!   records and enabling Amazon EVS Connectors.
//! * `siteId` (String) — The Broadcom Site ID that is allocated to you as part of your
//!   electronic software delivery. This ID allows customer access to the Broadcom portal, and
//!   is provided to you by Broadcom at the close of your software contract or contract
//!   renewal. Amazon EVS uses the Broadcom Site ID that you provide to meet Broadcom VCF
//!   license usage reporting requirements for Amazon EVS. Not supported when vcfVersion is
//!   SELF_DEPLOYED.
//! * `tags` (RequestTagMap, len 1..200) — Metadata that assists with categorization and
//!   organization. Each tag consists of a key and an optional value. You define both. Tags
//!   don't propagate to any other cluster or Amazon Web Services resources.
//! * `termsAccepted` (Boolean) **required** — Confirmation that the customer has purchased
//!   and will continue to maintain the required number of VCF software licenses to cover all
//!   physical processor cores in the Amazon EVS environment. Information about your VCF
//!   software in Amazon EVS will be shared with Broadcom to verify license compliance. Amazon
//!   EVS does not validate license keys. To validate license keys, visit the Broadcom support
//!   portal.
//! * `vcfHostnames` (VcfHostnames) — The DNS hostnames for the virtual machines that host the
//!   VCF management appliances. Provide hostnames for vCenter, NSX Manager, SDDC Manager, and
//!   Cloud Builder. Not supported when vcfVersion is SELF_DEPLOYED.
//! * `vcfVersion` (VcfVersion, one of: SELF_DEPLOYED | VCF-5.2.1 | VCF-5.2.2) **required** —
//!   The VCF version to use for the environment. SELF_DEPLOYED: You install VCF yourself. The
//!   licenseInfo, hosts, vcfHostnames, siteId, and connectivityInfo parameters are not
//!   supported. Any other valid value: Amazon EVS installs and configures VCF for you in the
//!   version you specify.
//! * `vpcId` (VpcId, len 12..21, pattern ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$) **required** — A
//!   unique ID for the VPC that the environment is deployed inside. Amazon EVS requires that
//!   all VPC subnets exist in a single Availability Zone in a Region where the service is
//!   available. The VPC that you specify must have a valid DHCP option set with domain name,
//!   at least two DNS servers, and an NTP server. These settings are used to configure your
//!   VCF appliances and hosts. The VPC cannot be used with any other deployed Amazon EVS
//!   environment. Amazon EVS does not provide multi-VPC support for environments at this
//!   time. Amazon EVS does not support the following Amazon Web Services networking options
//!   for NSX overlay connectivity: cross-Region VPC peering, Amazon S3 gateway endpoints, or
//!   Amazon …
//!
//! ## Output
//!
//! * `environment` (Environment) — A description of the created environment.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the specified constraints.
//!   You will see this exception if invalid inputs are provided for any of the Amazon EVS
//!   environment operations, or if a list operation is performed on an environment resource
//!   that is still initializing.

#[allow(unused_imports)]
use super::types::*;

// This service's OWN reading of an IPv4 CIDR block, shared with
// `CreateEnvironmentHost`, which places a host's management address inside the
// `vmkManagement` block with it. It judges a block the REQUEST supplied, before any
// subnet exists for it; a block judged against what Amazon EC2 holds goes through EC2's
// own CIDR helpers instead ([`crate::state::ec2_resources`]), because that is the store
// the answer comes from.
use crate::state::Ipv4Cidr;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEnvironmentError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEnvironmentError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.evs#CreateEnvironment",
    extra_errors = ["InternalServerException", "ServiceQuotaExceededException"]
)]
#[derive(Debug)]
pub enum CreateEnvironmentErrorModeled {
    ValidationException(ValidationException),
    /// Widened in from the service's own error set (this operation does not list it):
    /// the one honest answer when this service's store cannot be reached at all.
    InternalServerException(InternalServerException),
    /// Widened in from the service's own error set (`TagResource` lists it): the
    /// refusal when this emulator's store is full, which is a quota condition and not
    /// a defect in the request.
    ServiceQuotaExceededException(ServiceQuotaExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_environment::CreateEnvironmentError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEnvironmentError = crate::scaffolded::Scaffolded<CreateEnvironmentErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEnvironmentErrorModeled> for CreateEnvironmentError {
    fn from(modeled: CreateEnvironmentErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEnvironment` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEnvironment<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEnvironment<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEnvironment<Eng>
{
    type Input = CreateEnvironmentInput;
    type Output = CreateEnvironmentOutput;
    type Error = CreateEnvironmentError;

    const NAME: &'static str = "CreateEnvironment";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEnvironmentErrorModeled::InternalServerException(InternalServerException {
                message: "The Amazon EVS environment store is unavailable.".to_string(),
            })
        })?;
        // Authorization first, before anything about the request is evaluated: AWS
        // decides it at the IAM layer, and a create names no resource of its own yet.
        if let Some(denied) =
            crate::state::authorization::denied(&*self.engine, ctx, Self::NAME, None)
        {
            return Err(denied);
        }

        // Every request-level precondition, evaluated before anything is minted. They
        // are all collected rather than short-circuited: AWS reports a request's
        // failed preconditions together, so a caller can correct them in one round
        // trip instead of one refusal at a time.
        // Whether this environment's VCF version is one Amazon EVS INSTALLS, which is
        // what decides whether the request's `hosts` are added to the environment: "When
        // you specify SELF_DEPLOYED for vcfVersion, Amazon EVS provisions only the VLAN
        // subnets; no hosts are added and no VCF installation is performed", and "When
        // you specify any other VCF version, Amazon EVS installs and configures VCF for
        // you" (this operation's doc).
        let installs_vcf = crate::state::installs_vcf(&input.vcf_version);

        let mut failures: Vec<ValidationExceptionField> = Vec::new();
        // The account's own entitlement comes FIRST: an environment runs licensed VCF
        // software under an Amazon Web Services Premium Support subscription, so an
        // account that holds none creates none, however well formed the rest of the
        // request is. The subscription is the Support service's fact, read from its own
        // store ([`crate::state::premium_support`]).
        failures.extend(crate::state::premium_support::entitlement_failure(
            &*self.engine,
        ));
        failures.extend(terms_accepted_failure(&input));
        // A tag key in AWS's reserved `aws:` namespace is not the caller's to create,
        // whichever member carries it: the create path writes the request's `tags`
        // straight onto the environment, so it enforces the same reservation the
        // tagging operations do — here as one more entry of this operation's
        // aggregated `ValidationException`, the only client-input error it models.
        failures.extend(crate::state::reserved_tag_key_failure(
            "tags",
            input
                .tags
                .iter()
                .flat_map(|tags| tags.keys())
                .map(String::as_str),
        ));
        failures.extend(host_failures(input.hosts.as_deref().unwrap_or_default()));
        // "Each hostname must be unique" (`VcfHostnames` doc): the nine members name the
        // nine separate VMware appliances Amazon EVS installs, so a hostname one member
        // already claimed cannot name a second appliance, and the member that repeats it
        // is refused. Asked only where the appliances ARE installed, the same scope rule
        // 10 is judged at, because a mode that installs none claims no hostname.
        if installs_vcf {
            if let Some(hostnames) = input.vcf_hostnames.as_ref() {
                failures.extend(vcf_hostname_failures(hostnames));
            }
        }
        let vlan_plan = plan_vlans(&input.initial_vlans);
        failures.extend(vlan_failures(&input.initial_vlans, &vlan_plan));
        // The network the environment is deployed into, from Amazon EC2's own store,
        // read before this service's own lock is taken.
        //
        // LOCK ORDER. The checks and the provisioning below DO take EC2's store lock
        // while this service's guard is held, and `DeleteEnvironment` takes the two the
        // same way round; EC2 never reaches this service, so the order is consistent and
        // cannot cycle. Never add an EC2 read that takes the locks the other way round.
        failures.extend(crate::state::ec2_resources::network_failures(
            &*self.engine,
            &input.vpc_id,
            &input.service_access_subnet_id,
        ));
        // The EC2 resources each requested host is PLACED on, from that same store: a
        // `placementGroupId` or `dedicatedHostId` names a resource of the caller's own
        // account, which Amazon EVS does not create. An entry that names BOTH is already
        // refused for that by `host_failures`, so neither of its references is resolved.
        // Asked only where the hosts ARE added, exactly as the hostname rule over this
        // same member is: a mode that adds no host places none, so an id of an inert
        // entry names nothing that has to resolve.
        let host_member_prefixes: Vec<String> = (0..input.hosts.as_deref().unwrap_or_default().len())
            .map(|index| format!("hosts[{index}]"))
            .collect();
        if installs_vcf {
            let references: Vec<crate::state::ec2_resources::HostPlacementReferences<'_>> = input
                .hosts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .zip(host_member_prefixes.iter())
                .filter(|(host, _)| {
                    !(host.dedicated_host_id.is_some() && host.placement_group_id.is_some())
                })
                .map(
                    |(host, member_prefix)| crate::state::ec2_resources::HostPlacementReferences {
                        member_prefix: member_prefix.as_str(),
                        placement_group_id: host.placement_group_id.as_deref(),
                        dedicated_host_id: host.dedicated_host_id.as_deref(),
                    },
                )
                .collect();
            failures.extend(crate::state::ec2_resources::host_placement_failures(
                &*self.engine,
                &references,
            ));
        }
        // The SSH key pair each entry names is a resource of that same account too: a host
        // supplies the "EC2 SSH keypair name" (this operation's `hosts` doc), which Amazon
        // EC2 mints and Amazon EVS does not, so the name is resolved in the account's own
        // key pair store — read by name the way EC2 itself reads one. Resolved for every
        // entry, whatever its placement members say: a key pair is no placement reference,
        // and the launch of a host writes this name onto its instance either way. Asked,
        // like the placement references above, only where the hosts ARE added: a mode that
        // adds no host reaches none, so an inert entry's key pair name names nothing.
        if installs_vcf {
            let members: Vec<String> = host_member_prefixes
                .iter()
                .map(|member_prefix| format!("{member_prefix}.keyName"))
                .collect();
            let references: Vec<crate::state::ec2_resources::HostKeyPairReference<'_>> = input
                .hosts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .zip(members.iter())
                .map(
                    |(host, member)| crate::state::ec2_resources::HostKeyPairReference {
                        member,
                        key_name: host.key_name.as_str(),
                    },
                )
                .collect();
            failures.extend(crate::state::ec2_resources::host_key_pair_failures(
                &*self.engine,
                &references,
            ));
        }
        // "EC2 instance types provided by Amazon EVS for this VCF version for creating
        // environments" (`VcfVersionInfo$instanceTypes`): the hardware each entry is built
        // on is one of the instance types this service's own version catalogue provides for
        // the VCF version this request names — the per-version restriction
        // `CreateEnvironmentHost` applies to its single host, through the same helper. Asked
        // only where the hosts ARE added, exactly as the placement and key pair rules over
        // this same member are: a mode that adds no host builds none, so an inert entry's
        // instance type names no hardware anything runs on.
        if installs_vcf {
            failures.extend(
                input
                    .hosts
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .zip(host_member_prefixes.iter())
                    .filter_map(|(host, member_prefix)| {
                        crate::state::unprovided_instance_type_failure(
                            &format!("{member_prefix}.instanceType"),
                            &host.instance_type,
                            &input.vcf_version,
                        )
                    }),
            );
        }
        // The route server peers `connectivityInfo` names are resources of that same
        // account: "The unique IDs for private route server peers", and "during
        // environment creation, the route server endpoints peer with the NSX uplink VLAN"
        // (`ConnectivityInfo` docs) — Amazon EVS creates no peer, so each id has to name
        // one the account already holds, resolved exactly as the network is. Asked only
        // where Amazon EVS INSTALLS VCF, because that is the mode the peering is part of
        // and the member is "Not supported when vcfVersion is SELF_DEPLOYED", where it is
        // accepted and does nothing.
        if installs_vcf {
            let named_peers: Vec<(String, &str)> = input
                .connectivity_info
                .as_ref()
                .map(|info| info.private_route_server_peerings.as_slice())
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(index, peer_id)| {
                    (
                        format!("connectivityInfo.privateRouteServerPeerings[{index}]"),
                        peer_id.as_str(),
                    )
                })
                .collect();
            let references: Vec<crate::state::ec2_resources::RouteServerPeerReference<'_>> =
                named_peers
                    .iter()
                    .map(
                        |(member, peer_id)| crate::state::ec2_resources::RouteServerPeerReference {
                            member,
                            peer_id,
                        },
                    )
                    .collect();
            failures.extend(crate::state::ec2_resources::route_server_peer_failures(
                &*self.engine,
                &references,
            ));
        }

        let region = crate::state::request_region(ctx.parts());
        let mut state = state.lock();
        state.materialise_due_transitions();

        // The two preconditions that depend on stored state rather than on the
        // request alone, so they are gathered with the lock held.
        if let Some(name) = input.environment_name.as_deref() {
            if state.name_is_taken(&region, name) {
                failures.push(crate::state::field_failure(
                    "environmentName",
                    &format!("is already used by an environment in {region}"),
                ));
            }
        }
        if state.vpc_is_in_use(&region, &input.vpc_id) {
            failures.push(crate::state::field_failure(
                "vpcId",
                "is already used by another deployed Amazon EVS environment, which Amazon EVS \
                 does not support",
            ));
        }
        // "DNS hostnames for hosts must be unique across Amazon EVS environments and
        // within VCF" (`HostInfoForCreate$hostName` doc): the hosts this create adds take
        // their names, so a name a host of some environment already carries — or a name
        // this one request gives twice — names a host that cannot be added, and the entry
        // that named it is refused. Asked only where the hosts ARE added, because a mode
        // that adds none takes no name.
        //
        // Both readings compare the name as a DNS name ([`crate::state::same_dns_name`]):
        // two entries whose hostnames differ only in case name one host, exactly as an
        // entry colliding with a stored host does.
        if installs_vcf {
            let mut named: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for (host, member_prefix) in input
                .hosts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .zip(host_member_prefixes.iter())
            {
                let host_name = host.host_name.as_str();
                let clause = if state.host_name_is_taken(host_name) {
                    "is already used by a host of an Amazon EVS environment; host DNS \
                     hostnames must be unique across environments"
                } else if !named.insert(host_name.to_ascii_lowercase()) {
                    "is used by another host of this request; host DNS hostnames must be \
                     unique across environments"
                } else {
                    continue;
                };
                failures.push(crate::state::field_failure(
                    &format!("{member_prefix}.hostName"),
                    clause,
                ));
            }
        }

        // The VLAN subnets are provisioned INSIDE that VPC, so each block must be a
        // block of the VPC's own address space and overlap no subnet the VPC already
        // holds ("Amazon EVS VLAN subnet CIDR blocks must not overlap with other
        // subnets in the VPC", `InitialVlans` doc) — the same overlap rule as among the
        // environment's own ten blocks, at the scope the doc states it.
        //
        // Judged only when there IS a VPC this environment can be deployed into: a
        // `vpcId` already refused (it names no VPC, or it carries another environment)
        // leaves nothing to place the blocks in, so the refusal names only the VPC —
        // exactly as an unresolved VPC leaves no VPC to judge the service access subnet
        // against. And only for the members no clause above already refuses, so one
        // member draws one clause.
        let vpc_refused = failures.iter().any(|failure| failure.name == "vpcId");
        if !vpc_refused {
            let placeable: Vec<crate::state::ec2_resources::VlanSubnetRequest<'_>> = {
                let refused: std::collections::BTreeSet<&str> =
                    failures.iter().map(|failure| failure.name.as_str()).collect();
                vlan_plan
                    .iter()
                    .filter(|planned| !refused.contains(planned.member))
                    .map(|planned| crate::state::ec2_resources::VlanSubnetRequest {
                        member: planned.member,
                        cidr: planned.cidr,
                    })
                    .collect()
            };
            failures.extend(crate::state::ec2_resources::vlan_placement_failures(
                &*self.engine,
                &input.vpc_id,
                &placeable,
            ));

            // The security groups the request names are the caller's own too: they
            // "allow traffic between the Amazon EVS control plane and your VPC for
            // Amazon EVS service access" (`ServiceAccessSecurityGroups` doc) and Amazon
            // EVS creates none of them, so each has to be a group the account holds IN
            // that VPC — a security group belongs to one VPC, so a group of another one
            // cannot carry this VPC's traffic. Judged exactly as the service access
            // subnet is, from the same store, and only once there IS a VPC to judge
            // membership against.
            let named_groups: Vec<(String, &str)> = input
                .service_access_security_groups
                .as_ref()
                .and_then(|groups| groups.security_groups.as_deref())
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(|(index, group_id)| {
                    (
                        format!("serviceAccessSecurityGroups.securityGroups[{index}]"),
                        group_id.as_str(),
                    )
                })
                .collect();
            let references: Vec<crate::state::ec2_resources::SecurityGroupReference<'_>> =
                named_groups
                    .iter()
                    .map(
                        |(member, group_id)| crate::state::ec2_resources::SecurityGroupReference {
                            member,
                            group_id,
                        },
                    )
                    .collect();
            failures.extend(crate::state::ec2_resources::security_group_failures(
                &*self.engine,
                &input.vpc_id,
                &references,
            ));

            // The network access control list the HCX VLAN uses is one of the caller's own
            // too: it is "A unique ID for a network access control list that the HCX VLAN
            // uses" (`hcxNetworkAclId` doc), Amazon EVS creates no network ACL, and a
            // network ACL belongs to ONE VPC — so a list of some other VPC cannot govern a
            // VLAN subnet provisioned in this one. Resolved wherever the member is
            // SUPPLIED, public HCX VLAN or not, because the id is stored on the HCX VLAN
            // and reported back as `Vlan$networkAclId` either way; and, like the security
            // groups, only once there IS a VPC to judge membership against.
            let named_network_acls: Vec<crate::state::ec2_resources::NetworkAclReference<'_>> =
                input
                    .initial_vlans
                    .hcx_network_acl_id
                    .as_deref()
                    .map(
                        |network_acl_id| crate::state::ec2_resources::NetworkAclReference {
                            member: HCX_NETWORK_ACL_MEMBER,
                            network_acl_id,
                        },
                    )
                    .into_iter()
                    .collect();
            failures.extend(crate::state::ec2_resources::network_acl_failures(
                &*self.engine,
                &input.vpc_id,
                &named_network_acls,
            ));
        }

        if !failures.is_empty() {
            return Err(CreateEnvironmentErrorModeled::ValidationException(
                crate::state::aggregate_validation_error(failures),
            )
            .into());
        }
        let requested_hosts = input.hosts.unwrap_or_default();

        // The ten VLAN subnets have to fit in the account's EC2 store, and that is
        // asked BEFORE anything of this service's own is minted: a request that cannot
        // be served must not consume an environment id.
        if let Some((held, limit)) =
            crate::state::ec2_resources::subnet_room_shortfall(&*self.engine, vlan_plan.len())
        {
            return Err(subnet_room_refusal(held, limit, vlan_plan.len()));
        }

        // Bound the store before anything is minted: a generated emulator holds its
        // resources for the life of the process, so the create path is the one that
        // must refuse rather than grow without limit. This asks only whether there is
        // room — a pure read, so a request refused further down leaves every stored
        // record where it is; the purge that MAKES the room runs immediately before the
        // insert below ([`crate::state::ComAmazonawsEvsAmazonelasticvmwareserviceState::reclaim_environment_slot`]).
        if !state.has_room_for_environment() {
            return Err(CreateEnvironmentErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "This emulator holds at most {} environments; delete one before \
                         creating another.",
                        crate::state::MAX_ENVIRONMENTS
                    ),
                },
            )
            .into());
        }

        let created_at_ms = crate::state::now_ms();
        let environment_id = state.mint_environment_id();

        // The ten VLAN subnets are REAL subnets of the caller's VPC: Amazon EVS
        // "provisions the VLAN subnets", so they are created in the account's own
        // Amazon EC2 store and each VLAN reports the id, the block and the Availability
        // Zone EC2 gave its subnet. The EC2 store is written while this service's lock
        // is held, and in the same order by `DeleteEnvironment`, so the two locks are
        // always taken this way round.
        let provisioning = {
            let planned: Vec<crate::state::ec2_resources::VlanSubnetRequest<'_>> = vlan_plan
                .iter()
                .map(|planned| crate::state::ec2_resources::VlanSubnetRequest {
                    member: planned.member,
                    cidr: planned.cidr,
                })
                .collect();
            crate::state::ec2_resources::provision_vlan_subnets(
                &*self.engine,
                &region,
                &input.vpc_id,
                &input.service_access_subnet_id,
                &planned,
            )
        };
        let provisioned = match provisioning {
            // One subnet per planned VLAN, in the order they were asked for. A shorter
            // answer than the plan would leave some VLAN naming no subnet, so it is an
            // internal inconsistency rather than something to paper over: the subnets
            // that WERE created go back, and nothing is stored.
            crate::state::ec2_resources::VlanSubnetProvisioning::Provisioned(subnets)
                if subnets.len() == vlan_plan.len() =>
            {
                subnets
            }
            crate::state::ec2_resources::VlanSubnetProvisioning::Provisioned(subnets) => {
                release(&*self.engine, &subnets);
                return Err(CreateEnvironmentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: format!(
                            "Amazon EC2 provisioned {provisioned} of the {planned} VLAN \
                             subnets the environment needs.",
                            provisioned = subnets.len(),
                            planned = vlan_plan.len(),
                        ),
                    },
                )
                .into());
            }
            // No EC2 store to provision them in is evidence of nothing, so the
            // environment is still created — with ids of this service's own, which is
            // all there is to name its VLANs by, and no Availability Zone.
            crate::state::ec2_resources::VlanSubnetProvisioning::NoEc2Store => Vec::new(),
            // A zone every subnet of the VPC is in is part of what a VLAN subnet IS, so
            // a Region that names none leaves nothing to provision: the same server
            // error EC2's own `CreateSubnet` answers for a Region with no zone, and
            // nothing was written.
            crate::state::ec2_resources::VlanSubnetProvisioning::ZoneUnknown => {
                return Err(CreateEnvironmentErrorModeled::InternalServerException(
                    InternalServerException {
                        message: format!(
                            "No Availability Zone could be resolved for the VLAN subnets of an \
                             environment in {region}."
                        ),
                    },
                )
                .into());
            }
            // Asked before anything was minted, so this is only reachable if another
            // request filled the EC2 store in between; nothing was written either way.
            crate::state::ec2_resources::VlanSubnetProvisioning::SubnetLimitExceeded {
                held,
                limit,
            } => return Err(subnet_room_refusal(held, limit, vlan_plan.len())),
        };

        let vlans = vlan_plan
            .into_iter()
            .enumerate()
            .map(|(index, planned)| {
                let subnet = provisioned.get(index);
                crate::state::StoredVlan {
                    // The 802.1Q tag the traffic type is carried on: unique within the
                    // environment, assigned in the order the `InitialVlans` members are
                    // modelled, and clear of the reserved default VLAN 1.
                    vlan_id: FIRST_VLAN_ID + index as i32,
                    // The block of the subnet EC2 holds, which is the requested block in
                    // the canonical form EC2 stores one in — so `Vlan$cidr` and the
                    // subnet's `CidrBlock` are one block and cannot disagree.
                    cidr: match subnet {
                        Some(subnet) => subnet.cidr_block.clone(),
                        None => planned.cidr.to_string(),
                    },
                    function_name: planned.function_name.to_string(),
                    subnet_id: match subnet {
                        Some(subnet) => subnet.subnet_id.clone(),
                        None => state.mint_subnet_id(),
                    },
                    availability_zone: subnet
                        .map(|subnet| subnet.availability_zone.clone()),
                    vlan_state: crate::state::STATE_CREATING.to_string(),
                    is_public: planned.is_public,
                    network_acl_id: planned.network_acl_id.map(str::to_string),
                    created_at_ms,
                    modified_at_ms: created_at_ms,
                    eip_associations: Vec::new(),
                    removed_eip_association_ids: Vec::new(),
                }
            })
            .collect();

        let environment = crate::state::StoredEnvironment {
            environment_arn: crate::state::environment_arn(&region, &environment_id),
            environment_id: environment_id.clone(),
            region,
            environment_name: input.environment_name,
            // Creation has started but is not finished: the clock owes this
            // environment, and its VLAN subnets, the move to CREATED.
            state: crate::state::STATE_CREATING.to_string(),
            state_details: None,
            created_at_ms,
            modified_at_ms: created_at_ms,
            vpc_id: input.vpc_id,
            service_access_subnet_id: input.service_access_subnet_id,
            vcf_version: input.vcf_version,
            terms_accepted: input.terms_accepted,
            kms_key_id: input.kms_key_id,
            site_id: input.site_id,
            license_info: input.license_info.unwrap_or_default(),
            connectivity_info: input.connectivity_info,
            vcf_hostnames: input.vcf_hostnames,
            service_access_security_groups: input.service_access_security_groups,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            vlans,
            // Health is DERIVED from the record: nothing is checked on an environment
            // that is still CREATING, and every reporting operation recomputes the
            // checks from the store
            // (`ComAmazonawsEvsAmazonelasticvmwareserviceState::recompute_environment_checks`).
            checks: Vec::new(),
            impaired_since_ms: std::collections::BTreeMap::new(),
            entitlements: std::collections::BTreeMap::new(),
            connectors: std::collections::BTreeMap::new(),
            // The hosts the request asked for are added to this map below, once the
            // record they belong to is stored.
            hosts: std::collections::BTreeMap::new(),
            pending: Some(crate::state::PendingTransition::starting_now(
                crate::state::STATE_CREATED,
                crate::state::STATE_CREATED,
            )),
            // No Custom Addon depot token has been issued for this environment yet:
            // `GetDepotUrl` is the action that issues one and the action that rotates it.
            depot_token: None,
        };

        let transition_due_at = environment.pending.as_ref().map(|pending| pending.due_at);
        // Unreachable: ids are minted from a strictly increasing seed. Answering
        // honestly beats overwriting another environment or reporting it as the created
        // one — and it is asked BEFORE the slot is reclaimed, so a create that cannot be
        // completed purges no finished record.
        if state.environments.contains_key(&environment_id) {
            // Nothing of this environment is stored, so the subnets provisioned for
            // it are released rather than left in the account's EC2 store with no
            // environment to delete them.
            release(&*self.engine, &provisioned);
            return Err(CreateEnvironmentErrorModeled::InternalServerException(
                InternalServerException {
                    message: format!("The environment id {environment_id} is already in use."),
                },
            )
            .into());
        }
        // Everything that can fail has run: make the room the check above found and
        // store the environment, so the purge and the insert cannot come apart.
        state.reclaim_environment_slot();
        let stored = state
            .environments
            .entry(environment_id)
            .or_insert(environment);
        // The ESX hosts the request asked Amazon EVS to ADD to the environment, added to
        // the record just stored: each one a host of this environment, which is what
        // `ListEnvironmentHosts` reports and what the next `CreateEnvironmentHost` finds
        // the name of. Done after the record is stored so the hosts, and the EC2
        // instances they run on, belong to an environment that exists.
        let host_transitions_due =
            add_requested_hosts(&*self.engine, stored, &requested_hosts, installs_vcf);
        let created = stored.to_wire();
        if let Some(due_at) = transition_due_at {
            state.note_transition_due(due_at);
        }
        // Each host runs on its own clock, so each owed move to CREATED is noted.
        for due_at in host_transitions_due {
            state.note_transition_due(due_at);
        }
        // A license key this environment carries is a key the environments that already
        // carry it now share, which is a check input of THEIRS, so the checks are
        // recomputed from the store this create leaves behind. The new environment is
        // still `CREATING`, so no check of it has run and it reports none.
        state.recompute_environment_checks();
        Ok(CreateEnvironmentOutput {
            environment: Some(created),
        })
    }
}

/// The first 802.1Q VLAN tag this service assigns. VLAN 1 is the reserved default
/// VLAN of an 802.1Q trunk, so an environment's own VLANs start clear of it.
const FIRST_VLAN_ID: i32 = 100;

/// Add the ESX hosts `requested` names to the stored `environment`, and answer the
/// instant each one's owed move to `CREATED` comes due.
///
/// `hosts` is "The ESX hosts to add to the environment", and Amazon EVS adds them while
/// it installs VCF, so each entry becomes a host of the environment and not a note about
/// one: the record `ListEnvironmentHosts` reports, carrying the values the entry supplied,
/// `CREATING` until the build the clock owes it, a management address out of the
/// environment's own `vmkManagement` VLAN subnet, and the EC2 bare metal instance an ESX
/// host IS. Every part of that is the rule `CreateEnvironmentHost` follows for the single
/// host it adds, through the same state constructor and the same EC2 writer, so a host of
/// a created environment and a host added afterwards are one kind of record.
///
/// `installs_vcf` false adds NOTHING: "When you specify SELF_DEPLOYED for vcfVersion,
/// Amazon EVS provisions only the VLAN subnets; no hosts are added and no VCF installation
/// is performed. After the environment is created, you can add hosts with
/// CreateEnvironmentHost" — in that mode the member is accepted and does nothing, exactly
/// as `clientToken` is.
fn add_requested_hosts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    environment: &mut crate::state::StoredEnvironment,
    requested: &[HostInfoForCreate],
    installs_vcf: bool,
) -> Vec<::std::time::Instant> {
    if !installs_vcf {
        return Vec::new();
    }
    // The Region and the service access subnet the launch reads are the environment's
    // own, cloned so the hosts can be written into the same record.
    let region = environment.region.clone();
    let service_access_subnet_id = environment.service_access_subnet_id.clone();
    // "When Amazon EVS installs VCF, the default ESX version for the selected VCF version
    // will be used" (this operation's doc): no member of this request names a version for
    // the hosts it adds, so each runs the default its VCF version's Bill Of Materials
    // names — the same catalogue `CreateEnvironmentHost` defaults from.
    let esx_version = crate::state::default_esx_version(&environment.vcf_version).map(str::to_string);
    let created_at_ms = environment.created_at_ms;
    let mut transitions_due = Vec::with_capacity(requested.len());
    for host in requested {
        // Defensive: the names are unique by the precondition above, so no entry can
        // displace a host already added. Read as a DNS name
        // ([`crate::state::StoredEnvironment::host`]), the way that precondition decided
        // two names are one.
        if environment.host(&host.host_name).is_some() {
            continue;
        }
        // The addresses the host takes out of the environment's own `vmkManagement` VLAN
        // subnet ([`crate::state::StoredEnvironment::next_host_ips`]) — its management
        // address and one more per further elastic network interface Amazon EVS provisions
        // for it, the rule that also bounds how many hosts one environment holds. Too few
        // addresses left, or no such VLAN, leaves the host unaddressable, so it is not added
        // rather than stored with an address no subnet of the environment covers.
        let Some(management_subnet_id) = environment
            .vmk_management_vlan()
            .map(|vlan| vlan.subnet_id.clone())
        else {
            break;
        };
        let Some(addresses) = environment.next_host_ips() else {
            break;
        };
        // "An ESX host that runs on an Amazon EC2 bare metal instance" (`Host` doc): the
        // instance IS the host, so it is launched into the account's own Amazon EC2 store.
        // An EC2 store this emulator cannot reach leaves the host reporting neither
        // `ec2InstanceId` nor `networkInterfaces`.
        let ec2 = crate::state::ec2_resources::launch_host_instance(
            engine,
            crate::state::ec2_resources::HostLaunch {
                region: &region,
                instance_type: &host.instance_type,
                key_name: &host.key_name,
                management_ip: &addresses.management,
                // …and the addresses its other interfaces hold, as the single-host path
                // passes them.
                standby_ips: &addresses.secondary,
                management_subnet_id: &management_subnet_id,
                service_access_subnet_id: &service_access_subnet_id,
                // Where the entry placed the host, which the instance records just as the
                // single-host path's does.
                placement_group_id: host.placement_group_id.as_deref(),
                dedicated_host_id: host.dedicated_host_id.as_deref(),
            },
        );
        // The host is CREATING until it has been built and joined the environment, which
        // is the move to CREATED the clock owes it.
        let pending = crate::state::PendingStatus::starting_now(crate::state::HOST_CREATED);
        transitions_due.push(pending.due_at);
        environment.insert_host(crate::state::StoredHost::creating(
            host,
            addresses,
            esx_version.clone(),
            created_at_ms,
            ec2,
            pending,
        ));
    }
    transitions_due
}

/// The refusal for an account whose Amazon EC2 store cannot take the environment's ten
/// VLAN subnets: a quota condition, not a defect in the request, so it is the
/// operation's `ServiceQuotaExceededException`. Stated in one place because both the
/// check before anything is reserved and the writer's own ceiling report it.
fn subnet_room_refusal(held: usize, limit: usize, planned: usize) -> CreateEnvironmentError {
    CreateEnvironmentErrorModeled::ServiceQuotaExceededException(ServiceQuotaExceededException {
        message: format!(
            "The account's Amazon EC2 store holds {held} of at most {limit} subnets, so the \
             {planned} VLAN subnets of an environment cannot be provisioned in it."
        ),
    })
    .into()
}

/// Give the subnets `provisioned` back to the account's Amazon EC2 store, for a create
/// that is refused after they were written: an environment that is not stored can never
/// be deleted, so nothing else would ever remove them.
fn release<Eng: Send + Sync + 'static>(
    engine: &Eng,
    provisioned: &[crate::state::ec2_resources::ProvisionedVlanSubnet],
) {
    let subnet_ids: Vec<String> = provisioned
        .iter()
        .map(|subnet| subnet.subnet_id.clone())
        .collect();
    crate::state::ec2_resources::release_vlan_subnets(engine, &subnet_ids);
}

/// The largest VLAN subnet prefix length: "Amazon EVS VLAN subnets have a minimum
/// CIDR block size of /28 and a maximum size of /24" (`InitialVlans` doc).
const MAX_VLAN_PREFIX_LEN: u8 = 28;
/// The smallest VLAN subnet prefix length. See [`MAX_VLAN_PREFIX_LEN`].
const MIN_VLAN_PREFIX_LEN: u8 = 24;
/// The one prefix length a VLAN subnet Amazon EVS provisions PUBLIC may carry: "If you
/// plan to use a public HCX VLAN subnet, the following requirements must be met: Must
/// have a /28 netmask" (`InitialVlans$hcx` doc).
const PUBLIC_VLAN_PREFIX_LEN: u8 = 28;

/// The prefix lengths a VLAN subnet block may carry: the published /24../28 range in
/// general, narrowed to exactly [`PUBLIC_VLAN_PREFIX_LEN`] for a VLAN Amazon EVS
/// provisions public, which the public case's own requirement adds.
///
/// Keyed on whether the VLAN is PUBLIC, not on which `InitialVlans` member it came
/// from: the narrowing belongs to the public provisioning, so it holds for every VLAN
/// a request asks to be public.
fn allowed_prefix_lens(is_public: bool) -> std::ops::RangeInclusive<u8> {
    if is_public {
        PUBLIC_VLAN_PREFIX_LEN..=PUBLIC_VLAN_PREFIX_LEN
    } else {
        MIN_VLAN_PREFIX_LEN..=MAX_VLAN_PREFIX_LEN
    }
}

/// One VLAN subnet `CreateEnvironment` is about to materialise.
struct PlannedVlan<'a> {
    /// The `functionName` the VLAN carries — the `InitialVlans` member's own name,
    /// which is the form the `Vlan` doc gives (a `functionName` of `hcx`).
    function_name: &'static str,
    /// The path of the `InitialVlans` member this VLAN came from, so a refusal names
    /// the member the caller sent.
    member: &'static str,
    /// The CIDR block the request supplied for it.
    cidr: &'a str,
    /// Whether Amazon EVS provisions this VLAN as public.
    is_public: bool,
    /// The network ACL the VLAN uses, when the request named one.
    network_acl_id: Option<&'a str>,
}

/// The failure, if any, of the precondition that the caller accepted the terms:
/// `termsAccepted` is the "Confirmation that the customer has purchased and will
/// continue to maintain the required number of VCF software licenses to cover all
/// physical processor cores in the Amazon EVS environment" (its own doc), so an
/// environment cannot be created while it is false.
fn terms_accepted_failure(input: &CreateEnvironmentInput) -> Option<ValidationExceptionField> {
    (!input.terms_accepted).then(|| {
        crate::state::field_failure_stated(
            "termsAccepted",
            TERMS_NOT_ACCEPTED_CLAUSE,
        )
    })
}

/// The clause Amazon EVS states for a request that has not accepted the terms. Fixed
/// text in full: it names no member and interpolates nothing from the request.
const TERMS_NOT_ACCEPTED_CLAUSE: &str =
    "Terms and Conditions must be accepted before creating an environment";

/// Every host that names both placement mechanisms, refused through the shape's own
/// rule ([`crate::state::placement_conflict_failure`]), which `CreateEnvironmentHost`
/// applies to its single `host` for the same reason: "You cannot use the
/// `dedicatedHostId` and `placementGroupId` parameters together in the same
/// `HostInfoForCreate` object. This results in a `ValidationException` response."
fn host_failures(hosts: &[HostInfoForCreate]) -> Vec<ValidationExceptionField> {
    hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| host.dedicated_host_id.is_some() && host.placement_group_id.is_some())
        .map(|(index, _)| crate::state::placement_conflict_failure(&format!("hosts[{index}]")))
        .collect()
}

/// The nine `vcfHostnames` members, in the order `VcfHostnames` models them: the input
/// member path a refusal names each one by, and the hostname the request gave it.
///
/// One table, so the uniqueness rule below covers every member of the shape and cannot
/// silently miss one.
fn vcf_hostname_members(hostnames: &VcfHostnames) -> [(&'static str, &str); 9] {
    [
        ("vcfHostnames.vCenter", hostnames.v_center.as_str()),
        ("vcfHostnames.nsx", hostnames.nsx.as_str()),
        ("vcfHostnames.nsxManager1", hostnames.nsx_manager1.as_str()),
        ("vcfHostnames.nsxManager2", hostnames.nsx_manager2.as_str()),
        ("vcfHostnames.nsxManager3", hostnames.nsx_manager3.as_str()),
        ("vcfHostnames.nsxEdge1", hostnames.nsx_edge1.as_str()),
        ("vcfHostnames.nsxEdge2", hostnames.nsx_edge2.as_str()),
        ("vcfHostnames.sddcManager", hostnames.sddc_manager.as_str()),
        ("vcfHostnames.cloudBuilder", hostnames.cloud_builder.as_str()),
    ]
}

/// Every `vcfHostnames` member that repeats a hostname an earlier member of the same
/// object already carries: "Each hostname must be unique" (`VcfHostnames` doc).
///
/// The FIRST member to carry a hostname keeps it and the ones that repeat it are
/// refused, one `fieldList` entry each naming the member it collides with — so three
/// members sharing one hostname draw two entries, and the caller is told which member
/// claimed the name. The hostnames are compared as DNS names
/// ([`crate::state::same_dns_name`]): a member whose spelling differs from an earlier
/// member's only in case names the same appliance, as the sibling DNS-hostname rule reads
/// host names
/// ([`crate::state::ComAmazonawsEvsAmazonelasticvmwareserviceState::host_name_is_taken`]).
fn vcf_hostname_failures(hostnames: &VcfHostnames) -> Vec<ValidationExceptionField> {
    let members = vcf_hostname_members(hostnames);
    let mut failures = Vec::new();
    for (index, (member, hostname)) in members.iter().enumerate() {
        if let Some((claimed_by, _)) = members[..index]
            .iter()
            .find(|(_, earlier)| crate::state::same_dns_name(earlier, hostname))
        {
            failures.push(crate::state::field_failure(
                member,
                &format!(
                    "repeats the hostname given as {claimed_by}; each hostname in \
                     vcfHostnames must be unique"
                ),
            ));
        }
    }
    failures
}

/// The ten VLAN subnets to materialise, in the order `InitialVlans` models its
/// members. The layout alone: the rules that govern the blocks are
/// [`vlan_failures`].
fn plan_vlans(initial: &InitialVlans) -> Vec<PlannedVlan<'_>> {
    vec![
        PlannedVlan {
            function_name: crate::state::VMK_MANAGEMENT_FUNCTION,
            member: "initialVlans.vmkManagement",
            cidr: &initial.vmk_management.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vmManagement",
            member: "initialVlans.vmManagement",
            cidr: &initial.vm_management.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vMotion",
            member: "initialVlans.vMotion",
            cidr: &initial.v_motion.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vSan",
            member: "initialVlans.vSan",
            cidr: &initial.v_san.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "vTep",
            member: "initialVlans.vTep",
            cidr: &initial.v_tep.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "edgeVTep",
            member: "initialVlans.edgeVTep",
            cidr: &initial.edge_v_tep.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "nsxUplink",
            member: "initialVlans.nsxUplink",
            cidr: &initial.nsx_uplink.cidr,
            is_public: false,
            network_acl_id: None,
        },
        // Only the HCX VLAN can be public, and only it carries a network ACL id.
        PlannedVlan {
            function_name: "hcx",
            member: "initialVlans.hcx",
            cidr: &initial.hcx.cidr,
            is_public: initial.is_hcx_public,
            network_acl_id: initial.hcx_network_acl_id.as_deref(),
        },
        PlannedVlan {
            function_name: "expansionVlan1",
            member: "initialVlans.expansionVlan1",
            cidr: &initial.expansion_vlan1.cidr,
            is_public: false,
            network_acl_id: None,
        },
        PlannedVlan {
            function_name: "expansionVlan2",
            member: "initialVlans.expansionVlan2",
            cidr: &initial.expansion_vlan2.cidr,
            is_public: false,
            network_acl_id: None,
        },
    ]
}

/// The input path of the member that names the network access control list the HCX VLAN
/// uses, so the clause for its absence and the clause for an id the account's own Amazon
/// EC2 store cannot resolve name the same member.
const HCX_NETWORK_ACL_MEMBER: &str = "initialVlans.hcxNetworkAclId";

/// Every rule the request's VLAN subnets break: the HCX network ACL the public HCX
/// VLAN requires, and, per VLAN subnet, a parsable CIDR block of a size that VLAN is
/// allowed ([`allowed_prefix_lens`], which narrows for a public one) and that overlaps
/// none of the environment's other VLAN subnets.
fn vlan_failures(
    initial: &InitialVlans,
    planned: &[PlannedVlan<'_>],
) -> Vec<ValidationExceptionField> {
    let mut failures = Vec::new();

    // "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).
    if initial.is_hcx_public && initial.hcx_network_acl_id.is_none() {
        failures.push(crate::state::field_failure(
            HCX_NETWORK_ACL_MEMBER,
            "is required when isHcxPublic is true",
        ));
    }

    let mut networks: Vec<(&'static str, Ipv4Cidr)> = Vec::with_capacity(planned.len());
    for vlan in planned {
        let member = vlan.member;
        // The modelled `Cidr` pattern already refused anything that is not a
        // dotted-quad with a prefix length, so a parse failure here is a value that
        // pattern admits and no IPv4 subnet can be.
        let Some(network) = Ipv4Cidr::parse(vlan.cidr) else {
            failures.push(crate::state::field_failure(
                member,
                "is not a valid IPv4 CIDR block",
            ));
            continue;
        };
        let prefix_len = network.prefix_len;
        let allowed = allowed_prefix_lens(vlan.is_public);
        if !allowed.contains(&prefix_len) {
            let (smallest, largest) = (*allowed.start(), *allowed.end());
            let problem = if smallest == largest {
                format!(
                    "must have a /{smallest} netmask when the VLAN subnet is provisioned \
                     public, but was /{prefix_len}"
                )
            } else {
                format!(
                    "must have a prefix length between /{smallest} and /{largest}, but \
                     was /{prefix_len}"
                )
            };
            failures.push(crate::state::field_failure(member, &problem));
        }
        // "Amazon EVS VLAN subnet CIDR blocks must not overlap with other subnets"
        // (`InitialVlans` doc).
        if let Some((other, _)) = networks.iter().find(|(_, other)| other.overlaps(&network)) {
            failures.push(crate::state::field_failure(
                member,
                &format!("overlaps the CIDR block of {other}"),
            ));
        }
        networks.push((member, network));
    }

    failures
}

// The `InitialVlans` member a VLAN's `functionName` came from is carried on
// `PlannedVlan::member`, so no reverse lookup from the wire name is needed.
