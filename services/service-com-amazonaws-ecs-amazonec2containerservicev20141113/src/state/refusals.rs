// SPDX-License-Identifier: CC0-1.0

//! What a REQUEST is refused for, and how each refusal is worded.
//!
//! The cross-field rules no model can express (a network configuration an `awsvpc`
//! revision needs, a launch type a revision was validated against, a public IP only
//! Fargate attaches), the resource-not-found sentences every reader answers with, and the
//! derived cluster counters a refusal is weighed against.

use super::*;

/// What one pass over the task map and one over the service map count for a cluster:
/// the lifecycle counts the `Cluster` members report, and the same numbers split by
/// launch type that `Cluster$statistics` reports.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct TaskTally {
    pub(super) running: usize,
    pub(super) pending: usize,
    pub(super) active_services: usize,
    /// One entry per [`STATISTICS_LAUNCH_TYPES`] entry, in that order.
    pub(super) by_launch_type: [LaunchTypeTally; STATISTICS_LAUNCH_TYPES.len()],
}

/// What a cluster's tasks and services count for ONE launch type: the four
/// `Cluster$statistics` counters that launch type contributes.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LaunchTypeTally {
    pub(super) running_tasks: usize,
    pub(super) pending_tasks: usize,
    pub(super) active_services: usize,
    pub(super) draining_services: usize,
}

/// Where `launch_type` sits in [`STATISTICS_LAUNCH_TYPES`], or `None` when the task or
/// service was placed through a capacity provider rather than a launch type — there is
/// no launch-type counter for it to contribute to.
pub(super) fn statistics_launch_type_index(launch_type: Option<&str>) -> Option<usize> {
    let launch_type = launch_type?;
    STATISTICS_LAUNCH_TYPES
        .iter()
        .position(|(name, _)| *name == launch_type)
}

/// `Cluster$statistics`: "the task and service count ... separated by launch type".
///
/// One entry per counter per launch type ([`STATISTICS_LAUNCH_TYPES`]), read off the
/// single pass in [`ComAmazonawsEcsAmazonec2containerservicev20141113State::tally`]. The
/// ORDER is the one recorded ECS responses carry, so a client that reads the list
/// positionally reads the same metric the real service named: the launch types the
/// `Cluster$statistics` doc lists come first, counter by counter in the order it lists
/// them, and the launch types it does not name follow, each one's task counters before
/// any of their service counters.
pub(super) fn cluster_statistics(tally: &TaskTally) -> Vec<KeyValuePair> {
    let segment = |index: usize| STATISTICS_LAUNCH_TYPES[index].1;
    let running_tasks = |index: usize| {
        pair(
            &format!("running{}TasksCount", segment(index)),
            tally.by_launch_type[index].running_tasks.to_string(),
        )
    };
    let pending_tasks = |index: usize| {
        pair(
            &format!("pending{}TasksCount", segment(index)),
            tally.by_launch_type[index].pending_tasks.to_string(),
        )
    };
    let active_services = |index: usize| {
        pair(
            &format!("active{}ServiceCount", segment(index)),
            tally.by_launch_type[index].active_services.to_string(),
        )
    };
    let draining_services = |index: usize| {
        pair(
            &format!("draining{}ServiceCount", segment(index)),
            tally.by_launch_type[index].draining_services.to_string(),
        )
    };

    let documented = 0..DOCUMENTED_STATISTICS_LAUNCH_TYPES;
    let rest = DOCUMENTED_STATISTICS_LAUNCH_TYPES..STATISTICS_LAUNCH_TYPES.len();
    let mut statistics = Vec::with_capacity(4 * STATISTICS_LAUNCH_TYPES.len());
    for index in documented.clone() {
        statistics.push(running_tasks(index));
    }
    for index in documented.clone() {
        statistics.push(pending_tasks(index));
    }
    for index in documented.clone() {
        statistics.push(active_services(index));
    }
    for index in documented {
        statistics.push(draining_services(index));
    }
    for index in rest.clone() {
        statistics.push(running_tasks(index));
        statistics.push(pending_tasks(index));
    }
    for index in rest {
        statistics.push(active_services(index));
        statistics.push(draining_services(index));
    }
    statistics
}

/// A cluster response plus the top-level `clusterCount` Amazon ECS answers a
/// cluster-mutating call with.
///
/// `clusterCount` is RECORDED behaviour, not modelled behaviour: the Smithy model's
/// `CreateClusterResponse`/`DeleteClusterResponse` declare the single member `cluster`
/// (`smithy_query(query="operation", operation="CreateCluster,DeleteCluster")`), while
/// every recorded 200 from either operation carries `clusterCount` beside `cluster`,
/// counting the clusters the account holds under the name the call named once the call
/// has been applied (1 from a create, 0 from a delete — see
/// [`ComAmazonawsEcsAmazonec2containerservicev20141113State::cluster_count_for`]).
/// Because the member is not in the model it has
/// no generated field to fill, so this envelope writes it after the modelled payload's
/// own members, with the member schema the operation declares — the modelled shapes and
/// the model file stay exactly as shipped, and an SDK that knows only the modelled shape
/// ignores the extra key.
#[derive(Debug)]
pub struct ClusterCountEnvelope<T> {
    /// The modelled response — the only part an SDK-generated client reads.
    pub payload: T,
    /// How many clusters the account holds after this call.
    pub cluster_count: i32,
    /// The member schema `clusterCount` is written under, declared by the operation
    /// that answers (each response shape names its own member).
    pub member: &'static ::aws_smithy_schema::Schema,
}

impl<T: ::aws_smithy_schema::serde::SerializableStruct> ::aws_smithy_schema::serde::SerializableStruct
    for ClusterCountEnvelope<T>
{
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.payload.serialize_members(serializer)?;
        serializer.write_integer(self.member, self.cluster_count)
    }
}

/// One `KeyValuePair` of `Cluster$statistics`.
pub(super) fn pair(name: &str, value: String) -> KeyValuePair {
    KeyValuePair {
        name: Some(name.to_string()),
        value: Some(value),
    }
}

/// The values `DescribeClusters$include` accepts: the model's `ClusterField` enum, which
/// [`ClusterFields::from_include`] reads. Also what identifies a refused `include` value
/// as a CLUSTER read's, in `crate::service`.
pub const CLUSTER_FIELDS: [&str; 5] = [
    "ATTACHMENTS",
    "CONFIGURATIONS",
    "SETTINGS",
    "STATISTICS",
    "TAGS",
];

/// Which optional cluster fields a response carries, as `DescribeClusters$include`
/// (the `ClusterField` enum: `ATTACHMENTS`, `CONFIGURATIONS`, `SETTINGS`, `STATISTICS`,
/// `TAGS`) selects them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClusterFields {
    pub attachments: bool,
    pub configurations: bool,
    pub settings: bool,
    pub statistics: bool,
    pub tags: bool,
}

impl ClusterFields {
    /// The fields the `include` list of a describe request asks for. Unknown entries
    /// cannot reach here: `include` targets the closed `ClusterField` enum.
    pub fn from_include(include: Option<&Vec<String>>) -> Self {
        let mut fields = Self::default();
        for field in include.into_iter().flatten() {
            match field.as_str() {
                "ATTACHMENTS" => fields.attachments = true,
                "CONFIGURATIONS" => fields.configurations = true,
                "SETTINGS" => fields.settings = true,
                "STATISTICS" => fields.statistics = true,
                "TAGS" => fields.tags = true,
                _ => {}
            }
        }
        fields
    }
}

/// A `Failure` for a revision a delete cannot act on because it has not been
/// deregistered: the `DeleteTaskDefinitions` doc's own requirement, "You must deregister
/// a task definition revision before you delete it", which is also why the operation
/// reports it per entry rather than refusing the whole batch — one entry of a request may
/// be deletable while another is not.
///
/// The entry carries the ARN of the revision the entry named and the reason, and NOTHING
/// else: the status the revision is in is what the reason already states.
pub fn active_task_definition_failure(arn: &str) -> Failure {
    Failure {
        arn: Some(arn.to_string()),
        reason: Some(TASK_DEFINITION_ACTIVE_FAILURE_REASON.to_string()),
        detail: None,
    }
}

/// `Failure$reason` for a revision a delete cannot act on because it is still `ACTIVE`.
/// Amazon ECS states the status that blocks the delete and the step that clears it, in
/// one fixed sentence carrying none of the request's values.
pub const TASK_DEFINITION_ACTIVE_FAILURE_REASON: &str =
    "The specified task definition is still in ACTIVE status. Please deregister the target \
     and try again.";

/// A `Failure` for an id that names nothing: `reason: "MISSING"`.
pub fn missing_failure(arn: &str) -> Failure {
    Failure {
        arn: Some(arn.to_string()),
        reason: Some(FAILURE_MISSING.to_string()),
        detail: None,
    }
}

/// The `Failure` a batch read of capacity providers reports for an identifier that names
/// no provider the account holds, naming the identifier AS THE CALLER SENT IT.
///
/// The capacity-provider surface states what went wrong in a sentence of its own rather
/// than with the `MISSING` code the other batch reads answer ([`unresolved_failure`]): the
/// provider is named by "the short name or full Amazon Resource Name (ARN)", so the
/// failure echoes the reference the request carried — the only value a client can match
/// its entry back to — and the sentence is fixed text naming nothing of the request.
pub fn capacity_provider_missing_failure(reference: &str) -> Failure {
    Failure {
        arn: Some(reference.to_string()),
        reason: Some(CAPACITY_PROVIDER_MISSING_FAILURE_REASON.to_string()),
        detail: None,
    }
}

/// `Failure$reason` for a capacity-provider identifier that names nothing: the whole
/// sentence, which states both halves of the rule — the identifier is a name or an ARN,
/// and no provider of the account answers to this one.
pub const CAPACITY_PROVIDER_MISSING_FAILURE_REASON: &str =
    "The specified capacity provider does not exist. Specify a valid name or ARN and try \
     again.";

/// The `MISSING` `Failure` a batch read reports for an identifier that resolves to no
/// resource of `resource_type`, naming the identifier AS THE CALLER SENT IT: the value
/// itself when it is already an ARN, otherwise that id alone under the resource type.
///
/// The cluster segment of a task or container-instance ARN
/// ([`task_arn`], [`container_instance_arn`]) identifies the cluster the resource is
/// REGISTERED IN, which only a stored resource has; the id of one that does not exist
/// identifies it account-wide by itself, so no cluster is inserted into the ARN the
/// failure echoes. That is what lets a caller match each failure entry back to the
/// identifier it sent.
pub fn unresolved_failure(region: &str, resource_type: &str, reference: &str) -> Failure {
    let arn = if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("{resource_type}/{reference}"))
    };
    missing_failure(&arn)
}

/// Check a requested `launch_type` against the launch types `definition` supports,
/// answering the message an `InvalidParameterException` carries when it is not one of
/// them.
///
/// `TaskDefinition$compatibilities`: "Amazon ECS validates the task definition
/// parameters with those supported by the launch type" — a revision whose parameters do
/// not support a launch type was never validated against it, so it cannot be placed
/// with it, whichever operation does the placing. Shared by every operation that takes a
/// `launchType`, so a launch type `RunTask` refuses is refused by `CreateService` too.
pub fn validate_launch_type(
    definition: &TaskDefinitionRecord,
    launch_type: &str,
) -> Result<(), String> {
    if definition
        .compatibilities()
        .iter()
        .any(|supported| supported == launch_type)
    {
        return Ok(());
    }
    Err(format!(
        "Task definition does not support launch_type {launch_type}."
    ))
}

/// Check the launch type a placement's tasks will actually RUN with against the launch
/// types `definition` supports, answering the message an `InvalidParameterException`
/// carries when it is not one of them.
///
/// A placement says where its tasks run in one of two ways —
/// "A task can use either a launch type or a capacity provider strategy" — and the
/// revision has to support the infrastructure either way names, since it is the same
/// infrastructure. A `launchType` says it outright and is what the tasks then run with; a
/// placement that names none runs on the capacity providers of its strategy, and the
/// Fargate ones place on Fargate ("When you use Fargate, the capacity providers are FARGATE
/// or FARGATE_SPOT", [`places_on_fargate`]), so a revision that was never validated against
/// the Fargate launch type cannot be placed through them either. A placement that names
/// neither runs on the capacity registered to the cluster, which every revision is
/// validated against ([`DEFAULT_VALIDATED_LAUNCH_TYPES`]), so there is nothing to check.
pub fn validate_placement_launch_type(
    definition: &TaskDefinitionRecord,
    launch_type: Option<&str>,
    strategy: &[CapacityProviderStrategyItem],
) -> Result<(), String> {
    let placed = match launch_type {
        // A launch type the request named is where the tasks run, whatever strategy the
        // cluster holds as its default: that default applies only to a placement that names
        // neither.
        Some(launch_type) => Some(launch_type),
        None if places_on_fargate(None, strategy) => Some(LAUNCH_TYPE_FARGATE),
        None => None,
    };
    match placed {
        Some(launch_type) => validate_launch_type(definition, launch_type),
        None => Ok(()),
    }
}

/// The platform version a request asks for, if it names one at all: a blank member
/// names no platform version, so it reads the same as an absent one ("If one isn't
/// specified, the LATEST platform version is used").
pub fn requested_platform_version(platform_version: Option<&str>) -> Option<&str> {
    platform_version
        .map(str::trim)
        .filter(|version| !version.is_empty())
}

/// Whether a request's `platformVersion` names a platform version this service knows
/// ([`PLATFORM_VERSIONS`]). A request that names one it does not is answered with
/// `PlatformUnknownException` — "The specified platform version doesn't exist." —
/// whichever operation carries it.
pub fn is_unknown_platform_version(platform_version: Option<&str>) -> bool {
    requested_platform_version(platform_version)
        .is_some_and(|version| !PLATFORM_VERSIONS.contains(&version))
}

/// Whether a task placed with `launch_type` needs a container instance registered to
/// its cluster ([`REGISTERED_CAPACITY_LAUNCH_TYPES`]). A task placed through a capacity
/// provider carries no launch type, and the provider's own capacity is what places it,
/// so it does not.
pub fn needs_registered_capacity(launch_type: Option<&str>) -> bool {
    launch_type.is_some_and(|launch_type| REGISTERED_CAPACITY_LAUNCH_TYPES.contains(&launch_type))
}

/// Whether a task placed through `provider` needs a container instance registered to its
/// cluster ALREADY — which is the other half of [`needs_registered_capacity`], for the
/// placement that carries a capacity provider instead of a launch type.
///
/// An Auto Scaling group provider's capacity is the container instances its group
/// registers into the cluster, so a cluster with none has nowhere to place the task —
/// unless the provider scales that group itself. `ManagedScaling`'s doc: "When managed
/// scaling is turned on, Amazon ECS manages the scale-in and scale-out actions of the
/// Auto Scaling group", so such a request is one Amazon ECS satisfies by adding capacity;
/// "If managed scaling is off, the user must manage the scaling of the Auto Scaling
/// group", so then nothing is coming. A managed-instances provider manages its own
/// instances, so it never needs capacity to be there first.
pub fn needs_registered_capacity_now(provider: &CapacityProviderRecord) -> bool {
    let Some(group) = provider.auto_scaling_group_provider.as_ref() else {
        return false;
    };
    group
        .managed_scaling
        .as_ref()
        .and_then(|scaling| scaling.status.as_deref())
        != Some(MANAGED_SCALING_ENABLED)
}

/// Whether a task routed through `provider` still has to have capacity ACQUIRED for it
/// before Amazon ECS can place it, given the `registered_instances` its cluster holds.
///
/// It is the accepted half of [`needs_registered_capacity_now`]: an Auto Scaling group
/// provider whose managed scaling Amazon ECS runs ("Amazon ECS manages the scale-in and
/// scale-out actions of the Auto Scaling group") turns a placement into one ECS can
/// satisfy by adding capacity, and until that capacity is registered as a container
/// instance there is nothing to place the task ON. Such a task is accepted but not placed,
/// which is what [`STATUS_PROVISIONING`] reports; a cluster that already holds registered
/// capacity places the task now, and a provider that names no Auto Scaling group of this
/// account (a predefined Fargate provider, a managed-instances provider) acquires no
/// container instance for this cluster, so neither parks a task.
pub fn awaits_provisioned_capacity(
    provider: Option<&CapacityProviderRecord>,
    registered_instances: usize,
) -> bool {
    registered_instances == 0
        && provider.is_some_and(|provider| {
            provider.auto_scaling_group_provider.is_some()
                && !needs_registered_capacity_now(provider)
        })
}

/// The `lastStatus` a task is placed with: [`STATUS_PROVISIONING`] while its capacity is
/// still being acquired, else [`STATUS_PENDING`]. Shared by every path that places a task,
/// so a task starts in the same lifecycle state whoever placed it.
pub fn initial_task_status(awaiting_capacity: bool) -> &'static str {
    if awaiting_capacity {
        STATUS_PROVISIONING
    } else {
        STATUS_PENDING
    }
}

/// Where a task the SERVICE's own scheduler places takes its capacity from. "A service
/// will return either a `capacityProviderStrategy` or `launchType` parameter, but not
/// both", so exactly one of the two decides the placement.
pub fn service_placement(service: &ServiceRecord) -> Placement {
    let (launch_type, capacity_provider_name) = match &service.launch_type {
        Some(launch_type) => (Some(launch_type.clone()), None),
        None => (
            None,
            service
                .capacity_provider_strategy
                .first()
                .map(|item| item.capacity_provider.clone()),
        ),
    };
    Placement {
        launch_type,
        capacity_provider_name,
        platform_version: service.platform_version.clone(),
        network_configuration: service.network_configuration.clone(),
        started_by: service.deployment_id.clone(),
    }
}

/// Where a task a TASK SET places takes its capacity from. The task set carries its own
/// launch type or capacity provider strategy ("If a `launchType` is specified, the
/// `capacityProviderStrategy` parameter must be omitted"), so its tasks are placed on
/// that rather than on the service's.
pub fn task_set_placement(task_set: &TaskSetRecord) -> Placement {
    Placement {
        launch_type: task_set.launch_type.clone(),
        capacity_provider_name: task_set
            .capacity_provider_strategy
            .first()
            .map(|item| item.capacity_provider.clone()),
        platform_version: task_set.platform_version.clone(),
        network_configuration: task_set.network_configuration.clone(),
        started_by: task_set.id.clone(),
    }
}

/// `ManagedScaling$status` when Amazon ECS scales the Auto Scaling group itself, from the
/// `ManagedScalingStatus` enum (`ENABLED` / `DISABLED`). The member is optional and the
/// doc states the default is off ("the default is off"), so an absent or `DISABLED`
/// status is managed scaling OFF.
pub const MANAGED_SCALING_ENABLED: &str = "ENABLED";

/// Check the `ClusterSettings` of a request against [`CLUSTER_SETTING_VALUES`],
/// answering the message an `InvalidParameterException` carries when one is outside it.
///
/// Shared by every operation that accepts cluster settings, so the same value is
/// accepted or refused whichever of them a client calls.
pub fn validate_cluster_settings(settings: &[ClusterSetting]) -> Result<(), String> {
    for setting in settings {
        let value = setting.value.as_deref().unwrap_or_default();
        if !CLUSTER_SETTING_VALUES.contains(&value) {
            return Err(INVALID_CLUSTER_SETTING_VALUE_MESSAGE.to_string());
        }
    }
    Ok(())
}

/// Check a `ClusterConfiguration` a request carries, answering the message an
/// `InvalidParameterException` carries when it is not usable.
///
/// `ExecuteCommandConfiguration$logConfiguration`: "When `logging=OVERRIDE` is
/// specified, a `logConfiguration` must be provided." Shared by every operation that
/// accepts a cluster configuration.
pub fn validate_cluster_configuration(configuration: &ClusterConfiguration) -> Result<(), String> {
    if let Some(execute_command) = &configuration.execute_command_configuration {
        if execute_command.logging.as_deref() == Some(EXECUTE_COMMAND_LOGGING_OVERRIDE)
            && execute_command.log_configuration.is_none()
        {
            return Err(format!(
                "configuration.executeCommandConfiguration.logConfiguration is required \
                 when logging is {EXECUTE_COMMAND_LOGGING_OVERRIDE}"
            ));
        }
    }
    Ok(())
}

/// The stored default Service Connect namespace a request's
/// `ClusterServiceConnectDefaultsRequest` asks for.
///
/// The member's doc: "If you update the cluster with an empty string `\"\"` for the
/// namespace name, the cluster configuration for Service Connect is removed" — so an
/// empty namespace CLEARS the default rather than storing a nameless one.
pub fn service_connect_defaults(namespace: &str) -> Option<ClusterServiceConnectDefaults> {
    if namespace.is_empty() {
        None
    } else {
        Some(ClusterServiceConnectDefaults {
            namespace: Some(namespace.to_string()),
        })
    }
}

/// The longest a Cloud Map namespace reference may be, from the pattern the refusal of a
/// malformed one states.
pub const MAX_NAMESPACE_LENGTH: usize = 1024;

/// The pattern a `namespace` reference must match, as the refusal of one that does not
/// quotes it: between one and [`MAX_NAMESPACE_LENGTH`] printable ASCII characters, none of
/// them one of the four the class excludes.
pub const NAMESPACE_PATTERN: &str = r#"^[!-~&&[^<>/"]]{1,1024}$"#;

/// The Cloud Map service namespace an ARN of a namespace is named under
/// (`arn:aws:servicediscovery:<region>:<account>:namespace/<id>`), as
/// [`is_namespace_arn`] reads it back.
pub const NAMESPACE_ARN_SERVICE: &str = "servicediscovery";

/// The resource type of a Cloud Map namespace ARN.
pub const NAMESPACE_ARN_RESOURCE_TYPE: &str = "namespace";

/// Whether `reference` is a full Cloud Map namespace ARN —
/// `arn:<partition>:servicediscovery:<region>:<account>:namespace/<id>`.
///
/// A SYNTAX question, not a lookup: whether the account holds that namespace is the
/// namespace store's answer ([`cloud_map_holds_namespace`]), so an ARN of another Region
/// names a namespace nothing here holds rather than a malformed value.
pub fn is_namespace_arn(reference: &str) -> bool {
    // The ONE parse of a caller-supplied ARN (`service_common::arn`): the vendor says the
    // namespace is Cloud Map's, and the resource names one namespace of it.
    let Some(parsed) = ::service_common::arn::parse(reference) else {
        return false;
    };
    if parsed.vendor != NAMESPACE_ARN_SERVICE || parsed.account.is_empty() {
        return false;
    }
    ::service_common::arn::resource_id_of_type(reference, NAMESPACE_ARN_RESOURCE_TYPE)
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

/// Whether `namespace` is a value a `namespace` member takes: "The namespace name or full
/// Amazon Resource Name (ARN) of the Cloud Map namespace", so either of the two FORMS the
/// member documents.
///
/// A NAME is one [`NAMESPACE_PATTERN`] accepts: printable ASCII (`!` through `~`, so no
/// space and no control character) less `<`, `>`, `/` and `"`, and between one and
/// [`MAX_NAMESPACE_LENGTH`] of them. An ARN is the other form the member names
/// ([`is_namespace_arn`]) — it carries the `/` and the `:` no name may, so it is read as the
/// ARN it is rather than refused as a name that is not one.
pub fn is_namespace_reference(namespace: &str) -> bool {
    is_namespace_arn(namespace) || is_namespace_name(namespace)
}

/// Whether `namespace` is a NAME [`NAMESPACE_PATTERN`] accepts — the one half of
/// [`is_namespace_reference`] the pattern governs.
pub fn is_namespace_name(namespace: &str) -> bool {
    let length = namespace.chars().count();
    (1..=MAX_NAMESPACE_LENGTH).contains(&length)
        && namespace
            .chars()
            .all(|letter| matches!(letter, '!'..='~') && !matches!(letter, '<' | '>' | '/' | '"'))
}

/// `InvalidParameterException$message` for a `namespace` outside
/// [`NAMESPACE_PATTERN`]. DERIVED from the request: Amazon ECS names the member, the value
/// it was given and the pattern that value has to match.
pub fn namespace_pattern_message(namespace: &str) -> String {
    format!("Namespace ({namespace}) must match {NAMESPACE_PATTERN}.")
}

/// `NamespaceNotFoundException$message` for a namespace this account has nothing in.
///
/// DERIVED from the request: Amazon ECS reports the lookup it could not make and names the
/// `namespace` reference the caller passed, so a client is told WHICH namespace could not be
/// resolved rather than that some namespace could not be. The fixed text is
/// [`NAMESPACE_NOT_FOUND_PREFIX`]; the variable part is the value the request carried,
/// spelled exactly as it was sent (a name or a full ARN, the two forms every `namespace`
/// member documents).
pub fn namespace_not_found_message(namespace: &str) -> String {
    format!("{NAMESPACE_NOT_FOUND_PREFIX}{namespace}")
}

/// The fixed half of [`namespace_not_found_message`].
pub const NAMESPACE_NOT_FOUND_PREFIX: &str = "Failed to retrieve namespace for ";

/// The KEY two Cloud Map namespace references are the same namespace by: the name itself
/// when the reference is a name, and the last segment of a namespace ARN when it is one —
/// every `namespace` member's doc, "The namespace name or full Amazon Resource Name (ARN)
/// of the Cloud Map namespace". Naming one namespace either way is naming the same one, so
/// a listing cannot miss a service that spelled it the other way.
pub fn namespace_key(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

/// The DNS suffix the Region-scoped hosts of this service live under.
pub(super) const ENDPOINT_DNS_SUFFIX: &str = "amazonaws.com";

/// One Region-scoped host of this service, as `<host>.<region>.<suffix>` served over
/// HTTPS: the form every endpoint this service hands an agent takes.
pub(super) fn regional_endpoint_url(host: &str, region: &str) -> String {
    format!("https://{host}.{region}.{ENDPOINT_DNS_SUFFIX}/")
}

/// The host of the agent-communication service, whose endpoint the agent polls for the
/// work assigned to its container instance — this service's endpoint prefix, `-a-`, and
/// the shard the account's instances are served from.
pub(super) const AGENT_POLL_HOST: &str = "ecs-a-1";

/// The host of the agent-telemetry service, whose endpoint the agent reports its
/// containers' metrics to — the same prefix with `-t-` for telemetry.
pub(super) const AGENT_TELEMETRY_HOST: &str = "ecs-t-1";

/// The host of the Service Connect configuration service, whose endpoint the agent polls
/// for "Service Connect configuration" (`DiscoverPollEndpoint$serviceConnectEndpoint`).
pub(super) const AGENT_SERVICE_CONNECT_HOST: &str = "ecs-sc";

/// "The endpoint for the Amazon ECS agent to poll", in the Region the request was signed
/// for.
pub fn agent_poll_endpoint(region: &str) -> String {
    regional_endpoint_url(AGENT_POLL_HOST, region)
}

/// "The telemetry endpoint for the Amazon ECS agent", in the Region the request was signed
/// for.
pub fn agent_telemetry_endpoint(region: &str) -> String {
    regional_endpoint_url(AGENT_TELEMETRY_HOST, region)
}

/// "The endpoint for the Amazon ECS agent to poll for Service Connect configuration", in
/// the Region the request was signed for.
pub fn agent_service_connect_endpoint(region: &str) -> String {
    regional_endpoint_url(AGENT_SERVICE_CONNECT_HOST, region)
}

/// The cluster NAME a `cluster` request member refers to: the short name as given, or
/// the last segment of a cluster ARN. Every `cluster` member's doc: "The short name or
/// full Amazon Resource Name (ARN) of the cluster".
pub fn cluster_name_from_reference(reference: Option<&str>) -> String {
    let reference = reference
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CLUSTER_NAME);
    match reference.rsplit_once(":cluster/") {
        Some((_, name)) => name.to_string(),
        None => reference.to_string(),
    }
}

/// The task ARN a `task`/`tasks` entry refers to: the value as given when it is already
/// an ARN, otherwise the ARN a task of that id in `cluster_name` would carry. Every
/// such member's doc: "the task ID or full ARN entry of the task".
pub fn task_arn_from_reference(region: &str, cluster_name: &str, reference: &str) -> String {
    if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        task_arn(region, cluster_name, reference)
    }
}

/// The ARN of a cluster: `arn:aws:ecs:<region>:<account>:cluster/<name>`.
pub fn cluster_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster/{name}"))
}

/// The ARN of the identity every caller of this emulator authenticates as, and so the
/// principal a "who did this" member names (`TaskDefinition$registeredBy`: "The
/// principal that registered the task definition").
///
/// Every request this emulator serves is signed with the emulator's own credentials, so
/// the caller is the one IAM user it publishes — the very principal its
/// `sts:GetCallerIdentity` answers, built the same way, so the principal a response
/// names is one the caller can look up. A name spelled out of the request's own access
/// key id would not be one: a member AWS fills from the AUTHENTICATED IAM identity has to
/// name an identity that identity service knows, and the same caller holds the same
/// identity on every call it makes.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The ARN of the account's ROOT user — the principal whose account settings are the
/// account-wide defaults (`PutAccountSettingDefault`: "Modifies an account setting for
/// all users on an account for whom no individual account setting has been specified").
pub fn root_principal_arn() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// An account setting value together with the principal it came from.
#[derive(Debug, Clone)]
pub struct ResolvedSetting {
    pub value: String,
    pub principal_arn: String,
}

/// The `Setting` a response reports for one name, value and principal.
///
/// `Setting$type` comes from the catalog rather than the request: it "indicates whether
/// Amazon Web Services manages the account setting, or if the user manages it", which is
/// a property of the NAME ([`ACCOUNT_SETTINGS`]).
pub fn setting_view(name: &str, value: &str, principal_arn: &str) -> Setting {
    Setting {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
        principal_arn: Some(principal_arn.to_string()),
        r#type: Some(
            account_setting_spec(name)
                .map_or(SETTING_TYPE_USER, |setting| setting.setting_type)
                .to_string(),
        ),
    }
}

/// Check the `value` a write of the account setting `name` carries against the values
/// that name accepts ([`ACCOUNT_SETTINGS`]), answering the message an
/// `InvalidParameterException` carries when it is outside them.
///
/// Shared by `PutAccountSetting` and `PutAccountSettingDefault`, so the same value is
/// accepted or refused whichever of them a client calls. The sentence names the MEMBER at
/// fault and nothing else: neither the value the request carried nor the set of values the
/// setting accepts, both of which a caller reads out of the API reference.
pub fn validate_account_setting_value(name: &str, value: &str) -> Result<(), String> {
    let Some(spec) = account_setting_spec(name) else {
        // Unreachable from the wire: `name` targets the closed `SettingName` enum, which
        // the request walk has already checked.
        return Err(invalid_setting_message("name"));
    };
    if !spec.accepted_values.contains(&value) {
        return Err(invalid_setting_message("value"));
    }
    Ok(())
}

/// `InvalidParameterException$message` for an account-setting member whose value is not
/// one the named setting takes: `Invalid setting '<member>'`.
pub fn invalid_setting_message(member: &str) -> String {
    format!("Invalid setting '{member}'")
}

/// `InvalidParameterException$message` for an account-setting request that leaves out a
/// member ANOTHER member it carries requires — a `value` to filter by without the `name`
/// whose values it filters ("You must also specify an account setting name to use this
/// parameter").
///
/// The two members are named the way every other account-setting refusal names one
/// ([`invalid_setting_message`]): quoted, after the noun this API states its setting
/// refusals under.
pub fn setting_member_required_message(required: &str, present: &str) -> String {
    format!("Setting '{required}' must be provided when '{present}' is present")
}

/// The segment a validation path marks a step from a COLLECTION into one of its elements
/// with: it stands after the element's position (a list) or the entry's key (a map), so a
/// refused element reads `requiresCompatibilities.1.member`. It names no member of this
/// API, and neither does the position or the key beside it, so a sentence that names a
/// member reads through that pair to the collection member holding it.
pub(super) const COLLECTION_PATH_STEP: &str = "member";

/// The words Amazon ECS names a member by in a refusal it builds from the request: the
/// member's own name split at its camelCase boundaries, lower-cased
/// (`assignPublicIp` -> `assign public ip`). A dotted validation path is named by its LAST
/// segment that names a member — a nested member by its own name, an element or an entry
/// of a collection by the collection member that holds it, never by the position or the
/// key the caller put it at.
pub fn member_words(member_path: &str) -> String {
    let mut segments: Vec<&str> = member_path.split('.').collect();
    // A collection step is always preceded by the position or the key it stepped through,
    // so the pair is dropped together, one pair per level of nesting. The leading segment
    // is a member name whatever it spells, so it always stays.
    while segments.len() > 2 && segments.last() == Some(&COLLECTION_PATH_STEP) {
        segments.truncate(segments.len() - 2);
    }
    let member = segments.last().copied().unwrap_or(member_path);
    let mut words = String::with_capacity(member.len() + 4);
    for letter in member.chars() {
        if letter.is_uppercase() && !words.is_empty() {
            words.push(' ');
        }
        words.extend(letter.to_lowercase());
    }
    words
}

/// `InvalidParameterException$message` for a member whose value is not one of the values
/// Amazon ECS accepts there: the member as the sentence names it, and the accepted values
/// between brackets, comma-separated with no spaces.
///
/// The ONE place this service words that refusal, whichever member earns it — the member's
/// own name for the members the recorded sentences spell that way (`instanceId length`,
/// `attachmentStatus`), and the camelCase-split words [`member_words`] renders for the
/// modelled enum members ([`enum_value_message`]).
pub fn value_set_message(member: &str, accepted: &[&str]) -> String {
    format!("{member} should be one of [{}]", accepted.join(","))
}

/// `InvalidParameterException$message` for a member whose value is not one its modelled
/// enum names: the member in the words above, and the values it does accept in ascending
/// order between brackets.
///
/// The ONE place this service words that refusal, so every enum member states it alike —
/// `assignPublicIp` is where the corpus records it.
pub fn enum_value_message(member_path: &str, accepted: &[String]) -> String {
    let mut ascending: Vec<&str> = accepted.iter().map(String::as_str).collect();
    ascending.sort_unstable();
    value_set_message(&member_words(member_path), &ascending)
}

/// The member a task listing names the desired status it filters on, as the words a
/// refusal of it carries are built from ([`member_words`]).
pub const DESIRED_STATUS_MEMBER: &str = "desiredStatus";

/// The members of a task listing that identify the tasks to report BY THEMSELVES, so each
/// of them is the only filter its listing may carry.
///
/// `ListTasksRequest$startedBy` states the rule outright — "When you specify startedBy as
/// the filter, it must be the only filter that you use" — and `$serviceName`, which selects
/// "tasks that belong to that service", is the other filter Amazon ECS holds to it: a
/// service's tasks are the service's, so there is nothing for a second filter to narrow.
pub const EXCLUSIVE_TASK_FILTERS: [&str; 2] = ["serviceName", "startedBy"];

/// The refusal a task listing earns when it carries one of [`EXCLUSIVE_TASK_FILTERS`]
/// BESIDE another filter, or `None` when it carries at most one filter.
///
/// `filters` is every filter member of the request paired with whether the request carried
/// it. `cluster` is not among them — it scopes the listing to a cluster rather than
/// selecting tasks within one — and neither are `maxResults`/`nextToken`, which page what
/// the filters selected, nor `desiredStatus`, which every listing filters on whether or not
/// the request named it ("The default status filter is RUNNING").
///
/// The sentence names the exclusive member at fault, in the member's own spelling, and
/// nothing else of the request.
pub fn exclusive_task_filter_violation(filters: &[(&str, bool)]) -> Option<String> {
    let present = || filters.iter().filter(|(_, carried)| *carried);
    if present().count() < 2 {
        return None;
    }
    let exclusive = EXCLUSIVE_TASK_FILTERS
        .iter()
        .find(|member| present().any(|(carried, _)| carried == *member))?;
    Some(format!("cannot specify {exclusive} with other arguments"))
}

/// The desired status a task LISTING filters on, resolved from the value the request
/// carried — or the refusal a value that names no desired status at all earns.
///
/// `ListTasksRequest$desiredStatus` is "The task desired status to use when filtering the
/// ListTasks results", and "The default status filter is RUNNING", so a request that names
/// none filters on RUNNING. A request that names one is resolved against the member's value
/// set WITHOUT regard to ASCII case — Amazon ECS serves a lower-case spelling of a member
/// and answers the tasks of that desired status — and the status a stored task carries is
/// the set's own spelling, so the resolved filter is that member rather than the string the
/// request spelled. A value the set holds in no case names no desired status, which is what
/// the member's own refusal reports ([`enum_value_message`]).
pub fn resolved_desired_status_filter(requested: Option<&str>) -> Result<String, String> {
    let Some(requested) = requested else {
        return Ok(STATUS_RUNNING.to_string());
    };
    crate::ops::types::DESIRED_STATUS_VALUES
        .iter()
        .find(|accepted| accepted.eq_ignore_ascii_case(requested))
        .map(|accepted| (*accepted).to_string())
        .ok_or_else(|| {
            let accepted: Vec<String> = crate::ops::types::DESIRED_STATUS_VALUES
                .iter()
                .map(|value| (*value).to_string())
                .collect();
            enum_value_message(DESIRED_STATUS_MEMBER, &accepted)
        })
}

/// The member of a Managed Instances launch template that names which of the two
/// CloudWatch monitoring categories its instances run under.
pub const MONITORING_MEMBER: &str = "monitoring";

/// The option a Managed Instances launch template's `monitoring` names, in the spelling
/// the modelled value set lists — or the sentence a value that names none of them is
/// refused with ([`enum_value_message`]).
///
/// The value is matched WITHOUT regard to ASCII case: the member's own documentation
/// publishes the two categories in lower case ("basic monitoring and detailed
/// monitoring"), and Amazon ECS serves a request that spells the option that way. The
/// stored template reports this member back through a shape that targets the modelled
/// enum, so the option is stored in the spelling that set lists and a read reports a value
/// inside it whichever spelling created it. A value that matches no option in any case is
/// outside the set, and is refused as any other out-of-set enumerated value of this
/// service is.
pub fn managed_instances_monitoring_option(value: &str) -> Result<String, String> {
    let accepted = crate::ops::types::MANAGED_INSTANCES_MONITORING_OPTIONS_VALUES;
    accepted
        .iter()
        .find(|option| option.eq_ignore_ascii_case(value))
        .map(|option| (*option).to_string())
        .ok_or_else(|| {
            let accepted: Vec<String> = accepted.iter().map(|option| option.to_string()).collect();
            enum_value_message(MONITORING_MEMBER, &accepted)
        })
}

/// `InvalidParameterException$message` for a `containerInstance` whose id is not one
/// Amazon ECS mints: the ids it names are the hexadecimal and UUID forms
/// [`CONTAINER_INSTANCE_ID_PATTERN`] describes, and the refusal states that pattern
/// rather than the value it was given.
pub const CONTAINER_INSTANCE_ID_PATTERN: &str = "^[a-f0-9-]{1,36}$";

/// The sentence a `containerInstance` whose id does not match
/// [`CONTAINER_INSTANCE_ID_PATTERN`] is refused with.
pub fn container_instance_id_pattern_message() -> String {
    pattern_message("ContainerInstanceId", CONTAINER_INSTANCE_ID_PATTERN)
}

/// Whether `instance_id` is a container-instance id of the form
/// [`CONTAINER_INSTANCE_ID_PATTERN`]: 1 to 36 characters, each a lowercase hexadecimal
/// digit or a hyphen — the alphabet of both forms this service mints an id in.
pub fn is_container_instance_id(instance_id: &str) -> bool {
    !instance_id.is_empty()
        && instance_id.chars().count() <= 36
        && instance_id
            .chars()
            .all(|letter| matches!(letter, '0'..='9' | 'a'..='f' | '-'))
}

/// The two lengths a container-instance id has. An id Amazon ECS mints is a UUID, written
/// either as the 32 hexadecimal digits alone (the form this store mints, see
/// `next_container_instance_id`) or as the 36 characters the hyphenated form spells, so a
/// value of any other length names no container instance of any account.
pub const CONTAINER_INSTANCE_ID_LENGTHS: [usize; 2] = [32, 36];

/// `InvalidParameterException$message` for a container-instance identifier whose id part
/// is of neither length [`CONTAINER_INSTANCE_ID_LENGTHS`] holds: the whole sentence is
/// fixed text, naming the two forms of the id rather than the value the request carried.
pub fn container_instance_id_length_message() -> String {
    let lengths: Vec<String> = CONTAINER_INSTANCE_ID_LENGTHS
        .iter()
        .map(usize::to_string)
        .collect();
    format!("instanceId length should be one of [{}]", lengths.join(","))
}

/// Whether `instance_id` is of a length a container-instance id has
/// ([`CONTAINER_INSTANCE_ID_LENGTHS`]).
pub fn is_container_instance_id_length(instance_id: &str) -> bool {
    CONTAINER_INSTANCE_ID_LENGTHS.contains(&instance_id.chars().count())
}

/// The refusal a list of container-instance identifiers earns when one of its entries
/// carries an id of neither length a container-instance id has, or `None` when every
/// entry does.
///
/// Each entry is reduced to the id it names first ([`container_instance_id_from_reference`]),
/// since "The container instance ID or full ARN" are both forms a member accepts and the
/// length belongs to the id either way.
pub fn container_instance_id_length_violation<'a>(
    references: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    references
        .into_iter()
        .any(|reference| {
            !is_container_instance_id_length(container_instance_id_from_reference(reference))
        })
        .then(container_instance_id_length_message)
}

/// Whether `principal_arn` is a principal an account setting may be written for, and a
/// key this store may hold.
///
/// `PutAccountSetting$principalArn`: "The ARN of the principal, which can be a user,
/// role, or the root user" — an IAM ARN, which is the six colon-separated fields
/// `arn:<partition>:iam::<account>:<resource>` ([`::engine::emulator_arn`] builds every
/// one this emulator emits the same way, with the empty region a global service carries).
/// A value that is not one names no principal a setting could belong to, and is refused
/// before it can become a key — as is one longer than [`MAX_PRINCIPAL_ARN_LENGTH`].
///
/// This is the FORM of the value alone. WHOSE settings the caller may then reach — its own,
/// or any principal of the account when it is the root user — is
/// [`account_setting_principal_refusal`].
pub fn is_principal_arn(principal_arn: &str) -> bool {
    if principal_arn.len() > MAX_PRINCIPAL_ARN_LENGTH {
        return false;
    }
    // The resource is the LAST field, so it keeps any colon of its own.
    let fields: Vec<&str> = principal_arn.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && fields[2] == "iam"
        && fields[3].is_empty()
        && !fields[4].is_empty()
        && !fields[5].is_empty()
}

/// The message an `InvalidParameterException` carries when a request's `principalArn`
/// is not a principal an account setting may belong to ([`is_principal_arn`]): the
/// member that is wrong and the value it was given.
///
/// Shared by every account-setting operation that takes a `principalArn`, so the same
/// value is refused with the same sentence whichever of them a client calls.
pub fn invalid_principal_arn_message(principal_arn: &str) -> String {
    format!("Invalid PrincipalArn: {principal_arn}")
}

/// `InvalidParameterException$message` for an account-setting request that names a
/// principal OTHER than the one it is signed for, from a caller that is not the account's
/// root user.
///
/// One account setting belongs to one principal, and whose settings a caller may reach is
/// the account's own boundary: the root user's setting is the account-wide default
/// ("Modifies an account setting for all users on an account for whom no individual account
/// setting has been specified"), so only the root user may read or write what another
/// principal holds, and every other caller reaches its own. Recorded behavior: this is the
/// sentence the refusal carries.
pub const PRINCIPAL_ARN_ROOT_ONLY_MESSAGE: &str =
    "Only the root user can view or modify the account settings for another user. You do \
     not need to specify a principalArn value to change your own account settings.";

/// `Some(message)` when the request's `principalArn` names a principal this caller may not
/// view or modify the account settings of ([`PRINCIPAL_ARN_ROOT_ONLY_MESSAGE`]), `None`
/// when it may.
///
/// A caller reaches its OWN principal (the identity every request of this emulator is
/// authenticated as, [`caller_identity_arn`]) — and the ROOT user reaches every principal
/// of the account, which is what makes its own setting the account-wide default. Shared by
/// every account-setting operation that takes a `principalArn`, so the same request is
/// refused by all of them alike, whether it reads or writes.
pub fn account_setting_principal_refusal(principal_arn: &str) -> Option<String> {
    let caller = caller_identity_arn();
    let reachable = principal_arn == caller || caller == root_principal_arn();
    (!reachable).then(|| PRINCIPAL_ARN_ROOT_ONLY_MESSAGE.to_string())
}

/// The ARN resource type a task-definition revision is named under, as the resource
/// segment of [`task_definition_arn`] and the one
/// [`parse_task_definition_reference`] reads a reference's family out of.
pub const TASK_DEFINITION_RESOURCE_TYPE: &str = "task-definition";

/// The ARN resource type a DAEMON task-definition revision is named under — its own
/// resource, so a daemon revision's ARN is never a plain revision's and neither
/// reference form resolves to the other's store.
pub const DAEMON_TASK_DEFINITION_RESOURCE_TYPE: &str = "daemon-task-definition";

/// The ARN of a task-definition revision:
/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:<revision>`.
pub fn task_definition_arn(region: &str, family: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_DEFINITION_RESOURCE_TYPE}/{family}:{revision}"),
    )
}

/// The ARN of a daemon task-definition revision:
/// `arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:<revision>`.
pub fn daemon_task_definition_arn(region: &str, family: &str, revision: i32) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DAEMON_TASK_DEFINITION_RESOURCE_TYPE}/{family}:{revision}"),
    )
}

/// The ARN resource type a DAEMON is named under, as the resource segment of
/// [`daemon_arn`] — its own resource, beside the cluster's services and tasks.
pub const DAEMON_RESOURCE_TYPE: &str = "daemon";

/// The ARN of a daemon: `arn:aws:ecs:<region>:<account>:daemon/<cluster>/<name>`. A
/// daemon is created IN a cluster, so its ARN carries that cluster the way a service's
/// does ([`service_arn`]).
pub fn daemon_arn(region: &str, cluster_name: &str, daemon_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{DAEMON_RESOURCE_TYPE}/{cluster_name}/{daemon_name}"),
    )
}

/// The ARN resource type a DAEMON DEPLOYMENT is named under, as the resource segment of
/// [`daemon_deployment_arn`].
pub const DAEMON_DEPLOYMENT_RESOURCE_TYPE: &str = "daemon-deployment";

/// The ARN of a daemon deployment:
/// `arn:aws:ecs:<region>:<account>:daemon-deployment/<cluster>/<daemon>/<id>`.
pub fn daemon_deployment_arn(
    region: &str,
    cluster_name: &str,
    daemon_name: &str,
    deployment_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "{DAEMON_DEPLOYMENT_RESOURCE_TYPE}/{cluster_name}/{daemon_name}/{deployment_id}"
        ),
    )
}

/// The ARN resource type a container instance is named under, as the resource segment of
/// [`container_instance_arn`].
pub const CONTAINER_INSTANCE_RESOURCE_TYPE: &str = "container-instance";

/// The ARN of a container instance:
/// `arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`.
pub fn container_instance_arn(region: &str, cluster_name: &str, instance_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CONTAINER_INSTANCE_RESOURCE_TYPE}/{cluster_name}/{instance_id}"),
    )
}

/// The container-instance ARN a `containerInstance`/`containerInstances` entry refers
/// to: the value as given when it is already an ARN, otherwise the ARN an instance of
/// that id in `cluster_name` would carry. Every such member's doc: "The container
/// instance ID or full ARN of the container instance".
pub fn container_instance_arn_from_reference(
    region: &str,
    cluster_name: &str,
    reference: &str,
) -> String {
    if reference.starts_with("arn:") {
        reference.to_string()
    } else {
        container_instance_arn(region, cluster_name, reference)
    }
}

/// The container-instance ARN an instance that is registered in NO cluster is named by:
/// the id alone under the resource type
/// (`arn:aws:ecs:<region>:<account>:container-instance/<id>`), with no cluster segment in
/// the resource part.
///
/// The cluster segment of [`container_instance_arn`] names the cluster an instance is
/// REGISTERED IN, which only a stored instance has, so an answer about an instance this
/// service holds none of names it by its id alone.
pub fn unqualified_container_instance_arn(region: &str, instance_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CONTAINER_INSTANCE_RESOURCE_TYPE}/{instance_id}"),
    )
}

/// The `MISSING` `Failure` a container-instance STATE CHANGE reports for an entry that
/// resolves to no instance it can move: the id the entry named
/// ([`container_instance_id_from_reference`], whichever of the member's two forms the
/// entry took) under [`unqualified_container_instance_arn`].
///
/// A state change answers about an instance it did not move, so the entry names that
/// instance alone and never the cluster the request looked for it in — which is also what
/// lets a caller draining instances in batches correlate the entry back to the id it
/// submitted however it spelled it. Distinct from [`unresolved_failure`], which is what a
/// batch READ echoes: there the identifier is reported exactly as the caller sent it.
pub fn unresolved_container_instance_state_failure(region: &str, reference: &str) -> Failure {
    missing_failure(&unqualified_container_instance_arn(
        region,
        container_instance_id_from_reference(reference),
    ))
}

/// The deployment configuration a service stores: what the request supplied, with the
/// documented defaults materialised for the members a response always carries.
///
/// `maximumPercent`: "The default maximumPercent value for a service using the REPLICA
/// service scheduler is 200%" — a daemon runs exactly one task per container instance,
/// so its deployment cannot exceed the desired count. `minimumHealthyPercent`: "The
/// default value for a replica service for minimumHealthyPercent is 100%. The default
/// minimumHealthyPercent value for a service using the DAEMON service schedule is 0% for
/// the CLI, the Amazon Web Services SDKs, and the APIs".
///
/// The two members that belong to the ROLLING update the ECS controller performs —
/// `strategy` and `deploymentCircuitBreaker` ("The deployment circuit breaker can only be
/// used for services using the rolling update (ECS) deployment type") — are defaulted
/// only for a service whose scheduler owns that rollout; a service driven by an external
/// deployment controller carries neither. `bakeTimeInMinutes` belongs to the
/// configuration itself and is defaulted for every service.
pub fn deployment_configuration(
    requested: Option<DeploymentConfiguration>,
    daemon: bool,
    scheduler_owned: bool,
) -> DeploymentConfiguration {
    let mut configuration = requested.unwrap_or_default();
    configuration.maximum_percent = Some(configuration.maximum_percent.unwrap_or(if daemon {
        DAEMON_MAXIMUM_PERCENT
    } else {
        REPLICA_MAXIMUM_PERCENT
    }));
    configuration.minimum_healthy_percent =
        Some(
            configuration
                .minimum_healthy_percent
                .unwrap_or(if daemon {
                    DAEMON_MINIMUM_HEALTHY_PERCENT
                } else {
                    REPLICA_MINIMUM_HEALTHY_PERCENT
                }),
        );
    configuration.bake_time_in_minutes = Some(
        configuration
            .bake_time_in_minutes
            .unwrap_or(DEFAULT_BAKE_TIME_MINUTES),
    );
    if scheduler_owned {
        configuration.strategy = Some(
            configuration
                .strategy
                .unwrap_or_else(|| DEPLOYMENT_STRATEGY_ROLLING.to_string()),
        );
        configuration.deployment_circuit_breaker = Some(
            configuration
                .deployment_circuit_breaker
                .unwrap_or_else(default_deployment_circuit_breaker),
        );
    }
    configuration
}

/// The deployment configuration an UPDATE leaves the service with: the members its request
/// supplied, over the ones the service already holds.
///
/// Every member of `DeploymentConfiguration` is optional, so an update carries just the
/// knobs it changes — and a member it does not carry is not a request to clear that knob,
/// it is one the service keeps: `UpdateService`'s own statement for the members beside it,
/// "If you do not want to override the value that was set when the service was created, you
/// can set this to null when performing this action", holds for the members INSIDE the
/// structure too. The merge is per MEMBER and one level deep: a member the request does
/// supply replaces the one it stood for, structure or not, because that member is one
/// value.
pub fn merged_deployment_configuration(
    stored: &DeploymentConfiguration,
    requested: DeploymentConfiguration,
) -> DeploymentConfiguration {
    DeploymentConfiguration {
        deployment_circuit_breaker: requested
            .deployment_circuit_breaker
            .or_else(|| stored.deployment_circuit_breaker.clone()),
        maximum_percent: requested.maximum_percent.or(stored.maximum_percent),
        minimum_healthy_percent: requested
            .minimum_healthy_percent
            .or(stored.minimum_healthy_percent),
        alarms: requested.alarms.or_else(|| stored.alarms.clone()),
        strategy: requested.strategy.or_else(|| stored.strategy.clone()),
        bake_time_in_minutes: requested
            .bake_time_in_minutes
            .or(stored.bake_time_in_minutes),
        lifecycle_hooks: requested
            .lifecycle_hooks
            .or_else(|| stored.lifecycle_hooks.clone()),
        linear_configuration: requested
            .linear_configuration
            .or_else(|| stored.linear_configuration.clone()),
        canary_configuration: requested
            .canary_configuration
            .or_else(|| stored.canary_configuration.clone()),
        early_success_criteria: requested
            .early_success_criteria
            .or_else(|| stored.early_success_criteria.clone()),
    }
}

/// The deployment circuit breaker a service that configured none carries: off, with the
/// documented failure threshold.
///
/// `DeploymentCircuitBreaker$enable` is what "determines whether to use the deployment
/// circuit breaker logic for the service", so a service that configured none has it
/// turned off and rolls nothing back. `resetOnHealthyTask`: "The default is true".
/// `ThresholdConfiguration`: "By default, the threshold configuration uses a type of
/// BOUNDED_PERCENT with a value of 50."
pub(super) fn default_deployment_circuit_breaker() -> Defaulted<DeploymentCircuitBreaker> {
    Defaulted(DeploymentCircuitBreaker {
        enable: false,
        rollback: false,
        reset_on_healthy_task: Some(CIRCUIT_BREAKER_RESETS_ON_HEALTHY_TASK),
        threshold_configuration: Some(Defaulted(ThresholdConfiguration {
            r#type: THRESHOLD_BOUNDED_PERCENT.to_string(),
            value: THRESHOLD_DEFAULT_VALUE,
        })),
    })
}

/// The status a failure-detection mechanism of a rollout reports.
///
/// `ServiceDeploymentRollbackMonitorsStatus`: a mechanism the deployment configuration did
/// not turn on is [`ROLLBACK_MONITORS_DISABLED`] ("Amazon ECS is not using alarms for
/// service deployment failures when the status is `DISABLED`"); one that is on watches the
/// rollout while it runs and has nothing left to watch once it has ended.
pub(super) fn rollback_monitors_status(enabled: bool, finished: bool) -> &'static str {
    match (enabled, finished) {
        (false, _) => ROLLBACK_MONITORS_DISABLED,
        (true, false) => ROLLBACK_MONITORS_MONITORING,
        (true, true) => ROLLBACK_MONITORS_MONITORING_COMPLETE,
    }
}

/// The `DeploymentLifecycleHookDetail` one reached PAUSE hook is reported as.
///
/// `targetArn` is left out for every hook here: "For `AWS_LAMBDA` hooks, this is the Lambda
/// function ARN. For `PAUSE` hooks, this field is not set."
pub(super) fn lifecycle_hook_detail(hook: &LifecycleHookRecord) -> DeploymentLifecycleHookDetail {
    DeploymentLifecycleHookDetail {
        hook_id: Some(hook.hook_id.clone()),
        target_type: Some(LIFECYCLE_HOOK_TARGET_PAUSE.to_string()),
        target_arn: None,
        status: Some(hook.status.clone()),
        expires_at: Some(DateTime::from(hook.expires_at)),
        timeout_action: Some(hook.timeout_action.clone()),
    }
}

/// `ServiceDeploymentRecord::status_reason` of a rollout whose rollback could not be
/// carried out: there is no earlier revision of its service to put back.
pub(super) fn no_previous_revision_reason(service_arn: &str) -> String {
    format!(
        "This service deployment couldn't be rolled back because service {service_arn} has \
         no previous service revision."
    )
}

/// The `ServiceDeploymentNotFoundException` message for an ARN that names no stored
/// rollout, from that error's own doc: "The service deploy ARN that you specified in the
/// `ContinueServiceDeployment` doesn't exist. You can use `ListServiceDeployments` to
/// retrieve the service deployment ARNs."
pub fn service_deployment_not_found_message(arn: &str) -> String {
    format!(
        "The service deployment ARN {arn} that you specified doesn't exist. You can use \
         ListServiceDeployments to retrieve the service deployment ARNs."
    )
}

/// The `InvalidParameterException` message for a hook action asked for where no such hook
/// is waiting for one — the one state `ContinueServiceDeployment` acts in: "When a service
/// deployment reaches a lifecycle stage that has a PAUSE hook configured, the deployment
/// pauses and waits for an explicit action."
///
/// The fixed text follows the error shape's own doc — "The specified parameter isn't valid.
/// Review the available parameters for the API request" — naming the member that isn't
/// (`hookId`); the hook id and the service deployment are the ones the request named.
pub fn lifecycle_hook_not_awaiting_message(hook_id: &str, arn: &str) -> String {
    format!(
        "The specified hookId {hook_id} isn't valid for service deployment {arn}. Review \
         the available parameters for the API request."
    )
}

/// The literal first segment of every lifecycle hook id, as the format
/// [`LIFECYCLE_HOOK_ID_FORMAT`] spells it.
pub(super) const LIFECYCLE_HOOK_ID_PREFIX: &str = "ecs";

/// The form of a `DeploymentLifecycleHookDetail$hookId`: the literal `ecs`, the type of the
/// hook, and the id that makes one hook of that type distinct from another, joined by
/// hyphens. A refusal states this format rather than the value it was given.
pub const LIFECYCLE_HOOK_ID_FORMAT: &str = "ecs-{hookType}-{id}";

/// The id of ONE lifecycle hook of `hook_type`: the [`LIFECYCLE_HOOK_ID_FORMAT`] form, whose
/// hook-type segment is the hook's `DeploymentLifecycleHookTargetType` spelled the way an id
/// spells it.
///
/// Minting and checking ([`is_lifecycle_hook_id`]) read the same format, so an id this
/// service hands a client back is always one it accepts again.
pub fn lifecycle_hook_id(hook_type: &str, id: &str) -> String {
    format!(
        "{LIFECYCLE_HOOK_ID_PREFIX}-{}-{id}",
        hook_type.to_ascii_lowercase()
    )
}

/// Whether `hook_id` has the [`LIFECYCLE_HOOK_ID_FORMAT`] form: the `ecs` segment, then a
/// non-empty hook type, then a non-empty id. The id segment keeps any hyphen of its own, so
/// an id spelled as a UUID is one value and not three.
pub fn is_lifecycle_hook_id(hook_id: &str) -> bool {
    hook_id
        .strip_prefix(LIFECYCLE_HOOK_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(hook_type, id)| !hook_type.is_empty() && !id.is_empty())
}

/// The `InvalidParameterException` message for a `hookId` that is not of the form
/// [`LIFECYCLE_HOOK_ID_FORMAT`]: the whole sentence is fixed text, stating the format the
/// value has to take and echoing nothing the request carried.
pub fn invalid_lifecycle_hook_id_message() -> String {
    format!("Invalid hookId: hookId must follow the format: {LIFECYCLE_HOOK_ID_FORMAT}")
}

/// The `InvalidParameterException` message for a ROLLBACK asked for on a rollout that has
/// no earlier service revision to roll back to.
pub fn no_previous_service_revision_message(arn: &str) -> String {
    format!(
        "The service deployment {arn} can't be rolled back because its service has no \
         previous service revision."
    )
}

/// The `ConflictException` message for a stop asked for on a rollout that is not ongoing:
/// `StopServiceDeployment` "stops an ongoing service deployment", and a rollout that
/// reached an end nobody asked for is no longer one — the end the message names.
pub fn service_deployment_not_ongoing_message(arn: &str) -> String {
    format!("ServiceDeployment is stopping or has already finished for arn: {arn}")
}

/// `Rollback$reason` ("The reason the rollback happened") of a rollback the action taken on
/// a paused lifecycle hook asked for.
pub(super) fn hook_rollback_reason() -> String {
    format!(
        "The {LIFECYCLE_HOOK_ACTION_ROLLBACK} action was taken on the paused lifecycle hook \
         of this service deployment."
    )
}

/// `Rollback$reason` of a rollback a request to STOP the rollout asked for, with the stop
/// type that rolls it back.
pub(super) fn stop_rollback_reason() -> String {
    format!(
        "This service deployment was stopped with the {STOP_TYPE_ROLLBACK} stop type, which \
         rolls it back to the previous service revision."
    )
}

/// `ServiceDeploymentRecord::status_reason` of a rollout a caller has asked to stop, while
/// the stop is still being carried out.
pub(super) fn stop_requested_reason(deployment_id: &str, rollback: bool) -> String {
    if rollback {
        format!(
            "A request was made to stop ECS deployment {deployment_id} and roll the service \
             back to the previous service revision."
        )
    } else {
        format!("A request was made to stop ECS deployment {deployment_id}.")
    }
}

/// `ServiceDeploymentRecord::status_reason` of a rollout a caller stopped where it stood.
pub(super) fn stopped_by_request_reason(deployment_id: &str) -> String {
    format!("ECS deployment {deployment_id} was stopped by a request to stop it.")
}

/// `ServiceDeployment$alarms`: "The CloudWatch alarms that determine when a service
/// deployment fails", as the rollout's own deployment configuration set them.
pub(super) fn deployment_alarms_view(
    configuration: &DeploymentConfiguration,
    finished: bool,
) -> ServiceDeploymentAlarms {
    let alarms = configuration.alarms.as_ref();
    ServiceDeploymentAlarms {
        status: Some(
            rollback_monitors_status(alarms.is_some_and(|alarms| alarms.enable), finished)
                .to_string(),
        ),
        // "The name of the CloudWatch alarms that determine when a service deployment
        // failed": the names the configuration carries, and none when it carries none.
        alarm_names: Some(alarms.map(|alarms| alarms.alarm_names.clone()).unwrap_or_default()),
        // "One or more CloudWatch alarm names that have been triggered during the service
        // deployment": an alarm is moved into its `ALARM` state by the metric it watches,
        // which nothing in this slice publishes, so none has been triggered.
        triggered_alarm_names: Some(Vec::new()),
    }
}

/// `ServiceDeployment$deploymentCircuitBreaker`: "The circuit breaker configuration that
/// determines a service deployment failed", as the rollout's own deployment configuration
/// set it.
pub(super) fn deployment_circuit_breaker_view(
    configuration: &DeploymentConfiguration,
    desired_count: i32,
    failure_count: i32,
    triggered: bool,
    finished: bool,
) -> ServiceDeploymentCircuitBreaker {
    let breaker = configuration.deployment_circuit_breaker.as_ref();
    let enabled = breaker.is_some_and(|breaker| breaker.enable);
    ServiceDeploymentCircuitBreaker {
        // A breaker that FAILED the rollout it watched reports that it fired; one whose watch
        // merely ran its course reports the end of the watch.
        status: Some(
            if triggered {
                ROLLBACK_MONITORS_TRIGGERED
            } else {
                rollback_monitors_status(enabled, finished)
            }
            .to_string(),
        ),
        // "The number of times the circuit breaker detected a service deployment failure":
        // the failed launches of this rollout, counted off the tasks it placed — and a true
        // zero for a breaker that is watching a rollout that has lost none.
        failure_count,
        // "The threshhold which determines that the service deployment failed" — a
        // threshold only a breaker that is watching the rollout has at all.
        threshold: if enabled {
            circuit_breaker_threshold(breaker, desired_count)
        } else {
            0
        },
    }
}

/// The failure threshold a deployment circuit breaker tolerates before it triggers.
///
/// The `ThresholdConfiguration$type` doc states the whole rule: "For the percentage types
/// (`BOUNDED_PERCENT` and `UNBOUNDED_PERCENT`), Amazon ECS multiplies `value` by the
/// latest service desired count. For `COUNT`, Amazon ECS uses `value` directly as the
/// threshold", and `BOUNDED_PERCENT` additionally clamps the product "to a minimum of `3`
/// and a maximum of `200`". A breaker that configured no threshold takes the documented
/// default, [`THRESHOLD_BOUNDED_PERCENT`] with [`THRESHOLD_DEFAULT_VALUE`].
pub(super) fn circuit_breaker_threshold(
    breaker: Option<&Defaulted<DeploymentCircuitBreaker>>,
    desired_count: i32,
) -> i32 {
    let configuration = breaker.and_then(|breaker| breaker.threshold_configuration.as_ref());
    let kind = configuration.map_or(THRESHOLD_BOUNDED_PERCENT, |configuration| {
        configuration.r#type.as_str()
    });
    let value = configuration.map_or(THRESHOLD_DEFAULT_VALUE, |configuration| configuration.value);
    if kind == THRESHOLD_COUNT {
        return value;
    }
    let scaled = i64::from(value) * i64::from(desired_count) / 100;
    let bounded = if kind == THRESHOLD_UNBOUNDED_PERCENT {
        scaled
    } else {
        scaled.clamp(
            i64::from(MIN_CIRCUIT_BREAKER_THRESHOLD),
            i64::from(MAX_CIRCUIT_BREAKER_THRESHOLD),
        )
    };
    i32::try_from(bounded).unwrap_or(i32::MAX)
}

/// The ARN of the IAM role the CREATE named, resolved from the `role` reference it
/// carried, and `None` when it named none.
///
/// `CreateService$role`: "The name or full Amazon Resource Name (ARN) of the IAM role".
/// Which services report the ECS SERVICE-LINKED role instead is decided by what the
/// service IS, not by this request alone, so it is resolved where the service is reported
/// ([`ComAmazonawsEcsAmazonec2containerservicev20141113State::service_role_arn`]).
pub fn requested_role_arn(requested: Option<&str>) -> Option<String> {
    match requested {
        // "If your specified role has a path other than /, then you must either specify
        // the full role ARN (this is recommended) or prefix the role name with the path."
        Some(reference) if reference.starts_with("arn:") => Some(reference.to_string()),
        Some(name) => Some(::engine::emulator_arn(
            "iam",
            "",
            &format!("role/{}", name.trim_start_matches('/')),
        )),
        None => None,
    }
}

/// The ARN of the ECS service-linked role, `AWSServiceRoleForECS`. An IAM ARN carries no
/// region, which is what the empty region renders.
pub fn service_linked_role_arn() -> String {
    ::engine::emulator_arn("iam", "", SERVICE_LINKED_ROLE_RESOURCE)
}

/// The ARN resource type a service revision is named under, as the resource segment of
/// [`service_revision_arn`].
pub const SERVICE_REVISION_RESOURCE_TYPE: &str = "service-revision";

/// The ARN of a service revision:
/// `arn:aws:ecs:<region>:<account>:service-revision/<cluster>/<service>/<id>`.
pub fn service_revision_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    revision_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{SERVICE_REVISION_RESOURCE_TYPE}/{cluster_name}/{service_name}/{revision_id}"),
    )
}

/// The ARN resource type a service deployment is named under, as the resource segment of
/// [`service_deployment_arn`] and the one [`is_service_deployment_arn`] reads back.
pub const SERVICE_DEPLOYMENT_RESOURCE_TYPE: &str = "service-deployment";

/// The width of the deployment-id token that ends a service-deployment ARN: the fixed-width
/// counter the store mints it with, so sorting two of these ARNs orders the deployments they
/// name oldest first.
pub const SERVICE_DEPLOYMENT_ID_WIDTH: usize = 19;

/// The ARN of a service deployment:
/// `arn:aws:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<id>`.
pub fn service_deployment_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    deployment_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "{SERVICE_DEPLOYMENT_RESOURCE_TYPE}/{cluster_name}/{service_name}/{deployment_id}"
        ),
    )
}

/// Whether `token` is a deployment-id token of a service-deployment ARN: the fixed-width
/// decimal id [`service_deployment_arn`]'s last segment carries
/// ([`SERVICE_DEPLOYMENT_ID_WIDTH`]).
pub fn is_service_deployment_id(token: &str) -> bool {
    token.len() == SERVICE_DEPLOYMENT_ID_WIDTH && token.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `arn` is SHAPED like a service-deployment ARN —
/// `arn:<partition>:ecs:<region>:<account>:service-deployment/<cluster>/<service>/<id>`,
/// with names a cluster and a service may carry ([`is_valid_name`]) and a deployment-id
/// token ([`is_service_deployment_id`]).
///
/// This is a SYNTAX question, not a lookup: the partition, Region and account are part of a
/// deployment's identity, not of its shape, so an ARN of another Region names a deployment
/// this store does not hold rather than a malformed one.
pub fn is_service_deployment_arn(arn: &str) -> bool {
    // `arn:<partition>:ecs:<region>:<account>:<resource>`. The resource is the LAST field,
    // so it keeps any colon of its own.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() != 6
        || fields[0] != "arn"
        || fields[1].is_empty()
        || fields[2] != ARN_NAMESPACE
        || fields[3].is_empty()
        || fields[4].is_empty()
    {
        return false;
    }
    let Some(resource) = fields[5].strip_prefix(SERVICE_DEPLOYMENT_RESOURCE_TYPE) else {
        return false;
    };
    let Some(resource) = resource.strip_prefix('/') else {
        return false;
    };
    let segments: Vec<&str> = resource.split('/').collect();
    matches!(
        segments.as_slice(),
        [cluster_name, service_name, deployment_id]
            if is_valid_name(cluster_name)
                && is_valid_name(service_name)
                && is_service_deployment_id(deployment_id)
    )
}

/// The `InvalidParameterException` message for a `serviceDeploymentArn` that is not shaped
/// like one ([`is_service_deployment_arn`]): the whole sentence is fixed text, naming the
/// parameter whose syntax is wrong and echoing nothing the request carried.
pub fn invalid_service_deployment_arn_message() -> String {
    "The ServiceDeployment ARN is invalid.".to_string()
}

/// Why a rollout is in the state it is in, for the deployment that rolls it out.
///
/// `Deployment$rolloutStateReason` is "A description of the rollout state of a
/// deployment"; the service deployment that records the SAME rollout reports it as its own
/// `statusReason` ("Information about why the service deployment is in the current
/// status"), so both read it from here and cannot drift.
pub fn rollout_state_reason(deployment_id: &str, settled: bool) -> String {
    if settled {
        format!("ECS deployment {deployment_id} completed.")
    } else {
        format!("ECS deployment {deployment_id} in progress.")
    }
}

/// The task-set ID a `taskSet` reference names: the value as given when it is an id,
/// otherwise the id a task-set ARN ends with
/// (`task-set/<cluster>/<service>/<id>`, where the id itself carries an `ecs-svc/`
/// prefix). The member's doc: "The ID or full Amazon Resource Name (ARN) of task sets",
/// so a reference is either form and an ARN is never wrapped in another one.
pub fn task_set_id_from_reference(reference: &str) -> String {
    let reference = reference.trim();
    match reference.rsplit_once(":task-set/") {
        Some((_, tail)) => tail.splitn(3, '/').nth(2).unwrap_or(tail).to_string(),
        None => reference.to_string(),
    }
}

/// Why a `clientToken` a request carries cannot be stored, or `None` when it can.
///
/// Every such member's doc: "It must be unique and is case sensitive. Up to 36 ASCII
/// characters in the range of 33-126 (inclusive) are allowed." Shared by every operation
/// that takes one, so a token one of them refuses is refused by the others too.
pub fn client_token_violation(token: &str) -> Option<String> {
    let legal = token.len() <= MAX_CLIENT_TOKEN_LENGTH
        && !token.is_empty()
        && token.bytes().all(|byte| (33..=126).contains(&byte));
    if legal {
        return None;
    }
    Some(format!(
        "clientToken must be up to {MAX_CLIENT_TOKEN_LENGTH} ASCII characters in the \
         range 33-126; got {token:?}"
    ))
}

/// `InvalidParameterException$message` for a `serviceName` the cluster already holds a
/// service under, whatever `clientToken` the second create carried.
///
/// `clientToken` is "An identifier that you provide to ensure the idempotency of the
/// request", so Amazon ECS words this refusal as the failed idempotent CREATE it is: a
/// create of a name that is taken is not the request that created it, so it cannot be
/// answered with that request's service. ONE fixed sentence, naming neither the service
/// nor the cluster — the collision is scoped to the cluster (the same name is free in
/// another one), but the scope is not part of the wording.
///
/// Shared by every create that takes a `clientToken` BESIDE the service name, so those
/// operations cannot word the class differently.
pub const SERVICE_CREATION_NOT_IDEMPOTENT_MESSAGE: &str = "Creation of service was not idempotent.";

/// The refusal a service name the cluster already holds a service under earns from a
/// create that carries NO idempotency token: `CreateExpressGatewayService` models no
/// `clientToken`, so a repeat of it is not a request Amazon ECS can recognise as the same
/// one, and the condition it reports is the name rule its own member states ("This name
/// must be unique within the specified cluster") rather than a create that was not
/// idempotent ([`SERVICE_CREATION_NOT_IDEMPOTENT_MESSAGE`], which the creates that take a
/// token answer). Built from the request, since the name at fault is what the caller has
/// to change.
pub fn service_name_taken_message(service_name: &str, cluster_name: &str) -> String {
    format!("service {service_name:?} already exists in cluster {cluster_name:?}")
}

/// The ARN of a service: `arn:aws:ecs:<region>:<account>:service/<cluster>/<name>`.
pub fn service_arn(region: &str, cluster_name: &str, service_name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("service/{cluster_name}/{service_name}"),
    )
}

/// The ARN a TASK SET reports for the service it exists in:
/// `arn:aws:ecs:<region>:<account>:service/<name>`.
///
/// Recorded behavior only. `TaskSet$serviceArn` is documented as nothing more than "The
/// Amazon Resource Name (ARN) of the service the task set exists in", and the recorded
/// task sets carry the service ARN WITHOUT the cluster qualification their sibling
/// `taskSetArn` and `clusterArn` carry — which is why a task set stores the ARN it
/// reports rather than deriving it from the service's own.
pub fn task_set_service_arn(region: &str, service_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("service/{service_name}"))
}

/// The resource type a task-set ARN names.
pub const TASK_SET_RESOURCE_TYPE: &str = "task-set";

/// The ARN of a task set:
/// `arn:aws:ecs:<region>:<account>:task-set/<cluster>/<service>/<task set id>`. The id a
/// task set carries already has its own `ecs-svc/` prefix, so the ARN ends with it.
pub fn task_set_arn(
    region: &str,
    cluster_name: &str,
    service_name: &str,
    task_set_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_SET_RESOURCE_TYPE}/{cluster_name}/{service_name}/{task_set_id}"),
    )
}

/// The task-set ARN a task set that exists in NO service is named by: the id alone under
/// the resource type (`arn:aws:ecs:<region>:<account>:task-set/<id>`), with neither a
/// cluster nor a service segment in the resource part.
///
/// The cluster and service segments of [`task_set_arn`] name the service a task set
/// exists IN, which only a stored task set has, so an answer about a task set this
/// service holds none of names it by its id alone.
pub fn unqualified_task_set_arn(region: &str, task_set_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_SET_RESOURCE_TYPE}/{task_set_id}"),
    )
}

/// The `MISSING` [`Failure`] a task-set batch read reports for a reference that resolves
/// to no task set: the id the entry named ([`task_set_id_from_reference`], whichever of
/// the member's two forms the entry took) under [`unqualified_task_set_arn`].
///
/// A read answers about a task set it did not find, so the entry names that task set
/// alone and never the service the request looked for it in — which is also what lets a
/// caller correlate the entry back to the identifier it submitted however it spelled it.
pub fn unresolved_task_set_failure(region: &str, reference: &str) -> Failure {
    missing_failure(&unqualified_task_set_arn(
        region,
        &task_set_id_from_reference(reference),
    ))
}

/// The ARN resource type a SERVICE is named under, as the resource segment of
/// [`service_arn`] and the one [`cluster_and_service_from_reference`] reads back.
pub const SERVICE_RESOURCE_TYPE: &str = "service";

/// The CLUSTER and the service NAME a `service`/`services` entry refers to.
///
/// Every such member accepts "the short name or full Amazon Resource Name (ARN)" of the
/// service, and a service ARN carries the cluster that hosts it
/// (`service/<cluster>/<name>`), so an ARN names the cluster as well as the service. A
/// short name, or the old single-segment ARN form, names no cluster.
///
/// The ARN is read through the project's ONE parse of a caller-supplied ARN
/// (`service_common::arn`), the same reading [`cluster_from_scoped_arn`] gives every other
/// cluster-scoped resource, so a value that is not an ARN is the short name it is.
pub fn cluster_and_service_from_reference(reference: &str) -> (Option<String>, String) {
    let reference = reference.trim();
    let Some(resource) =
        ::service_common::arn::resource_id_of_type(reference, SERVICE_RESOURCE_TYPE)
    else {
        return (None, reference.to_string());
    };
    match resource.split_once('/') {
        Some((cluster_name, service_name)) => {
            (Some(cluster_name.to_string()), service_name.to_string())
        }
        None => (None, resource.to_string()),
    }
}

/// The service NAME a `service`/`services` entry refers to: the value as given when it
/// is a short name, otherwise the last segment of a service ARN.
pub fn service_name_from_reference(reference: &str) -> String {
    cluster_and_service_from_reference(reference).1
}

/// The ARN resource type a task is named under, as the resource segment of [`task_arn`].
pub const TASK_RESOURCE_TYPE: &str = "task";

/// The ARN of a task: `arn:aws:ecs:<region>:<account>:task/<cluster>/<id>`.
pub fn task_arn(region: &str, cluster_name: &str, task_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{TASK_RESOURCE_TYPE}/{cluster_name}/{task_id}"),
    )
}

/// The ARN of a container:
/// `arn:aws:ecs:<region>:<account>:container/<cluster>/<task id>/<container id>`.
pub fn container_arn(
    region: &str,
    cluster_name: &str,
    task_id: &str,
    container_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("container/{cluster_name}/{task_id}/{container_id}"),
    )
}

/// Whether `name` is a legal cluster name or task-definition family: "Up to 255 letters
/// (uppercase and lowercase), numbers, underscores, and hyphens are allowed" — the
/// `CreateCluster$clusterName` and `RegisterTaskDefinition$family` docs.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_LENGTH
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The pattern a name [`is_valid_name`] accepts, spelled as a refusal quotes it — the
/// character set and the length bounds the name members document, in one regular
/// expression.
pub const RESOURCE_NAME_PATTERN: &str = r"^[a-zA-Z0-9\-_]{1,255}$";

/// The sentence a value outside a documented `pattern` is refused with: the subject the
/// refusal names the member by, and the pattern the value has to match. The whole of it
/// is fixed text — a read states the RULE the value broke and echoes nothing the request
/// carried.
pub fn pattern_message(subject: &str, pattern: &str) -> String {
    format!("{subject} must match {pattern}")
}

/// `InvalidParameterException$message` for a caller-supplied NAME outside
/// [`RESOURCE_NAME_PATTERN`]: [`pattern_message`] for that pattern, plus the value the
/// request spelled, appended bare after the fixed `but was ` prefix — what a call asked
/// to TAKE the name adds, so the caller is told which of the names it sent it must change.
///
/// Spelled once for every name member in this crate, so the refusal cannot drift between
/// the operations that take one — a cluster name, a service name, a daemon name and a
/// task-definition family are all the same rule, stated the same way.
pub fn name_pattern_message(subject: &str, name: &str) -> String {
    format!(
        "{}, but was {name}",
        pattern_message(subject, RESOURCE_NAME_PATTERN)
    )
}

/// The refusal a caller-supplied `name` earns when it falls outside
/// [`RESOURCE_NAME_PATTERN`], under the `subject` the refusal names its member by, or
/// `None` when [`is_valid_name`] accepts it.
pub fn name_pattern_violation(subject: &str, name: &str) -> Option<String> {
    (!is_valid_name(name)).then(|| name_pattern_message(subject, name))
}

/// The same refusal for a member Amazon ECS names by the member itself: the subject is
/// [`member_subject`] of the member's own name.
pub fn member_name_pattern_violation(member: &str, name: &str) -> Option<String> {
    name_pattern_violation(&member_subject(member), name)
}

/// The words a refusal that reads as a SENTENCE names a member by: [`member_words`] with
/// the first letter upper-cased, so `clusterName` opens the sentence as `Cluster name`.
pub fn member_subject(member_path: &str) -> String {
    let words = member_words(member_path);
    let mut letters = words.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => words,
    }
}

/// The subject the refusal of a cluster reference a READ carries names it by: a describe's
/// entries are "cluster names or full cluster Amazon Resource Name (ARN) entries", so the
/// refusal names the cluster NAME the entry resolves to rather than the list member that
/// carried it — one name, spelled as the member it stands for and not as a sentence's
/// words.
pub const READ_CLUSTER_NAME_SUBJECT: &str = "ClusterName";

/// The name rule [`RESOURCE_NAME_PATTERN`] states, as the READ's refusal spells it: the
/// same character set and the same bounds, with the escape before the literal hyphen
/// doubled.
///
/// Recorded text of that ONE sentence, and the reason it is spelled here rather than
/// shared with the pattern the other refusals carry: the recorded refusal of a cluster
/// reference a read names carries the doubled escape, where the recorded refusal of a
/// name a create was asked to take carries the single one. Two sentences of two different
/// calls, each reproduced as that call states it.
pub const READ_CLUSTER_NAME_PATTERN: &str = r"^[a-zA-Z0-9\\-_]{1,255}$";

/// The refusal a cluster reference a READ carries earns when the name it resolves to
/// falls outside [`RESOURCE_NAME_PATTERN`], or `None` when [`is_valid_name`] accepts it.
///
/// A name outside that set names no cluster this account could ever hold, so the read
/// refuses the parameter instead of reporting a cluster that merely happens to be
/// missing. Unlike a create's refusal the sentence echoes no value: a read states the
/// rule, as every other pattern refusal a read answers does
/// ([`container_instance_id_pattern_message`]).
pub fn read_cluster_name_violation(name: &str) -> Option<String> {
    (!is_valid_name(name))
        .then(|| pattern_message(READ_CLUSTER_NAME_SUBJECT, READ_CLUSTER_NAME_PATTERN))
}

/// The ARN of a capacity provider: `arn:aws:ecs:<region>:<account>:capacity-provider/<name>`.
pub fn capacity_provider_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("capacity-provider/{name}"))
}

/// The record of one of the capacity providers Amazon ECS MANAGES
/// ([`MANAGED_CAPACITY_PROVIDERS`]), or `None` for a name that is not one of them.
///
/// Every account holds these in every Region: nothing creates them and nothing can take
/// their names, so they are resolved from the name and the request's own Region rather
/// than stored — which is also what keeps the ARN they report in the Region the caller
/// asked in. They bind onto Fargate capacity, which is neither a caller's Auto Scaling
/// group nor the managed instances Amazon ECS runs for a cluster, so they carry neither
/// provider configuration; `CapacityProvider$type` is the Fargate capacity class each one
/// binds onto, which `CapacityProviderType` spells with the provider's own name, and
/// `CapacityProvider$status` is [`CAPACITY_PROVIDER_ACTIVE`] — they are there and usable,
/// and nothing can delete them.
pub fn managed_capacity_provider(name: &str, region: &str) -> Option<CapacityProviderRecord> {
    if !MANAGED_CAPACITY_PROVIDERS.contains(&name) {
        return None;
    }
    Some(CapacityProviderRecord {
        name: name.to_string(),
        arn: capacity_provider_arn(region, name),
        status: CAPACITY_PROVIDER_ACTIVE.to_string(),
        update_status: None,
        active_due_at: None,
        cluster: None,
        auto_scaling_group_provider: None,
        managed_instances_provider: None,
        provider_type: Some(name.to_string()),
        tags: Vec::new(),
    })
}

/// The message the refusal of a capacity-provider `name` carries. A fixed sentence, as
/// the recorded refusal carries it, restating the whole of the name rule
/// `CreateCapacityProvider$name` documents — the length, the character set and the
/// prefixes Amazon ECS keeps for its own providers — whichever half of it the value broke.
pub const CAPACITY_PROVIDER_NAME_INVALID_MESSAGE: &str =
    "The specified capacity provider name is invalid. Up to 255 characters are allowed, \
     including letters (upper and lowercase), numbers, underscores, and hyphens. The name \
     cannot be prefixed with \"aws\", \"ecs\", or \"fargate\". Specify a valid name and \
     try again.";

/// Whether a capacity-provider `name` is one the caller may take, or the refusal it earns.
///
/// The `CreateCapacityProvider$name` doc: "Up to 255 characters are allowed. They include
/// letters (both upper and lowercase letters), numbers, underscores (_), and hyphens (-).
/// The name can't be prefixed with \"aws\", \"ecs\", or \"fargate\"." The character rule is
/// [`is_valid_name`]'s, shared with every other ECS name; the reserved prefixes are this
/// member's own, and are what keeps a caller from shadowing `FARGATE`/`FARGATE_SPOT`.
///
/// The name is the provider's resource IDENTIFIER (the model's `resourceIdentifier` on
/// this member), so a value outside that one rule is refused as the `ClientException`
/// modelled for "specifying an identifier that isn't valid", carrying the one sentence
/// that states the rule.
pub fn validate_capacity_provider_name(name: &str) -> Result<(), &'static str> {
    let lowered = name.to_ascii_lowercase();
    let reserved = CAPACITY_PROVIDER_RESERVED_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(prefix));
    if !is_valid_name(name) || reserved {
        return Err(CAPACITY_PROVIDER_NAME_INVALID_MESSAGE);
    }
    Ok(())
}

/// One kind of Amazon ECS resource that carries its OWN tag set, and so can be named by
/// a tagging operation's `resourceArn`.
///
/// The kinds are the ones those members document — `ListTagsForResource$resourceArn`:
/// "the supported resources are Amazon ECS tasks, services, task definitions, clusters,
/// and container instances", and `TagResource$resourceArn`, which adds the capacity
/// provider — and each one's ARN form is the one the builder named on its variant emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// [`capacity_provider_arn`].
    CapacityProvider,
    /// [`cluster_arn`].
    Cluster,
    /// [`container_instance_arn`].
    ContainerInstance,
    /// [`service_arn`].
    Service,
    /// [`task_arn`].
    Task,
    /// [`task_definition_arn`].
    TaskDefinition,
}

/// The ARN resource-type segment each taggable kind carries, as that kind's ARN builder
/// in this module spells it. The segments are the catalog [`TaggedResourceKind::from_arn`]
/// resolves an ARN against, and the list a refusal names.
pub const TAGGED_RESOURCE_TYPES: [(&str, TaggedResourceKind); 6] = [
    ("capacity-provider", TaggedResourceKind::CapacityProvider),
    ("cluster", TaggedResourceKind::Cluster),
    ("container-instance", TaggedResourceKind::ContainerInstance),
    ("service", TaggedResourceKind::Service),
    ("task", TaggedResourceKind::Task),
    (
        TASK_DEFINITION_RESOURCE_TYPE,
        TaggedResourceKind::TaskDefinition,
    ),
];

/// Which MODELLED error a "this `resourceArn` names no such resource" refusal takes.
///
/// Spelled as a kind rather than as one operation's error enum, so every operation that
/// resolves a `resourceArn` refuses the same ARN with the same error code and the same
/// sentence, whichever of them the client called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceRefusal {
    /// `ClusterNotFoundException` — the refusal ECS models for a cluster that is not
    /// there, and the one every operation in this crate answers for that condition.
    ClusterNotFound,
    /// `ClientException`.
    Client,
    /// `InvalidParameterException`.
    InvalidParameter,
}

impl TaggedResourceKind {
    /// The kind `resource_arn` names, or `None` when it is not the ARN of a taggable
    /// Amazon ECS resource at all — a value in another service's namespace, a malformed
    /// ARN, or an ECS resource that carries no tag set of its own.
    ///
    /// The KIND is read from the resource type alone. The partition, Region and account
    /// are part of the resource's IDENTITY, not of its kind, and the lookups above
    /// compare a whole ARN against the whole ARN the record stores (which carries the
    /// serving Region and account) — so an ECS ARN of another partition, Region or
    /// account names a resource of a known kind that this store does not hold, and earns
    /// that kind's own not-found refusal rather than being called a malformed parameter.
    pub fn from_arn(resource_arn: &str) -> Option<Self> {
        // `arn:<partition>:ecs:<region>:<account>:<resource>`. The resource is the LAST
        // field, so it keeps any colon of its own (a task definition's revision).
        let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
        if fields.len() != 6 || fields[0] != "arn" || fields[2] != ARN_NAMESPACE {
            return None;
        }
        let resource_type = fields[5].split('/').next().unwrap_or_default();
        TAGGED_RESOURCE_TYPES
            .iter()
            .find(|(segment, _)| *segment == resource_type)
            .map(|(_, kind)| *kind)
    }

    /// The refusal a `resourceArn` of this kind earns when the store holds no such
    /// resource: the modelled error it takes and the sentence that error carries.
    ///
    /// Each sentence is the one this crate already answers wherever that kind fails to
    /// resolve, so "this ARN names no cluster" reads the same from a tagging operation as
    /// from a describe.
    pub fn not_found_refusal(self, resource_arn: &str) -> (ResourceRefusal, String) {
        match self {
            Self::Cluster => (
                ResourceRefusal::ClusterNotFound,
                CLUSTER_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::TaskDefinition => (
                ResourceRefusal::Client,
                TASK_DEFINITION_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::Service => (
                ResourceRefusal::InvalidParameter,
                SERVICE_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::Task => (
                ResourceRefusal::InvalidParameter,
                TASK_NOT_FOUND_MESSAGE.to_string(),
            ),
            Self::ContainerInstance => (
                ResourceRefusal::InvalidParameter,
                container_instance_not_found_message(resource_arn),
            ),
            Self::CapacityProvider => (
                ResourceRefusal::InvalidParameter,
                resource_not_found_message(resource_arn),
            ),
        }
    }
}

/// The `message` a refusal carries for a `resourceArn` that names no resource of a kind
/// this crate holds no sentence of its own for: the `ResourceNotFoundException` doc
/// sentence ("The specified resource wasn't found.") with the ARN the request named, so
/// a caller can tell which ARN was rejected.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("The specified resource wasn't found: {resource_arn}")
}

/// `InvalidParameterException$message` for a `resourceArn` that is not the ARN of a
/// taggable Amazon ECS resource ([`TaggedResourceKind::from_arn`] answers `None`).
///
/// DERIVED from the request, like the other reference refusals in this module: it names
/// the member, the resource types the member accepts ([`TAGGED_RESOURCE_TYPES`]) and the
/// value that is none of them.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    let kinds: Vec<&str> = TAGGED_RESOURCE_TYPES
        .iter()
        .map(|(segment, _)| *segment)
        .collect();
    format!(
        "resourceArn must be an Amazon ECS resource ARN naming one of {}; got \
         {resource_arn:?}",
        kinds.join(", ")
    )
}

/// The prefix Amazon Web Services keeps for its OWN tags, so no caller-supplied tag may
/// carry it: the `Tag` doc — "Do not use `aws:`, `AWS:`, or any upper or lowercase
/// combination of such as a prefix for either keys or values as it is reserved for Amazon
/// Web Services use. You cannot edit or delete tag keys or values with this prefix."
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The refusal a request's `tags` list earns when one of its entries would write into the
/// reserved namespace above, or `None` when every entry is the caller's own to write.
///
/// The restriction is the service's, not the model's: `TagKey`'s pattern admits ':', so a
/// reserved key is a well-formed value the SERVICE refuses — which is why every operation
/// that takes a `tags` list checks it here, and reports it as the bad parameter it is.
/// "any upper or lowercase combination of such", so the prefix compares without regard to
/// case, and it is checked on "either keys or values".
///
/// The message names the MEMBER of the entry that failed and the prefix it may not carry,
/// so a caller is pointed at the one thing to change; like the other fixed refusals in
/// this module it carries none of the request's values, since the condition is about the
/// prefix and not about what follows it.
pub fn reserved_tag_prefix_violation(tags: &[Tag]) -> Option<String> {
    tags.iter().find_map(|tag| {
        [("key", tag.key.as_deref()), ("value", tag.value.as_deref())]
            .into_iter()
            .find_map(|(member, value)| {
                starts_with_reserved_tag_prefix(value.unwrap_or_default()).then(|| {
                    format!("Tag {member} must not start with {RESERVED_TAG_PREFIX}")
                })
            })
    })
}

/// Whether `value` carries [`RESERVED_TAG_PREFIX`], in any combination of cases.
pub(super) fn starts_with_reserved_tag_prefix(value: &str) -> bool {
    value
        .get(..RESERVED_TAG_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_PREFIX))
}

/// Associate `requested` with the tag set `stored`, the way a tagging write does:
/// "Associates the specified tags to a resource with the specified resourceArn. If
/// existing tags on a resource aren't specified in the request parameters, they aren't
/// changed."
///
/// So a key the request names takes the value the request gives it ("each tag key must be
/// unique, and each tag key can have only one value"), a key the resource does not carry
/// yet is added, and a key the request does not name keeps the value it had. Keys compare
/// EXACTLY — "Tag keys and values are case-sensitive" — and the request's entries are
/// applied in order, so the resource ends up with one entry per key whatever the request
/// listed. The value is stored as the request gave it, so an empty value stays an empty
/// value rather than reading as a key with none.
///
/// `Err` carries the message of the resource limit the merged set would break ("Maximum
/// number of tags per resource - 50", [`MAX_TAGS_PER_RESOURCE`]); `stored` is left
/// untouched in that case, so a refused write changes nothing.
///
/// The limit is what bounds the WORK one write may ask for: how many tags the write would
/// leave the resource carrying is ONE entry per distinct key across the two lists, counted
/// in one pass before anything is copied, so a write over the limit is refused before the
/// merge is done; and the merge itself is keyed through an index over the stored keys, so
/// applying the request costs one lookup per entry rather than a walk over the set it is
/// growing.
pub fn merge_tags(stored: &mut Vec<Tag>, requested: &[Tag]) -> Result<(), String> {
    let merged_count = stored
        .iter()
        .chain(requested)
        .map(|tag| tag.key.as_deref())
        .collect::<HashSet<Option<&str>>>()
        .len();
    if merged_count > MAX_TAGS_PER_RESOURCE {
        return Err(tags_per_resource_limit_message(merged_count));
    }
    let mut merged = stored.clone();
    let mut positions: HashMap<Option<String>, usize> = HashMap::with_capacity(merged.len());
    for (position, tag) in merged.iter().enumerate() {
        positions.entry(tag.key.clone()).or_insert(position);
    }
    for tag in requested {
        match positions.get(&tag.key) {
            Some(&position) => merged[position].value = tag.value.clone(),
            None => {
                positions.insert(tag.key.clone(), merged.len());
                merged.push(tag.clone());
            }
        }
    }
    *stored = merged;
    Ok(())
}

/// Delete the tags `tag_keys` names from the tag set `stored`: "Deletes specified tags
/// from a resource", where the keys are "the keys of the tags to be removed".
///
/// A key the resource does not carry removes nothing, so asking twice leaves the same tag
/// set as asking once, and a key the request does not name is left where it is.
pub fn remove_tags(stored: &mut Vec<Tag>, tag_keys: &[String]) {
    stored.retain(|tag| {
        !tag_keys
            .iter()
            .any(|removed| tag.key.as_deref() == Some(removed.as_str()))
    });
}

/// `LimitExceededException$message` for a tagging write whose merged tag set would carry
/// more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// DERIVED from the request: it names the limit and how many tags the resource would be
/// left carrying, so a caller can tell how far over it went.
pub fn tags_per_resource_limit_message(merged_count: usize) -> String {
    format!(
        "The limit of {MAX_TAGS_PER_RESOURCE} tags per resource was exceeded: the request \
         would leave the resource with {merged_count} tags"
    )
}

/// `AwsVpcConfiguration$assignPublicIp` as Amazon ECS materialises it when a service
/// leaves it out — the member's own doc: "When you use `create-service` or
/// `update-service`, the default is `DISABLED`." One of the `AssignPublicIp` enum's two
/// values (`smithy_query(query="shape", shape="AssignPublicIp")`).
pub const ASSIGN_PUBLIC_IP_DISABLED: &str = "DISABLED";

/// The other value of the `AssignPublicIp` enum: the task's elastic network interface is
/// given a public IP address.
pub const ASSIGN_PUBLIC_IP_ENABLED: &str = "ENABLED";

/// `InvalidParameterException$message` for a placement that asks for a public IP on
/// infrastructure that cannot give one. One fixed sentence, naming neither the launch type
/// nor the member.
pub const ASSIGN_PUBLIC_IP_UNSUPPORTED_MESSAGE: &str =
    "Assign public IP is not supported for this launch type.";

/// Whether a placement runs its tasks on FARGATE: the launch type it names, and otherwise
/// the capacity providers its strategy names — "When you use Fargate, the capacity providers
/// are FARGATE or FARGATE_SPOT" ([`MANAGED_CAPACITY_PROVIDERS`]). A placement that names
/// neither runs on the capacity registered to the cluster ([`DEFAULT_LAUNCH_TYPE`]), which
/// is not Fargate.
///
/// `capacityProvider` is "The short name or full Amazon Resource Name (ARN) of the capacity
/// provider", so each entry is read as the provider it REFERS to
/// ([`capacity_provider_name_from_reference`]) rather than as the string it was spelled in.
///
/// ONE predicate for every reader: the refusal of a request that asks for what only Fargate
/// does ([`assign_public_ip_violation`]) and the projections that report the Fargate-only
/// members (`Service$platformFamily`, `$platformVersion`, and the same two on a deployment
/// and a task set) must not disagree about where a service's tasks run.
pub fn places_on_fargate(
    launch_type: Option<&str>,
    strategy: &[CapacityProviderStrategyItem],
) -> bool {
    launch_type == Some(LAUNCH_TYPE_FARGATE)
        || strategy.iter().any(|item| {
            MANAGED_CAPACITY_PROVIDERS
                .contains(&capacity_provider_name_from_reference(&item.capacity_provider).as_str())
        })
}

/// The refusal a `networkConfiguration` earns when it asks for a public IP on a placement
/// that is not on Fargate, or `None` when it asks for none (or the tasks run on Fargate).
///
/// `AwsVpcConfiguration$assignPublicIp` gives the task's own elastic network interface a
/// public address, which only the Fargate infrastructure attaches: a task on Amazon EC2 or
/// external capacity is reached through the instance that runs it, whose addressing belongs
/// to the instance and not to this request.
pub fn assign_public_ip_violation(
    network_configuration: Option<&NetworkConfiguration>,
    fargate: bool,
) -> Option<&'static str> {
    if fargate {
        return None;
    }
    let requested = network_configuration?
        .awsvpc_configuration
        .as_ref()?
        .assign_public_ip
        .as_deref()?;
    (requested == ASSIGN_PUBLIC_IP_ENABLED).then_some(ASSIGN_PUBLIC_IP_UNSUPPORTED_MESSAGE)
}

/// The network configuration a service STORES: the one the request carried, with the
/// members Amazon ECS fills in filled in, so every later read reports the same rendered
/// configuration rather than only what the caller sent.
///
/// `assignPublicIp` takes the value its own doc gives it — "When you use `create-service`
/// or `update-service`, the default is `DISABLED`" — and `securityGroups`, which a
/// request may leave out, is MATERIALISED as an empty list, as the recorded read-back of
/// a configuration that named none carries it: the group the task actually gets is then
/// "the default security group for the VPC", whose id belongs to the VPC rather than to
/// this configuration, so none is invented in its place. `subnets` is required, so there
/// is nothing to fill in for it.
pub fn defaulted_network_configuration(
    requested: Option<&NetworkConfiguration>,
) -> Option<NetworkConfiguration> {
    let configuration = requested?;
    let awsvpc_configuration = configuration
        .awsvpc_configuration
        .as_ref()
        .map(|awsvpc| AwsVpcConfiguration {
            assign_public_ip: Some(
                awsvpc
                    .assign_public_ip
                    .clone()
                    .unwrap_or_else(|| ASSIGN_PUBLIC_IP_DISABLED.to_string()),
            ),
            security_groups: Some(awsvpc.security_groups.clone().unwrap_or_default()),
            ..awsvpc.clone()
        });
    Some(NetworkConfiguration {
        awsvpc_configuration,
    })
}

/// `InvalidParameterException$message` for a launch that carries a `networkConfiguration`
/// the network mode of its task definition gives it nothing to configure.
///
/// A fixed sentence: it POINTS AT the network mode of the task definition the launch runs
/// rather than quoting the mode, so the refusal carries no value from the request or from
/// the stored revision — the caller reads the mode off the task definition the sentence
/// names.
pub const NETWORK_CONFIGURATION_NOT_VALID_MESSAGE: &str =
    "Network Configuration is not valid for the given networkMode of this task definition.";

/// The refusal an exec-enabled launch earns when NO task role resolves for the task it
/// launches, or `None` when one does (or when the launch does not enable ECS Exec at all).
///
/// The rule: ECS Exec runs through the SSM agent inside the task, which signs its calls
/// with the task's own IAM role — so a launch that turns `enableExecuteCommand` on
/// requires the task to have one. `task_role_arn` is the role the task ACTUALLY assumes,
/// which for a placement that carries overrides is what those overrides resolved to
/// ([`declared_task_role_arn`]) rather than the stored revision's own member: the role is
/// what the rule is about, not where it was supplied. `subject` names what the refusal is
/// about ("The service couldn't be created", "The task couldn't be launched"), since the
/// sentence AWS words is the one the calling operation's own answer carries. Its
/// remediation clause names WHERE the role belongs — in the task definition, which is the
/// resource the caller has to change when the launch itself supplied none.
pub fn execute_command_violation(
    enabled: bool,
    task_role_arn: Option<&str>,
    subject: &str,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let has_role = declared_task_role_arn(task_role_arn).is_some();
    (!has_role).then(|| {
        format!(
            "{subject} because a valid taskRoleArn is not being used. Specify a valid \
             task role in your task definition and try again."
        )
    })
}

/// What [`execute_command_violation`] names in a `CreateService` refusal.
pub const EXECUTE_COMMAND_SERVICE_SUBJECT: &str = "The service couldn't be created";

/// What [`execute_command_violation`] names in a `RunTask` refusal: the launch of the
/// task itself, which is what ECS Exec attaches to.
pub const EXECUTE_COMMAND_TASK_SUBJECT: &str = "The task couldn't be launched";

/// What [`execute_command_violation`] names in an `UpdateService` refusal.
pub const EXECUTE_COMMAND_SERVICE_UPDATE_SUBJECT: &str = "The service couldn't be updated";

/// The refusal a `loadBalancers` list earns when one of its bindings names a container,
/// or a container port, the task definition it is bound to does not declare — or `None`
/// when every binding resolves.
///
/// The rule: a load-balancer binding attaches ONE container port of the task to a target
/// group, so the container it names must be one of the revision's `containerDefinitions`
/// and the port must be one that container declares in its `portMappings` (as a single
/// `containerPort`, or inside a `containerPortRange`). The refusal names the container and
/// the port at fault, because it is derived from the request.
pub fn load_balancer_binding_violation(
    definition: Option<&TaskDefinitionRecord>,
    load_balancers: &[LoadBalancer],
) -> Option<String> {
    let definition = definition?;
    for binding in load_balancers {
        let Some(container_name) = binding.container_name.as_deref() else {
            return Some(
                "A load balancer configuration must specify the containerName it \
                 attaches."
                    .to_string(),
            );
        };
        let Some(container) = definition
            .container_definitions
            .iter()
            .find(|container| container.name.as_deref() == Some(container_name))
        else {
            return Some(format!(
                "The container {container_name} does not exist in the task definition."
            ));
        };
        let Some(container_port) = binding.container_port else {
            return Some(format!(
                "The container {container_name} load balancer configuration must specify \
                 a containerPort."
            ));
        };
        let declared = container
            .port_mappings
            .iter()
            .flatten()
            .any(|mapping| port_mapping_covers(mapping, container_port));
        if !declared {
            return Some(format!(
                "The container {container_name} did not have a container port \
                 {container_port} defined."
            ));
        }
    }
    None
}

/// Whether one `portMapping` declares `port` as a container port: either as its own
/// `containerPort`, or inside the `containerPortRange` it declares instead.
pub(super) fn port_mapping_covers(mapping: &PortMapping, port: i32) -> bool {
    if mapping.container_port == Some(port) {
        return true;
    }
    container_port_range(mapping).is_some_and(|range| range.contains(&port))
}

/// The inclusive range of container ports one `portMapping` declares instead of a single
/// `containerPort` ("The port number range on the container that's bound to the dynamically
/// mapped host port range", written `<start>-<end>`, whose "first port in the range must be
/// less than last port in the range").
///
/// `None` for a mapping that declares no range, or one whose value is not two port numbers
/// in that order — a value no range covers rather than one read backwards.
pub(super) fn container_port_range(
    mapping: &PortMapping,
) -> Option<::std::ops::RangeInclusive<i32>> {
    let (start, end) = mapping.container_port_range.as_deref()?.split_once('-')?;
    let (start, end) = (
        start.trim().parse::<i32>().ok()?,
        end.trim().parse::<i32>().ok()?,
    );
    (start <= end).then_some(start..=end)
}

/// The target groups of `arns` that NO load balancer forwards to, read from the Elastic
/// Load Balancing v2 service's OWN store through the one shared engine — the same
/// one-directional, guard-free read [`target_group_target_types`] makes, and under the
/// same LOCK ORDER: it takes the ELBv2 store's lock, holds no other while it does, and
/// hands back owned values, so it is called with NO guard of this service's store held.
///
/// An ARN that names no stored target group is NOT reported here: whether a group exists
/// at all is [`load_balancer_refusal`]'s answer, and reporting it twice would give one
/// condition two sentences.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own store
/// being unreachable.
pub fn unattached_target_groups<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arns: &[String],
) -> Option<Vec<String>> {
    let state =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        arns.iter()
            .filter(|arn| {
                store
                    .target_groups
                    .get(*arn)
                    .is_some_and(|group| group.load_balancer_arns.is_empty())
            })
            .cloned()
            .collect(),
    )
}

/// The refusal a `loadBalancers` list earns when one of the target groups it names has no
/// load balancer forwarding to it — or `None` when every named group is attached to one.
///
/// The rule: a service registers its tasks as TARGETS of the group, and only a listener
/// on a load balancer can send traffic to a group — so a group no load balancer forwards
/// to can never reach the tasks, and ECS refuses the binding at create time. The
/// association is LIVE: `unattached_target_groups` reads it at request time out of the
/// records the ELBv2 service holds (`TargetGroupRecord::load_balancer_arns`, which its own
/// `DescribeTargetGroups` reports as `LoadBalancerArns` and its listeners maintain), so
/// the same group is refused before a listener forwards to it and accepted after.
///
/// `arns` fixes the order the bindings are reported in, so a request naming two
/// unattached groups is always refused on the same one.
pub fn orphan_target_group_refusal(arns: &[String], unattached: &[String]) -> Option<String> {
    arns.iter()
        .find(|arn| unattached.contains(*arn))
        .map(|arn| {
            format!(
                "The target group with targetGroupArn {arn} does not have an associated \
                 load balancer."
            )
        })
}

/// Whether an `include` list asks for the resource's TAGS.
///
/// Every ECS `include` member targets a closed `…Field` enum that declares `TAGS`
/// (`CapacityProviderField`, `ClusterField`, `ContainerInstanceField`, `ServiceField`,
/// `TaskField`, `ExpressGatewayServiceInclude`), and a field outside the enum cannot
/// reach a handler — the boundary validator refuses it — so the question is only whether
/// `TAGS` is among them.
pub fn include_requests_tags(include: Option<&Vec<String>>) -> bool {
    include
        .into_iter()
        .flatten()
        .any(|field| field == INCLUDE_FIELD_TAGS)
}

/// The `include` field every ECS describe gates its tag set on.
pub const INCLUDE_FIELD_TAGS: &str = "TAGS";

/// The capacity-provider NAME a `capacityProviders` entry refers to: the value as given
/// when it is a short name, and the name out of a capacity-provider ARN
/// (`arn:aws:ecs:<region>:<account>:capacity-provider/<name>`) when it is one — the
/// member's doc, "The short name or full Amazon Resource Name (ARN) of one or more
/// capacity providers".
pub fn capacity_provider_name_from_reference(reference: &str) -> String {
    match reference.rsplit_once(":capacity-provider/") {
        Some((_, name)) => name.to_string(),
        None => reference.to_string(),
    }
}

/// The Auto Scaling group an `autoScalingGroupArn` member names, as the key two
/// capacity providers are compared on: the group NAME, which is the last segment of an
/// Auto Scaling group ARN
/// (`arn:aws:autoscaling:<region>:<account>:autoScalingGroup:<id>:autoScalingGroupName/<name>`)
/// and the value itself when the member carries the name ("The Amazon Resource Name
/// (ARN) that identifies the Auto Scaling group, or the Auto Scaling group name").
///
/// Naming one group by ARN and by name is naming the SAME group, so the reuse rule
/// cannot be slipped by spelling it the other way.
pub fn auto_scaling_group_key(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

/// The `family` and optional `revision` a `taskDefinition` request member names.
///
/// The member's doc: "The family and revision (family:revision) or full ARN of the task
/// definition to run. If a revision isn't specified, the latest ACTIVE revision is
/// used." `None` when the value is not a task-definition reference at all.
pub fn parse_task_definition_reference(reference: &str) -> Option<(String, Option<i32>)> {
    parse_revision_reference(reference, TASK_DEFINITION_RESOURCE_TYPE)
}

/// The `family` and optional `revision` a `daemonTaskDefinition` request member names.
///
/// The member's doc: "The family for the latest ACTIVE revision, family and revision
/// (family:revision) for a specific revision in the family, or full Amazon Resource Name
/// (ARN) of the daemon task definition" — the same three forms
/// [`parse_task_definition_reference`] reads, under this resource's own ARN type.
pub fn parse_daemon_task_definition_reference(reference: &str) -> Option<(String, Option<i32>)> {
    parse_revision_reference(reference, DAEMON_TASK_DEFINITION_RESOURCE_TYPE)
}

/// Whether a `taskDefinition` reference SPELLS a revision at all: the `family:revision`
/// form and the ARN of one carry a revision number beside the family, while a value that
/// names a bare family, or `family:*` ("every revision" of one), or that is no reference
/// to a task definition at all, carries none.
///
/// Answered from the value's SHAPE, before it is read as a family and a revision
/// ([`parse_task_definition_reference`]), so an operation that acts on one revision sees
/// a value miss the revision it requires however else that value is malformed.
pub fn names_task_definition_revision(reference: &str) -> bool {
    matches!(
        split_revision_reference(reference, TASK_DEFINITION_RESOURCE_TYPE),
        Some((_, Some(revision))) if revision.parse::<i32>().is_ok()
    )
}

/// The `family` and optional `revision` a reference to a revision of `resource_type`
/// names: a family, a `family:revision`, or the full ARN of either.
///
/// One function for both resources whose references take those three forms, so a value
/// is read the same way whichever member carried it.
pub(super) fn parse_revision_reference(
    reference: &str,
    resource_type: &str,
) -> Option<(String, Option<i32>)> {
    let (family, revision) = split_revision_reference(reference, resource_type)?;
    if !is_valid_name(family) {
        return None;
    }
    match revision {
        None => Some((family.to_string(), None)),
        // `family:*` names every revision, which the doc calls out for IAM resources;
        // for a run it behaves as "no revision specified", i.e. the latest ACTIVE one.
        Some("*") => Some((family.to_string(), None)),
        Some(revision) => revision
            .parse::<i32>()
            .ok()
            .filter(|revision| *revision > 0)
            .map(|revision| (family.to_string(), Some(revision))),
    }
}

/// The FAMILY part of a reference to a revision of `resource_type`, and the revision text
/// beside it when the reference spells one — both BEFORE either is checked against the
/// rule it follows. The parts come from the ARN's last segment when the value is an ARN of
/// that resource, and from the value itself when it is not an ARN at all; `None` for an
/// ARN that names some other resource, which carries no family of this one.
pub(super) fn split_revision_reference<'a>(
    reference: &'a str,
    resource_type: &str,
) -> Option<(&'a str, Option<&'a str>)> {
    let tail = match reference.strip_prefix("arn:") {
        Some(_) => reference.rsplit_once(&format!(":{resource_type}/"))?.1,
        None => reference,
    };
    Some(match tail.rsplit_once(':') {
        Some((family, revision)) => (family, Some(revision)),
        None => (tail, None),
    })
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// How many seconds one minute of a wait is, so a minutes-valued member is converted to a
/// duration in one place.
pub(super) const SECONDS_PER_MINUTE: u64 = 60;

/// The instant a wait of `minutes` from `from` falls due.
///
/// Every minutes-valued member of this API states the range it accepts
/// (`DeploymentLifecycleHookTimeoutConfiguration$timeoutInMinutes` is `1..=20160`,
/// `UpdateTaskProtection$expiresInMinutes` `1..=2880`) and a value outside it is refused
/// before any of this runs, so the conversion always has an answer. It is done CHECKED all
/// the same, so no instant this store keeps can be the product of overflowing arithmetic: a
/// wait that is not a number of minutes, or that this clock cannot represent, is read as a
/// wait that is already over rather than wrapping or panicking.
///
/// Spelled once for every such member, so two waits cannot be converted differently.
pub fn minutes_from(from: SystemTime, minutes: i32) -> SystemTime {
    u64::try_from(minutes)
        .ok()
        .and_then(|minutes| minutes.checked_mul(SECONDS_PER_MINUTE))
        .and_then(|seconds| from.checked_add(Duration::from_secs(seconds)))
        .unwrap_or(from)
}

/// A wire timestamp as a key that ORDERS and COMPARES: whole seconds and the nanoseconds
/// within them. [`DateTime`] is not itself ordered, and comparing two of them through
/// floating-point seconds would make two instants a few nanoseconds apart compare equal.
pub fn timestamp_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The `TargetType` a target group must carry to receive the tasks of a revision whose
/// `networkMode` is `network_mode`.
///
/// `LoadBalancer$targetGroupArn`: "If your service's task definition uses the `awsvpc`
/// network mode, you must choose `ip` as the target type, not `instance`. Do this when
/// creating your target groups because tasks that use the `awsvpc` network mode are
/// associated with an elastic network interface, not an Amazon EC2 instance." Every
/// other mode (`bridge`, [`NETWORK_MODE_HOST`], [`NETWORK_MODE_NONE`]) leaves the task
/// on the container instance's network interface, so the load balancer reaches it at
/// that instance — the `instance` target type.
pub fn required_target_type(network_mode: &str) -> &'static str {
    if network_mode == NETWORK_MODE_AWSVPC {
        TARGET_TYPE_IP
    } else {
        TARGET_TYPE_INSTANCE
    }
}

/// Every ELBv2 target group a `loadBalancers` list names, in request order: each entry's
/// `targetGroupArn` plus the `alternateTargetGroupArn` of its `advancedConfiguration`,
/// which a blue/green deployment registers the SAME tasks into and which is therefore
/// bound by the same rule.
///
/// The list is caller-supplied, so the duplicate check is a hash-set lookup rather than a
/// scan — the walk is linear in the list, and the list itself is bounded by
/// [`MAX_LOAD_BALANCERS`] before a handler calls this.
pub fn requested_target_group_arns(load_balancers: &[LoadBalancer]) -> Vec<String> {
    let mut arns = Vec::new();
    let mut seen = HashSet::new();
    for entry in load_balancers {
        for arn in [
            entry.target_group_arn.as_deref(),
            entry
                .advanced_configuration
                .as_ref()
                .and_then(|advanced| advanced.alternate_target_group_arn.as_deref()),
        ]
        .into_iter()
        .flatten()
        {
            if seen.insert(arn.to_string()) {
                arns.push(arn.to_string());
            }
        }
    }
    arns
}

/// The `TargetType` of each ELBv2 target group in `arns`, read from the Elastic Load
/// Balancing v2 service's OWN store through the one shared engine — so a target group
/// created over that API is seen here with no propagation step and no second copy of it
/// kept in this service's state. An ARN that names no stored target group is absent from
/// the answer.
///
/// `None` when that store is unreachable (the engine is not the shared in-memory one),
/// which a handler answers with the same modelled server error it uses for its own
/// store being unreachable.
///
/// LOCK ORDER: this takes the ELBv2 store's lock and holds no other while it does, and
/// it hands back owned values rather than a guard — so it is called with NO guard of
/// this service's store held. Keeping the cross-service read one-directional and
/// guard-free is what makes a lock cycle between the two stores unreachable. The map it
/// allocates is bounded by the request's own `loadBalancers` list.
pub fn target_group_target_types<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arns: &[String],
) -> Option<HashMap<String, String>> {
    let state =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        arns.iter()
            .filter_map(|arn| {
                store
                    .target_groups
                    .get(arn)
                    .map(|group| (arn.clone(), group.target_type.clone()))
            })
            .collect(),
    )
}

/// The refusal a `loadBalancers` list a request NAMES earns, or `None` when every target
/// group it names exists and can carry the service's tasks.
///
/// `target_types` is what [`target_group_target_types`] read for
/// [`requested_target_group_arns`] of the same list: an ARN it does not hold names no
/// target group at all, and one it does is checked by [`target_group_type_refusal`]. A
/// service with no resolved task definition (a deployment controller that takes its tasks
/// from a task set) has no network mode to check against, so only the existence of the
/// target group is.
pub fn load_balancer_refusal(
    load_balancers: &[LoadBalancer],
    network_mode: Option<&str>,
    target_types: &HashMap<String, String>,
) -> Option<String> {
    let arns = requested_target_group_arns(load_balancers);
    for arn in &arns {
        if !target_types.contains_key(arn) {
            return Some(format!(
                "The target group with targetGroupArn {arn} does not exist"
            ));
        }
    }
    network_mode.and_then(|mode| target_group_type_refusal(&arns, mode, target_types))
}

/// The compatibility half of [`load_balancer_refusal`]: the refusal the target groups in
/// `arns` earn against a revision whose `networkMode` is `network_mode`, over those the
/// ELBv2 store still holds. `None` when every one of them carries the target type
/// [`required_target_type`] names for that mode.
pub fn target_group_type_refusal(
    arns: &[String],
    network_mode: &str,
    target_types: &HashMap<String, String>,
) -> Option<String> {
    let required = required_target_type(network_mode);
    for arn in arns {
        let Some(target_type) = target_types.get(arn) else {
            continue;
        };
        if target_type != required {
            // The form Amazon ECS words this refusal in: ONE sentence, composed from the
            // request and the resolved record — the target group the entry named, the
            // target type that group has, and the network mode the revision declares — and
            // it ends there. The target type the mode does register its tasks as is used to
            // DECIDE the refusal, not reported in it: the service names what is wrong, and
            // appending advice of our own would put a sentence on the wire that Amazon ECS
            // never emits.
            return Some(format!(
                "The provided target group {arn} has target type {target_type}, which is \
                 incompatible with the {network_mode} network mode specified in the task \
                 definition."
            ));
        }
    }
    None
}

/// The Cloud Map namespace each of `references` names, as the IDENTITY two references of the
/// SAME namespace share: the id of the namespace record that service holds for it, and
/// `None` for a reference it holds no namespace for.
///
/// A Cloud Map namespace is not an Amazon ECS resource: `ServiceConnectConfiguration
/// $namespace` is "The namespace name or full Amazon Resource Name (ARN) of the Cloud Map
/// namespace", and the id an ARN ends with is a third spelling of the same thing — so what
/// decides whether two references name ONE namespace is the record they resolve to and not
/// the text they are written in. Read from THAT service's own store through the one shared
/// engine, so a namespace created over its API is seen here with no propagation step: a name
/// is matched against the namespace's own name and against the `HttpName` its instances are
/// discovered under, and an ARN or a bare id against the id the record is held by
/// ([`namespace_key`]).
///
/// `None` — the whole answer — when that store is unreachable (the engine is not the shared
/// in-memory one), which a handler answers with the same modelled server error it uses for
/// its own store being unreachable.
///
/// LOCK ORDER: this takes the Cloud Map store's lock, holds no other while it does, and
/// hands back an owned value — so it is called with NO guard of this service's store held,
/// which is what keeps a lock cycle between the two stores unreachable.
pub fn cloud_map_namespace_ids<Eng: Send + Sync + 'static>(
    engine: &Eng,
    references: impl IntoIterator<Item = String>,
) -> Option<HashMap<String, Option<String>>> {
    let state =
        ::service_com_amazonaws_servicediscovery_route53autonaming_v20170314::state::service_state(
            engine,
        )?;
    let store = state.lock();
    Some(
        references
            .into_iter()
            .map(|reference| {
                let key = namespace_key(&reference);
                let by_id = store.namespaces.get(key).map(|held| held.id.clone());
                let resolved = by_id.or_else(|| {
                    store
                        .namespaces
                        .values()
                        .find(|held| {
                            held.name == key || held.http_name.as_deref() == Some(key)
                        })
                        .map(|held| held.id.clone())
                });
                (reference, resolved)
            })
            .collect(),
    )
}

/// The identity a namespace reference is COMPARED by: the id
/// [`cloud_map_namespace_ids`] resolved it to, and otherwise the reference's own key
/// ([`namespace_key`]).
///
/// The fallback is what keeps two references to a namespace this emulator holds no record of
/// — one a service stored and one a listing asks for — grouping together when they are
/// written the same way, rather than matching nothing at all.
pub fn namespace_identity(reference: &str, resolved: &HashMap<String, Option<String>>) -> String {
    resolved
        .get(reference)
        .cloned()
        .flatten()
        .unwrap_or_else(|| namespace_key(reference).to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcsAmazonec2containerservicev20141113State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcsAmazonec2containerservicev20141113State>)
}
