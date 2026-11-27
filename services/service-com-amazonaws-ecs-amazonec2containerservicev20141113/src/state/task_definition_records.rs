// SPDX-License-Identifier: CC0-1.0

//! The STORED task-definition revision and the projection every task-definition
//! operation reports it through.
//!
//! One record per registered revision ([`TaskDefinitionRecord`]), the members Amazon ECS
//! fills in when it stores one ([`defaulted_container_definitions`]), and the
//! `TaskDefinition` shape read back off it ([`TaskDefinitionRecord::view`]) — so what a
//! registration persists and what every later describe reports are decided in one place.

use super::*;

/// A task-definition revision, as `RegisterTaskDefinition` stored it.
#[derive(Debug, Clone)]
pub struct TaskDefinitionRecord {
    pub arn: String,
    pub family: String,
    pub revision: i32,
    pub status: String,
    pub container_definitions: Vec<ContainerDefinition>,
    /// The network mode the task this revision describes RUNS IN: the one the
    /// registration declared, else the documented default ("If no network mode is
    /// specified, the default is `bridge`"), so every rule keyed on the mode reads one
    /// value.
    pub network_mode: String,
    /// The same member AS THE REGISTRATION DECLARED IT, which is what a read of the
    /// revision reports: `networkMode` targets the `NetworkMode` enum with no `@default`,
    /// so a request that named none leaves the member absent from the projection even
    /// though the task runs in the default mode above.
    pub declared_network_mode: Option<String>,
    pub cpu: Option<String>,
    pub memory: Option<String>,
    pub volumes: Vec<Volume>,
    pub placement_constraints: Vec<TaskDefinitionPlacementConstraint>,
    pub requires_compatibilities: Vec<String>,
    pub task_role_arn: Option<String>,
    pub execution_role_arn: Option<String>,
    pub pid_mode: Option<String>,
    pub ipc_mode: Option<String>,
    pub proxy_configuration: Option<ProxyConfiguration>,
    pub ephemeral_storage: Option<EphemeralStorage>,
    pub runtime_platform: Option<RuntimePlatform>,
    pub enable_fault_injection: Option<bool>,
    /// `TaskDefinition$trustedExecutionConfiguration`: the isolation mode, and the
    /// attestation policy where one was given, as the registration set it — "You set this
    /// configuration when you register a task definition".
    pub trusted_execution_configuration: Option<TrustedExecutionConfiguration>,
    pub tags: Vec<Tag>,
    /// The principal that registered this revision, resolved when the registration ran
    /// — not when it is read back.
    pub registered_by: String,
    pub registered_at: DateTime,
    pub deregistered_at: Option<DateTime>,
    /// "The Unix timestamp for the time when the task definition delete was requested" —
    /// absent until `DeleteTaskDefinitions` asks for one, and kept as the instant THAT
    /// call ran.
    pub delete_requested_at: Option<DateTime>,
}

/// The container-definition collection members a task-definition projection always
/// MATERIALISES: a registration that declared none of them reads back with each member
/// present as an EMPTY LIST, never with the key omitted, so a client may index them
/// unconditionally (`definition["environment"]` is `[]`, not a missing key).
///
/// Recorded ECS behavior, and what AWS's own published `DescribeTaskDefinition` example
/// response shows for a container that declares none of them (`portMappings`,
/// `environment`, `mountPoints`, `volumesFrom` each rendered as `[]`); `systemControls`
/// is the same shape. Members with no empty form are NOT invented: a structure such as
/// `logConfiguration` or a scalar such as `user` stays absent when it was not declared,
/// and a declared value is reported exactly as it was stored.
fn container_definition_view(stored: &ContainerDefinition) -> ContainerDefinition {
    ContainerDefinition {
        port_mappings: Some(stored.port_mappings.clone().unwrap_or_default()),
        environment: Some(stored.environment.clone().unwrap_or_default()),
        mount_points: Some(stored.mount_points.clone().unwrap_or_default()),
        volumes_from: Some(stored.volumes_from.clone().unwrap_or_default()),
        system_controls: Some(stored.system_controls.clone().unwrap_or_default()),
        ..stored.clone()
    }
}

/// The `ContainerDefinition$user` Amazon ECS materialises on a task's LOG ROUTER — the
/// container that carries a `firelensConfiguration`. The FireLens router runs as the root
/// user, which `user` names by its uid, as the recorded registration carries it.
pub const FIRELENS_ROUTER_USER: &str = "0";

/// Whether `network_mode` maps a container port onto the SAME host port — a static
/// mapping.
///
/// `PortMapping$hostPort`: under `awsvpc` and `host` "the hostPort can either be left
/// blank or set to the same value as the containerPort", while `bridge` binds an open
/// host port of its own choosing. Read both where a mapping is rendered and where a
/// registration is checked, so one rule decides both.
pub fn is_static_port_mapping(network_mode: &str) -> bool {
    network_mode == NETWORK_MODE_AWSVPC || network_mode == NETWORK_MODE_HOST
}

/// The `hostPort` and `protocol` Amazon ECS fills in for a port mapping the
/// registration left partly blank, under `network_mode`.
///
/// `PortMapping$protocol`: "The default is tcp." `PortMapping$hostPort`: under `awsvpc`
/// and `host` "the hostPort can either be left blank or set to the same value as the
/// containerPort" (a STATIC mapping), while under `bridge` omitting it "(or set it to 0)"
/// asks for a port out of the ephemeral range (a DYNAMIC mapping), which is picked when
/// the task is placed. A mapping that declares a `containerPortRange` instead carries no
/// single host port — the doc: "leave this field empty" — so nothing is filled in for it.
/// The `none` network mode never reaches here with a mapping: it "cannot specify port
/// mappings", so `RegisterTaskDefinition` refuses one.
fn defaulted_port_mapping(mapping: &PortMapping, network_mode: &str) -> PortMapping {
    let static_mapping = is_static_port_mapping(network_mode);
    let host_port = match (mapping.host_port, mapping.container_port) {
        (Some(declared), _) => Some(declared),
        (None, _) if mapping.container_port_range.is_some() => None,
        (None, Some(container_port)) if static_mapping => Some(container_port),
        (None, Some(_)) => Some(DYNAMIC_HOST_PORT),
        (None, None) => None,
    };
    PortMapping {
        host_port,
        protocol: Some(
            mapping
                .protocol
                .clone()
                .unwrap_or_else(|| DEFAULT_PORT_PROTOCOL.to_string()),
        ),
        ..mapping.clone()
    }
}

/// The container definitions a registration STORES: the caller's own, with the members
/// Amazon ECS fills in at registration time filled in, so every later read of the
/// revision reports the same rendered definition.
///
/// Two members are rendered here: a port mapping's ([`defaulted_port_mapping`]), and the
/// `user` of a LOG ROUTER. A container that carries a `firelensConfiguration` is the
/// task's log router ("The FireLens configuration for the container. This is used to
/// specify and configure a log router for container logs"), which Amazon ECS runs as root
/// — [`FIRELENS_ROUTER_USER`] — so the registered definition carries that user when the
/// caller named none. Nothing else is invented for a member the caller left out.
pub fn defaulted_container_definitions(
    containers: &[ContainerDefinition],
    network_mode: &str,
) -> Vec<ContainerDefinition> {
    containers
        .iter()
        .map(|container| ContainerDefinition {
            port_mappings: container.port_mappings.as_ref().map(|mappings| {
                mappings
                    .iter()
                    .map(|mapping| defaulted_port_mapping(mapping, network_mode))
                    .collect()
            }),
            user: match (&container.user, &container.firelens_configuration) {
                (Some(user), _) => Some(user.clone()),
                (None, Some(_)) => Some(FIRELENS_ROUTER_USER.to_string()),
                (None, None) => None,
            },
            ..container.clone()
        })
        .collect()
}

impl TaskDefinitionRecord {
    /// The launch types this revision was validated against — `TaskDefinition$
    /// compatibilities`: "Amazon ECS validates the task definition parameters with
    /// those supported by the launch type."
    ///
    /// Derived from the STORED revision and reported in [`COMPATIBILITY_ORDER`]: a launch
    /// type is reported when the revision's parameters meet that launch type's documented
    /// requirements — Fargate needs `awsvpc` ("For Amazon ECS tasks on Fargate, the awsvpc
    /// network mode is required") plus task-level `cpu` and `memory` ("This field is
    /// required for Fargate"); `awsvpc` is in turn not available to the external
    /// (on-premises) launch type; EC2 supports every network mode ("For Amazon ECS tasks on
    /// Amazon EC2 Linux instances, any network mode can be used"); and
    /// Fargate additionally
    /// needs a principal for every image fetch the revision's containers need authenticating
    /// ([`TaskDefinitionRecord::pull_needs_execution_role`]), because a Fargate task runs
    /// on capacity Amazon ECS owns and so has no container instance whose own role the
    /// agent could pull under.
    ///
    /// The launch types this list is drawn from are [`COMPATIBILITY_ORDER`], the launch
    /// types recorded ECS projections report. Which launch types a PLACEMENT may name
    /// outright is a further and narrower question ([`PLACEMENT_LAUNCH_TYPES`]), decided
    /// where a `launchType` is checked ([`validate_launch_type`]).
    ///
    /// What the REGISTRATION asked to be validated against (`requiresCompatibilities`) is
    /// reported separately and unchanged: the derived list neither adds a launch type the
    /// revision was not validated against nor withholds one it was, so it is a function of
    /// the revision's own parameters rather than of what the request asked for or of which
    /// capacity providers the account happens to hold.
    pub fn compatibilities(&self) -> Vec<String> {
        let awsvpc = self.network_mode == NETWORK_MODE_AWSVPC;
        COMPATIBILITY_ORDER
            .iter()
            .filter(|launch_type| match **launch_type {
                // Every revision is validated against the capacity of the caller's own
                // account, whatever network mode it declares ("For Amazon ECS tasks on
                // Amazon EC2 Linux instances, any network mode can be used").
                "EC2" => true,
                "EXTERNAL" => !awsvpc,
                "FARGATE" => {
                    awsvpc
                        && self.cpu.is_some()
                        && self.memory.is_some()
                        && !self.pull_needs_execution_role()
                        // And none of its members asks for capacity Fargate has none of
                        // ([`TaskDefinitionRecord::needs_capacity_fargate_lacks`]).
                        && !self.needs_capacity_fargate_lacks()
                }
                // Every launch type is classified above by its own documented
                // requirements, so one added to [`COMPATIBILITY_ORDER`] later has to be
                // classified deliberately rather than inheriting a neighbour's rule.
                _ => false,
            })
            .map(|launch_type| (*launch_type).to_string())
            .collect()
    }

    /// Whether an image this revision's containers run can be fetched only AS the task
    /// execution role, which the revision names none of.
    ///
    /// Amazon ECS pulls an image held in Amazon ECR as "the task execution role that grants
    /// the Amazon ECS container agent permission to make Amazon Web Services API calls on
    /// your behalf" (`TaskDefinition$executionRoleArn`), since that registry serves a pull
    /// only to an authenticated principal ([`needs_execution_role_to_pull`]). On capacity of
    /// the caller's own the agent falls back on the container instance's own IAM role, so
    /// such a revision still runs there; Amazon ECS's own capacity has no such instance,
    /// which is why this is read where a FARGATE compatibility is derived and where a
    /// registration that REQUIRES Fargate is checked.
    pub fn pull_needs_execution_role(&self) -> bool {
        self.execution_role_arn.is_none() && self.pull_needs_authentication()
    }

    /// The `TaskDefinition` every task-definition operation returns, built from what is
    /// stored — never from the request asking for it. The container definitions go
    /// through [`container_definition_view`], so the collections ECS materialises come
    /// back as empty lists rather than as missing keys.
    pub fn view(&self) -> TaskDefinition {
        TaskDefinition {
            task_definition_arn: Some(self.arn.clone()),
            container_definitions: Some(
                self.container_definitions
                    .iter()
                    .map(container_definition_view)
                    .collect(),
            ),
            family: Some(self.family.clone()),
            task_role_arn: self.task_role_arn.clone(),
            execution_role_arn: self.execution_role_arn.clone(),
            // The mode the registration DECLARED, not the one its task runs in: the
            // member carries no default of its own to materialise.
            network_mode: self.declared_network_mode.clone(),
            revision: self.revision,
            volumes: Some(self.volumes.clone()),
            status: Some(self.status.clone()),
            requires_attributes: self.requires_attributes(),
            placement_constraints: Some(self.placement_constraints.clone()),
            compatibilities: Some(self.compatibilities()),
            runtime_platform: self.runtime_platform.clone(),
            // "The task launch types the task definition was validated against" is
            // `compatibilities`, which Amazon ECS DERIVES; `requiresCompatibilities` is
            // what the REGISTRATION asked for, so a registration that asked for nothing
            // surfaces nothing rather than an explicit empty list the caller never sent.
            requires_compatibilities: (!self.requires_compatibilities.is_empty())
                .then(|| self.requires_compatibilities.clone()),
            cpu: self.cpu.clone(),
            memory: self.memory.clone(),
            inference_accelerators: None,
            pid_mode: self.pid_mode.clone(),
            ipc_mode: self.ipc_mode.clone(),
            proxy_configuration: self.proxy_configuration.clone(),
            registered_at: Some(self.registered_at),
            deregistered_at: self.deregistered_at,
            delete_requested_at: self.delete_requested_at,
            // "The principal that registered the task definition", as it was resolved
            // when the registration ran.
            registered_by: Some(self.registered_by.clone()),
            ephemeral_storage: self.ephemeral_storage.clone(),
            enable_fault_injection: self.enable_fault_injection,
            // The configuration the registration set, reported back as it was stored.
            trusted_execution_configuration: self.trusted_execution_configuration.clone(),
            // The status the revision held before THIS call is a property of a call that
            // moved it, not of the stored revision, so a projection that moved nothing
            // reports none ([`TaskDefinitionRecord::transition_view`]).
            previous_status: None,
        }
    }

    /// The same projection, as the response of a call that MOVED this revision's status
    /// reports it: `previousStatus` is the status the revision held when the call arrived,
    /// beside the `status` it holds now.
    ///
    /// `previous_status` is read before the call writes, so a call that found the revision
    /// already in the status it was asked to move it to reports that status at both members
    /// — which is what tells a caller a repeat from the transition it asked for.
    pub fn transition_view(&self, previous_status: &str) -> TaskDefinition {
        TaskDefinition {
            previous_status: Some(previous_status.to_string()),
            ..self.view()
        }
    }
}
