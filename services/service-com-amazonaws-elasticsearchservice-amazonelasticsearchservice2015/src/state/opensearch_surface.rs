// SPDX-License-Identifier: CC0-1.0

//! The domains the OTHER API surface of this same service holds — the ONE place this
//! surface reaches them, so every read of this surface resolves a domain the same way.
//!
//! A domain is ONE resource of one account and Region, published through TWO API versions:
//! the 2015-01-01 Elasticsearch surface this crate serves and the 2021-01-01 OpenSearch
//! surface (`service-com-amazonaws-opensearch-amazonopensearchservice`). Which API version
//! a client speaks decides the SHAPE it reads a domain in, never whether the domain is
//! there: a domain created with `opensearch:CreateDomain` is described by
//! `es:DescribeElasticsearchDomain`, with the configuration the create recorded — the log
//! publishing options among it — because the two surfaces describe the same resource.
//!
//! So a read of this surface resolves a name in this crate's own store first, and in the
//! OpenSearch surface's store ([`opensearch::service_state`]) when this one holds no domain
//! under it — the same store that surface's own `CreateDomain` writes and its
//! `DescribeDomain` reports, reached through the SAME shared engine the way every service
//! crate of this emulator reaches a store that is not its own. An engine that holds no
//! domain of that surface resolves nothing here, and then this surface answers exactly what
//! it held before.
//!
//! What a read ANSWERS is built from that record's own projection — the `DomainStatus` and
//! `DomainConfig` the other surface reports it through
//! ([`opensearch_service::domain_configuration`]) — restated in the shapes THIS surface's
//! model declares. So the two surfaces cannot disagree about a domain: one record, one
//! projection, and this module only renames it into the 2015-01-01 vocabulary. A member
//! that surface reports and this model does not declare (a dual-stack endpoint, an off-peak
//! window, the AI/ML options) is simply not part of what this surface publishes, and is
//! dropped rather than reported under a name this model does not carry.
//!
//! Reporting a domain does not CHANGE it: the only write is the configuration change the
//! clock already owed the record at the instant of the read
//! ([`opensearch::ComAmazonawsOpensearchAmazonopensearchserviceState::settled_mut`]), which
//! would have fallen due whether or not anybody called — exactly what the other surface's
//! own describe does.

use std::time::SystemTime;

use ::service_com_amazonaws_opensearch_amazonopensearchservice::ops::types as os;
use ::service_com_amazonaws_opensearch_amazonopensearchservice::service as opensearch_service;
use ::service_com_amazonaws_opensearch_amazonopensearchservice::state as opensearch;

use crate::ops::types::*;

/// The prefix the OpenSearch surface spells a release of the ELASTICSEARCH line with
/// (`EngineVersion`'s pattern is `^Elasticsearch_[0-9]\.[0-9]{1,2}$|^OpenSearch_…`), which
/// this surface's own member does not admit.
const ELASTICSEARCH_RELEASE_PREFIX: &str = "Elasticsearch_";

/// The release `version` as THIS surface spells it.
///
/// `ElasticsearchVersionString`'s pattern admits `X.Y`, `OpenSearch_X.Y` and `OS_X.Y`
/// (`smithy_query(query="shape",shape="ElasticsearchVersionString")`): a release of the
/// Elasticsearch line is a bare `X.Y` here and carries the engine's name only on the other
/// surface, so the prefix is dropped — one release, two spellings, and this surface reports
/// the one its own member declares. An OpenSearch release is spelled the same on both.
fn legacy_version(version: &str) -> String {
    version
        .strip_prefix(ELASTICSEARCH_RELEASE_PREFIX)
        .unwrap_or(version)
        .to_string()
}

/// The domain `domain_name` names in `region`, projected by `project`, or `None` when the
/// OpenSearch surface holds no domain of that name in that Region (or is not mounted at
/// all).
///
/// A domain belongs to ONE Region — the one its ARN and endpoint name — so a record held
/// under this name in another Region is no domain of this request's account and Region and
/// resolves nothing here, the way this crate's own store is keyed by Region.
fn resolved<Eng, T, Project>(
    engine: &Eng,
    region: &str,
    domain_name: &str,
    now: SystemTime,
    project: Project,
) -> Option<T>
where
    Eng: Send + Sync + 'static,
    Project: FnOnce(&opensearch::Domain) -> T,
{
    let state = opensearch::service_state(engine)?;
    let mut store = state.lock();
    // The Region is read BEFORE the record is settled, so a name this Region holds no
    // domain under leaves the other surface's store untouched.
    if store
        .domains
        .get(domain_name)
        .is_none_or(|domain| domain.region != region)
    {
        return None;
    }
    let domain = store.settled_mut(domain_name, now)?;
    Some(project(domain))
}

/// The `ElasticsearchDomainStatus` this surface reports for a domain the OpenSearch surface
/// holds — what `DescribeElasticsearchDomain` and `DescribeElasticsearchDomains` answer
/// with for it.
pub fn domain_status<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    domain_name: &str,
    now: SystemTime,
) -> Option<ElasticsearchDomainStatus> {
    resolved(engine, region, domain_name, now, |domain| {
        reported_status(&opensearch_service::domain_configuration::reported_status(
            domain,
        ))
    })
}

/// The `ElasticsearchDomainConfig` this surface reports for one of those domains — what
/// `DescribeElasticsearchDomainConfig` answers with for it, every option wrapped in the
/// status the other surface reports it under.
pub fn domain_config<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    domain_name: &str,
    now: SystemTime,
) -> Option<ElasticsearchDomainConfig> {
    resolved(engine, region, domain_name, now, |domain| {
        reported_config(&opensearch_service::domain_configuration::reported_config(
            domain,
        ))
    })
}

/// The engine release one of those domains runs, as THIS surface spells it
/// ([`legacy_version`]) — `ElasticsearchVersion` here, `EngineVersion` on the other surface
/// — which is the source version its upgrade paths are computed from.
pub fn domain_engine_version<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    domain_name: &str,
    now: SystemTime,
) -> Option<String> {
    resolved(engine, region, domain_name, now, |domain| {
        legacy_version(&domain.engine_version)
    })
}

/// Every domain the OpenSearch surface holds in `region`, as this surface's
/// `ListDomainNames` entry for it: the domain's name and the engine type its release names.
///
/// `DomainInfo$EngineType` is which engine the domain runs, so it is DERIVED from the
/// release the record holds ([`crate::state::engine_type_of`]) — the same reading this
/// surface's own records are reported through, so one listing cannot name the two kinds of
/// domain by different rules. Order-stable: the other surface keys its domains by name in a
/// `BTreeMap`, so two listings report them in the same order.
///
/// Settles nothing: a listing reports which domains exist and under which engine, neither
/// of which a configuration change in flight moves.
pub fn domain_names<Eng: Send + Sync + 'static>(engine: &Eng, region: &str) -> Vec<DomainInfo> {
    let Some(state) = opensearch::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .domains
        .values()
        .filter(|domain| domain.region == region)
        .map(|domain| DomainInfo {
            domain_name: Some(domain.domain_name.clone()),
            engine_type: Some(
                crate::state::engine_type_of(Some(&domain.engine_version)).to_string(),
            ),
        })
        .collect()
}

/// The tags attached to the domain `arn` names, when it is a domain the OpenSearch surface
/// holds.
///
/// Addressed by ARN rather than by name, the way every tag operation of this surface
/// addresses a resource: the ARN names its own Region and domain, so it is resolved against
/// every domain the other surface holds rather than against the Region the call was signed
/// for. A domain carrying no tags answers an empty list — it HAS a tag set, which happens
/// to be empty.
pub fn domain_tags<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> Option<Vec<Tag>> {
    let state = opensearch::service_state(engine)?;
    let store = state.lock();
    let domain = store.domains.values().find(|domain| domain.arn == arn)?;
    Some(
        domain
            .tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The projections, restated in this surface's shapes
//
// One function per shape the other surface reports, each a pure rename of the
// members this model declares. Nothing is computed here: what a member says
// about the domain was decided by the record's own projection.
// ---------------------------------------------------------------------------

/// `DomainStatus` as `ElasticsearchDomainStatus`.
fn reported_status(status: &os::DomainStatus) -> ElasticsearchDomainStatus {
    ElasticsearchDomainStatus {
        domain_id: status.domain_id.clone(),
        domain_name: status.domain_name.clone(),
        arn: status.arn.clone(),
        created: status.created,
        deleted: status.deleted,
        endpoint: status.endpoint.clone(),
        endpoints: status
            .endpoints
            .as_ref()
            .map(|hosts| hosts.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        processing: status.processing,
        upgrade_processing: status.upgrade_processing,
        // `ElasticsearchVersion` is this surface's name for the release the domain runs,
        // whichever engine that release is of: a domain running an OpenSearch release
        // reports that release here, which is what lets a legacy client tell the two
        // engines apart, and one running an Elasticsearch release reports it in the
        // spelling this surface's own member admits ([`legacy_version`]).
        elasticsearch_version: status.engine_version.as_deref().map(legacy_version),
        elasticsearch_cluster_config: cluster_config(&status.cluster_config),
        ebs_options: status.ebs_options.as_ref().map(ebs_options),
        access_policies: status.access_policies.clone(),
        snapshot_options: status.snapshot_options.as_ref().map(snapshot_options),
        vpc_options: status.vpc_options.as_ref().map(vpc_derived_info),
        cognito_options: status.cognito_options.as_ref().map(cognito_options),
        encryption_at_rest_options: status
            .encryption_at_rest_options
            .as_ref()
            .map(encryption_at_rest_options),
        node_to_node_encryption_options: status
            .node_to_node_encryption_options
            .as_ref()
            .map(node_to_node_encryption_options),
        advanced_options: status.advanced_options.clone(),
        log_publishing_options: status
            .log_publishing_options
            .as_ref()
            .map(log_publishing_options),
        // The 2021-01-01 surface's model declares no catalog integration, so its record
        // holds nothing to report here: a domain of that surface publishes to no catalog,
        // rather than to one this projection would have to invent.
        catalog_options: None,
        service_software_options: status
            .service_software_options
            .as_ref()
            .map(service_software_options),
        domain_endpoint_options: status
            .domain_endpoint_options
            .as_ref()
            .map(domain_endpoint_options),
        advanced_security_options: status
            .advanced_security_options
            .as_ref()
            .map(advanced_security_options),
        auto_tune_options: status.auto_tune_options.as_ref().map(auto_tune_output),
        change_progress_details: status
            .change_progress_details
            .as_ref()
            .map(change_progress_details),
        domain_processing_status: status.domain_processing_status.clone(),
        modifying_properties: status
            .modifying_properties
            .as_ref()
            .map(|properties| properties.iter().map(modifying_properties).collect()),
        deployment_strategy_options: status
            .deployment_strategy_options
            .as_ref()
            .map(deployment_strategy_options),
        automated_snapshot_pause_options: status
            .automated_snapshot_pause_options
            .as_ref()
            .map(automated_snapshot_pause_options),
        use_case: status.use_case.clone(),
        engine_mode: status.engine_mode.clone(),
    }
}

/// `DomainConfig` as `ElasticsearchDomainConfig`.
fn reported_config(config: &os::DomainConfig) -> ElasticsearchDomainConfig {
    ElasticsearchDomainConfig {
        elasticsearch_version: config
            .engine_version
            .as_ref()
            .map(|version| ElasticsearchVersionStatus {
                options: legacy_version(&version.options),
                status: option_status(&version.status),
            }),
        elasticsearch_cluster_config: config.cluster_config.as_ref().map(|cluster| {
            ElasticsearchClusterConfigStatus {
                options: cluster_config(&cluster.options),
                status: option_status(&cluster.status),
            }
        }),
        ebs_options: config.ebs_options.as_ref().map(|ebs| EBSOptionsStatus {
            options: ebs_options(&ebs.options),
            status: option_status(&ebs.status),
        }),
        access_policies: config
            .access_policies
            .as_ref()
            .map(|policies| AccessPoliciesStatus {
                options: policies.options.clone(),
                status: option_status(&policies.status),
            }),
        snapshot_options: config
            .snapshot_options
            .as_ref()
            .map(|snapshots| SnapshotOptionsStatus {
                options: snapshot_options(&snapshots.options),
                status: option_status(&snapshots.status),
            }),
        vpc_options: config.vpc_options.as_ref().map(|vpc| VPCDerivedInfoStatus {
            options: vpc_derived_info(&vpc.options),
            status: option_status(&vpc.status),
        }),
        cognito_options: config
            .cognito_options
            .as_ref()
            .map(|cognito| CognitoOptionsStatus {
                options: cognito_options(&cognito.options),
                status: option_status(&cognito.status),
            }),
        encryption_at_rest_options: config.encryption_at_rest_options.as_ref().map(
            |encryption| EncryptionAtRestOptionsStatus {
                options: encryption_at_rest_options(&encryption.options),
                status: option_status(&encryption.status),
            },
        ),
        node_to_node_encryption_options: config.node_to_node_encryption_options.as_ref().map(
            |encryption| NodeToNodeEncryptionOptionsStatus {
                options: node_to_node_encryption_options(&encryption.options),
                status: option_status(&encryption.status),
            },
        ),
        advanced_options: config
            .advanced_options
            .as_ref()
            .map(|advanced| AdvancedOptionsStatus {
                options: advanced.options.clone(),
                status: option_status(&advanced.status),
            }),
        log_publishing_options: config
            .log_publishing_options
            .as_ref()
            .map(|logs| LogPublishingOptionsStatus {
                options: logs.options.as_ref().map(log_publishing_options),
                status: logs.status.as_ref().map(option_status),
            }),
        // No catalog integration on that surface (see [`reported_status`]), so the
        // configured catalog options of one of its domains are nothing.
        catalog_options: None,
        domain_endpoint_options: config
            .domain_endpoint_options
            .as_ref()
            .map(|endpoint| DomainEndpointOptionsStatus {
                options: domain_endpoint_options(&endpoint.options),
                status: option_status(&endpoint.status),
            }),
        advanced_security_options: config.advanced_security_options.as_ref().map(|security| {
            AdvancedSecurityOptionsStatus {
                options: advanced_security_options(&security.options),
                status: option_status(&security.status),
            }
        }),
        auto_tune_options: config
            .auto_tune_options
            .as_ref()
            .map(|auto_tune| AutoTuneOptionsStatus {
                options: auto_tune.options.as_ref().map(auto_tune_options),
                status: auto_tune.status.as_ref().map(auto_tune_status),
            }),
        change_progress_details: config
            .change_progress_details
            .as_ref()
            .map(change_progress_details),
        modifying_properties: config
            .modifying_properties
            .as_ref()
            .map(|properties| properties.iter().map(modifying_properties).collect()),
        deployment_strategy_options: config.deployment_strategy_options.as_ref().map(
            |deployment| DeploymentStrategyOptionsStatus {
                options: deployment_strategy_options(&deployment.options),
                status: option_status(&deployment.status),
            },
        ),
        automated_snapshot_pause_options: config.automated_snapshot_pause_options.as_ref().map(
            |pause| AutomatedSnapshotPauseOptionsStatus {
                options: automated_snapshot_pause_options(&pause.options),
                status: option_status(&pause.status),
            },
        ),
        use_case: config.use_case.as_ref().map(|use_case| UseCaseStatus {
            options: use_case.options.clone(),
            status: option_status(&use_case.status),
        }),
        engine_mode: config.engine_mode.as_ref().map(|mode| EngineModeStatus {
            options: mode.options.clone(),
            status: option_status(&mode.status),
        }),
    }
}

/// `ClusterConfig` as `ElasticsearchClusterConfig`.
fn cluster_config(cluster: &os::ClusterConfig) -> ElasticsearchClusterConfig {
    ElasticsearchClusterConfig {
        instance_type: cluster.instance_type.clone(),
        instance_count: cluster.instance_count,
        dedicated_master_enabled: cluster.dedicated_master_enabled,
        zone_awareness_enabled: cluster.zone_awareness_enabled,
        zone_awareness_config: cluster
            .zone_awareness_config
            .as_ref()
            .map(|zones| ZoneAwarenessConfig {
                availability_zone_count: zones.availability_zone_count,
            }),
        dedicated_master_type: cluster.dedicated_master_type.clone(),
        dedicated_master_count: cluster.dedicated_master_count,
        warm_enabled: cluster.warm_enabled,
        warm_type: cluster.warm_type.clone(),
        warm_count: cluster.warm_count,
        cold_storage_options: cluster
            .cold_storage_options
            .as_ref()
            .map(|cold| ColdStorageOptions {
                enabled: cold.enabled,
            }),
    }
}

fn ebs_options(ebs: &os::EBSOptions) -> EBSOptions {
    EBSOptions {
        ebs_enabled: ebs.ebs_enabled,
        volume_type: ebs.volume_type.clone(),
        volume_size: ebs.volume_size,
        iops: ebs.iops,
        throughput: ebs.throughput,
    }
}

fn snapshot_options(snapshots: &os::SnapshotOptions) -> SnapshotOptions {
    SnapshotOptions {
        automated_snapshot_start_hour: snapshots.automated_snapshot_start_hour,
    }
}

fn vpc_derived_info(vpc: &os::VPCDerivedInfo) -> VPCDerivedInfo {
    VPCDerivedInfo {
        vpc_id: vpc.vpc_id.clone(),
        subnet_ids: vpc.subnet_ids.clone(),
        availability_zones: vpc.availability_zones.clone(),
        security_group_ids: vpc.security_group_ids.clone(),
    }
}

fn cognito_options(cognito: &os::CognitoOptions) -> CognitoOptions {
    CognitoOptions {
        enabled: cognito.enabled,
        user_pool_id: cognito.user_pool_id.clone(),
        identity_pool_id: cognito.identity_pool_id.clone(),
        role_arn: cognito.role_arn.clone(),
    }
}

fn encryption_at_rest_options(encryption: &os::EncryptionAtRestOptions) -> EncryptionAtRestOptions {
    EncryptionAtRestOptions {
        enabled: encryption.enabled,
        kms_key_id: encryption.kms_key_id.clone(),
    }
}

fn node_to_node_encryption_options(
    encryption: &os::NodeToNodeEncryptionOptions,
) -> NodeToNodeEncryptionOptions {
    NodeToNodeEncryptionOptions {
        enabled: encryption.enabled,
    }
}

/// The `LogPublishingOptions` map: the log types the domain publishes, each with the
/// destination it publishes to and whether it is enabled. Both surfaces key the map by a
/// `LogType` value, so the keys carry over as they are.
fn log_publishing_options(
    options: &std::collections::HashMap<String, os::LogPublishingOption>,
) -> std::collections::HashMap<String, LogPublishingOption> {
    options
        .iter()
        .map(|(log_type, option)| {
            (
                log_type.clone(),
                LogPublishingOption {
                    cloud_watch_logs_log_group_arn: option.cloud_watch_logs_log_group_arn.clone(),
                    enabled: option.enabled,
                },
            )
        })
        .collect()
}

fn service_software_options(software: &os::ServiceSoftwareOptions) -> ServiceSoftwareOptions {
    ServiceSoftwareOptions {
        current_version: software.current_version.clone(),
        new_version: software.new_version.clone(),
        update_available: software.update_available,
        cancellable: software.cancellable,
        update_status: software.update_status.clone(),
        description: software.description.clone(),
        automated_update_date: software.automated_update_date,
        optional_deployment: software.optional_deployment,
    }
}

fn domain_endpoint_options(endpoint: &os::DomainEndpointOptions) -> DomainEndpointOptions {
    DomainEndpointOptions {
        enforce_https: endpoint.enforce_https,
        tls_security_policy: endpoint.tls_security_policy.clone(),
        custom_endpoint_enabled: endpoint.custom_endpoint_enabled,
        custom_endpoint: endpoint.custom_endpoint.clone(),
        custom_endpoint_certificate_arn: endpoint.custom_endpoint_certificate_arn.clone(),
    }
}

/// `AdvancedSecurityOptions` as this surface declares it: the switches and the SAML
/// configuration. The other surface also reports a JWT and an IAM-federation configuration,
/// which this model declares no member for — they are not part of what this surface
/// publishes.
fn advanced_security_options(security: &os::AdvancedSecurityOptions) -> AdvancedSecurityOptions {
    AdvancedSecurityOptions {
        enabled: security.enabled,
        internal_user_database_enabled: security.internal_user_database_enabled,
        saml_options: security.saml_options.as_ref().map(|saml| SAMLOptionsOutput {
            enabled: saml.enabled,
            idp: saml.idp.as_ref().map(|idp| SAMLIdp {
                metadata_content: idp.metadata_content.clone(),
                entity_id: idp.entity_id.clone(),
            }),
            subject_key: saml.subject_key.clone(),
            roles_key: saml.roles_key.clone(),
            session_timeout_minutes: saml.session_timeout_minutes,
        }),
        anonymous_auth_disable_date: security.anonymous_auth_disable_date,
        anonymous_auth_enabled: security.anonymous_auth_enabled,
    }
}

fn auto_tune_output(auto_tune: &os::AutoTuneOptionsOutput) -> AutoTuneOptionsOutput {
    AutoTuneOptionsOutput {
        state: auto_tune.state.clone(),
        error_message: auto_tune.error_message.clone(),
    }
}

fn auto_tune_options(auto_tune: &os::AutoTuneOptions) -> AutoTuneOptions {
    AutoTuneOptions {
        desired_state: auto_tune.desired_state.clone(),
        rollback_on_disable: auto_tune.rollback_on_disable.clone(),
        maintenance_schedules: auto_tune.maintenance_schedules.as_ref().map(|schedules| {
            schedules
                .iter()
                .map(|schedule| AutoTuneMaintenanceSchedule {
                    start_at: schedule.start_at,
                    duration: schedule.duration.as_ref().map(|duration| Duration {
                        value: duration.value,
                        unit: duration.unit.clone(),
                    }),
                    cron_expression_for_recurrence: schedule
                        .cron_expression_for_recurrence
                        .clone(),
                })
                .collect()
        }),
    }
}

fn auto_tune_status(status: &os::AutoTuneStatus) -> AutoTuneStatus {
    AutoTuneStatus {
        creation_date: status.creation_date,
        update_date: status.update_date,
        update_version: status.update_version,
        state: status.state.clone(),
        error_message: status.error_message.clone(),
        pending_deletion: status.pending_deletion,
    }
}

fn change_progress_details(change: &os::ChangeProgressDetails) -> ChangeProgressDetails {
    ChangeProgressDetails {
        change_id: change.change_id.clone(),
        message: change.message.clone(),
        config_change_status: change.config_change_status.clone(),
        start_time: change.start_time,
        last_updated_time: change.last_updated_time,
        initiated_by: change.initiated_by.clone(),
    }
}

fn modifying_properties(property: &os::ModifyingProperties) -> ModifyingProperties {
    ModifyingProperties {
        name: property.name.clone(),
        active_value: property.active_value.clone(),
        pending_value: property.pending_value.clone(),
        value_type: property.value_type.clone(),
    }
}

fn deployment_strategy_options(
    deployment: &os::DeploymentStrategyOptions,
) -> DeploymentStrategyOptions {
    DeploymentStrategyOptions {
        deployment_strategy: deployment.deployment_strategy.clone(),
    }
}

fn automated_snapshot_pause_options(
    pause: &os::AutomatedSnapshotPauseOptions,
) -> AutomatedSnapshotPauseOptions {
    AutomatedSnapshotPauseOptions {
        enabled: pause.enabled,
        start_time: pause.start_time,
        end_time: pause.end_time,
        state: pause.state.clone(),
    }
}

/// One option's `OptionStatus`: when it was first set, when it last changed, the config
/// version it changed at and the state it is in. Both surfaces declare the same five
/// members, so the status carries over as it stands — what a client reads of an option's
/// provenance is the same whichever surface it asks.
fn option_status(status: &os::OptionStatus) -> OptionStatus {
    OptionStatus {
        creation_date: status.creation_date,
        update_date: status.update_date,
        update_version: status.update_version,
        state: status.state.clone(),
        pending_deletion: status.pending_deletion,
    }
}
