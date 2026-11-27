// SPDX-License-Identifier: CC0-1.0

//! The container-definition and volume features FARGATE capacity provides nothing for.
//!
//! A Fargate task runs on capacity Amazon ECS owns, so a revision may only declare what
//! that capacity can present to a container. The members below name something the CALLER's own
//! container instance would have to supply, and each member's own documentation says so:
//!
//! * `ResourceRequirement$type` `GPU` is "the number of physical `GPUs` the Amazon ECS
//!   container agent reserves for the container", bounded by "the number of available GPUs
//!   on the container instance that the task is launched on";
//! * `HostVolumeProperties$sourcePath` is "the path on the host container instance that's
//!   presented to the container", and the member adds "If you're using the Fargate launch
//!   type, the `sourcePath` parameter is not supported."
//! * the `linuxParameters` members that configure the HOST the container runs on, each of
//!   which says the same of itself ([`FARGATE_UNSUPPORTED_LINUX_PARAMETERS`]).
//!
//! So the rule is read BOTH ways from here, as one statement of it: a registration that
//! REQUIRES Fargate and declares one of these is refused before any revision exists
//! ([`fargate_feature_violation`]), and a stored revision that declares one reports no
//! Fargate compatibility ([`TaskDefinitionRecord::needs_capacity_fargate_lacks`]), so the
//! launch types a revision admits and the registrations Amazon ECS accepts cannot
//! disagree.

use super::*;
use crate::ops::types::LinuxParameters;

/// The `ResourceType` naming a physical device the container instance attaches
/// (`smithy_query(query="shape",shape="ResourceType")`: `GPU`, `InferenceAccelerator`,
/// `NeuronDevice`), of which `GPU` is the one Amazon ECS words a Fargate refusal for.
const RESOURCE_TYPE_GPU: &str = "GPU";

/// `ClientException$message` for a Fargate registration whose containers reserve a GPU.
/// One fixed sentence: the member is the same on every container, so nothing of the
/// request narrows it.
pub const FARGATE_GPU_UNSUPPORTED_MESSAGE: &str =
    "Tasks using the Fargate launch type do not support GPU resource requirements.";

/// `ClientException$message` for a Fargate registration that bind-mounts a path of a
/// container instance, which Amazon ECS states by naming the one member that asked for it
/// ([`fargate_unsupported_member_message`]).
pub const FARGATE_SOURCE_PATH_UNSUPPORTED_MESSAGE: &str =
    "Fargate compatible task definitions do not support sourcePath";

/// `ClientException$message` for a Fargate registration that declares a member Fargate
/// capacity supports nothing for: the one sentence this API states for the class, naming the
/// MEMBER at fault and nothing else of the request — the member is the one thing the caller
/// has to take out.
pub fn fargate_unsupported_member_message(member: &str) -> String {
    format!("Fargate compatible task definitions do not support {member}")
}

/// ONE `linuxParameters` member a launch type provides nothing for: the member's name as the
/// API spells it, and the reading that answers whether a container DECLARED it.
pub type UnsupportedLinuxParameter = (&'static str, fn(&LinuxParameters) -> bool);

/// The `linuxParameters` members a Fargate task's capacity provides nothing for, in the order
/// `LinuxParameters` declares them, each with its own doc saying so:
///
/// * `devices` — "If you're using tasks that use the Fargate launch type, the `devices`
///   parameter isn't supported."
/// * `sharedMemorySize` — "If you are using tasks that use the Fargate launch type, the
///   `sharedMemorySize` parameter is not supported."
/// * `maxSwap` — "If you're using tasks that use the Fargate launch type, the `maxSwap`
///   parameter isn't supported."
/// * `swappiness` — "If you're using tasks that use the Fargate launch type, the `swappiness`
///   parameter isn't supported."
///
/// Each names something of the HOST a container runs on — a device of the instance, the size
/// of its shared-memory volume, and how it swaps — which is the instance's own and not the
/// container's, so a Fargate task has none of it to configure.
///
/// Each member is paired with the reading that finds it DECLARED, so a member added here
/// cannot be read as another one's: the pair is the whole of the rule for that member.
pub const FARGATE_UNSUPPORTED_LINUX_PARAMETERS: [UnsupportedLinuxParameter; 4] = [
    ("devices", |parameters: &LinuxParameters| {
        parameters
            .devices
            .as_ref()
            .is_some_and(|devices| !devices.is_empty())
    }),
    ("sharedMemorySize", |parameters: &LinuxParameters| {
        parameters.shared_memory_size.is_some()
    }),
    ("maxSwap", |parameters: &LinuxParameters| {
        parameters.max_swap.is_some()
    }),
    ("swappiness", |parameters: &LinuxParameters| {
        parameters.swappiness.is_some()
    }),
];

/// The one Linux capability a Fargate task may ADD to the set Docker gives its containers.
/// `KernelCapabilities$add`: "Tasks launched on Fargate only support adding the `SYS_PTRACE`
/// kernel capability." Dropping capabilities is not bounded that way — the member's own doc
/// bounds only `add` — so `drop` keeps the whole catalog.
pub const FARGATE_ADDABLE_CAPABILITY: &str = "SYS_PTRACE";

/// `ClientException$message` for a Fargate registration that adds a capability other than
/// [`FARGATE_ADDABLE_CAPABILITY`], which Amazon ECS states by naming the capability: that name
/// is the one value the caller has to take out. RECORDED behaviour, trailing period included.
pub fn fargate_capability_not_allowed_message(capability: &str) -> String {
    format!("{capability} is not allowed on Fargate.")
}

/// The first capability any container of `containers` ADDS that a Fargate task may not add, or
/// `None` when none of them adds one.
///
/// Only a name the member's own catalog holds is weighed here
/// ([`super::KERNEL_CAPABILITY_VALUES`]): a name outside that catalog names no capability at
/// all, which is the condition the member states for every launch type
/// ([`super::kernel_capabilities_violation`]), so it is left to that rule rather than reported
/// as a capability Fargate does not allow.
pub fn fargate_unsupported_added_capability(containers: &[ContainerDefinition]) -> Option<&str> {
    containers
        .iter()
        .filter_map(|container| container.linux_parameters.as_ref())
        .filter_map(|parameters| parameters.capabilities.as_ref())
        .flat_map(|capabilities| capabilities.add.iter().flatten())
        .map(String::as_str)
        .find(|name| {
            *name != FARGATE_ADDABLE_CAPABILITY && super::KERNEL_CAPABILITY_VALUES.contains(name)
        })
}

/// The first `linuxParameters` member of `containers` that Fargate capacity provides nothing
/// for, in the order [`FARGATE_UNSUPPORTED_LINUX_PARAMETERS`] declares them — or `None` when
/// no container declares one.
///
/// A member is DECLARED when the request carried it, whatever value it carried: each of them
/// configures the host, so naming it at all asks Fargate for something it has none of.
pub fn fargate_unsupported_linux_parameter(
    containers: &[ContainerDefinition],
) -> Option<&'static str> {
    FARGATE_UNSUPPORTED_LINUX_PARAMETERS
        .into_iter()
        .find(|(_, declared)| {
            containers
                .iter()
                .filter_map(|container| container.linux_parameters.as_ref())
                .any(declared)
        })
        .map(|(member, _)| member)
}

/// Whether any container of the list reserves a GPU — `resourceRequirements` of type
/// [`RESOURCE_TYPE_GPU`], whatever count it asks for.
pub fn reserves_a_gpu(containers: &[ContainerDefinition]) -> bool {
    containers.iter().any(|container| {
        container
            .resource_requirements
            .iter()
            .flatten()
            .any(|requirement| requirement.r#type == RESOURCE_TYPE_GPU)
    })
}

/// Whether any volume of the list bind-mounts a path of the container instance — a `host`
/// volume that names a `sourcePath`.
///
/// A `host` volume that names NONE is a different volume: "If this parameter is empty,
/// then the Docker daemon has assigned a host path for you", which asks for no path of the
/// instance, so it is not what the refusal is about.
pub fn binds_a_host_path(volumes: &[Volume]) -> bool {
    volumes.iter().any(|volume| {
        volume.host.as_ref().is_some_and(|host| {
            host.source_path
                .as_deref()
                .is_some_and(|path| !path.trim().is_empty())
        })
    })
}

/// The refusal a FARGATE registration earns for declaring a feature Fargate provides
/// nothing for, or `None` when it declares none.
///
/// The members are weighed in the order the operation declares them,
/// `containerDefinitions` before `volumes`, so the refusal a request earns never depends
/// on anything but its values — and within the container definitions, the reservation before
/// the `linuxParameters` the shape declares after it.
pub fn fargate_feature_violation(
    containers: &[ContainerDefinition],
    volumes: &[Volume],
) -> Option<String> {
    if reserves_a_gpu(containers) {
        return Some(FARGATE_GPU_UNSUPPORTED_MESSAGE.to_string());
    }
    if let Some(member) = fargate_unsupported_linux_parameter(containers) {
        return Some(fargate_unsupported_member_message(member));
    }
    if let Some(capability) = fargate_unsupported_added_capability(containers) {
        return Some(fargate_capability_not_allowed_message(capability));
    }
    if binds_a_host_path(volumes) {
        return Some(FARGATE_SOURCE_PATH_UNSUPPORTED_MESSAGE.to_string());
    }
    None
}

impl TaskDefinitionRecord {
    /// Whether this stored revision declares a feature Fargate provides nothing for, so
    /// Fargate is a launch type it was never validated against
    /// ([`TaskDefinitionRecord::compatibilities`]) — the same rule the registration above
    /// is refused by, read off the record.
    pub fn needs_capacity_fargate_lacks(&self) -> bool {
        fargate_feature_violation(&self.container_definitions, &self.volumes).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{HostVolumeProperties, ResourceRequirement};

    /// A container that declares nothing but its name, the base each row below adds one
    /// member to.
    fn plain(name: &str) -> ContainerDefinition {
        ContainerDefinition {
            name: Some(name.to_string()),
            ..ContainerDefinition::default()
        }
    }

    /// A container reserving `count` of resource `kind`.
    fn reserving(kind: &str, count: &str) -> ContainerDefinition {
        ContainerDefinition {
            resource_requirements: Some(vec![ResourceRequirement {
                r#type: kind.to_string(),
                value: Some(count.to_string()),
                sharing_strategy: None,
            }]),
            ..plain("app")
        }
    }

    /// RULE: a Fargate registration is refused for a GPU reservation whatever count it
    /// asks for and whichever container of the list carries it, and for nothing else in
    /// the member.
    #[test]
    fn a_gpu_reservation_is_refused_on_fargate() {
        // (case, the container list, refused)
        let rows: &[(&str, Vec<ContainerDefinition>, bool)] = &[
            ("no resource requirements", vec![plain("app")], false),
            ("one GPU", vec![reserving("GPU", "1")], true),
            ("every GPU on the instance", vec![reserving("GPU", "ALL")], true),
            ("several GPUs", vec![reserving("GPU", "4")], true),
            (
                "a GPU on the second container",
                vec![plain("app"), reserving("GPU", "1")],
                true,
            ),
            // The other `ResourceType` values name their own members and their own
            // documented rules, so this sentence is not theirs.
            (
                "an inference accelerator",
                vec![reserving("InferenceAccelerator", "device1")],
                false,
            ),
            ("a Neuron device", vec![reserving("NeuronDevice", "ALL")], false),
        ];
        for (case, containers, refused) in rows {
            let answer = fargate_feature_violation(containers, &[]);
            assert_eq!(
                answer.as_deref(),
                refused.then_some(FARGATE_GPU_UNSUPPORTED_MESSAGE),
                "case {case:?}: expected refused == {refused}, got {answer:?}"
            );
        }
    }

    /// RULE: a Fargate registration is refused for every `linuxParameters` member that
    /// configures the HOST, naming the member, and accepted for the ones that configure the
    /// container.
    #[test]
    fn a_host_linux_parameter_is_refused_on_fargate() {
        use crate::ops::types::{Device, LinuxParameters};

        /// A container whose `linuxParameters` are `parameters`.
        fn declaring(parameters: LinuxParameters) -> ContainerDefinition {
            ContainerDefinition {
                linux_parameters: Some(parameters),
                ..plain("app")
            }
        }

        // (case, the parameters one container declares, the member the refusal names)
        let rows: &[(&str, LinuxParameters, Option<&str>)] = &[
            (
                "a device of the instance",
                LinuxParameters {
                    devices: Some(vec![Device {
                        host_path: "/dev/nvidia0".to_string(),
                        container_path: None,
                        permissions: None,
                    }]),
                    ..LinuxParameters::default()
                },
                Some("devices"),
            ),
            (
                "the size of the shared-memory volume",
                LinuxParameters {
                    shared_memory_size: Some(64),
                    ..LinuxParameters::default()
                },
                Some("sharedMemorySize"),
            ),
            (
                "swap the instance would provide",
                LinuxParameters {
                    max_swap: Some(0),
                    ..LinuxParameters::default()
                },
                Some("maxSwap"),
            ),
            (
                "how the instance swaps",
                LinuxParameters {
                    swappiness: Some(60),
                    ..LinuxParameters::default()
                },
                Some("swappiness"),
            ),
            (
                "the members that configure the container itself",
                LinuxParameters {
                    init_process_enabled: Some(true),
                    ..LinuxParameters::default()
                },
                None,
            ),
            (
                "no device at all",
                LinuxParameters {
                    devices: Some(Vec::new()),
                    ..LinuxParameters::default()
                },
                None,
            ),
        ];
        for (case, parameters, member) in rows {
            let containers = [declaring(parameters.clone())];
            let answer = fargate_feature_violation(&containers, &[]);
            assert_eq!(
                answer,
                member.map(fargate_unsupported_member_message),
                "case {case:?}: expected {member:?} to be named, got {answer:?}"
            );
        }
    }

    /// RULE: a Fargate registration may ADD only the one capability the member documents for
    /// it, and may drop any of them.
    #[test]
    fn only_one_added_capability_is_allowed_on_fargate() {
        use crate::ops::types::{KernelCapabilities, LinuxParameters};

        /// A container adding `add` and dropping `drop`.
        fn configuring(add: &[&str], drop: &[&str]) -> ContainerDefinition {
            ContainerDefinition {
                linux_parameters: Some(LinuxParameters {
                    capabilities: Some(KernelCapabilities {
                        add: Some(add.iter().map(|name| (*name).to_string()).collect()),
                        drop: Some(drop.iter().map(|name| (*name).to_string()).collect()),
                    }),
                    ..LinuxParameters::default()
                }),
                ..plain("app")
            }
        }

        /// One row of the table below: the case, the capabilities the container adds, the ones
        /// it drops, and the capability the refusal names.
        type Row<'a> = (&'a str, &'a [&'a str], &'a [&'a str], Option<&'a str>);

        let rows: &[Row<'_>] = &[
            ("the one capability Fargate allows", &[FARGATE_ADDABLE_CAPABILITY], &[], None),
            ("adding nothing", &[], &["MKNOD", "ALL"], None),
            ("another capability", &["NET_ADMIN"], &[], Some("NET_ADMIN")),
            ("the whole default set", &["ALL"], &[], Some("ALL")),
            (
                "one Fargate allows beside one it does not",
                &[FARGATE_ADDABLE_CAPABILITY, "SYS_ADMIN"],
                &[],
                Some("SYS_ADMIN"),
            ),
            // A name outside the member's own catalog is that rule's, not this one's.
            ("a name no catalog holds", &["CAP_NET_ADMIN"], &[], None),
        ];
        for (case, add, drop, capability) in rows {
            let containers = [configuring(add, drop)];
            let answer = fargate_feature_violation(&containers, &[]);
            assert_eq!(
                answer,
                capability.map(fargate_capability_not_allowed_message),
                "case {case:?}: expected {capability:?} to be named, got {answer:?}"
            );
        }
    }

    /// RULE: a Fargate registration is refused for a `host` volume that names a
    /// `sourcePath`, and accepted for every other volume shape.
    #[test]
    fn a_host_source_path_is_refused_on_fargate() {
        /// A volume whose `host` carries `source_path`, when it carries a `host` at all.
        fn volume(name: &str, host: Option<Option<&str>>) -> Volume {
            Volume {
                name: Some(name.to_string()),
                host: host.map(|source_path| HostVolumeProperties {
                    source_path: source_path.map(str::to_string),
                }),
                ..Volume::default()
            }
        }

        // (case, the volume list, refused)
        let rows: &[(&str, Vec<Volume>, bool)] = &[
            ("no volumes", vec![], false),
            ("a volume with no host", vec![volume("scratch", None)], false),
            (
                "an empty host volume",
                vec![volume("scratch", Some(None))],
                false,
            ),
            (
                "a host volume naming a path",
                vec![volume("state", Some(Some("/var/lib/app")))],
                true,
            ),
            (
                "a host volume naming a Windows path",
                vec![volume("state", Some(Some("C:\\ProgramData\\app")))],
                true,
            ),
            (
                "a path on the second volume",
                vec![
                    volume("scratch", Some(None)),
                    volume("state", Some(Some("/data"))),
                ],
                true,
            ),
        ];
        for (case, volumes, refused) in rows {
            let answer = fargate_feature_violation(&[plain("app")], volumes);
            assert_eq!(
                answer.as_deref(),
                refused.then_some(FARGATE_SOURCE_PATH_UNSUPPORTED_MESSAGE),
                "case {case:?}: expected refused == {refused}, got {answer:?}"
            );
        }
    }
}
