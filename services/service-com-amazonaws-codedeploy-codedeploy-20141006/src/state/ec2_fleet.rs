// SPDX-License-Identifier: CC0-1.0

//! The Amazon EC2 instances a deployment group's EC2 tag selectors SELECT,
//! resolved against the service that owns instances.
//!
//! `ec2TagFilters` is "the Amazon EC2 tags on which to filter. The deployment
//! group includes Amazon EC2 instances with any of the specified tags"
//! (`smithy_query(query="doc",shape="CreateDeploymentGroupInput",member="ec2TagFilters")`),
//! and an `ec2TagSet` is "a list that contains other lists of Amazon EC2 instance
//! tag groups. For an instance to be included in the deployment group, it must be
//! identified by all of the tag groups in the list"
//! (`smithy_query(query="shape",shape="EC2TagSet")`) — so both members describe
//! instances of ANOTHER service: the fleet belongs to Amazon EC2, and that service
//! is the one answer to "which instances of this account carry these tags".
//!
//! CodeDeploy therefore RESOLVES the fleet when a deployment is created, over
//! Amazon EC2's own modelled `DescribeInstances` through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so a deployment's
//! targets are the very instances `RunInstances` launched and nothing is cached: an
//! instance launched a moment ago is deployed to at once, and one that is no longer
//! running is not. Nothing leaves the process: the read goes over the in-process
//! callback, never a socket.
//!
//! Only RUNNING instances are candidates — a deployment installs a revision on a
//! host and runs the agent's lifecycle event scripts on it, which an instance that
//! is not running does not do — so the read asks Amazon EC2 for exactly those
//! (`instance-state-name`, the filter its own `DescribeInstances` documents for an
//! instance's state) in the Region the request was signed for.
//!
//! What a read that could not be made means: the reading
//! `services/common/src/network.rs` states for a gate on another service's records
//! applies here too — a hop that could not be made at all is evidence of NOTHING,
//! so it resolves no instance and says so at `warn`, and the deployment then
//! reports the fleet it could resolve (none, for a group whose only selector is an
//! EC2 one) rather than instances nobody confirmed.

use ::smithy_server_runtime::{EmbeddedEmulator, OperationContext};

use crate::ops::types::{EC2TagFilter, Tag, TagFilter};

use super::DeploymentGroup;

/// Amazon EC2's endpoint prefix, which is what the in-process address of the read
/// is built from.
const EC2_ENDPOINT_PREFIX: &str = "ec2";

/// The `DescribeInstances` filter that selects instances by the state they are in,
/// and the one state a deployment can be carried out on.
///
/// Both come from that operation's own `Filters` documentation
/// (`instance-state-name` - "the state of the instance (pending | running |
/// shutting-down | terminated | stopping | stopped)").
const INSTANCE_STATE_FILTER: &str = "instance-state-name";
/// See [`INSTANCE_STATE_FILTER`].
const INSTANCE_STATE_RUNNING: &str = "running";

/// How many instances ONE resolution reads, which is also the page size it asks
/// for.
///
/// Neither `ec2TagFilters` nor `ec2TagSet` carries a modelled length, and the
/// account's fleet is not something a CodeDeploy request chooses, so this is THIS
/// EMULATOR's own bound on the work one `CreateDeployment` costs — not an AWS
/// quota — and it is far above the fleet any emulated deployment group is wired
/// with.
pub const MAX_INSTANCES_RESOLVED: usize = 500;

/// One Amazon EC2 instance of the account, as the owner reports it: the id a
/// deployment target is named by, and the tags its group's selectors are matched
/// against.
#[derive(Debug, Clone)]
pub struct Ec2Instance {
    /// The instance id, which is the id CodeDeploy reports an EC2 deployment
    /// target under
    /// (`smithy_query(query="doc",shape="BatchGetDeploymentTargetsInput",member="targetIds")`).
    pub instance_id: String,
    /// The tags the owner reports on it, in the form this service's own tag
    /// filters are matched against.
    pub tags: Vec<Tag>,
}

/// The EC2 tag selectors ONE deployment group carries: its `ec2TagFilters`, else
/// the groups of its `ec2TagSet`.
///
/// Cloned out of the stored group so the resolution below can be made with this
/// service's store UNLOCKED — the two stores are never held at once — exactly as a
/// group's `ecsServices` are loaded ([`super::ecs_services`]).
#[derive(Debug, Clone, Default)]
pub struct Ec2Selectors {
    /// An instance carrying ANY of these tags is selected.
    filters: Vec<EC2TagFilter>,
    /// An instance carrying at least one tag from EVERY one of these groups is
    /// selected.
    tag_set: Vec<Vec<EC2TagFilter>>,
}

impl Ec2Selectors {
    /// Whether this group selects any Amazon EC2 instance at all, which is what
    /// decides whether a deployment reads that service's fleet.
    #[must_use]
    pub fn selects_anything(&self) -> bool {
        !self.filters.is_empty() || !self.tag_set.is_empty()
    }

    /// Whether these selectors take in an instance carrying `tags`.
    ///
    /// The two forms work as the members document them and as the on-premises
    /// selectors of the same group do ([`super::deployment_targets`]): a FILTER
    /// LIST takes in an instance matching ANY of its filters, while a TAG SET
    /// takes in one matching at least one filter in EVERY one of its groups. Each
    /// filter is matched by the one tag-filter matcher this service has
    /// ([`super::tag_filter_matches`]), so `EC2TagFilterType` and `TagFilterType`
    /// — the same three values, KEY_ONLY | VALUE_ONLY | KEY_AND_VALUE — cannot be
    /// read two ways.
    #[must_use]
    fn selects(&self, tags: &[Tag]) -> bool {
        let by_filters = self
            .filters
            .iter()
            .any(|filter| matches(filter, tags));
        let by_tag_set = !self.tag_set.is_empty()
            && self
                .tag_set
                .iter()
                .all(|group| group.iter().any(|filter| matches(filter, tags)));
        by_filters || by_tag_set
    }
}

/// Whether `tags` satisfies one EC2 tag filter; see [`Ec2Selectors::selects`].
fn matches(filter: &EC2TagFilter, tags: &[Tag]) -> bool {
    super::tag_filter_matches(
        &TagFilter {
            key: filter.key.clone(),
            value: filter.value.clone(),
            r#type: filter.r#type.clone(),
        },
        tags,
    )
}

/// The EC2 tag selectors `group` carries, for a group on a compute platform whose
/// targets are instances at all.
///
/// A group of the Lambda or Amazon ECS platform names its targets some other way
/// (a function inside the revision, an `ecsServices` pair), so no EC2 fleet is read
/// for one.
#[must_use]
pub fn selectors_of(group: &DeploymentGroup) -> Ec2Selectors {
    if matches!(group.compute_platform.as_str(), "Lambda" | "ECS") {
        return Ec2Selectors::default();
    }
    Ec2Selectors {
        filters: group.ec2_tag_filters.clone(),
        tag_set: group
            .ec2_tag_set
            .as_ref()
            .and_then(|set| set.ec2_tag_set_list.clone())
            .unwrap_or_default(),
    }
}

/// The RUNNING Amazon EC2 instances of this account that `selectors` select, in
/// the order the owner reports them.
///
/// Empty for a group that selects no instance, and empty when Amazon EC2 could not
/// be asked at all — see this module's note on what silence means. Bounded by
/// [`MAX_INSTANCES_RESOLVED`] rather than by the account's fleet.
///
/// Call it from a handler with no store guard held: the read reaches Amazon EC2's
/// own handler, which takes that service's lock.
pub async fn selected_instances(
    ctx: &OperationContext<'_>,
    region: &str,
    selectors: &Ec2Selectors,
) -> Vec<Ec2Instance> {
    if !selectors.selects_anything() {
        return Vec::new();
    }
    running_instances(ctx, region)
        .await
        .into_iter()
        .filter(|instance| selectors.selects(&instance.tags))
        .collect()
}

/// The running instances of the account in `region`, as the service that owns them
/// reports them.
async fn running_instances(ctx: &OperationContext<'_>, region: &str) -> Vec<Ec2Instance> {
    let running = ::aws_sdk_ec2::types::Filter::builder()
        .name(INSTANCE_STATE_FILTER)
        .values(INSTANCE_STATE_RUNNING)
        .build();
    let page_size = i32::try_from(MAX_INSTANCES_RESOLVED).unwrap_or(i32::MAX);
    let answered = ec2_client(ctx.callback(), region)
        .describe_instances()
        .filters(running)
        .max_results(page_size)
        .send()
        .await;
    let output = match answered {
        Ok(output) => output,
        Err(error) => {
            // At `warn` for the reason an unanswered `ecsServices` load is logged
            // there: past this point a deployment resolves no EC2 target, so it
            // ends up reporting that it found nothing to deploy to, and that is
            // worth seeing.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Amazon EC2 instances a deployment group selects could not be resolved"
            );
            return Vec::new();
        }
    };
    output
        .reservations()
        .iter()
        .flat_map(::aws_sdk_ec2::types::Reservation::instances)
        .filter_map(|instance| {
            Some(Ec2Instance {
                instance_id: instance.instance_id()?.to_string(),
                tags: instance
                    .tags()
                    .iter()
                    .map(|tag| Tag {
                        key: tag.key().map(str::to_string),
                        value: tag.value().map(str::to_string),
                    })
                    .collect(),
            })
        })
        .take(MAX_INSTANCES_RESOLVED)
        .collect()
}

/// Amazon EC2's client for this read: the IN-PROCESS callback into this same
/// emulator, so the fleet is resolved against the records that service's own
/// handlers wrote.
///
/// The `endpoint_url` is an ADDRESS the in-process router reads, never a host any
/// resolver looks up, and `region` is the one the request being served was signed
/// for, so a caller cannot steer that authority. There is nothing to retry, time
/// out, cache an identity for or watch for a stalled stream over that transport —
/// each would need a sleep implementation this client is not given — and the
/// credentials the callback hands over are the emulator's own, which only sign this
/// operation's in-process calls and are never put in a response or a log.
fn ec2_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{EC2_ENDPOINT_PREFIX}.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    //! WHICH instances a group's EC2 selectors take in, over the forms the two
    //! members state a selection in. That the instances themselves come from the
    //! service that owns them is asserted end to end, through the operations a
    //! client calls, in
    //! `core/router/tests/all/codedeploy_minimum_healthy_hosts_paces_instance_targets.rs`.

    use super::*;
    use crate::ops::types::EC2TagSet;

    /// One EC2 tag filter.
    fn filter(key: Option<&str>, value: Option<&str>, kind: Option<&str>) -> EC2TagFilter {
        EC2TagFilter {
            key: key.map(str::to_string),
            value: value.map(str::to_string),
            r#type: kind.map(str::to_string),
        }
    }

    /// The tags an instance carries.
    fn tags(pairs: &[(&str, &str)]) -> Vec<Tag> {
        pairs
            .iter()
            .map(|(key, value)| Tag {
                key: Some((*key).to_string()),
                value: Some((*value).to_string()),
            })
            .collect()
    }

    /// A deployment group on `platform` carrying `filters` and `tag_set`.
    fn group(
        platform: &str,
        filters: Vec<EC2TagFilter>,
        tag_set: Option<EC2TagSet>,
    ) -> DeploymentGroup {
        DeploymentGroup {
            deployment_group_id: "dgid".to_string(),
            deployment_group_name: "dg".to_string(),
            application_name: "app".to_string(),
            service_role_arn: "arn:aws:iam::123456789012:role/CodeDeployRole".to_string(),
            deployment_config_name: "CodeDeployDefault.OneAtATime".to_string(),
            compute_platform: platform.to_string(),
            ec2_tag_filters: filters,
            on_premises_instance_tag_filters: Vec::new(),
            auto_scaling_groups: Vec::new(),
            trigger_configurations: Vec::new(),
            alarm_configuration: None,
            auto_rollback_configuration: None,
            outdated_instances_strategy: None,
            deployment_style: None,
            blue_green_deployment_configuration: None,
            load_balancer_info: None,
            ec2_tag_set: tag_set,
            on_premises_tag_set: None,
            ecs_services: Vec::new(),
            ec2_deployment_options: None,
            termination_hook_enabled: false,
            tags: Vec::new(),
            create_time_ms: 0,
            region: "us-east-1".to_string(),
        }
    }

    /// RULE: a filter list takes in an instance matching ANY of its filters, a tag
    /// set one matching a filter in EVERY group, and a group with neither takes in
    /// nothing.
    #[test]
    fn the_selectors_take_in_the_instances_their_tags_identify() {
        let prod_web = tags(&[("Env", "prod"), ("Role", "web")]);
        let prod_only = tags(&[("Env", "prod")]);
        let untagged: Vec<Tag> = Vec::new();
        // Rows differ along the selector form, the filter type, how many filters
        // or groups it states, and the tags the instance carries.
        let cases: &[(&str, DeploymentGroup, &Vec<Tag>, bool)] = &[
            (
                "a KEY_AND_VALUE filter the instance satisfies",
                group(
                    "Server",
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    None,
                ),
                &prod_web,
                true,
            ),
            (
                "a KEY_AND_VALUE filter on another value",
                group(
                    "Server",
                    vec![filter(Some("Env"), Some("staging"), Some("KEY_AND_VALUE"))],
                    None,
                ),
                &prod_web,
                false,
            ),
            (
                "a KEY_ONLY filter, whose value is immaterial",
                group(
                    "Server",
                    vec![filter(Some("Role"), Some("ignored"), Some("KEY_ONLY"))],
                    None,
                ),
                &prod_web,
                true,
            ),
            (
                "a VALUE_ONLY filter, whose key is immaterial",
                group(
                    "Server",
                    vec![filter(Some("ignored"), Some("web"), Some("VALUE_ONLY"))],
                    None,
                ),
                &prod_web,
                true,
            ),
            (
                "two filters, the second of which the instance satisfies",
                group(
                    "Server",
                    vec![
                        filter(Some("Role"), Some("api"), Some("KEY_AND_VALUE")),
                        filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE")),
                    ],
                    None,
                ),
                &prod_only,
                true,
            ),
            (
                "a tag set whose every group the instance satisfies",
                group(
                    "Server",
                    Vec::new(),
                    Some(EC2TagSet {
                        ec2_tag_set_list: Some(vec![
                            vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                            vec![filter(Some("Role"), Some("web"), Some("KEY_AND_VALUE"))],
                        ]),
                    }),
                ),
                &prod_web,
                true,
            ),
            (
                "a tag set with a group the instance does not satisfy",
                group(
                    "Server",
                    Vec::new(),
                    Some(EC2TagSet {
                        ec2_tag_set_list: Some(vec![
                            vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                            vec![filter(Some("Role"), Some("web"), Some("KEY_AND_VALUE"))],
                        ]),
                    }),
                ),
                &prod_only,
                false,
            ),
            (
                "a selector against an instance carrying no tag",
                group(
                    "Server",
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    None,
                ),
                &untagged,
                false,
            ),
            (
                "a group carrying no EC2 selector at all",
                group("Server", Vec::new(), None),
                &prod_web,
                false,
            ),
            (
                "an EC2 selector on a platform whose targets are not instances",
                group(
                    "Lambda",
                    vec![filter(Some("Env"), Some("prod"), Some("KEY_AND_VALUE"))],
                    None,
                ),
                &prod_web,
                false,
            ),
        ];

        for (case, group, instance_tags, expected) in cases {
            let selectors = selectors_of(group);
            let got = selectors.selects_anything() && selectors.selects(instance_tags);
            assert_eq!(
                got, *expected,
                "case {case:?}: expected the instance to be selected = {expected}, got {got}"
            );
        }
    }
}
