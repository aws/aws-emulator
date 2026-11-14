// SPDX-License-Identifier: CC0-1.0

//! What the compute target a request names IS — whether it exists in the
//! caller's Region, and which databases it was provisioned with: facts this
//! service RESOLVES rather than holds.
//!
//! The Data API runs a statement on a data warehouse it does not own. A
//! provisioned cluster is a resource of Amazon Redshift and a serverless
//! workgroup one of Amazon Redshift Serverless, so whether there is such an
//! endpoint in the Region the request addressed is a question only those
//! services can answer — and it is asked the way any emulated service reaches
//! another: the AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
//! no other service's crate.
//!
//! Nothing is invented and nothing is assumed from silence. A control plane that
//! answers that it holds no such cluster or workgroup settles the question
//! ([`TargetResolution::Absent`]); one that cannot be asked at all — an emulator
//! with no Redshift control plane mounted knows of no cluster — leaves it
//! UNDECIDED ([`TargetResolution::Unresolvable`]), and a submit admits the
//! statement rather than refusing it on the strength of a peer that did not
//! answer. The databases a warehouse was created with are read the same way: an
//! owner that was not asked reports none, so none is claimed
//! ([`TargetFacts::provisioned_databases`]).

use super::ConnectionTarget;

/// The Region a request is resolved in when it carries no SigV4 credential
/// scope that names one. Matches the Region the emulator's own clients default
/// to.
const DEFAULT_REGION: &str = "us-east-1";

/// How many workgroups each `ListWorkgroups` page asks for — the modelled
/// maximum (`ListWorkgroupsRequest$maxResults`, range 1..100), so the walk below
/// makes as few calls as it can.
const WORKGROUP_PAGE_SIZE: i32 = 100;

/// How many pages that walk reads before giving up. The cursor comes from
/// Redshift Serverless rather than from this service, so the loop is bounded
/// here: a peer that kept handing out a token can never spin this handler.
const MAX_WORKGROUP_PAGES: usize = 64;

/// What asking the owning control plane about one compute target found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetResolution {
    /// The target names a cluster or workgroup that exists in the Region.
    Exists,
    /// The owning control plane was asked and holds no such cluster or
    /// workgroup, so there is no endpoint for the statement to run on.
    Absent,
    /// The owning control plane could not be asked, so whether the target
    /// exists is UNKNOWN. That is this emulator failing to resolve a fact about
    /// itself, not a caller naming something that does not exist, so the two are
    /// never answered the same way.
    Unresolvable,
}

/// Everything the control plane that OWNS one compute target answers about it.
///
/// A data warehouse is CREATED with a database: a Redshift Serverless workgroup
/// serves the database its namespace was created with (`dbName`) and a
/// provisioned cluster the one its create named (`DBName`). That database is a
/// real database of the warehouse from the moment the warehouse exists, before
/// any statement has run on it, so the Data API's catalogue has to ask the owner
/// for it rather than report only what statements here have built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetFacts {
    /// Whether the target exists in the Region at all.
    pub resolution: TargetResolution,
    /// The databases the owner reports the warehouse was provisioned with, in
    /// the order the owner reports them. Empty for a target that does not exist
    /// and for one whose owner could not be asked — nothing is invented from
    /// silence.
    pub provisioned_databases: Vec<String>,
    /// The ARN of the warehouse itself — the address a statement that ran on it
    /// is reported against. A provisioned cluster's ARN is a documented function
    /// of its identifier, Region and account ([`cluster_arn`]); a serverless
    /// workgroup's is minted by the control plane that owns it, so it is READ
    /// from that owner and is none when the owner could not be asked and the
    /// request did not name the workgroup by ARN.
    pub arn: Option<String>,
}

impl TargetFacts {
    /// A target the owner answered about in `resolution` and provisioned with no
    /// database this service could read.
    fn without_databases(resolution: TargetResolution) -> Self {
        Self {
            resolution,
            provisioned_databases: Vec::new(),
            arn: None,
        }
    }

    /// The same facts, carrying the warehouse's own ARN.
    fn with_arn(mut self, arn: Option<String>) -> Self {
        self.arn = arn;
        self
    }

    /// Whether the warehouse was provisioned with a database of this name.
    pub fn provisions_database(&self, database: &str) -> bool {
        self.provisioned_databases
            .iter()
            .any(|provisioned| provisioned == database)
    }

    /// Whether the owner's answer DECIDES that the warehouse has no database of
    /// this name — the question a statement's `Database` is resolved against.
    ///
    /// Only an owner that answered can decide it: a target whose owner could not
    /// be asked, and one whose owner reported no database at all, leaves the
    /// catalogue UNKNOWN, and a statement is admitted rather than failed on the
    /// strength of a peer that did not answer. `held` are the databases this
    /// service's own catalogue of the warehouse names
    /// ([`ComAmazonawsRedshiftdataRedshiftdataState::database_names`]), which are
    /// databases of it too: a caller that created one may run statements in it.
    pub fn decides_database_absent(&self, held: &[String], database: &str) -> bool {
        self.resolution == TargetResolution::Exists
            && !self.provisioned_databases.is_empty()
            && !catalogue_databases(held.to_vec(), &self.provisioned_databases)
                .iter()
                .any(|name| name == database)
    }
}

/// The ARN of a provisioned cluster:
/// `arn:aws:redshift:<region>:<account>:cluster:<identifier>`.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Constructing an Amazon
/// Redshift Amazon Resource Name (ARN)" —
/// `arn:aws:redshift:region:account-id:cluster:cluster-name`, which is also the
/// example the Redshift model's own `CreateTagsMessage$ResourceName` carries
/// ("arn:aws:redshift:us-east-2:123456789:cluster:t1"). A pure function of the
/// identifier, the Region and this emulator's account, so the Data API names a
/// cluster by the same ARN the service that owns it does.
pub fn cluster_arn(identifier: &str, region: &str) -> String {
    ::engine::emulator_arn("redshift", region, &format!("cluster:{identifier}"))
}

/// The message a connection to a database the warehouse has none of fails with.
///
/// The Data API accepts the submission and reports this as the statement's
/// `Error`: the backend the statement was handed to is what answers, and it
/// answers with the fatal line it ends the connection attempt on, naming the
/// database the request asked for.
pub fn absent_database_message(database: &str) -> String {
    format!("FATAL: database \"{database}\" does not exist")
}

/// Every database of one warehouse the Data API's catalogue holds: the databases
/// this emulator opened because a statement ran on them (`held`), together with
/// the databases the warehouse's own control plane provisioned it with
/// (`provisioned`) — in name order, each name once.
///
/// One place, so that the operations which LIST the catalogue and those which
/// resolve one database against it cannot disagree about which databases a
/// warehouse has.
pub fn catalogue_databases(held: Vec<String>, provisioned: &[String]) -> Vec<String> {
    let mut names = held;
    names.extend(provisioned.iter().cloned());
    names.sort();
    names.dedup();
    names
}

/// The Region this request was signed for, which is the Region its compute
/// target is resolved in. Falls back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Amazon Redshift client a provisioned cluster is resolved through.
///
/// `region` is the request's own signing region, so the hop stays in the Region
/// the caller addressed. The client is given the callback's own HTTP client, so
/// the endpoint URL below is only the address that transport routes on IN
/// PROCESS: no socket is opened, no name is resolved and nothing leaves this
/// process. The credentials are the callback's, which sign this in-process hop
/// and never appear in a response.
fn redshift(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_redshift::Client {
    let config = ::aws_sdk_redshift::Config::builder()
        .behavior_version(::aws_sdk_redshift::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_redshift::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://redshift.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials the callback hands over are used directly.
        .retry_config(::aws_sdk_redshift::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_redshift::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_redshift::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_redshift::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_redshift::Client::from_conf(config)
}

/// The Amazon Redshift Serverless client a workgroup is resolved through,
/// configured exactly as [`redshift`] is and for the same reasons.
fn redshift_serverless(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_redshiftserverless::Client {
    let config = ::aws_sdk_redshiftserverless::Config::builder()
        .behavior_version(::aws_sdk_redshiftserverless::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_redshiftserverless::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!(
            "https://redshift-serverless.{region}.amazonaws.com"
        ))
        .retry_config(::aws_sdk_redshiftserverless::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_redshiftserverless::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_redshiftserverless::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_redshiftserverless::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_redshiftserverless::Client::from_conf(config)
}

/// The parameter values the WAREHOUSE's engine is running with: the values its
/// own control plane reports as APPLIED to it.
///
/// A provisioned cluster's parameters live in the cluster parameter group it is
/// configured with, and each of them carries the status of the change the
/// cluster has seen: a parameter whose change is "`pending-reboot`: the
/// parameter value will be applied after the cluster reboots"
/// (`ClusterParameterStatus$ParameterApplyStatus`) has NOT reached the engine,
/// so it is left out and the engine's own default stands for it until a restart
/// applies it — which is exactly what makes a static parameter change
/// observable only after a reboot. A serverless workgroup carries its settings
/// on itself (`Workgroup$configParameters`), and they are in force as the
/// workgroup reports them.
///
/// Empty for a target whose owner could not be asked, so nothing is claimed
/// from silence: the published defaults then stand for every parameter
/// ([`super::session_settings`]).
pub async fn engine_settings(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &ConnectionTarget,
) -> std::collections::BTreeMap<String, String> {
    let region = request_region(ctx);
    match target {
        ConnectionTarget::Cluster(identifier) => cluster_settings(ctx, &region, identifier).await,
        ConnectionTarget::Workgroup(name) => workgroup_settings(ctx, &region, name).await,
    }
}

/// The parameters one provisioned cluster's engine is running with.
async fn cluster_settings(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
) -> std::collections::BTreeMap<String, String> {
    let client = redshift(ctx, region);
    let Ok(described) = client
        .describe_clusters()
        .cluster_identifier(identifier)
        .send()
        .await
    else {
        return std::collections::BTreeMap::new();
    };
    let mut settings = std::collections::BTreeMap::new();
    for cluster in described.clusters() {
        for membership in cluster.cluster_parameter_groups() {
            let Some(group) = membership.parameter_group_name() else {
                continue;
            };
            // The parameters this cluster has not seen the change of yet.
            let pending: Vec<&str> = membership
                .cluster_parameter_status_list()
                .iter()
                .filter(|status| {
                    status.parameter_apply_status() == Some(PENDING_REBOOT_APPLY_STATUS)
                })
                .filter_map(::aws_sdk_redshift::types::ClusterParameterStatus::parameter_name)
                .collect();
            let Ok(parameters) = client
                .describe_cluster_parameters()
                .parameter_group_name(group)
                .max_records(PARAMETER_PAGE_SIZE)
                .send()
                .await
            else {
                continue;
            };
            for parameter in parameters.parameters() {
                let (Some(name), Some(value)) =
                    (parameter.parameter_name(), parameter.parameter_value())
                else {
                    continue;
                };
                if pending.contains(&name) {
                    continue;
                }
                settings.insert(name.to_ascii_lowercase(), value.to_string());
            }
        }
    }
    settings
}

/// The parameters one serverless workgroup's engine is running with.
async fn workgroup_settings(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> std::collections::BTreeMap<String, String> {
    let Ok(described) = redshift_serverless(ctx, region)
        .get_workgroup()
        .workgroup_name(name)
        .send()
        .await
    else {
        return std::collections::BTreeMap::new();
    };
    described
        .workgroup()
        .map(|workgroup| {
            workgroup
                .config_parameters()
                .iter()
                .filter_map(|parameter| {
                    Some((
                        parameter.parameter_key()?.to_ascii_lowercase(),
                        parameter.parameter_value()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The apply status of a parameter change the cluster has not seen yet.
const PENDING_REBOOT_APPLY_STATUS: &str = "pending-reboot";

/// How many parameters one page of a group's parameters asks for — the modelled
/// maximum of `DescribeClusterParametersMessage$MaxRecords` (range 20..100), so
/// one call reads as much of the group as a page can hold.
const PARAMETER_PAGE_SIZE: i32 = 100;

/// Resolve `target` against the control plane that owns it.
///
/// Asked BEFORE a submit admits a statement: a statement runs on an endpoint, so
/// a target that resolves to none is a request the service cannot serve. The
/// answer is the owner's, never this service's own state — the Data API holds no
/// cluster and no workgroup of its own.
pub async fn resolve_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &ConnectionTarget,
) -> TargetResolution {
    describe_target(ctx, target).await.resolution
}

/// Ask the control plane that owns `target` everything this service needs to
/// know about it: whether it exists, and the databases it was provisioned with.
///
/// One round of questions per request, so an operation that both admits a
/// request on the target's existence and reads its catalogue asks the owner
/// once.
pub async fn describe_target(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &ConnectionTarget,
) -> TargetFacts {
    let region = request_region(ctx);
    match target {
        ConnectionTarget::Cluster(identifier) => describe_cluster(ctx, &region, identifier).await,
        ConnectionTarget::Workgroup(name) => describe_workgroup(ctx, &region, name).await,
    }
}

/// Resolve one `ClusterIdentifier` against Amazon Redshift.
///
/// `DescribeClusters` answers `ClusterNotFoundFault` for an identifier that
/// "does not refer to an existing cluster", which is exactly this question; any
/// OTHER failure says nothing about whether the cluster exists, so it leaves the
/// question undecided.
/// Its `DBName` — "the name of the initial database that was created when the
/// cluster was created" — is the database the cluster holds from the moment it
/// exists, so it is reported beside the resolution.
async fn describe_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    identifier: &str,
) -> TargetFacts {
    let answer = redshift(ctx, region)
        .describe_clusters()
        .cluster_identifier(identifier)
        .send()
        .await;
    // A cluster's ARN is a documented function of its identifier, Region and
    // account, so it is the same string whatever the owner answers.
    let arn = Some(cluster_arn(identifier, region));
    match answer {
        Ok(answer) => TargetFacts {
            resolution: TargetResolution::Exists,
            provisioned_databases: answer
                .clusters()
                .iter()
                .filter_map(::aws_sdk_redshift::types::Cluster::db_name)
                .map(str::to_string)
                .collect(),
            arn,
        },
        Err(error) => {
            if error
                .as_service_error()
                .is_some_and(::aws_sdk_redshift::operation::describe_clusters::DescribeClustersError::is_cluster_not_found_fault)
            {
                return TargetFacts::without_databases(TargetResolution::Absent).with_arn(arn);
            }
            // Visible at default verbosity: a statement admitted because the
            // owner could not be asked reads exactly like one admitted because
            // the cluster is really there, so the degraded path says so rather
            // than passing quietly.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the cluster a statement names could not be resolved"
            );
            TargetFacts::without_databases(TargetResolution::Unresolvable).with_arn(arn)
        }
    }
}

/// Resolve one `WorkgroupName` against Amazon Redshift Serverless, and read the
/// databases of the NAMESPACE it serves.
///
/// A workgroup is compute; its databases belong to the namespace it is attached
/// to, whose `dbName` is "the name of the first database created in the
/// namespace" — so the workgroup is resolved first and its namespace then asked
/// for that database.
async fn describe_workgroup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> TargetFacts {
    let resolved = resolve_workgroup(ctx, region, name).await;
    // A workgroup's ARN is the serverless control plane's own, so it is taken
    // from that owner's answer; a request that named the workgroup BY its ARN
    // already carries it, which is what a target the owner could not be asked
    // about is still addressable by.
    let arn = resolved
        .arn
        .clone()
        .or_else(|| name.starts_with("arn:").then(|| name.to_string()));
    if resolved.resolution != TargetResolution::Exists {
        return TargetFacts::without_databases(resolved.resolution).with_arn(arn);
    }
    let provisioned_databases = match resolved.namespace {
        Some(namespace) => namespace_databases(ctx, region, &namespace).await,
        None => Vec::new(),
    };
    TargetFacts {
        resolution: resolved.resolution,
        provisioned_databases,
        arn,
    }
}

/// What asking Amazon Redshift Serverless about one workgroup found: whether it
/// exists, the namespace it serves, and its own ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedWorkgroup {
    resolution: TargetResolution,
    namespace: Option<String>,
    arn: Option<String>,
}

impl ResolvedWorkgroup {
    /// A workgroup the owner answered about in `resolution` and reported nothing
    /// else of.
    fn unresolved(resolution: TargetResolution) -> Self {
        Self {
            resolution,
            namespace: None,
            arn: None,
        }
    }
}

/// Resolve one `WorkgroupName` against Amazon Redshift Serverless, and answer
/// the namespace the workgroup belongs to when it resolves.
///
/// `WorkgroupNameString` admits the workgroup's NAME or its ARN, and the
/// serverless control plane addresses a workgroup by name — so a name is asked
/// for directly (`GetWorkgroup`, whose `ResourceNotFoundException` settles the
/// question) and an ARN is matched against the workgroups the Region holds.
async fn resolve_workgroup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> ResolvedWorkgroup {
    if name.starts_with("arn:") {
        return resolve_workgroup_arn(ctx, region, name).await;
    }
    let answer = redshift_serverless(ctx, region)
        .get_workgroup()
        .workgroup_name(name)
        .send()
        .await;
    match answer {
        Ok(answer) => ResolvedWorkgroup {
            resolution: TargetResolution::Exists,
            namespace: answer
                .workgroup()
                .and_then(::aws_sdk_redshiftserverless::types::Workgroup::namespace_name)
                .map(str::to_string),
            arn: answer
                .workgroup()
                .and_then(::aws_sdk_redshiftserverless::types::Workgroup::workgroup_arn)
                .map(str::to_string),
        },
        Err(error) => {
            if error
                .as_service_error()
                .is_some_and(::aws_sdk_redshiftserverless::operation::get_workgroup::GetWorkgroupError::is_resource_not_found_exception)
            {
                return ResolvedWorkgroup::unresolved(TargetResolution::Absent);
            }
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the workgroup a statement names could not be resolved"
            );
            ResolvedWorkgroup::unresolved(TargetResolution::Unresolvable)
        }
    }
}

/// The databases one serverless NAMESPACE was created with: its `dbName`, read
/// from the control plane that owns the namespace. Empty when that control plane
/// could not be asked — a database is never invented from silence.
async fn namespace_databases(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    namespace: &str,
) -> Vec<String> {
    let answer = redshift_serverless(ctx, region)
        .get_namespace()
        .namespace_name(namespace)
        .send()
        .await;
    match answer {
        Ok(answer) => answer
            .namespace()
            .and_then(::aws_sdk_redshiftserverless::types::Namespace::db_name)
            .map(str::to_string)
            .into_iter()
            .collect(),
        Err(error) => {
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the namespace a workgroup serves could not be read, so the databases it was \
                 created with are unknown"
            );
            Vec::new()
        }
    }
}

/// Resolve a workgroup named by ARN: the Region's workgroups are walked and
/// their own ARNs compared, so the ARN form and the name form resolve to the
/// same workgroup — and to the same namespace. A walk that cannot be completed
/// leaves the question undecided, exactly as an unanswerable `GetWorkgroup`
/// does.
async fn resolve_workgroup_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> ResolvedWorkgroup {
    let client = redshift_serverless(ctx, region);
    let mut token: Option<String> = None;
    for page_number in 0..MAX_WORKGROUP_PAGES {
        let answer = client
            .list_workgroups()
            .max_results(WORKGROUP_PAGE_SIZE)
            .set_next_token(token)
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the Region's workgroups could not be read, so the workgroup a statement \
                     names by ARN is unresolved"
                );
                return ResolvedWorkgroup::unresolved(TargetResolution::Unresolvable);
            }
        };
        if let Some(workgroup) = page
            .workgroups()
            .iter()
            .find(|workgroup| workgroup.workgroup_arn() == Some(arn))
        {
            return ResolvedWorkgroup {
                resolution: TargetResolution::Exists,
                namespace: workgroup.namespace_name().map(str::to_string),
                arn: workgroup.workgroup_arn().map(str::to_string),
            };
        }
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            return ResolvedWorkgroup::unresolved(TargetResolution::Absent);
        }
        if page_number + 1 == MAX_WORKGROUP_PAGES {
            ::tracing::warn!(
                pages = MAX_WORKGROUP_PAGES,
                "stopped reading workgroups at the page bound, so the workgroup a statement \
                 names by ARN is unresolved"
            );
        }
    }
    ResolvedWorkgroup::unresolved(TargetResolution::Unresolvable)
}
