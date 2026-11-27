// SPDX-License-Identifier: CC0-1.0

//! The CPU and memory a container instance has LEFT once the tasks placed on it are
//! accounted for, and the placement that is held to it.
//!
//! # The rule this module implements
//!
//! `ContainerInstance$remainingResources` is "the remaining CPU and memory that wasn't
//! already allocated to tasks and is therefore available for new tasks", and what a task
//! allocates is what it RESERVES. `TaskDefinition$memory` is "the amount of memory (in MiB)
//! used by the task" where a revision states one for the whole task; where it does not,
//! each container's own figure is what the task takes —
//! `ContainerDefinition$memoryReservation` is the soft limit that lets a container "only
//! reserve [that much] memory from the remaining resources on the container instance", and
//! a container stating only the hard `memory` limit reserves that. CPU is read the same way:
//! `TaskDefinition$cpu` for the whole task, else `ContainerDefinition$cpu`, "the number of
//! cpu units the Amazon ECS container agent reserves for the container".
//!
//! So a placement is made only on an instance whose remainder covers what the task
//! reserves — "if your cluster doesn't have any registered container instances with the
//! required CPU units available, the task fails" — and a run no instance of the cluster can
//! take places nothing: `RunTask` answers it with the `Failures` the placement earned,
//! `Failure$arn` being "the ARN of the failed resource" (the container instance the
//! placement was rejected on) and `Failure$reason` the structured code of the resource that
//! ran out ([`resource_failure_reason`]).
//!
//! An instance that reports no figure for a resource is held to nothing by it: the figures
//! are the AGENT's — `registeredResources` is "the amount of each resource that was
//! available on the container instance when the container agent registered it" — so a
//! registration that reported none leaves that resource unknown rather than zero, and a
//! placement is not refused for capacity nobody reported.

use super::*;

/// `Resource$name` of the CPU a container instance offers, counted in the CPU units Amazon
/// ECS expresses CPU in ("1024 CPU units is the equivalent of 1 vCPU").
pub const RESOURCE_CPU: &str = "CPU";

/// `Resource$name` of the memory it offers, in MiB.
pub const RESOURCE_MEMORY: &str = "MEMORY";

/// The prefix the structured `Failure$reason` of a placement short of a resource carries,
/// which the resource's own name completes ([`resource_failure_reason`]).
pub const RESOURCE_FAILURE_PREFIX: &str = "RESOURCE:";

/// `Failure$reason` for a placement a container instance could not take for want of
/// `resource`: the structured `RESOURCE:<name>` code, over the resource's own
/// `Resource$name` — so the reason a client reads names the same resource
/// `DescribeContainerInstances` reports the instance's remainder of.
pub fn resource_failure_reason(resource: &str) -> String {
    format!("{RESOURCE_FAILURE_PREFIX}{resource}")
}

/// What ONE task takes out of the container instance that hosts it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskReservation {
    /// The CPU units it reserves.
    pub cpu: i64,
    /// The MiB of memory it reserves.
    pub memory: i64,
}

impl TaskReservation {
    /// What a task of `definition` reserves: the revision's own task-level figures where it
    /// states them, else the sum of what its containers reserve.
    pub fn of_revision(definition: &TaskDefinitionRecord) -> Self {
        Self {
            cpu: task_level(definition.cpu.as_deref(), cpu_units).unwrap_or_else(|| {
                definition
                    .container_definitions
                    .iter()
                    .map(|container| i64::from(container.cpu.max(0)))
                    .sum()
            }),
            memory: task_level(definition.memory.as_deref(), memory_mib).unwrap_or_else(|| {
                definition
                    .container_definitions
                    .iter()
                    .map(|container| {
                        declared_reservation(
                            container.memory.map(i64::from),
                            container.memory_reservation.map(i64::from),
                        )
                    })
                    .sum()
            }),
        }
    }

    /// The same for a task that is already placed, read off the record the placement left:
    /// a task carries the size it was placed with ([`TaskRecord::cpu`], `memory`) and its
    /// containers the reservations their definitions declared, so what a host has allocated
    /// is read off the tasks on it rather than from a counter that could drift from them.
    ///
    /// The ONE reader of what a placed task reserves: the host choice and the remainder an
    /// instance reports go through it, and so does the reserved half of the utilization a
    /// cluster or a service publishes ([`super::container_consumption`]) — so what a cluster
    /// reports as reserved and what its container instances report as taken cannot disagree.
    pub fn of_task(task: &TaskRecord) -> Self {
        Self {
            cpu: task_level(task.cpu.as_deref(), cpu_units).unwrap_or_else(|| {
                task.containers
                    .iter()
                    .map(|container| amount(container.cpu.as_deref(), cpu_units).unwrap_or(0))
                    .sum()
            }),
            memory: task_level(task.memory.as_deref(), memory_mib).unwrap_or_else(|| {
                task.containers
                    .iter()
                    .map(|container| {
                        declared_reservation(
                            amount(container.memory.as_deref(), memory_mib),
                            amount(container.memory_reservation.as_deref(), memory_mib),
                        )
                    })
                    .sum()
            }),
        }
    }

    /// The two reservations added, as the tasks of one host add up.
    fn and(self, other: Self) -> Self {
        Self {
            cpu: self.cpu.saturating_add(other.cpu),
            memory: self.memory.saturating_add(other.memory),
        }
    }
}

/// The units a TASK-LEVEL `cpu` or `memory` value states, and `None` for a revision that
/// states none at all — which is the case the containers' own reservations are summed for.
///
/// A value that spells no number of units states none either: `RegisterTaskDefinition`
/// refuses such a value, so a stored revision carries one that reads.
fn task_level(value: Option<&str>, read: impl Fn(&str) -> Option<i64>) -> Option<i64> {
    amount(value, read).filter(|units| *units > 0)
}

/// One stated amount, read through `read` ([`cpu_units`], [`memory_mib`] — the same reading
/// a registration's own validation is made with).
fn amount(value: Option<&str>, read: impl Fn(&str) -> Option<i64>) -> Option<i64> {
    read(value?)
}

/// What one CONTAINER reserves of a resource it states a soft and a hard limit for: the
/// soft limit where it has one ("allow the container to only reserve [the
/// `memoryReservation`] of memory from the remaining resources on the container instance"),
/// else the hard limit it states instead.
fn declared_reservation(hard_limit: Option<i64>, soft_limit: Option<i64>) -> i64 {
    soft_limit.or(hard_limit).unwrap_or(0).max(0)
}

/// The amount of `name` the agent registered for `instance`, or `None` when its
/// registration reported no figure for it.
///
/// `Resource` carries the amount in the member its own `type` names ("Valid values: INTEGER,
/// DOUBLE, LONG, or STRINGSET"), so each of the numeric members is read in turn and a
/// resource reported as none of them carries no amount — a `STRINGSET` resource such as the
/// ports an agent holds is not an amount a task spends. A reported zero is read as no figure
/// for the reason the module header gives: it is what an agent that reported nothing leaves.
pub fn registered_resource_amount(instance: &ContainerInstanceRecord, name: &str) -> Option<i64> {
    instance
        .registered_resources
        .iter()
        .filter(|resource| resource.name.as_deref() == Some(name))
        .find_map(|resource| {
            [
                i64::from(resource.integer_value),
                resource.long_value,
                whole_amount(resource.double_value),
            ]
            .into_iter()
            .find(|amount| *amount > 0)
        })
}

/// A `DOUBLE`-typed amount as the whole number of units it covers: a host offers the units it
/// has whole, so a fractional figure is read DOWN rather than rounded up into capacity the
/// instance does not have. Zero for a figure that is no number at all.
fn whole_amount(value: f64) -> i64 {
    if !value.is_finite() || value < 1.0 {
        return 0;
    }
    value.trunc().min(i64::MAX as f64) as i64
}

impl ComAmazonawsEcsAmazonec2containerservicev20141113State {
    /// What the tasks hosted on `instance_arn` reserve between them.
    ///
    /// A task that has reached `STOPPED` reserves nothing: its containers are gone, so what
    /// they held is the host's again — the same reading the host ports a stopped task held
    /// are released under ([`Self::bind_task_host_ports`]).
    pub fn reserved_instance_resources(&self, instance_arn: &str) -> TaskReservation {
        self.tasks
            .values()
            .filter(|task| {
                task.container_instance_arn.as_deref() == Some(instance_arn) && task.is_active()
            })
            .map(TaskReservation::of_task)
            .fold(TaskReservation::default(), TaskReservation::and)
    }

    /// Which resource of `instance` a task reserving `needs` would exhaust, or `None` when
    /// the instance can take that task.
    ///
    /// The MEMORY is answered first, so a placement short of both names the resource a
    /// client sizes a cluster by. A resource the registration reported no figure for holds
    /// nothing back ([`registered_resource_amount`]).
    pub fn instance_resource_shortfall(
        &self,
        instance: &ContainerInstanceRecord,
        needs: TaskReservation,
    ) -> Option<&'static str> {
        let reserved = self.reserved_instance_resources(&instance.arn);
        [
            (RESOURCE_MEMORY, reserved.memory, needs.memory),
            (RESOURCE_CPU, reserved.cpu, needs.cpu),
        ]
        .into_iter()
        .find(|(name, held, wanted)| {
            registered_resource_amount(instance, name)
                .is_some_and(|registered| *wanted > registered.saturating_sub(*held))
        })
        .map(|(name, _, _)| name)
    }

    /// Whether `instance` can take a task reserving `needs`.
    pub fn instance_admits_task(
        &self,
        instance: &ContainerInstanceRecord,
        needs: TaskReservation,
    ) -> bool {
        self.instance_resource_shortfall(instance, needs).is_none()
    }

    /// The `remainingResources` of `instance`: "the remaining CPU and memory that wasn't
    /// already allocated to tasks", which is its registered set with what the tasks on it
    /// reserve taken off.
    ///
    /// Every other resource the registration reported is carried through as it stands: the
    /// member is documented over the CPU and the memory, and the ports an agent reports in
    /// use are not an amount a task spends.
    pub fn remaining_instance_resources(&self, instance: &ContainerInstanceRecord) -> Vec<Resource> {
        let reserved = self.reserved_instance_resources(&instance.arn);
        instance
            .registered_resources
            .iter()
            .map(|resource| {
                let held = match resource.name.as_deref() {
                    Some(RESOURCE_CPU) => reserved.cpu,
                    Some(RESOURCE_MEMORY) => reserved.memory,
                    _ => return resource.clone(),
                };
                Resource {
                    integer_value: i32::try_from(
                        i64::from(resource.integer_value).saturating_sub(held).max(0),
                    )
                    .unwrap_or(i32::MAX),
                    long_value: resource.long_value.saturating_sub(held).max(0),
                    ..resource.clone()
                }
            })
            .collect()
    }

    /// The `Failures` a placement of a task reserving `needs` into `cluster_name` earns when
    /// no container instance of that cluster can take it: one per instance the placement
    /// could otherwise have been made on, each naming the resource that ran out there.
    ///
    /// Empty where the placement HAS a host to go on — one candidate that admits the task is
    /// all a placement needs — and empty for a launch type that takes no container instance
    /// of the cluster at all ([`needs_registered_capacity`]): a Fargate task is placed on
    /// capacity AWS owns, which no registration of this cluster reports.
    ///
    /// Each entry names its instance by the id alone
    /// ([`unqualified_container_instance_arn`]): a failure answers about the instance a
    /// placement was rejected on, not about the cluster the placement was made against.
    /// Ordered by that id, so two runs refused by the same cluster report the same list.
    pub fn resource_placement_failures(
        &self,
        region: &str,
        cluster_name: &str,
        launch_type: Option<&str>,
        needs: TaskReservation,
    ) -> Vec<Failure> {
        if !needs_registered_capacity(launch_type) {
            return Vec::new();
        }
        let mut candidates: Vec<&ContainerInstanceRecord> = self
            .container_instances
            .values()
            .filter(|instance| {
                instance.cluster_name == cluster_name && instance.accepts_task_placement()
            })
            .collect();
        candidates.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let mut failures = Vec::with_capacity(candidates.len());
        for instance in candidates {
            let Some(resource) = self.instance_resource_shortfall(instance, needs) else {
                return Vec::new();
            };
            failures.push(Failure {
                arn: Some(unqualified_container_instance_arn(
                    region,
                    &instance.instance_id,
                )),
                reason: Some(resource_failure_reason(resource)),
                detail: None,
            });
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A revision stating `cpu`/`memory` at the task level and/or on its containers.
    fn revision(
        cpu: Option<&str>,
        memory: Option<&str>,
        containers: &[(i32, Option<i32>, Option<i32>)],
    ) -> TaskDefinitionRecord {
        TaskDefinitionRecord {
            arn: "arn:aws:ecs:us-west-2:000000000000:task-definition/sizing:1".to_string(),
            family: "sizing".to_string(),
            revision: 1,
            status: TASK_DEFINITION_ACTIVE.to_string(),
            container_definitions: containers
                .iter()
                .map(|(cpu, memory, memory_reservation)| ContainerDefinition {
                    name: Some("app".to_string()),
                    cpu: *cpu,
                    memory: *memory,
                    memory_reservation: *memory_reservation,
                    ..ContainerDefinition::default()
                })
                .collect(),
            network_mode: NETWORK_MODE_BRIDGE.to_string(),
            declared_network_mode: None,
            cpu: cpu.map(str::to_string),
            memory: memory.map(str::to_string),
            volumes: Vec::new(),
            placement_constraints: Vec::new(),
            requires_compatibilities: Vec::new(),
            task_role_arn: None,
            execution_role_arn: None,
            pid_mode: None,
            ipc_mode: None,
            proxy_configuration: None,
            ephemeral_storage: None,
            runtime_platform: None,
            enable_fault_injection: None,
            trusted_execution_configuration: None,
            tags: Vec::new(),
            registered_by: "arn:aws:iam::000000000000:root".to_string(),
            registered_at: reported_instant(::std::time::UNIX_EPOCH),
            deregistered_at: None,
            delete_requested_at: None,
        }
    }

    /// One row of the table below: the size a revision states at the task level and on each
    /// of its containers (`cpu`, `memory`, `memoryReservation`), and what a task of it
    /// reserves on the host that runs it.
    struct Row<'a> {
        case: &'a str,
        cpu: Option<&'a str>,
        memory: Option<&'a str>,
        containers: &'a [(i32, Option<i32>, Option<i32>)],
        reserves: TaskReservation,
    }

    /// RULE: a task reserves the task-level size its revision states, and the sum of what
    /// its containers reserve when the revision states none — a container's soft limit
    /// where it has one, else its hard limit.
    ///
    /// The rows span a task-level size, a task-level size stated in the other units the
    /// members accept, containers with a hard limit, with a soft limit, with both, with
    /// neither, and several containers of one revision.
    #[test]
    fn a_task_reserves_the_size_its_revision_states() {
        let rows = [
            Row {
                case: "a task-level size",
                cpu: Some("256"),
                memory: Some("512"),
                containers: &[(0, None, None)],
                reserves: TaskReservation {
                    cpu: 256,
                    memory: 512,
                },
            },
            Row {
                case: "a task-level size in vCPUs and GB",
                cpu: Some("1 vCPU"),
                memory: Some("1 GB"),
                containers: &[(0, None, None)],
                reserves: TaskReservation {
                    cpu: 1024,
                    memory: 1024,
                },
            },
            Row {
                case: "no task-level size, a container hard limit",
                cpu: None,
                memory: None,
                containers: &[(128, Some(300), None)],
                reserves: TaskReservation {
                    cpu: 128,
                    memory: 300,
                },
            },
            Row {
                case: "no task-level size, a container soft limit",
                cpu: None,
                memory: None,
                containers: &[(128, None, Some(128))],
                reserves: TaskReservation {
                    cpu: 128,
                    memory: 128,
                },
            },
            Row {
                case: "no task-level size, a container stating both: the soft limit reserves",
                cpu: None,
                memory: None,
                containers: &[(128, Some(300), Some(128))],
                reserves: TaskReservation {
                    cpu: 128,
                    memory: 128,
                },
            },
            Row {
                case: "no task-level size and a container stating neither",
                cpu: None,
                memory: None,
                containers: &[(0, None, None)],
                reserves: TaskReservation { cpu: 0, memory: 0 },
            },
            Row {
                case: "two containers of one revision",
                cpu: None,
                memory: None,
                containers: &[(128, Some(256), None), (64, None, Some(128))],
                reserves: TaskReservation {
                    cpu: 192,
                    memory: 384,
                },
            },
        ];
        for row in rows {
            let reserved = TaskReservation::of_revision(&revision(row.cpu, row.memory, row.containers));
            assert_eq!(
                reserved, row.reserves,
                "case {:?}: expected {:?}, got {reserved:?}",
                row.case, row.reserves
            );
        }
    }

    /// RULE: the amount a registration reported for a resource is the amount read for it,
    /// whichever numeric member carries it — a fractional figure down to the whole units it
    /// covers — and a resource no figure was reported for carries none, which is what a
    /// zero, a string-set resource and a resource the registration never named all leave.
    #[test]
    fn a_registered_amount_is_read_off_the_member_that_carries_it() {
        let instance = ContainerInstanceRecord {
            registered_resources: vec![
                Resource {
                    name: Some(RESOURCE_MEMORY.to_string()),
                    integer_value: 3955,
                    ..Resource::default()
                },
                Resource {
                    name: Some(RESOURCE_CPU.to_string()),
                    long_value: 2048,
                    ..Resource::default()
                },
                Resource {
                    name: Some("GPU".to_string()),
                    double_value: 2.75,
                    ..Resource::default()
                },
                Resource {
                    name: Some("NEURON".to_string()),
                    integer_value: 0,
                    ..Resource::default()
                },
                Resource {
                    name: Some("PORTS".to_string()),
                    string_set_value: Some(vec!["22".to_string()]),
                    ..Resource::default()
                },
            ],
            ..probe_instance()
        };
        let rows: [(&str, &str, Option<i64>); 5] = [
            ("an integer amount", RESOURCE_MEMORY, Some(3955)),
            ("a long amount", RESOURCE_CPU, Some(2048)),
            ("a fractional amount, read down to whole units", "GPU", Some(2)),
            ("an amount reported as zero", "NEURON", None),
            ("a resource nothing was reported for", "ANYTHING", None),
        ];
        for (case, name, expected) in rows {
            let read = registered_resource_amount(&instance, name);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
        // A resource reported as a set of strings is no amount a task spends either.
        let ports = registered_resource_amount(&instance, "PORTS");
        assert_eq!(
            ports, None,
            "case \"a string-set resource\": expected no amount, got {ports:?}"
        );
    }

    /// A container instance with nothing registered for it, for the rows above to fill in.
    fn probe_instance() -> ContainerInstanceRecord {
        ContainerInstanceRecord {
            arn: container_instance_arn("us-west-2", "capacity", "11111111"),
            instance_id: "11111111".to_string(),
            cluster_name: "capacity".to_string(),
            ec2_instance_id: "i-0123456789abcdef0".to_string(),
            private_ipv4_address: None,
            status: CONTAINER_INSTANCE_ACTIVE.to_string(),
            status_reason: None,
            agent_connected: true,
            version_info: None,
            agent_update_status: None,
            agent_update_due_at: None,
            agent_connection_lost_due_at: None,
            registered_resources: Vec::new(),
            attributes: Vec::new(),
            platform_devices: Vec::new(),
            tags: Vec::new(),
            registered_at: reported_instant(::std::time::UNIX_EPOCH),
            version: 0,
        }
    }
}
