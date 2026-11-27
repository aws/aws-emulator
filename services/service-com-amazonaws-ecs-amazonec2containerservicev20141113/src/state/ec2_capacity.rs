// SPDX-License-Identifier: CC0-1.0

//! The capacity an Amazon EC2 instance LAUNCHED INTO A CLUSTER gives that cluster: the
//! registration the container agent running on the instance makes for it.
//!
//! # The rule this module implements
//!
//! "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
//! cluster", and `RegisterContainerInstance` — "this action is only used by the Amazon ECS
//! agent, and it is not intended for use outside of the agent" — is how an instance becomes
//! one of them. The agent is not a client: it runs ON the instance, reads the cluster it is
//! to join from its own configuration file ([`AGENT_CONFIG_PATH`]), and registers itself once
//! the instance is up. That file is what the instance's own user-data program writes — the
//! agent's `ECS_CLUSTER` variable, which the documented way of joining a cluster appends to
//! it — and Amazon EC2 RUNS that program in the instance's guest when it comes up, so the
//! file on the machine's storage is what this module reads ([`joined_cluster`]), whatever the
//! document the launch carried did to write it.
//!
//! So: a RUNNING instance of the account whose user data configures the agent with a cluster
//! of the account is a registered container instance of that cluster, from one
//! [`AGENT_REGISTRATION_SPAN`] after its launch — and it stops being capacity when the
//! instance is no longer there to run anything: a stopped instance runs no agent (so its
//! registration reports no connected agent, and nothing is placed on it), and a terminated
//! instance's registration is deregistered, which is the `INACTIVE` status a deregistration
//! leaves ("the instance then transitions to an `INACTIVE` status").
//!
//! The registration is owed by the CLOCK and not by any call: the agent registers on its
//! own, with no client request between the launch and the cluster reporting the capacity, so
//! the pass that materialises it ([`settle_ec2_capacity`]) is run by the operations that
//! REPORT or USE that capacity and by the clock pump — each of them materialising only what
//! was already due at the instant it looks, so two reads at one instant answer identically.
//!
//! What is NOT read here is the image the instance boots: the agent ships in the
//! ECS-optimized AMI, but which AMI id that is per Region is not a fact this emulator
//! publishes, whereas the cluster in an instance's user data is a fact the launch itself
//! carried. An instance configured for a cluster the account does not hold registers
//! nowhere, exactly as its agent would be refused.
//!
//! LOCK ORDER: the Amazon EC2 store's lock is taken first and RELEASED before this service's
//! is taken, so no two store locks are ever held at once — the same contract
//! [`super::evaluate_image_pulls`] has, which is what keeps a lock cycle between the stores
//! unreachable.

use super::*;

use ::service_com_amazonaws_ec2_amazonec2::state as ec2;

/// The agent variable an instance's user data names its cluster with: `ECS_CLUSTER`, the
/// variable the agent's configuration file carries ("echo ECS_CLUSTER=<cluster> >>
/// /etc/ecs/ecs.config", the documented way to launch an instance into a cluster).
pub const AGENT_CLUSTER_VARIABLE: &str = "ECS_CLUSTER";

/// How long after a launch the agent on the instance has registered it.
///
/// EMULATOR-CHOSEN — AWS publishes no duration for it — and deliberately non-zero so a
/// caller that reads the cluster straight after the launch still sees a cluster with no
/// capacity yet, which is what the real one reports while the instance is still coming up.
pub const AGENT_REGISTRATION_SPAN: Duration = Duration::from_secs(3);

/// How long after the machine under a container instance stops running the agent's
/// CONNECTION to Amazon ECS times out, which is when the registration stops reporting the
/// agent connected.
///
/// EMULATOR-CHOSEN — AWS publishes no duration for the agent's connection timeout — and
/// deliberately non-zero: the connection is the agent's own, so a caller that reads the
/// registration straight after stopping the instance still sees the agent connected, exactly
/// as the real service reports it until the connection it holds times out.
pub const AGENT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);

/// The ports the agent reports as already in use on the instance it registers: the agent's
/// own `ECS_RESERVED_PORTS` default, `[22, 2375, 2376, 51678, 51679]`.
const AGENT_RESERVED_PORTS: [&str; 5] = ["22", "2375", "2376", "51678", "51679"];

/// The `Resource$name` of the CPU an instance offers, as the agent reports it.
const RESOURCE_CPU: &str = "CPU";

/// The `Resource$name` of the memory an instance offers.
const RESOURCE_MEMORY: &str = "MEMORY";

/// The `Resource$name` of the TCP ports already in use on the instance.
const RESOURCE_PORTS: &str = "PORTS";

/// `Resource$type` for a resource measured as a whole number ("Valid values: INTEGER,
/// DOUBLE, LONG, or STRINGSET").
const RESOURCE_TYPE_INTEGER: &str = "INTEGER";

/// `Resource$type` for a resource that is a set of strings, which is how the ports are
/// reported.
const RESOURCE_TYPE_STRING_SET: &str = "STRINGSET";

/// The CPU units one vCPU is: Amazon ECS expresses CPU in units of which "1024 CPU units
/// is the equivalent of 1 vCPU" (`TaskDefinition$cpu`).
const CPU_UNITS_PER_VCPU: i32 = 1024;

/// The built-in attribute naming the instance type the agent runs on.
const ATTRIBUTE_INSTANCE_TYPE: &str = "ecs.instance-type";

/// The built-in attribute naming the AMI the instance booted.
const ATTRIBUTE_AMI_ID: &str = "ecs.ami-id";

/// What ONE running instance's agent reports when it registers the instance.
#[derive(Debug, Clone)]
struct AgentRegistration {
    /// The instance the agent runs on: "For Amazon EC2 instances, this value is the Amazon
    /// EC2 instance ID" (`ContainerInstance$ec2InstanceId`).
    ec2_instance_id: String,
    /// The cluster the instance's user data configured the agent with.
    cluster_name: String,
    /// The private IPv4 address of the machine, as Amazon EC2 holds it — the address a task
    /// of this instance that shares its network is reached at
    /// ([`ContainerInstanceRecord::private_ipv4_address`]).
    private_ipv4_address: Option<String>,
    /// The Region the instance runs in, read off its Availability Zone — the Region the
    /// container instance's own ARN is in.
    region: String,
    /// The instant the registration was DUE at, which is the instant it happened at: the
    /// pass that materialises it reports that instant rather than its own.
    registered_at: SystemTime,
    /// `totalResources`, as the agent reports them for this instance type.
    resources: Vec<Resource>,
    /// The built-in attributes the agent reports about the instance it runs on.
    attributes: Vec<Attribute>,
}

/// An instance this account holds that is NOT running, and so is not capacity: the value is
/// `true` for one that is terminating or terminated, which never comes back.
type DepartedInstances = HashMap<String, bool>;

/// Materialise every container-instance registration the account's Amazon EC2 instances
/// owe, as of `now`, and take the registration back off the instances that are no longer
/// capacity.
///
/// Safe to call from a REPORTING operation: it applies only what the clock had already made
/// due at `now` — the agent on an instance that has been running for one
/// [`AGENT_REGISTRATION_SPAN`] registered it then, whether or not anybody called — and
/// nothing it does depends on the request that happens to be in flight.
///
/// Called with NO guard of this service's store held (see the module's LOCK ORDER note). An
/// engine carrying no Amazon EC2 store holds no instances, so nothing is owed.
pub fn settle_ec2_capacity<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(ec2_state) = ec2::service_state(engine) else {
        return;
    };
    // Under the Amazon EC2 store's lock alone, and in one pass: every instance of the
    // account is read once however many clusters they join, and only owned values leave it.
    let (registrations, departed) = {
        let store = ec2_state.lock();
        read_launched_capacity(&store, now)
    };
    if registrations.is_empty() && departed.is_empty() {
        return;
    }
    let Some(state) = service_state(engine) else {
        return;
    };
    let mut guard = state.lock();
    apply_departures(&mut guard, &departed, now);
    apply_registrations(&mut guard, &registrations, now);
}

/// Read what the account's instances say about the clusters' capacity: the registrations
/// the agents on the RUNNING ones have made, and the instances that are no longer running.
///
/// An instance whose user data names no cluster is not one the agent joins anywhere, and
/// one whose registration is not due yet is not capacity yet — neither is reported.
fn read_launched_capacity(
    store: &::service_com_amazonaws_ec2_amazonec2::ComAmazonawsEc2Amazonec2State,
    now: SystemTime,
) -> (Vec<AgentRegistration>, DepartedInstances) {
    let mut registrations = Vec::new();
    let mut departed = DepartedInstances::new();
    for instance in store.instances.values() {
        match reached_state(instance) {
            ReachedState::Running => {
                let Some(cluster_name) = joined_cluster(store, instance) else {
                    continue;
                };
                let registered_at = instance.launch_time + AGENT_REGISTRATION_SPAN;
                if registered_at > now {
                    continue;
                }
                registrations.push(AgentRegistration {
                    ec2_instance_id: instance.instance_id.clone(),
                    cluster_name,
                    private_ipv4_address: (!instance.private_ip.is_empty())
                        .then(|| instance.private_ip.clone()),
                    // A zone's name is its Region's with a letter appended, and the service
                    // that owns zones is the one that reads it that way
                    // ([`ec2::region_of_zone_name`]), so this is its reading rather than a
                    // second one.
                    region: ec2::region_of_zone_name(&instance.placement_az).to_string(),
                    registered_at,
                    resources: reported_resources(instance),
                    attributes: reported_attributes(instance),
                });
            }
            ReachedState::Stopped => {
                departed.insert(instance.instance_id.clone(), false);
            }
            ReachedState::Gone => {
                departed.insert(instance.instance_id.clone(), true);
            }
        }
    }
    (registrations, departed)
}

/// Where one instance has got to, as far as the agent on it is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReachedState {
    /// Up, so its agent runs.
    Running,
    /// Not up — on its way up, or stopping, or stopped — so no agent of it runs.
    Stopped,
    /// Terminating or terminated: the instance is gone for good.
    Gone,
}

/// The state `instance` has REACHED as of this instant: the state it is recorded in, or the
/// one the clock has already moved it to.
///
/// The Amazon EC2 service materialises an instance's own state moves on its own clock, and
/// a move whose instant has passed happened then whether or not anybody called that service
/// — so an instance whose move to `running` is due is running, which is what its agent sees
/// from inside the instance. Nothing is written here: the other service's records are read,
/// never changed.
///
/// That instant is read off the MONOTONIC clock rather than off the `now` the pass was asked
/// about: the due instant Amazon EC2 records for an instance's own move is a
/// [`::std::time::Instant`] ([`ec2::PendingTransition`]), which no `SystemTime` can be
/// turned into — so this is the only clock the comparison can be made against.
fn reached_state(
    instance: &::service_com_amazonaws_ec2_amazonec2::state::InstanceRecord,
) -> ReachedState {
    let due = ::std::time::Instant::now();
    let state = instance
        .pending_transition
        .as_ref()
        .filter(|pending| due >= pending.due_at)
        .map_or(instance.state, |pending| pending.to);
    match state {
        ec2::InstanceStateName::Running => ReachedState::Running,
        ec2::InstanceStateName::ShuttingDown | ec2::InstanceStateName::Terminated => {
            ReachedState::Gone
        }
        _ => ReachedState::Stopped,
    }
}

/// The file the container agent reads its configuration out of ON the instance it runs on:
/// `/etc/ecs/ecs.config`, the file the documented way of joining a cluster appends the
/// [`AGENT_CLUSTER_VARIABLE`] assignment to ("echo ECS_CLUSTER=<cluster> >>
/// /etc/ecs/ecs.config").
pub const AGENT_CONFIG_PATH: &str = "/etc/ecs/ecs.config";

/// The cluster the agent on `instance` is configured with, or `None` when nothing on the
/// instance configures one — which is every instance that was not launched into a cluster.
///
/// The agent reads ITS OWN CONFIGURATION FILE on the machine it runs on
/// ([`AGENT_CONFIG_PATH`]), so that file is what this reads too: the one the instance's own
/// user-data program WROTE, whatever that program did to write it. Amazon EC2 runs that
/// program in the instance's guest when it comes up and keeps the storage it left behind, so
/// a launch that appends the assignment with a shell line, writes the file from a heredoc or
/// assembles it from values of its own all leave the same file, read the same way.
///
/// Before that file exists — an instance whose guest has not come up yet, or one whose user
/// data Amazon EC2 runs nothing for — the cluster is the one the LAUNCH configured
/// ([`launched_cluster`]): the document the user data carries is read for it rather than the
/// machine's storage.
fn joined_cluster(
    store: &::service_com_amazonaws_ec2_amazonec2::ComAmazonawsEc2Amazonec2State,
    instance: &::service_com_amazonaws_ec2_amazonec2::state::InstanceRecord,
) -> Option<String> {
    store
        .read_in_instance_guest(&instance.instance_id, AGENT_CONFIG_PATH)
        .as_deref()
        .and_then(assigned_cluster_in)
        .or_else(|| launched_cluster(instance.user_data.as_deref()))
}

/// The cluster the user data a launch carried configures the agent with, or `None` when it
/// configures none.
///
/// Read through Amazon EC2's OWN reading of user data
/// ([`::service_com_amazonaws_ec2_amazonec2::state::guest_program`]), which is what decides
/// what the instance's guest does with the document: that reading decompresses user data that
/// arrived compressed, unpacks an archive of documents and resolves a cloud-config module, so
/// a launch that configures the agent through any of those forms names its cluster here as
/// plainly as one carrying a shell script does. What is read of the program is what WRITES
/// the agent's configuration — the files it puts at [`AGENT_CONFIG_PATH`], then the commands
/// it runs — and, for user data that gives the guest no program at all, the document's own
/// text ([`configured_cluster`]), since a file of `NAME=value` lines configures the agent
/// without running anything.
fn launched_cluster(user_data: Option<&str>) -> Option<String> {
    let program = ::service_com_amazonaws_ec2_amazonec2::state::guest_program(user_data);
    program
        .written_files
        .iter()
        .rev()
        .filter(|file| names_agent_config(&file.path))
        .find_map(|file| assigned_cluster_in(&file.content))
        .or_else(|| {
            program
                .commands_for_boot(1)
                .iter()
                .rev()
                .find_map(|command| assigned_cluster_in(command))
        })
        .or_else(|| configured_cluster(user_data))
}

/// Whether `path` names the agent's configuration file ([`AGENT_CONFIG_PATH`]), resolved the
/// way the tree a guest program runs in resolves a path, so the file a document names and the
/// file the agent reads are one file however the document spelled it.
fn names_agent_config(path: &str) -> bool {
    ::service_common::posix_shell_volume::resolved_path(path).as_deref() == Some(AGENT_CONFIG_PATH)
}

/// The cluster `text` assigns, reading it as the agent reads its configuration: lines of
/// `NAME=value`, of which the LAST assignment of [`AGENT_CLUSTER_VARIABLE`] is the one in
/// force — a line appended after another is what the file ends up saying.
fn assigned_cluster_in(text: &str) -> Option<String> {
    text.lines().rev().find_map(assigned_cluster)
}

/// The cluster the agent on an instance with this `user_data` is configured with, or `None`
/// when the user data configures none — which is every instance that was not launched into
/// a cluster.
///
/// The value is the one the LAST assignment of [`AGENT_CLUSTER_VARIABLE`] in the user data
/// names: the configuration is a file of `NAME=value` lines the agent reads, and a line
/// appended after another is the one in force.
fn configured_cluster(user_data: Option<&str>) -> Option<String> {
    let raw = user_data?;
    let text = decoded_user_data(raw);
    assigned_cluster_in(&text)
}

/// The user data as the INSTANCE sees it: "user data must be base64-encoded"
/// (`RunInstancesRequest$UserData`), so the stored text is decoded here. Text that is not
/// base64 of valid UTF-8 is read as it stands, because that is what an instance launched
/// with plain text carries.
fn decoded_user_data(raw: &str) -> String {
    use ::base64::Engine as _;

    let packed: String = raw.chars().filter(|value| !value.is_whitespace()).collect();
    match ::base64::engine::general_purpose::STANDARD.decode(packed.as_bytes()) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| raw.to_string()),
        Err(_) => raw.to_string(),
    }
}

/// The cluster ONE line of the agent's configuration assigns, or `None` when it assigns
/// none.
///
/// The variable has to stand on its own — a longer name that merely ENDS in it assigns
/// something else — and the value ends where the line's next space does, so the shell line
/// that appends the assignment to the configuration file ("echo ECS_CLUSTER=<cluster> >>
/// /etc/ecs/ecs.config") names the same cluster as the bare assignment. Either quoting form
/// is stripped: a quoted value is the value, not the quotes.
fn assigned_cluster(line: &str) -> Option<String> {
    let start = line.find(AGENT_CLUSTER_VARIABLE)?;
    if line[..start]
        .chars()
        .next_back()
        .is_some_and(|value| value.is_ascii_alphanumeric() || value == '_')
    {
        return None;
    }
    let assignment = line[start + AGENT_CLUSTER_VARIABLE.len()..].trim_start();
    let value = assignment.strip_prefix('=')?.trim_start();
    let value = value
        .split_whitespace()
        .next()?
        .trim_matches(|value| value == '"' || value == '\'');
    (!value.is_empty()).then(|| value.to_string())
}

/// The resources the agent reports for the instance it registers — "the amount of each
/// resource that was available on the container instance when the container agent
/// registered it" (`ContainerInstance$registeredResources`).
///
/// The CPU and the memory are the instance TYPE's, as Amazon EC2 publishes them for it
/// (`::service_com_amazonaws_ec2_amazonec2::state::instance_type_record`), with the CPU
/// expressed in the units Amazon ECS counts it in ([`CPU_UNITS_PER_VCPU`]); the ports are
/// the ones the agent itself holds ([`AGENT_RESERVED_PORTS`]). An instance type Amazon EC2
/// publishes no facts for contributes neither number: what it offers is not known, so none
/// is reported.
fn reported_resources(
    instance: &::service_com_amazonaws_ec2_amazonec2::state::InstanceRecord,
) -> Vec<Resource> {
    let mut resources = Vec::new();
    if let Some(record) = ec2::instance_type_record(&instance.instance_type) {
        resources.push(Resource {
            name: Some(RESOURCE_CPU.to_string()),
            r#type: Some(RESOURCE_TYPE_INTEGER.to_string()),
            integer_value: record.default_vcpus.saturating_mul(CPU_UNITS_PER_VCPU),
            ..Resource::default()
        });
        resources.push(Resource {
            name: Some(RESOURCE_MEMORY.to_string()),
            r#type: Some(RESOURCE_TYPE_INTEGER.to_string()),
            integer_value: i32::try_from(record.memory_mib).unwrap_or(i32::MAX),
            ..Resource::default()
        });
    }
    resources.push(Resource {
        name: Some(RESOURCE_PORTS.to_string()),
        r#type: Some(RESOURCE_TYPE_STRING_SET.to_string()),
        string_set_value: Some(
            AGENT_RESERVED_PORTS
                .iter()
                .map(|port| (*port).to_string())
                .collect(),
        ),
        ..Resource::default()
    });
    resources
}

/// The built-in attributes the agent reports about the instance it runs on: the instance
/// type, the Availability Zone, the AMI it booted and its processor architecture — each read
/// off the instance's own Amazon EC2 record, so a placement constraint written over one is
/// evaluated against what that service holds.
fn reported_attributes(
    instance: &::service_com_amazonaws_ec2_amazonec2::state::InstanceRecord,
) -> Vec<Attribute> {
    let mut attributes = vec![
        attribute(ATTRIBUTE_INSTANCE_TYPE, instance.instance_type.clone()),
        attribute(ATTRIBUTE_AVAILABILITY_ZONE, instance.placement_az.clone()),
    ];
    if let Some(image_id) = instance.image_id.clone() {
        attributes.push(attribute(ATTRIBUTE_AMI_ID, image_id));
    }
    if let Some(record) = ec2::instance_type_record(&instance.instance_type) {
        attributes.push(attribute(ATTRIBUTE_CPU_ARCHITECTURE, record.architecture));
    }
    attributes
}

/// One reported attribute: a name and the value read for it.
fn attribute(name: &str, value: String) -> Attribute {
    Attribute {
        name: name.to_string(),
        value: Some(value),
        ..Attribute::default()
    }
}

/// Take the capacity back off the container instances whose Amazon EC2 instance is no
/// longer running — once the agent on it has stopped reporting in for long enough for the
/// connection it held to time out.
///
/// A stopped instance runs no agent, so its registration ends up reporting none connected —
/// "only instances connected to an agent can accept task placement requests" — and a
/// TERMINATED instance is gone for good, so its registration is deregistered, which is the
/// `INACTIVE` status a deregistration leaves. Neither is reported the instant the machine
/// stopped: what Amazon ECS holds is the agent's CONNECTION, which outlives the machine
/// until it times out ([`AGENT_CONNECTION_TIMEOUT`]), so the instant it falls due is
/// recorded on the registration and nothing moves before it. Each change bumps the
/// instance's version counter: "every time a container instance experiences a change ...
/// the version counter is incremented."
///
/// A registration already `INACTIVE` is left as it is: it has nothing left to lose, and a
/// caller that deregistered an instance of its own accord is not overruled here.
fn apply_departures(
    store: &mut ComAmazonawsEcsAmazonec2containerservicev20141113State,
    departed: &DepartedInstances,
    now: SystemTime,
) {
    for instance in store.container_instances.values_mut() {
        let Some(gone) = departed.get(&instance.ec2_instance_id).copied() else {
            continue;
        };
        if instance.status == CONTAINER_INSTANCE_INACTIVE {
            continue;
        }
        // The first pass that sees the machine stopped is the pass that sees the agent stop
        // reporting in, so that is when the connection starts timing out; a later pass reads
        // the instant it was already owed at rather than pushing it out.
        let due_at = *instance
            .agent_connection_lost_due_at
            .get_or_insert(now + AGENT_CONNECTION_TIMEOUT);
        if due_at > now {
            continue;
        }
        let mut changed = false;
        if gone {
            instance.status = CONTAINER_INSTANCE_INACTIVE.to_string();
            changed = true;
        }
        if instance.agent_connected {
            instance.agent_connected = false;
            changed = true;
        }
        if changed {
            instance.version += 1;
        }
    }
}

/// Register the instances whose agents have reported in, and place the tasks the capacity
/// they brought was being waited on for.
///
/// An instance already registered into the cluster is the SAME container instance however
/// often its agent reports: one instance is one capacity. What a repeat report does is
/// reconnect the agent of an instance that had stopped and is running again — the connection
/// it holds is good again, so nothing of it is timing out ([`AGENT_CONNECTION_TIMEOUT`]); a
/// registration a caller has deregistered stays deregistered, and a `DRAINING` one keeps
/// that status — "new tasks aren't placed on the container instance" is the caller's
/// decision, not the agent's.
fn apply_registrations(
    store: &mut ComAmazonawsEcsAmazonec2containerservicev20141113State,
    registrations: &[AgentRegistration],
    now: SystemTime,
) {
    let mut registered_into: Vec<String> = Vec::new();
    for registration in registrations {
        // An agent configured for a cluster the account does not hold in the instance's own
        // Region registers nowhere: a cluster belongs to the Region it was created in.
        if !store.has_active_cluster(&registration.region, &registration.cluster_name) {
            continue;
        }
        register_one(store, registration, &mut registered_into);
    }
    // The capacity a task parked on a capacity provider was waiting for has just arrived,
    // so Amazon ECS has somewhere to place it — the same placement the agent's own
    // registration gives such a task.
    for cluster_name in registered_into {
        store.place_tasks_awaiting_capacity(&cluster_name, now);
    }
}

/// Register ONE instance into the cluster its agent reported into, and note the cluster a
/// registration was made in so the tasks parked on it can be placed.
///
/// The write half of [`apply_registrations`], which decides WHETHER a registration is made;
/// one instance is one container instance of one cluster, so this is where that record is
/// minted and where a repeat report of an instance already registered reconnects its agent.
fn register_one(
    store: &mut ComAmazonawsEcsAmazonec2containerservicev20141113State,
    registration: &AgentRegistration,
    registered_into: &mut Vec<String>,
) {
    let existing = store
        .container_instances
        .values()
        .find(|instance| {
            instance.cluster_name == registration.cluster_name
                && instance.ec2_instance_id == registration.ec2_instance_id
        })
        .map(|instance| instance.arn.clone());
    if let Some(arn) = existing {
        if let Some(instance) = store.container_instances.get_mut(&arn) {
            if instance.status != CONTAINER_INSTANCE_INACTIVE {
                instance.private_ipv4_address = registration.private_ipv4_address.clone();
                instance.agent_connection_lost_due_at = None;
                if !instance.agent_connected {
                    instance.agent_connected = true;
                    instance.version += 1;
                }
            }
        }
        return;
    }
    if store.container_instances.len() >= MAX_CONTAINER_INSTANCES {
        return;
    }
    let instance_id = store.next_container_instance_id();
    let record = ContainerInstanceRecord {
        arn: container_instance_arn(
            &registration.region,
            &registration.cluster_name,
            &instance_id,
        ),
        instance_id,
        cluster_name: registration.cluster_name.clone(),
        ec2_instance_id: registration.ec2_instance_id.clone(),
        // The address of the machine the agent runs on, as Amazon EC2 holds it: what a
        // task of this instance that shares its network is reached at.
        private_ipv4_address: registration.private_ipv4_address.clone(),
        // The registration completes as it does for the agent's own call: the trunk
        // interface that would hold an instance in `REGISTERING` is only provisioned
        // for an account opted in to `awsvpcTrunking`, which nothing here turns on.
        status: CONTAINER_INSTANCE_ACTIVE.to_string(),
        status_reason: None,
        // The agent is what registered, so it is connected.
        agent_connected: true,
        version_info: Some(VersionInfo {
            agent_version: Some(LATEST_CONTAINER_AGENT_VERSION.to_string()),
            ..VersionInfo::default()
        }),
        // "If an update wasn't ever requested, this value is NULL."
        agent_update_status: None,
        agent_update_due_at: None,
        // The agent that registered is reporting in, so its connection is timing out
        // nowhere.
        agent_connection_lost_due_at: None,
        registered_resources: registration.resources.clone(),
        attributes: registration.attributes.clone(),
        platform_devices: Vec::new(),
        tags: Vec::new(),
        // The instant the registration was owed at, not the instant this pass ran.
        registered_at: reported_instant(registration.registered_at),
        version: 0,
    };
    let cluster_name = record.cluster_name.clone();
    store.container_instances.insert(record.arn.clone(), record);
    registered_into.push(cluster_name);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user data of an instance launched into `cluster` the documented way, as the
    /// launch carries it: base64 of the shell line that appends the agent's configuration.
    fn launched_into(cluster: &str) -> String {
        use ::base64::Engine as _;

        ::base64::engine::general_purpose::STANDARD.encode(format!(
            "#!/bin/bash\necho ECS_CLUSTER={cluster} >> /etc/ecs/ecs.config\n"
        ))
    }

    #[test]
    fn the_cluster_is_read_out_of_the_agent_configuration_a_launch_carried() {
        let base64_form = launched_into("metrics");
        let rows: [(&str, Option<&str>, &str); 8] = [
            ("no user data at all", None, ""),
            (
                "the documented shell line, base64 as a launch carries it",
                Some(base64_form.as_str()),
                "metrics",
            ),
            (
                "a bare assignment in plain text",
                Some("ECS_CLUSTER=plain"),
                "plain",
            ),
            (
                "a quoted value",
                Some("echo ECS_CLUSTER=\"quoted\" >> /etc/ecs/ecs.config"),
                "quoted",
            ),
            (
                "whitespace around the assignment",
                Some("ECS_CLUSTER = spaced"),
                "spaced",
            ),
            (
                "the last assignment is the one in force",
                Some("ECS_CLUSTER=first\nECS_CLUSTER=second"),
                "second",
            ),
            (
                "a variable that merely ends in the agent's one",
                Some("MY_ECS_CLUSTER=other"),
                "",
            ),
            (
                "user data that configures nothing",
                Some("#!/bin/bash\nyum update -y\n"),
                "",
            ),
        ];
        for (case, user_data, expected) in rows {
            let read = configured_cluster(user_data);
            let expected = (!expected.is_empty()).then(|| expected.to_string());
            assert_eq!(
                read, expected,
                "case {case:?}: expected the configured cluster {expected:?}, got {read:?}"
            );
        }
    }

}
