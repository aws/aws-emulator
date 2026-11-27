// SPDX-License-Identifier: CC0-1.0

//! @op CreateEnvironmentHost
//! Adds an ESX host to a stored environment and persists it in [`crate::state`]: the
//! host name, key and instance type the request gave, a management IP address taken
//! out of the environment's own `vmkManagement` VLAN subnet, and `HostState`
//! `CREATING` (the clock owes it `CREATED`). `ListEnvironmentHosts` reports what this
//! operation stored.
//!
//! An Amazon EVS host IS an EC2 bare metal instance — "An ESX host that runs on an
//! Amazon EC2 bare metal instance" (`Host` doc) — so this operation also LAUNCHES that
//! instance into the account's own Amazon EC2 store, from the `keyName` and
//! `instanceType` the request gave, in the environment's `vmkManagement` VLAN subnet and
//! on its management address, and attaches the two ENIs Amazon EVS gives a host
//! (`NetworkInterface` doc). The host reports that instance's id as `ec2InstanceId` and
//! those interfaces as `networkInterfaces`, so a caller can look the host's instance up
//! with EC2's own `DescribeInstances`. The launch happens after every precondition below
//! has passed, so a refused request launches nothing; an EC2 store this emulator cannot
//! reach leaves both members absent rather than naming resources that do not exist
//! ([`crate::state::ec2_resources::launch_host_instance`]).
//!
//! The rules it enforces, each from the documentation quoted under "Modeled contract"
//! below. This operation's error contract is `ThrottlingException` and
//! `ValidationException` ONLY — it declares no `ResourceNotFoundException` — so a
//! well-formed `environmentId` that names no environment is refused with the
//! `ValidationException` that names it ([`crate::state::environment_not_found_validation`]):
//!
//! 1. The `environmentId` names an environment this service holds, in the Region the
//!    request was addressed to. Refused on its own, because a request against an
//!    environment that is not there has nothing further to judge.
//! 2. "This action can only be used after the Amazon EVS environment is deployed": an
//!    environment that has not reached `CREATED` cannot take a host yet.
//! 3. "You cannot use the `dedicatedHostId` and `placementGroupId` parameters together
//!    in the same `CreateEnvironmentHost` action" — the `HostInfoForCreate` shape's own
//!    rule, refused through the one helper `CreateEnvironment` uses for it.
//! 4. "DNS hostnames for hosts must be unique across Amazon EVS environments and
//!    within VCF" (`hostName` doc). A DNS name is case-insensitive, so a name a host
//!    already carries is taken in every spelling of it
//!    ([`crate::state::same_dns_name`]).
//! 5. The environment's `vmkManagement` VLAN subnet has an address left for the host:
//!    an ESX host's management interface lives on that VLAN, so that subnet's usable
//!    range is what bounds how many hosts one environment can hold.
//! 6. The EC2 resources the host is PLACED on exist in the account's own Amazon EC2
//!    store: `placementGroupId` is "the unique ID of the placement group where the host
//!    is placed" and `dedicatedHostId` "the unique ID of the Amazon EC2 Dedicated Host",
//!    and Amazon EVS creates neither, so a host that names one names a resource the
//!    account already holds ([`crate::state::ec2_resources::host_placement_failures`]) —
//!    the rule `CreateEnvironment` already applies to an environment's `vpcId` and
//!    `serviceAccessSubnetId`. A host that names BOTH mechanisms is refused by rule 3
//!    alone, so neither reference is resolved for it.
//! 7. An `esxVersion` the request names is one of the versions Amazon EVS offers for the
//!    environment's VCF version: "To find the available ESX versions for a particular VCF
//!    version, use the GetVersions action", so the catalogue that action reports
//!    ([`crate::state::available_esx_versions`]) is what an asked-for version has to be
//!    in. A VCF version that catalogue states no versions for leaves nothing to judge a
//!    requested version against, so nothing is refused for it.
//! 8. The SSH key pair the host is reached with exists in that same store: `keyName` is
//!    "the name of the SSH key that is used to access the host" and a host supplies the
//!    "EC2 SSH keypair name" (`CreateEnvironment$hosts` doc), so it names an Amazon EC2
//!    key pair of the caller's own account — created there by `CreateKeyPair` or
//!    `ImportKeyPair`, never by Amazon EVS — and rule 6 holds of it for the same reason
//!    ([`crate::state::ec2_resources::host_key_pair_failures`]). It is resolved whatever
//!    the host's placement members say, because a key pair is no placement reference and
//!    the launch below carries the name onto the instance either way.
//!
//! Rules 2 to 8 are evaluated together, so a request that breaks several is refused
//! ONCE with one `ValidationException` whose `fieldList` carries an entry per failure —
//! the shape `CreateEnvironment` answers with. A refused request adds nothing.
//!
//! `esxVersion` is stored as the version the host RUNS: the one the request named, or
//! — when it named none — the default ESX version the service's own version catalogue
//! gives the environment's VCF version ([`crate::state::default_esx_version`], the
//! catalogue `GetVersions` reports). It is reported by nothing, because the modelled
//! `Host` shape carries no ESX version member.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEnvironmentHost::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `CreateEnvironmentHost`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-evs-amazonelasticvmwareservice/smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json")
//! ```
//!
//! Creates an ESX host and adds it to an Amazon EVS environment. This action can only be
//! used after the Amazon EVS environment is deployed. You can use the dedicatedHostId
//! parameter to specify an Amazon EC2 Dedicated Host for ESX host creation. You can use the
//! placementGroupId parameter to specify a cluster or partition placement group to launch
//! EC2 instances into. If you don't specify an ESX version when adding hosts using
//! CreateEnvironmentHost action, Amazon EVS automatically uses the default ESX version for
//! your environment's VCF version. To find the available ESX versions for a particular VCF
//! version, use the GetVersions action. You cannot use the dedicatedHostId and
//! placementGroupId parameters together in the same CreateEnvironmentHost action. This
//! results in a ValidationException response.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..100, pattern ^[!-~]+$, @idempotencyToken) — This
//!   parameter is not used in Amazon EVS currently. If you supply input for this parameter,
//!   it will have no effect. A unique, case-sensitive identifier that you provide to ensure
//!   the idempotency of the host creation request. If you do not specify a client token, a
//!   randomly generated token is used for the request to ensure idempotency.
//! * `environmentId` (EnvironmentId, pattern ^(env-[a-zA-Z0-9]{10})$) **required** — A unique
//!   ID for the environment that the host is added to.
//! * `esxVersion` (EsxVersion, len ..128) — The ESX version to use for the host.
//! * `host` (HostInfoForCreate) **required** — The host that is created and added to the
//!   environment.
//!
//! ## Output
//!
//! * `environmentSummary` (EnvironmentSummary) — A summary of the environment that the host
//!   is created in.
//! * `host` (Host) — A description of the created host.
//!
//! ## Errors
//!
//! * `ThrottlingException` (HTTP 429) — The operation could not be performed because the
//!   service is throttling requests. This exception is thrown when the service endpoint
//!   receives too many concurrent requests.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the specified constraints.
//!   You will see this exception if invalid inputs are provided for any of the Amazon EVS
//!   environment operations, or if a list operation is performed on an environment resource
//!   that is still initializing.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEnvironmentHostError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-evs-amazonelasticvmwareservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEnvironmentHostError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.evs#CreateEnvironmentHost",
    extra_errors = ["InternalServerException"]
)]
#[derive(Debug)]
pub enum CreateEnvironmentHostErrorModeled {
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
    /// Widened in from the service's own error set (this operation does not list it):
    /// the one honest answer when this service's store cannot be reached at all.
    InternalServerException(InternalServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_environment_host::CreateEnvironmentHostError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEnvironmentHostError = crate::scaffolded::Scaffolded<CreateEnvironmentHostErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEnvironmentHostErrorModeled> for CreateEnvironmentHostError {
    fn from(modeled: CreateEnvironmentHostErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEnvironmentHost` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEnvironmentHost<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEnvironmentHost<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEnvironmentHost<Eng>
{
    type Input = CreateEnvironmentHostInput;
    type Output = CreateEnvironmentHostOutput;
    type Error = CreateEnvironmentHostError;

    const NAME: &'static str = "CreateEnvironmentHost";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_HOST_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENVIRONMENT_HOST_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEnvironmentHostErrorModeled::InternalServerException(InternalServerException {
                message: "The Amazon EVS environment store is unavailable.".to_string(),
            })
        })?;
        // `clientToken` is read and ignored on purpose: "This parameter is not used in
        // Amazon EVS currently. If you supply input for this parameter, it will have no
        // effect." (`clientToken` doc). The unique-host-name rule below is what makes a
        // repeated create refuse rather than add a second host.
        let _ = &input.client_token;
        let region = crate::state::request_region(ctx.parts());
        let environment_arn = crate::state::environment_arn(&region, &input.environment_id);
        // Authorization first: AWS decides it at the IAM layer, before the resource the
        // request names is evaluated.
        if let Some(denied) = crate::state::authorization::denied(
            &*self.engine,
            ctx,
            Self::NAME,
            Some(&environment_arn),
        ) {
            return Err(denied);
        }
        let mut state = state.lock();
        // Reporting only: this materialises the state changes the clock already owed,
        // which were due whether or not anybody called, so "the environment is
        // deployed" is judged against the state that is due at this instant.
        state.materialise_due_transitions();
        // One instant for everything this call changes, so the new host record and the
        // environment whose inventory it joins are stamped alike. Read with the store lock
        // HELD and after the materialisation above: the host is added at this instant, not
        // at the one the request arrived at, so its stamp cannot predate a transition the
        // same call just materialised onto the environment and its VLANs — and the
        // re-stamp of the environment below is not dropped by the guard that keeps a stamp
        // from going backwards.
        let created_at_ms = crate::state::now_ms();
        // Whether some OTHER environment already has a host of this name: the
        // uniqueness the doc states is across environments, so it is read before this
        // environment is borrowed.
        let host_name_is_taken = state.host_name_is_taken(&input.host.host_name);

        // The environment this create addresses, resolved through the rule every handler
        // that acts on an environment shares (`live_environment_in_region_mut`). This
        // operation declares no `ResourceNotFoundException`, so an environment id that
        // resolves to nothing is refused with the ValidationException that names it.
        let environment = state
            .live_environment_in_region_mut(&region, &input.environment_id)
            .ok_or_else(|| {
                CreateEnvironmentHostErrorModeled::ValidationException(
                    crate::state::environment_not_found_validation(&input.environment_id),
                )
            })?;

        // Every remaining precondition, evaluated before anything is stored and all of
        // them together, so a caller can correct them in one round trip.
        let mut failures: Vec<ValidationExceptionField> = Vec::new();
        // "This action can only be used after the Amazon EVS environment is deployed."
        if environment.state != crate::state::STATE_CREATED {
            failures.push(crate::state::field_failure(
                "environmentId",
                &format!(
                    "names an environment whose state is {}; a host can only be added after \
                     the environment is deployed",
                    environment.state
                ),
            ));
        }
        let placement_conflict =
            input.host.dedicated_host_id.is_some() && input.host.placement_group_id.is_some();
        if placement_conflict {
            failures.push(crate::state::placement_conflict_failure("host"));
        }
        // The EC2 resources the host is PLACED on belong to the caller's account, and
        // Amazon EVS creates neither of them, so each reference the request names has to
        // resolve in the account's own Amazon EC2 store — the rule `CreateEnvironment`
        // applies to the `vpcId` and `serviceAccessSubnetId` an environment names. A host
        // naming BOTH mechanisms is already refused for that, so neither reference is
        // resolved for it: the pair has nothing further to judge.
        //
        // LOCK ORDER. This reads EC2's store while this service's guard is held, which is
        // the one order either lock is ever taken in (the launch below does the same).
        if !placement_conflict {
            failures.extend(crate::state::ec2_resources::host_placement_failures(
                &*self.engine,
                &[crate::state::ec2_resources::HostPlacementReferences {
                    member_prefix: "host",
                    placement_group_id: input.host.placement_group_id.as_deref(),
                    dedicated_host_id: input.host.dedicated_host_id.as_deref(),
                }],
            ));
        }
        // The SSH key pair the host is reached with is an EC2 resource of that same
        // account: `keyName` is "the name of the SSH key that is used to access the host"
        // and a host supplies the "EC2 SSH keypair name", which Amazon EC2 mints and
        // Amazon EVS does not — so the name is resolved in the account's own key pair
        // store, read by name the way EC2 itself reads one. Resolved whatever the
        // placement members say: it is no placement reference, and the launch below writes
        // this name onto the instance either way.
        failures.extend(crate::state::ec2_resources::host_key_pair_failures(
            &*self.engine,
            &[crate::state::ec2_resources::HostKeyPairReference {
                member: "host.keyName",
                key_name: &input.host.key_name,
            }],
        ));
        // "To find the available ESX versions for a particular VCF version, use the
        // GetVersions action" (this operation's doc): a version the request names is one
        // of the versions that catalogue offers for THIS environment's VCF version, and
        // any other string names no ESX version Amazon EVS can install on the host. A VCF
        // version the catalogue states no versions for leaves nothing to judge against, so
        // nothing is refused for it.
        if let Some(requested) = input.esx_version.as_deref() {
            let available = crate::state::available_esx_versions(&environment.vcf_version);
            if !available.is_empty() && !available.contains(&requested) {
                failures.push(crate::state::unavailable_esx_version_failure(
                    requested,
                    &environment.vcf_version,
                    &available,
                ));
            }
        }
        // "DNS hostnames for hosts must be unique across Amazon EVS environments and
        // within VCF": the scope is every environment, THIS one included, which is what
        // `host_name_is_taken` walks, comparing the name as the DNS name it is.
        if host_name_is_taken {
            failures.push(crate::state::field_failure(
                "host.hostName",
                "is already used by a host of an Amazon EVS environment; host DNS hostnames \
                 must be unique across environments",
            ));
        }
        // The addresses the host takes out of the environment's own `vmkManagement` VLAN
        // subnet: its management address, and one more for each further elastic network
        // interface Amazon EVS provisions for it, because no two interfaces of a subnet can
        // hold one address. `None` means that subnet has fewer addresses left than a host
        // needs, which is what bounds how many hosts one environment can hold.
        let addresses = environment.next_host_ips();
        if addresses.is_none() {
            failures.push(crate::state::field_failure(
                "host",
                &format!(
                    "cannot be added: the environment's {} VLAN subnet has no address left \
                     for every network interface of another host",
                    crate::state::VMK_MANAGEMENT_FUNCTION
                ),
            ));
        }
        if !failures.is_empty() {
            return Err(CreateEnvironmentHostErrorModeled::ValidationException(
                crate::state::aggregate_validation_error(failures),
            )
            .into());
        }
        // Reached only with addresses, because their absence is one of the failures above.
        let Some(addresses) = addresses else {
            return Err(CreateEnvironmentHostErrorModeled::InternalServerException(
                InternalServerException {
                    message: "No management address could be assigned to the host.".to_string(),
                },
            )
            .into());
        };

        // The host is CREATING until it has been built and joined the environment,
        // which is the move to CREATED the clock owes it.
        let pending = crate::state::PendingStatus::starting_now(crate::state::HOST_CREATED);
        let due_at = pending.due_at;
        // "An ESX host that runs on an Amazon EC2 bare metal instance" (`Host` doc): the
        // instance IS the host, so it is launched into the account's own Amazon EC2
        // store here, from the key pair and instance type the request gave, in the
        // environment's own `vmkManagement` VLAN subnet. It is launched only now, after
        // every precondition above has passed, so a refused request launches nothing.
        //
        // No `vmkManagement` VLAN means no subnet to attach the host's interfaces to, so
        // nothing is launched and the host reports neither member — rather than an
        // instance in a subnet named by the empty string. Unreachable while the host's
        // own management address comes out of that VLAN, which is the check above.
        let ec2 = environment.vmk_management_vlan().and_then(|vlan| {
            crate::state::ec2_resources::launch_host_instance(
                &*self.engine,
                crate::state::ec2_resources::HostLaunch {
                    region: &region,
                    instance_type: &input.host.instance_type,
                    key_name: &input.host.key_name,
                    management_ip: &addresses.management,
                    // …and the addresses the host's other interfaces hold, so each is a
                    // real interface of that subnet rather than one sharing an address.
                    standby_ips: &addresses.secondary,
                    management_subnet_id: &vlan.subnet_id,
                    service_access_subnet_id: &environment.service_access_subnet_id,
                    // Where the request placed the host, resolved above: the instance
                    // records it, so EC2 reports the host's placement for the same host
                    // Amazon EVS reports it for.
                    placement_group_id: input.host.placement_group_id.as_deref(),
                    dedicated_host_id: input.host.dedicated_host_id.as_deref(),
                },
            )
        });
        // "If you don't specify an ESX version when adding hosts using
        // CreateEnvironmentHost action, Amazon EVS automatically uses the default ESX
        // version for your environment's VCF version" (this operation's doc): the
        // version the host runs is the request's when it named one, and otherwise the
        // default this service's own catalogue gives this environment's VCF version —
        // the same catalogue `GetVersions` reports. A VCF version the catalogue knows no
        // Bill Of Materials for resolves to no version at all, rather than to a made-up
        // one.
        let esx_version = input.esx_version.clone().or_else(|| {
            crate::state::default_esx_version(&environment.vcf_version).map(str::to_string)
        });
        let host = crate::state::StoredHost::creating(
            &input.host,
            addresses,
            esx_version,
            created_at_ms,
            ec2,
            pending,
        );
        // The name is free across the hosts that still occupy an environment, checked
        // above, so the only record that can hold this name is a host
        // `DeleteEnvironmentHost` has finished with — kept for reporting only — and the
        // new host replaces it, whichever case that record was stored under
        // ([`crate::state::StoredEnvironment::insert_host`]): a host added under a freed
        // DNS hostname is a new host.
        let host_wire = host.to_wire();
        environment.insert_host(host);
        let host = host_wire;
        // Adding a host changes the environment's host inventory, which is part of the
        // environment, so the record is re-stamped and the summary this operation
        // answers with reports that — exactly as removing one does.
        environment.note_child_change(created_at_ms);
        state.note_transition_due(due_at);
        // The new host is part of what the environment's health checks read (the host
        // count, and the cores a license has to cover), so they are recomputed before
        // the summary is built.
        state.recompute_environment_checks();
        let environment_summary = state
            .environment_in_region(&region, &input.environment_id)
            .map(|environment| environment.to_summary())
            .ok_or_else(|| {
                CreateEnvironmentHostErrorModeled::InternalServerException(
                    InternalServerException {
                        message: "The environment the host was added to is no longer stored."
                            .to_string(),
                    },
                )
            })?;

        Ok(CreateEnvironmentHostOutput {
            environment_summary: Some(environment_summary),
            host: Some(host),
        })
    }
}
