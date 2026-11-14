// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the database and network services: Amazon GuardDuty,
//! Amazon MSK, Amazon MQ, Amazon Managed Grafana, Amazon RDS (instances, subnet groups and
//! proxies) and the Amazon EC2 VPC and subnet types. One file of [`super::super`]'s per-type
//! arms, dispatched to by its `Provisioner`.

use super::*;

/// Create the detector an `AWS::GuardDuty::Detector` declares.
///
/// The type's properties ARE `CreateDetector`'s own members, spelled with an initial
/// capital — `Enable`, `FindingPublishingFrequency`, `DataSources` and `Features` against
/// `enable`, `findingPublishingFrequency`, `dataSources` and `features`, every one of them
/// a `jsonName` of `smithy_query(operation="CreateDetector")` — and the correspondence
/// holds all the way down (`DataSources.Kubernetes.AuditLogs.Enable` is
/// `dataSources.kubernetes.auditLogs.enable`;
/// `DataSources.MalwareProtection.ScanEc2InstanceWithFindings.EbsVolumes` is
/// `dataSources.malwareProtection.scanEc2InstanceWithFindings.ebsVolumes`; a `Features`
/// entry's `Name`, `Status` and `AdditionalConfiguration[].Status` are `name`, `status` and
/// `additionalConfiguration[].status` — `smithy_query(shape="DataSourceConfigurations,
/// KubernetesConfiguration,MalwareProtectionConfiguration,ScanEc2InstanceWithFindings,
/// S3LogsConfiguration,DetectorFeatureConfiguration,DetectorAdditionalConfiguration")`).
/// So the whole evaluated document is carried through [`lower_camel_members`] rather than
/// mapped member by member, which is the same re-casing `AWS::ECS::Cluster` needs and is
/// safe for the same reason: no property of this type carries a MAP of caller data. The
/// one exception is `Tags`, a list of `{Key, Value}` pairs where the create takes a map
/// ([`tags_as_map`]), lifted out before the walk — carrying the tags CloudFormation gives
/// the resource beside the declared ones ([`tags_of`]).
///
/// VALUES are untouched: an enum value, a boolean and a number are what the template
/// declared, and GuardDuty's own validation is what judges them — its refusal leaves the
/// stack `CREATE_FAILED` rather than a detector configured differently from the template.
///
/// A detector's id is GuardDuty's OWN to mint — `CreateDetector` takes no identifier and
/// answers with `detectorId` — so that answer is reported back as the resource's physical
/// id, which is what `Ref` resolves to and what a client then passes as `DetectorId`. It is
/// also the type's one published `Fn::GetAtt` attribute (`Id`), so that is re-reported off
/// the identity GuardDuty assigned rather than the one the naming rules computed.
pub(crate) async fn create_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" {
                continue;
            }
            request.insert(lower_initial(property), lower_camel_members(value, 0)?);
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        GUARDDUTY_SIGNING_NAME,
        region,
        "POST",
        GUARDDUTY_DETECTOR_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(detector_id) = reported.get("detectorId").and_then(Value::as_str) else {
        // The create answered 2xx without the id its output models. Nothing addresses a
        // detector without one, so this is reported as a failure rather than recorded as
        // a resource with a physical id nothing minted.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateDetector answered no detectorId".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), detector_id.to_string());
    Ok(Created {
        physical_resource_id: Some(detector_id.to_string()),
        attributes,
    })
}

/// Delete the detector a stack created, addressed by its id — this resource's physical id,
/// which is what `Ref` returns for the type.
///
/// A detector GuardDuty no longer resolves the id to is already in the state the delete
/// asks for. GuardDuty answers every client rejection under one class, so the code alone
/// does not say WHICH rejection it was: a refusal is confirmed by reading the detector
/// back, and only a read that refuses it too — the id resolves to no detector — is treated
/// as already deleted. A refusal of a detector GuardDuty still holds is reported, so
/// nothing passes off a deletion that did not happen.
pub(crate) async fn delete_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!("{GUARDDUTY_DETECTOR_URI}/{physical_resource_id}");
    let refusal =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "DELETE", &uri, None).await {
            Ok(_) => return Ok(()),
            Err(refusal) => refusal,
        };
    if !guardduty_rejected(&refusal) {
        return Err(refusal);
    }
    match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
        // The detector is still there, so the delete really was refused.
        Ok(_) => Err(refusal),
        // GuardDuty does not resolve the id: the detector is already in the state the
        // delete asks for. Judged on the client-rejection class, so a transport or
        // internal failure of the confirming read is reported rather than mistaken for a
        // deletion — that failure says nothing about whether the detector is there.
        Err(confirmation) if guardduty_rejected(&confirmation) => Ok(()),
        Err(confirmation) => Err(confirmation),
    }
}

/// The detector's configuration as GuardDuty reports it, for drift detection.
///
/// A detector GuardDuty no longer resolves the id to was deleted: a READ has only that one
/// reading of the client-rejection class this service answers with (unlike
/// [`delete_detector`], where the same class could also be a refusal of the delete
/// itself), so that class is `Ok(None)`.
///
/// Only the two properties `GetDetector` reports in the SAME shape the type declares them
/// in are reported, because a property the actual document does not carry is not evidence
/// of anything (`super::drift::differs`): `Enable`, which the detector's required `Status`
/// is — "a Boolean value that specifies whether the detector is to be enabled", and
/// `DetectorStatus` is `ENABLED`/`DISABLED` — and `FindingPublishingFrequency`, the same
/// enum on both sides. `DataSources` and `Features` are reported as RESULT shapes, with a
/// per-switch status and the instant it last changed rather than the enable flags the
/// properties are, and `Tags` as a map where the property is a list of pairs; comparing
/// either would need a mapping this module would have to invent, so neither is reported
/// and neither is ever read as drift.
pub(crate) async fn describe_detector(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    /// The `DetectorStatus` of a detector that is enabled (`smithy_query(shape=
    /// "DetectorStatus")`: `DISABLED | ENABLED`).
    const ENABLED: &str = "ENABLED";

    let uri = format!("{GUARDDUTY_DETECTOR_URI}/{physical_resource_id}");
    let reported =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
            Ok(reported) => reported,
            Err(refusal) if guardduty_rejected(&refusal) => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    let mut actual = Map::new();
    if let Some(status) = reported.get("status").and_then(Value::as_str) {
        actual.insert("Enable".to_string(), Value::Bool(status == ENABLED));
    }
    if let Some(frequency) = reported.get("findingPublishingFrequency") {
        actual.insert("FindingPublishingFrequency".to_string(), frequency.clone());
    }
    Ok(Some(Value::Object(actual)))
}

/// Whether a refusal is GuardDuty REJECTING the request — the class a detector id that
/// resolves to no detector of this account comes back under — rather than a transport or
/// internal failure, which says nothing about whether the detector is there.
pub(crate) fn guardduty_rejected(refusal: &Refusal) -> bool {
    refusal.code == GUARDDUTY_INVALID_INPUT || refusal.code == GUARDDUTY_BAD_REQUEST
}

// ---------------------------------------------------------------------------
// Amazon GuardDuty: AWS::GuardDuty::IPSet
// ---------------------------------------------------------------------------

/// The collection a detector's trusted IP lists hang off, and the segment one list's own URI
/// starts from: `POST /detector/{DetectorId}/ipset` creates one,
/// `GET /detector/{DetectorId}/ipset/{IpSetId}` reads it and
/// `DELETE /detector/{DetectorId}/ipset/{IpSetId}` deletes it
/// (`smithy_query(operation="CreateIPSet,GetIPSet,DeleteIPSet")`).
pub(crate) const GUARDDUTY_IP_SET_SEGMENT: &str = "ipset";

/// The `AWS::GuardDuty::IPSet` properties that do NOT travel in `CreateIPSet`'s body.
///
/// `DetectorId` is the call's `@httpLabel` — it ADDRESSES the create rather than configuring
/// the list — `Name` is the resource's physical name, which CloudFormation decides (the
/// declared one or the one it generates, [`declared_name`]), and `Tags` is the `[{Key,
/// Value}]` list the type publishes where the create takes a MAP ([`tags_as_map`]). Every
/// other property of the type IS a member of that create spelled with an initial capital —
/// `Activate`, `Format`, `Location` and `ExpectedBucketOwner` against `activate`, `format`,
/// `location` and `expectedBucketOwner`, each one a `jsonName` of
/// `smithy_query(operation="CreateIPSet")` — so the rest of the evaluated document is
/// carried through [`lower_camel_members`], the same re-casing the detector arm above needs
/// and safe for the same reason: no property of this type carries a MAP of caller data.
pub(crate) const GUARDDUTY_IP_SET_LIFTED_PROPERTIES: &[&str] = &["DetectorId", "Name", "Tags"];

/// Create the trusted IP list an `AWS::GuardDuty::IPSet` declares, and report the `IpSetId`
/// GuardDuty ASSIGNED it.
///
/// A list is addressed by the PAIR `(DetectorId, IpSetId)`: the detector half is the
/// resource's own declared property — a declaration carrying none describes no list that
/// could be created, so it is refused here rather than sent as a request GuardDuty would
/// have to reject — and the id half is GuardDuty's to mint, `CreateIPSet` taking no
/// identifier and answering with `ipSetId`. That answer is what this reports back as the
/// resource's physical id, which is what `Ref` resolves to and what a client then passes as
/// `IpSetId`; it is also the type's one published `Fn::GetAtt` attribute (`Id`), so that is
/// re-reported off the identity GuardDuty assigned rather than the one the naming rules
/// computed.
///
/// VALUES are untouched: the format, the location and the activation flag are what the
/// template declared, and GuardDuty's own validation is what judges them — its refusal
/// leaves the stack `CREATE_FAILED` rather than a list configured differently from the
/// template. A property the declaration does not carry is absent from the request, including
/// the `Activate` the create models `@required`: which lists GuardDuty uses is the template's
/// statement to make, so a declaration that makes none is that service's refusal to word
/// rather than an activation this arm chose.
pub(crate) async fn create_guardduty_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let detector_id = guardduty_ip_set_detector(resource)?;
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if GUARDDUTY_IP_SET_LIFTED_PROPERTIES.contains(&property.as_str()) {
                continue;
            }
            request.insert(lower_initial(property), lower_camel_members(value, 0)?);
        }
    }
    request.insert("name".to_string(), Value::String(declared_name(resource)));
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        GUARDDUTY_SIGNING_NAME,
        region,
        "POST",
        &guardduty_ip_set_collection_uri(&detector_id),
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(ip_set_id) = reported.get("ipSetId").and_then(Value::as_str) else {
        // The create answered 2xx without the id its output models. Nothing addresses a
        // list without one, so this is reported as a failure rather than recorded as a
        // resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateIPSet answered no ipSetId".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), ip_set_id.to_string());
    Ok(Created {
        physical_resource_id: Some(ip_set_id.to_string()),
        attributes,
    })
}

/// Delete the trusted IP list a stack created, addressed by the pair that identifies it: the
/// detector its declaration names and the id GuardDuty minted, which is this resource's
/// physical id.
///
/// A list GuardDuty no longer resolves that pair to is already in the state the delete asks
/// for. GuardDuty answers every client rejection under one class, so the code alone does not
/// say WHICH rejection it was: a refusal is confirmed by reading the list back, exactly as
/// [`delete_detector`] confirms one, and only a read that refuses it too is treated as
/// already deleted.
pub(crate) async fn delete_guardduty_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = guardduty_ip_set_uri(
        &guardduty_ip_set_detector(resource)?,
        &resource.physical_resource_id,
    );
    let refusal =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "DELETE", &uri, None).await {
            Ok(_) => return Ok(()),
            Err(refusal) => refusal,
        };
    if !guardduty_rejected(&refusal) {
        return Err(refusal);
    }
    match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
        // The list is still there, so the delete really was refused.
        Ok(_) => Err(refusal),
        // GuardDuty does not resolve the pair: the list is already in the state the delete
        // asks for. Judged on the client-rejection class, so a transport or internal
        // failure of the confirming read is reported rather than mistaken for a deletion.
        Err(confirmation) if guardduty_rejected(&confirmation) => Ok(()),
        Err(confirmation) => Err(confirmation),
    }
}

/// The list's configuration as GuardDuty reports it, for drift detection. A pair GuardDuty
/// no longer resolves was deleted, which for a READ is the one reading its client-rejection
/// class has (the case [`describe_detector`] is in).
///
/// Only the properties `GetIPSet` reports in the SAME shape the type declares them in are
/// reported, because a property the actual document does not carry is not evidence of
/// anything (`super::drift::differs`): `Name`, `Format`, `Location` and
/// `ExpectedBucketOwner`, each one the member of that read spelled with an initial capital.
/// `Activate` is a FLAG where the read reports an `IpSetStatus` whose seven values also
/// carry the transitional and error states, and `Tags` is a map where the property is a list
/// of pairs; comparing either would need a mapping this module would have to invent, so
/// neither is reported and neither is ever read as drift.
pub(crate) async fn describe_guardduty_ip_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = guardduty_ip_set_uri(
        &guardduty_ip_set_detector(resource)?,
        &resource.physical_resource_id,
    );
    let reported =
        match call_rest_json(ctx, GUARDDUTY_SIGNING_NAME, region, "GET", &uri, None).await {
            Ok(reported) => reported,
            Err(refusal) if guardduty_rejected(&refusal) => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    Ok(Some(reported_as_properties(
        &reported,
        &[
            ("Name", "name"),
            ("Format", "format"),
            ("Location", "location"),
            ("ExpectedBucketOwner", "expectedBucketOwner"),
        ],
    )))
}

/// The detector a trusted IP list's declaration names — the other half of the list's
/// identity, and the `@httpLabel` of every GuardDuty call on one.
///
/// A declaration carrying no `DetectorId` is refused here, naming that property, rather than
/// sent as a request the owning service would reject.
pub(crate) fn guardduty_ip_set_detector(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "DetectorId").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no DetectorId", resource.resource_type),
    })
}

/// The collection URI a detector's trusted IP lists are created in.
pub(crate) fn guardduty_ip_set_collection_uri(detector_id: &str) -> String {
    format!(
        "{GUARDDUTY_DETECTOR_URI}/{}/{GUARDDUTY_IP_SET_SEGMENT}",
        percent_encoded(detector_id)
    )
}

/// The URI of ONE trusted IP list, which its read and its delete are both bound to.
pub(crate) fn guardduty_ip_set_uri(detector_id: &str, ip_set_id: &str) -> String {
    format!(
        "{}/{}",
        guardduty_ip_set_collection_uri(detector_id),
        percent_encoded(ip_set_id)
    )
}

// ---------------------------------------------------------------------------
// Amazon MSK: AWS::MSK::Cluster and AWS::MSK::ServerlessCluster
// ---------------------------------------------------------------------------

/// Amazon MSK's signing name, from its model (`sigv4Name: kafka`, `endpointPrefix: kafka`).
/// That service speaks `restJson1`, so its operations are bound to a METHOD and a URI
/// rather than to an `X-Amz-Target` header.
pub(crate) const MSK_SIGNING_NAME: &str = "kafka";

/// The URI `CreateCluster` is bound to, and the collection a cluster's own URI hangs off:
/// `POST /v1/clusters` creates a provisioned cluster, `GET /v1/clusters/{ClusterArn}` reads
/// one and `DELETE /v1/clusters/{ClusterArn}` deletes a cluster of EITHER kind
/// (`smithy_query(query="operation", operation="CreateCluster,DescribeCluster,\
/// DeleteCluster")` on that service's model).
pub(crate) const MSK_CLUSTERS_URI: &str = "/v1/clusters";

/// The v2 collection, which is the one that serves a SERVERLESS cluster: `CreateClusterV2`
/// is `POST /api/v2/clusters` and `DescribeClusterV2` is
/// `GET /api/v2/clusters/{ClusterArn}`. The v1 `DescribeCluster` answers only for a
/// provisioned cluster — its `ClusterInfo` describes broker nodes, which a serverless
/// cluster has none of — while the v2 `Cluster` shape models a member for each kind.
pub(crate) const MSK_CLUSTERS_V2_URI: &str = "/api/v2/clusters";

/// The error code Amazon MSK answers for a cluster ARN it holds no cluster for
/// (`NotFoundException`, a modelled error of `DescribeCluster`, `DescribeClusterV2` and
/// `DeleteCluster`).
pub(crate) const MSK_NOT_FOUND: &str = "NotFoundException";

/// The `AWS::MSK::Cluster` properties `CreateCluster` has no member for.
///
/// `CurrentVersion` is the version token an UPDATE of a cluster that already exists must
/// match ("The current version of the MSK cluster"), and a create has no version to match
/// — so it is not carried out, and nothing about the cluster the create makes differs for
/// it. Every other property of the type IS a `CreateCluster` member ([`msk_members`]).
pub(crate) const MSK_UPDATE_ONLY_PROPERTIES: &[&str] = &["CurrentVersion"];

/// The one `AWS::MSK::Cluster` property name that is not its API member's name with the
/// initial letter lower-cased: the type spells the initialism of
/// `StorageInfo.EBSStorageInfo` in full capitals where the member is
/// `storageInfo.ebsStorageInfo` (`smithy_query(query="shape", shape="StorageInfo")`).
pub(crate) const MSK_RENAMED_MEMBERS: &[(&str, &str)] = &[("EBSStorageInfo", "ebsStorageInfo")];

/// The members one `AWS::MSK::ServerlessCluster` `VpcConfigs` entry carries, as
/// (property, member) pairs.
///
/// The entry is Amazon MSK's `VpcConfig` (`smithy_query(query="shape", shape="VpcConfig")`:
/// `subnetIds`, `securityGroupIds`), and the type publishes the security-group list as
/// `SecurityGroups` — a per-member correspondence rather than a case change, which is why
/// this one structure is mapped by name instead of walked.
pub(crate) const MSK_SERVERLESS_VPC_CONFIG_MEMBERS: &[(&str, &str)] =
    &[("SubnetIds", "subnetIds"), ("SecurityGroups", "securityGroupIds")];

/// Create the provisioned cluster an `AWS::MSK::Cluster` declares, and report the
/// `ClusterArn` Amazon MSK ASSIGNED it.
///
/// The type's properties ARE `CreateCluster`'s own members spelled with an initial capital
/// — `BrokerNodeGroupInfo`, `ClientAuthentication`, `ConfigurationInfo`, `EncryptionInfo`,
/// `EnhancedMonitoring`, `KafkaVersion`, `LoggingInfo`, `NumberOfBrokerNodes`,
/// `OpenMonitoring` and `StorageMode` against `brokerNodeGroupInfo`,
/// `clientAuthentication`, `configurationInfo`, `encryptionInfo`, `enhancedMonitoring`,
/// `kafkaVersion`, `loggingInfo`, `numberOfBrokerNodes`, `openMonitoring` and
/// `storageMode`, every one of them a `jsonName` of
/// `smithy_query(query="closure", operation="CreateCluster")` — and the correspondence
/// holds all the way down (`BrokerNodeGroupInfo.ClientSubnets` is
/// `brokerNodeGroupInfo.clientSubnets`; `ClientAuthentication.Sasl.Iam.Enabled` is
/// `clientAuthentication.sasl.iam.enabled`;
/// `LoggingInfo.BrokerLogs.CloudWatchLogs.LogGroup` is
/// `loggingInfo.brokerLogs.cloudWatchLogs.logGroup`;
/// `OpenMonitoring.Prometheus.JmxExporter.EnabledInBroker` is
/// `openMonitoring.prometheus.jmxExporter.enabledInBroker`). So the whole evaluated
/// document is carried through [`msk_members`] rather than mapped member by member, which
/// is safe for the same reason the Amazon ECS and GuardDuty walks are: no property of this
/// type carries a MAP of caller data. The exceptions are the one member whose API spelling
/// is not a case change ([`MSK_RENAMED_MEMBERS`]), the property the create has no member
/// for ([`MSK_UPDATE_ONLY_PROPERTIES`]) and `Tags`, which the type publishes as a MAP of
/// tag key to value exactly as the create takes one ([`tags_as_map`]) — lifted out before
/// the walk so it carries the tags CloudFormation gives the resource beside the declared
/// ones ([`tags_of`]).
///
/// VALUES are untouched: an enum value, a number and a boolean are what the template
/// declared, and Amazon MSK's own validation is what judges them — its refusal leaves the
/// stack `CREATE_FAILED` rather than a cluster configured differently from the template.
///
/// The cluster's NAME is the one value CloudFormation decides
/// ([`msk_cluster_name`]), so it is set on the way out; its identity is not
/// CloudFormation's at all ("Ref returns the ARN of the cluster", whose resource path
/// carries an identifier only Amazon MSK mints), so the ARN the create answers with is
/// reported as the resource's physical id ([`msk_created`]).
pub(crate) async fn create_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" || MSK_UPDATE_ONLY_PROPERTIES.contains(&property.as_str()) {
                continue;
            }
            request.insert(
                renamed_member(property, MSK_RENAMED_MEMBERS),
                msk_members(value)?,
            );
        }
    }
    request.insert(
        "clusterName".to_string(),
        Value::String(msk_cluster_name(resource)?),
    );
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        MSK_SIGNING_NAME,
        region,
        "POST",
        MSK_CLUSTERS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    msk_created(resource, &reported)
}

/// Create the serverless cluster an `AWS::MSK::ServerlessCluster` declares, and report the
/// `ClusterArn` Amazon MSK ASSIGNED it.
///
/// A serverless cluster is created by `CreateClusterV2`, whose request carries the cluster
/// name and the tags beside ONE of its two alternative bodies — the `Serverless` one here.
/// That body's members are this type's `VpcConfigs` and `ClientAuthentication`
/// (`smithy_query(query="shape", shape="ServerlessRequest")`: `vpcConfigs`,
/// `clientAuthentication`), so the declaration's two configuration properties go under
/// `serverless` rather than at the top of the request. `ClientAuthentication` is the same
/// members with a lower-case initial all the way down (`Sasl.Iam.Enabled` is
/// `sasl.iam.enabled`) and is walked; a `VpcConfigs` entry is not, because the type
/// publishes its security-group list under a different name
/// ([`MSK_SERVERLESS_VPC_CONFIG_MEMBERS`]).
pub(crate) async fn create_msk_serverless_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut serverless = Map::new();
    if let Some(Value::Array(configs)) = resource.properties.get("VpcConfigs") {
        serverless.insert(
            "vpcConfigs".to_string(),
            Value::Array(
                configs
                    .iter()
                    .map(|config| mapped_members(config, MSK_SERVERLESS_VPC_CONFIG_MEMBERS))
                    .collect(),
            ),
        );
    }
    if let Some(authentication) = resource.properties.get("ClientAuthentication") {
        serverless.insert(
            "clientAuthentication".to_string(),
            msk_members(authentication)?,
        );
    }
    let mut request = Map::new();
    request.insert(
        "clusterName".to_string(),
        Value::String(msk_cluster_name(resource)?),
    );
    request.insert("serverless".to_string(), Value::Object(serverless));
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        MSK_SIGNING_NAME,
        region,
        "POST",
        MSK_CLUSTERS_V2_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    msk_created(resource, &reported)
}

/// Delete the cluster a stack created, addressed by the ARN Amazon MSK minted for it —
/// this resource's physical id, which is what `Ref` returns for either type.
///
/// `DeleteCluster` deletes a cluster of either kind, and a cluster Amazon MSK holds no
/// cluster for under that ARN (`NotFoundException`) is already in the state the delete asks
/// for. Judged on the error CODE, never on the message, which carries the request's own
/// ARN. The ARN goes into the URI percent-encoded, as a non-greedy `@httpLabel` is sent,
/// so its `:` and `/` cannot address a different operation.
pub(crate) async fn delete_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{MSK_CLUSTERS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, MSK_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == MSK_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The cluster's configuration as Amazon MSK reports it, for drift detection: the record
/// `DescribeClusterV2` answers with, read by the ARN that service minted.
///
/// The v2 read serves both kinds of cluster, which is why it is the one this uses. An ARN
/// Amazon MSK holds no cluster for (`NotFoundException`) is the resource having been
/// deleted. The record's members are Amazon MSK's own (lower camel), so a declared property
/// spelled identically on both sides takes part in the comparison and one spelled
/// differently leaves the cluster reported as existing rather than as modified — exactly
/// what [`super::drift`] documents for every arm here.
pub(crate) async fn describe_msk_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{MSK_CLUSTERS_V2_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, MSK_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(answer.get("clusterInfo").cloned()),
        Err(refusal) if refusal.code == MSK_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// What a cluster create answered, as the resource's identity: the `ClusterArn` Amazon MSK
/// minted.
///
/// "Ref returns the ARN of the cluster" for both types, and that ARN is also each type's
/// one published `Fn::GetAtt` attribute — so it is reported as the physical id
/// ([`Created::physical_resource_id`]), which
/// [`super::provisioning::reconcile`] adopts, and as `Arn`. The conventional
/// `<Type>Arn` name every resource also answers
/// ([`super::template::conventional_attributes`]) was computed from the stand-in identifier
/// the naming rules derived, so it is re-pointed at the minted ARN too: one resource must
/// not report two different ARNs.
pub(crate) fn msk_created(resource: &StoredResource, reported: &Value) -> Result<Created, Refusal> {
    let Some(arn) = reported.get("clusterArn").and_then(Value::as_str) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses an
        // MSK cluster without one, so this is reported as a failure rather than recorded
        // as a resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon MSK answered no clusterArn for the cluster it created".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.to_string());
    if let Some(segment) = resource.resource_type.rsplit("::").next() {
        attributes.insert(format!("{segment}Arn"), arn.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(arn.to_string()),
        attributes,
    })
}

/// The name the cluster is created under: the `ClusterName` the declaration asks for —
/// which both types REQUIRE — and otherwise the name CloudFormation generated for the
/// resource, which [`super::template`] records as its `Name`.
///
/// A declaration carrying neither describes no cluster that could be created, so it is
/// refused here rather than with a request Amazon MSK would have to reject.
pub(crate) fn msk_cluster_name(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "ClusterName")
        .or_else(|| {
            resource
                .attributes
                .get("Name")
                .filter(|name| !name.is_empty())
                .cloned()
        })
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{} declares no ClusterName", resource.resource_type),
        })
}

/// One Amazon MSK property value with every member name inside it spelled as that
/// service's model spells the very same member: the shared re-casing walk
/// ([`renamed_members`]) carrying the one member whose API name is not the property with
/// its initial letter lower-cased ([`MSK_RENAMED_MEMBERS`]).
pub(crate) fn msk_members(value: &Value) -> Result<Value, Refusal> {
    renamed_members(value, 0, MSK_RENAMED_MEMBERS)
}

// ---------------------------------------------------------------------------
// Amazon MQ: AWS::AmazonMQ::Broker
// ---------------------------------------------------------------------------

/// Amazon MQ's signing name, from its model (`sigv4Name: mq`). That service speaks
/// `restJson1`, so its operations are bound to a METHOD and a URI rather than to an
/// `X-Amz-Target` header: `CreateBroker` is `POST /v1/brokers`, `DeleteBroker` is
/// `DELETE /v1/brokers/{BrokerId}` and `DescribeBroker` is `GET /v1/brokers/{BrokerId}`
/// (`smithy_query(query="operation", operation="CreateBroker,DeleteBroker,DescribeBroker")`
/// on that service's model).
pub(crate) const MQ_SIGNING_NAME: &str = "mq";

/// The URI `CreateBroker` is bound to, and the collection the per-broker URIs hang off.
pub(crate) const MQ_BROKERS_URI: &str = "/v1/brokers";

/// The error code Amazon MQ answers for a broker id it holds no broker for
/// (`NotFoundException`, the modelled 404 of `DescribeBroker` and `DeleteBroker`).
pub(crate) const MQ_NOT_FOUND: &str = "NotFoundException";

/// Create the broker an `AWS::AmazonMQ::Broker` declares, so a client that deployed the
/// stack and then calls `mq DescribeBroker` with the id the stack publishes reads the broker
/// back rather than a `NotFoundException` for a broker nothing created.
///
/// Every published property of the type IS a `CreateBroker` member under the same name with
/// its initial letter lower-cased, all the way down — `AuthenticationStrategy`,
/// `AutoMinorVersionUpgrade`, `Configuration` (`Id`/`Revision`), `DataReplicationMode`,
/// `DataReplicationPrimaryBrokerArn`, `DeploymentMode`, `EncryptionOptions`
/// (`KmsKeyId`/`UseAwsOwnedKey`), `EngineType`, `EngineVersion`, `HostInstanceType`,
/// `LdapServerMetadata`, `Logs` (`Audit`/`General`), `MaintenanceWindowStartTime`
/// (`DayOfWeek`/`TimeOfDay`/`TimeZone`), `PubliclyAccessible`, `SecurityGroups`,
/// `StorageType`, `SubnetIds` and `Users` (`Username`/`Password`/`ConsoleAccess`/`Groups`/
/// `ReplicationUser`), each one a `jsonName` of
/// `smithy_query(query="closure", operation="CreateBroker")` — so the evaluated property
/// document is the request once its member names are re-cased ([`lower_camel_members`]).
/// `Tags` is the exception, the type publishing the `[{Key, Value}]` list where the create
/// takes a tag MAP ([`tags_as_map`]): it is lifted out before the walk so it carries the
/// tags CloudFormation gives the resource beside the declared ones ([`tags_of`]).
///
/// The broker's NAME is the one value CloudFormation decides ([`declared_name`]), so it is
/// set on the way out; its identity is not CloudFormation's at all ("Ref returns the Amazon
/// MQ broker ID", a value only that service mints), so the `brokerId` the create answers
/// with is reported as the resource's physical id and the `brokerArn` it built around that
/// id as the resource's ARN.
pub(crate) async fn create_mq_broker(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" {
                continue;
            }
            request.insert(lower_initial(property), lower_camel_members(value, 0)?);
        }
    }
    request.insert(
        "brokerName".to_string(),
        Value::String(declared_name(resource)),
    );
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        MQ_SIGNING_NAME,
        region,
        "POST",
        MQ_BROKERS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let (Some(broker_id), Some(broker_arn)) = (
        answered
            .get("brokerId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty()),
        answered
            .get("brokerArn")
            .and_then(Value::as_str)
            .filter(|arn| !arn.is_empty()),
    ) else {
        // The create answered 2xx without the two values its output models. Nothing
        // addresses a broker without that id, so this is reported as a failure rather than
        // recorded as a resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon MQ answered CreateBroker without a brokerId and brokerArn"
                .to_string(),
        });
    };
    Ok(Created {
        physical_resource_id: Some(broker_id.to_string()),
        attributes: super::template::arn_attributes(&resource.resource_type, broker_arn)
            .into_iter()
            .collect(),
    })
}

/// Delete the broker a stack created, addressed by the `BrokerId` Amazon MQ minted for it —
/// this resource's physical id, which is what `Ref` returns for the type. A broker that
/// service no longer holds (`NotFoundException`, judged on the CODE and never on a message
/// that carries the request's own identifier) is already in the state the delete asks for.
pub(crate) async fn delete_mq_broker(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = mq_broker_uri(physical_resource_id);
    match call_rest_json(ctx, MQ_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == MQ_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The broker's configuration as Amazon MQ reports it, for drift detection:
/// `DescribeBroker`'s own record, read by the id that service minted. One it no longer holds
/// (`NotFoundException`) was deleted.
///
/// Every member that read reports under a name this type publishes as a property is
/// compared; `Tags` is left out for the reason it is everywhere here (the tags a resource
/// carries change independently of its declaration), and a property the read does not report
/// is not evidence of anything ([`super::drift`]).
///
/// Four properties the read DOES report are deliberately not compared, because what it
/// reports for them is not the declaration's own shape: `Logs` comes back as a `LogsSummary`
/// (the switches plus the log groups Amazon MQ derived), `Users` as a `UserSummary` list
/// (usernames, without the passwords and groups a declaration carries), `Configuration` as
/// the `Configurations` record of current and pending revisions, and `LdapServerMetadata` as
/// the output shape that omits the service-account password. Comparing any of them against
/// the declared structure would report drift on every broker rather than on a changed one.
pub(crate) async fn describe_mq_broker(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = mq_broker_uri(physical_resource_id);
    match call_rest_json(ctx, MQ_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            MQ_BROKER_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == MQ_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE broker: the `BrokerId` `@httpLabel` every operation on a
/// broker binds, percent-encoded as a non-greedy label is sent.
pub(crate) fn mq_broker_uri(broker_id: &str) -> String {
    format!("{MQ_BROKERS_URI}/{}", percent_encoded(broker_id))
}

/// The members `DescribeBroker` reports under this type's own property names, as
/// `(property, member)` pairs.
pub(crate) const MQ_BROKER_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("AuthenticationStrategy", "authenticationStrategy"),
    ("AutoMinorVersionUpgrade", "autoMinorVersionUpgrade"),
    ("BrokerName", "brokerName"),
    ("DataReplicationMode", "dataReplicationMode"),
    ("DeploymentMode", "deploymentMode"),
    ("EncryptionOptions", "encryptionOptions"),
    ("EngineType", "engineType"),
    ("EngineVersion", "engineVersion"),
    ("HostInstanceType", "hostInstanceType"),
    ("MaintenanceWindowStartTime", "maintenanceWindowStartTime"),
    ("PubliclyAccessible", "publiclyAccessible"),
    ("SecurityGroups", "securityGroups"),
    ("StorageType", "storageType"),
    ("SubnetIds", "subnetIds"),
];

// ---------------------------------------------------------------------------
// Amazon Managed Grafana: AWS::Grafana::Workspace
// ---------------------------------------------------------------------------

/// Amazon Managed Grafana's signing name, from its model (`sigv4Name: grafana`). That
/// service speaks `restJson1`, so its operations are bound to a METHOD and a URI rather
/// than to an `X-Amz-Target` header: `CreateWorkspace` is `POST /workspaces`,
/// `DeleteWorkspace` is `DELETE /workspaces/{workspaceId}` and `DescribeWorkspace` is
/// `GET /workspaces/{workspaceId}` (`smithy_query(query="operation",
/// operation="CreateWorkspace,DeleteWorkspace,DescribeWorkspace")` on that service's
/// model).
pub(crate) const GRAFANA_SIGNING_NAME: &str = "grafana";

/// The URI `CreateWorkspace` is bound to, and the collection the per-workspace URIs hang
/// off.
pub(crate) const GRAFANA_WORKSPACES_URI: &str = "/workspaces";

/// The error code Amazon Managed Grafana answers for a workspace it does not hold
/// (`ResourceNotFoundException`, a modelled error of `DeleteWorkspace` and
/// `DescribeWorkspace`).
pub(crate) const GRAFANA_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The URI that addresses ONE workspace: the `workspaceId` `@httpLabel` `DeleteWorkspace`
/// and `DescribeWorkspace` bind, percent-encoded as a non-greedy label is sent — the same
/// discipline [`mq_broker_uri`] applies to the broker id, so an identifier carrying `/`,
/// `.` or `%` addresses a workspace under this collection rather than some other path of
/// that service.
pub(crate) fn grafana_workspace_uri(workspace_id: &str) -> String {
    format!(
        "{GRAFANA_WORKSPACES_URI}/{}",
        percent_encoded(workspace_id)
    )
}

/// The `AWS::Grafana::Workspace` properties `CreateWorkspace` takes whose value goes
/// through unchanged under the member named beside it, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateWorkspaceRequest` shape. The type spells the workspace's own fields without the
/// `workspace` prefix that API carries, so the correspondence is a per-member one rather
/// than a case change: `Description` is `workspaceDescription`, `RoleArn` is
/// `workspaceRoleArn`, `DataSources` is `workspaceDataSources`,
/// `NotificationDestinations` is `workspaceNotificationDestinations` and
/// `OrganizationalUnits` is `workspaceOrganizationalUnits`, while
/// `AccountAccessType`, `AuthenticationProviders`, `ClientToken`, `GrafanaVersion`,
/// `OrganizationRoleName`, `PermissionType` and `StackSetName` differ only in the first
/// character's case.
///
/// `Name` is NOT here: it is the one value CloudFormation decides (the name the template
/// declared, or the one it generated), so [`workspace_name`] is what the create carries
/// out. `VpcConfiguration` and `NetworkAccessControl` are not here either, because their
/// own members have to be mapped one structure at a time.
pub(crate) const GRAFANA_WORKSPACE_MEMBERS: &[(&str, &str)] = &[
    ("AccountAccessType", "accountAccessType"),
    ("AuthenticationProviders", "authenticationProviders"),
    ("ClientToken", "clientToken"),
    ("DataSources", "workspaceDataSources"),
    ("Description", "workspaceDescription"),
    ("GrafanaVersion", "grafanaVersion"),
    ("NotificationDestinations", "workspaceNotificationDestinations"),
    ("OrganizationRoleName", "organizationRoleName"),
    ("OrganizationalUnits", "workspaceOrganizationalUnits"),
    ("PermissionType", "permissionType"),
    ("RoleArn", "workspaceRoleArn"),
    ("StackSetName", "stackSetName"),
];

/// The members of the type's `VpcConfiguration` property, as `VpcConfiguration` declares
/// them.
pub(crate) const GRAFANA_VPC_MEMBERS: &[(&str, &str)] = &[
    ("SecurityGroupIds", "securityGroupIds"),
    ("SubnetIds", "subnetIds"),
];

/// The members of the type's `NetworkAccessControl` property, as
/// `NetworkAccessConfiguration` declares them.
pub(crate) const GRAFANA_NETWORK_ACCESS_MEMBERS: &[(&str, &str)] =
    &[("PrefixListIds", "prefixListIds"), ("VpceIds", "vpceIds")];

/// The properties of the type this provisioner does not map onto `CreateWorkspace`'s
/// request, because that operation takes neither: a workspace's SAML configuration is
/// written by `UpdateWorkspaceAuthentication` and its plugin administration by
/// `UpdateWorkspaceConfiguration`.
pub(crate) const GRAFANA_UNMAPPED_PROPERTIES: &[&str] = &["SamlConfiguration", "PluginAdminEnabled"];

/// Create the workspace an `AWS::Grafana::Workspace` declares, so a client that reads the
/// id out of the stack's outputs can call Amazon Managed Grafana with it
/// (`DescribeWorkspace`, `UpdateWorkspace`) rather than being handed an id that service
/// never minted.
///
/// Amazon Managed Grafana ASSIGNS the id — `Ref` of the type returns the workspace id,
/// a `^g-[0-9a-f]{10}$` value, while the type's `Name` is only what the create asks for —
/// so the id this call answers with becomes the resource's physical id, replacing the
/// stand-in the naming rules computed.
pub(crate) async fn create_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if let Some(property) = GRAFANA_UNMAPPED_PROPERTIES
        .iter()
        .find(|property| properties.contains_key(**property))
    {
        // The property is not mapped onto `CreateWorkspace`, so creating the workspace
        // would create one configured differently from the declaration. Refused rather
        // than silently dropped; mapping it is its own fix.
        return Err(Refusal {
            code: "NotImplemented".to_string(),
            message: format!(
                "this emulator does not implement the {property} property of {}",
                resource.resource_type
            ),
        });
    }
    let mut request = Map::new();
    copy_members(&properties, GRAFANA_WORKSPACE_MEMBERS, &mut request);
    request.insert(
        "workspaceName".to_string(),
        Value::String(declared_name(resource)),
    );
    if let Some(configuration) = properties.get("VpcConfiguration") {
        request.insert(
            "vpcConfiguration".to_string(),
            mapped_members(configuration, GRAFANA_VPC_MEMBERS),
        );
    }
    if let Some(configuration) = properties.get("NetworkAccessControl") {
        request.insert(
            "networkAccessControl".to_string(),
            mapped_members(configuration, GRAFANA_NETWORK_ACCESS_MEMBERS),
        );
    }
    let reported = call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "POST",
        GRAFANA_WORKSPACES_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(id) = reported
        .get("workspace")
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
    else {
        // The create answered a success identifying no workspace, so there is nothing a
        // client could call Amazon Managed Grafana with: reported as a refusal rather
        // than recorded as a created resource.
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "CreateWorkspace answered no workspace id".to_string(),
        });
    };
    Ok(Created {
        physical_resource_id: Some(id.to_string()),
        attributes: BTreeMap::new(),
    })
}

/// Delete the workspace a stack created, by the id Amazon Managed Grafana minted for it —
/// this resource's physical id. A workspace that service no longer holds
/// (`ResourceNotFoundException`) is already in the state the delete asks for.
pub(crate) async fn delete_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "DELETE",
        &grafana_workspace_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == GRAFANA_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon RDS: AWS::RDS::DBInstance, AWS::RDS::DBSubnetGroup,
// AWS::RDS::DBParameterGroup, AWS::RDS::DBClusterParameterGroup
// ---------------------------------------------------------------------------

/// Amazon RDS's signing name and API version, from its model (`sigv4Name: rds`, service
/// shape `AmazonRDSv19`, `version: 2014-10-31`), which speaks `awsQuery` — so its calls
/// ride on [`call_query`] rather than on [`call_json_rpc`].
pub(crate) const RDS_SIGNING_NAME: &str = "rds";
pub(crate) const RDS_API_VERSION: &str = "2014-10-31";

/// The error codes Amazon RDS answers for resources it does not hold:
/// `DBInstanceNotFoundFault`'s and `DBSubnetGroupNotFoundFault`'s own `awsQueryError`
/// codes, which is what an awsQuery client branches on.
pub(crate) const DB_INSTANCE_NOT_FOUND: &str = "DBInstanceNotFound";
pub(crate) const DB_SUBNET_GROUP_NOT_FOUND: &str = "DBSubnetGroupNotFoundFault";

/// `DBParameterGroupNotFoundFault`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="DeleteDBParameterGroup")`), which is what
/// an awsQuery client branches on for a parameter group Amazon RDS does not hold — one code
/// for both group kinds, since both levels' operations model that same error shape.
pub(crate) const DB_PARAMETER_GROUP_NOT_FOUND: &str = "DBParameterGroupNotFound";

/// How one member of a create reaches an awsQuery form body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryMember {
    /// One value under the member's own name (`Engine=mysql`).
    Scalar,
    /// A list, whose entries are indexed from 1 under the element name the list's own
    /// member `@xmlName` fixes (`SubnetIds.SubnetIdentifier.1`); the Smithy default for a
    /// list member that carries none is `member`. A STRUCTURED entry's own members follow
    /// the index (`Tags.Tag.1.Key`).
    List(&'static str),
    /// A STRUCTURE, whose own members follow the member's name (`Tier.Name=WebServer`,
    /// `SourceBundle.S3Bucket=…`), nested the same way however deep the structure goes.
    Structure,
}

/// The members `CreateDBInstance` takes, from Amazon RDS's own model
/// (`smithy_query(query="operation", operation="CreateDBInstance")`), with each list's
/// element name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="VpcSecurityGroupIdList")`).
///
/// This is the catalogue the declaration of an `AWS::RDS::DBInstance` is walked against:
/// the type's properties ARE these members, so a property whose name is one of them (or
/// renames to one, [`DB_INSTANCE_RENAMED_PROPERTIES`]) is passed through under it.
pub(crate) const CREATE_DB_INSTANCE_MEMBERS: &[(&str, QueryMember)] = &[
    ("AllocatedStorage", QueryMember::Scalar),
    ("AutoMinorVersionUpgrade", QueryMember::Scalar),
    ("AvailabilityZone", QueryMember::Scalar),
    ("BackupRetentionPeriod", QueryMember::Scalar),
    ("BackupTarget", QueryMember::Scalar),
    ("CACertificateIdentifier", QueryMember::Scalar),
    ("CharacterSetName", QueryMember::Scalar),
    ("CopyTagsToSnapshot", QueryMember::Scalar),
    ("CustomIamInstanceProfile", QueryMember::Scalar),
    ("DBCapacityReservationIdentifier", QueryMember::Scalar),
    ("DBClusterIdentifier", QueryMember::Scalar),
    ("DBInstanceClass", QueryMember::Scalar),
    ("DBInstanceIdentifier", QueryMember::Scalar),
    ("DBName", QueryMember::Scalar),
    ("DBParameterGroupName", QueryMember::Scalar),
    ("DBSecurityGroups", QueryMember::List("DBSecurityGroupName")),
    ("DBSubnetGroupName", QueryMember::Scalar),
    ("DBSystemId", QueryMember::Scalar),
    ("DatabaseInsightsMode", QueryMember::Scalar),
    ("DedicatedLogVolume", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("Domain", QueryMember::Scalar),
    ("DomainAuthSecretArn", QueryMember::Scalar),
    ("DomainDnsIps", QueryMember::List("member")),
    ("DomainFqdn", QueryMember::Scalar),
    ("DomainIAMRoleName", QueryMember::Scalar),
    ("DomainOu", QueryMember::Scalar),
    ("EnableCloudwatchLogsExports", QueryMember::List("member")),
    ("EnableCustomerOwnedIp", QueryMember::Scalar),
    ("EnableIAMDatabaseAuthentication", QueryMember::Scalar),
    ("EnablePerformanceInsights", QueryMember::Scalar),
    ("Engine", QueryMember::Scalar),
    ("EngineLifecycleSupport", QueryMember::Scalar),
    ("EngineVersion", QueryMember::Scalar),
    ("Iops", QueryMember::Scalar),
    ("KmsKeyId", QueryMember::Scalar),
    ("LicenseModel", QueryMember::Scalar),
    ("ManageMasterUserPassword", QueryMember::Scalar),
    ("MasterUserAuthenticationType", QueryMember::Scalar),
    ("MasterUserPassword", QueryMember::Scalar),
    ("MasterUserSecretKmsKeyId", QueryMember::Scalar),
    ("MasterUsername", QueryMember::Scalar),
    ("MaxAllocatedStorage", QueryMember::Scalar),
    ("MonitoringInterval", QueryMember::Scalar),
    ("MonitoringRoleArn", QueryMember::Scalar),
    ("MultiAZ", QueryMember::Scalar),
    ("MultiTenant", QueryMember::Scalar),
    ("NcharCharacterSetName", QueryMember::Scalar),
    ("NetworkType", QueryMember::Scalar),
    ("OptionGroupName", QueryMember::Scalar),
    ("PerformanceInsightsKMSKeyId", QueryMember::Scalar),
    ("PerformanceInsightsRetentionPeriod", QueryMember::Scalar),
    ("Port", QueryMember::Scalar),
    ("PreferredBackupWindow", QueryMember::Scalar),
    ("PreferredMaintenanceWindow", QueryMember::Scalar),
    ("ProcessorFeatures", QueryMember::List("ProcessorFeature")),
    ("PromotionTier", QueryMember::Scalar),
    ("PubliclyAccessible", QueryMember::Scalar),
    ("StorageEncrypted", QueryMember::Scalar),
    ("StorageThroughput", QueryMember::Scalar),
    ("StorageType", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
    ("TdeCredentialArn", QueryMember::Scalar),
    ("TdeCredentialPassword", QueryMember::Scalar),
    ("Timezone", QueryMember::Scalar),
    ("VpcSecurityGroupIds", QueryMember::List("VpcSecurityGroupId")),
];

/// The two properties of `AWS::RDS::DBInstance` whose `CreateDBInstance` member is spelled
/// differently: the resource type's `VPCSecurityGroups` is the API's
/// `VpcSecurityGroupIds` ("a list of the VPC security group IDs to assign to the DB
/// instance"), and its `CustomIAMInstanceProfile` is the API's
/// `CustomIamInstanceProfile`. Every other property is its member's own name.
pub(crate) const DB_INSTANCE_RENAMED_PROPERTIES: &[(&str, &str)] = &[
    ("VPCSecurityGroups", "VpcSecurityGroupIds"),
    ("CustomIAMInstanceProfile", "CustomIamInstanceProfile"),
];

/// The properties of `AWS::RDS::DBInstance` the walk over the declaration does NOT turn
/// into a `CreateDBInstance` member, each for a reason of its own. Every other property
/// the catalogue does not carry is refused rather than dropped ([`query_members`]).
pub(crate) const DB_INSTANCE_PROPERTIES_NOT_SENT: &[&str] = &[
    // The identifier CloudFormation decides, which is this resource's physical id ("Ref
    // returns the DBInstanceIdentifier"): the arm sets the member from that id, so the
    // walk must not also push the declared spelling of it.
    "DBInstanceIdentifier",
    // The tag list carries the declared tags AND the ones CloudFormation propagates to the
    // resource, so the arm builds it out of both ([`tags_of`]).
    "Tags",
    // Members of a DIFFERENT call, which say nothing about the instance a create makes:
    // `ModifyDBInstance` takes `AllowMajorVersionUpgrade`, `ApplyImmediately`,
    // `CertificateRotationRestart` and `UseDefaultProcessorFeatures` (each governs how a
    // LATER modification is applied), and `DeleteDBInstance` takes
    // `DeleteAutomatedBackups` — which is why [`delete_db_instance`] reads that one off
    // the resource's own record when it deletes the instance.
    "AllowMajorVersionUpgrade",
    "ApplyImmediately",
    "CertificateRotationRestart",
    "UseDefaultProcessorFeatures",
    "DeleteAutomatedBackups",
    // Read-only members CloudFormation publishes as this type's RETURN values: a create
    // neither takes them nor could be told them.
    "CertificateDetails",
    "DbiResourceId",
    "Endpoint",
    "MasterUserSecret",
];

/// The members `CreateDBSubnetGroup` takes, from the same model, read the same way.
pub(crate) const CREATE_DB_SUBNET_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("DBSubnetGroupDescription", QueryMember::Scalar),
    ("DBSubnetGroupName", QueryMember::Scalar),
    ("SubnetIds", QueryMember::List("SubnetIdentifier")),
    ("Tags", QueryMember::List("Tag")),
];

/// `AWS::RDS::DBSubnetGroup`'s two properties the arm itself pushes, for the same reasons
/// the DB instance's are ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]): the name CloudFormation
/// decides, and the tag list it merges.
pub(crate) const DB_SUBNET_GROUP_PROPERTIES_NOT_SENT: &[&str] = &["DBSubnetGroupName", "Tags"];

/// One Amazon RDS parameter-group TYPE, as the calls that make a group of it exist, carry a
/// caller's settings, stop existing and be read.
///
/// `AWS::RDS::DBParameterGroup` and `AWS::RDS::DBClusterParameterGroup` declare the same
/// document — `Description`, `Family`, the `Parameters` map, the name and the tag list — and
/// their owning APIs differ only in the member each addresses a group by and in the action
/// names, so ONE arm serves both and the difference between them is this record.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParameterGroupCalls {
    /// The resource type, which a property outside the create's catalogue is refused by
    /// name ([`query_members`]).
    pub(crate) resource_type: &'static str,
    /// The member every one of these calls addresses the group by, which is also what this
    /// type's `Ref` returns and so this resource's physical id.
    pub(crate) name_member: &'static str,
    /// The members the create takes, from Amazon RDS's own model, with each list's element
    /// name from its list shape's member `@xmlName`
    /// (`smithy_query(query="shape", shape="TagList")`).
    pub(crate) create_members: &'static [(&'static str, QueryMember)],
    pub(crate) create: &'static str,
    pub(crate) modify: &'static str,
    pub(crate) delete: &'static str,
    pub(crate) describe: &'static str,
    /// The element the describe reports one group as.
    pub(crate) record: &'static str,
}

impl ParameterGroupCalls {
    /// The declared properties the walk does NOT turn into a create member: the name
    /// CloudFormation decides and the tag list it merges, as for the two types above, plus
    /// the `Parameters` MAP, which is no member of the create at all — a group's parameter
    /// values are written by the modify [`modify_parameters`] makes next.
    pub(crate) fn properties_not_sent(&self) -> [&'static str; 3] {
        [self.name_member, "Parameters", "Tags"]
    }
}

/// `AWS::RDS::DBParameterGroup`, whose group is the one a DB instance's
/// `DBParameterGroupName` names and `DescribeDBParameters` reports the parameters of
/// (`smithy_query(query="operation", operation="CreateDBParameterGroup")`).
pub(crate) const DB_PARAMETER_GROUP_CALLS: ParameterGroupCalls = ParameterGroupCalls {
    resource_type: RDS_DB_PARAMETER_GROUP,
    name_member: "DBParameterGroupName",
    create_members: &[
        ("DBParameterGroupFamily", QueryMember::Scalar),
        ("DBParameterGroupName", QueryMember::Scalar),
        ("Description", QueryMember::Scalar),
        ("Tags", QueryMember::List("Tag")),
    ],
    create: "CreateDBParameterGroup",
    modify: "ModifyDBParameterGroup",
    delete: "DeleteDBParameterGroup",
    describe: "DescribeDBParameterGroups",
    record: "DBParameterGroup",
};

/// `AWS::RDS::DBClusterParameterGroup`, the cluster-scoped sibling: the group a DB cluster's
/// `DBClusterParameterGroupName` names and `DescribeDBClusterParameters` reports the
/// parameters of. Its create takes the family under the SAME member name as the
/// instance-scoped one (`DBParameterGroupFamily`) and the group under its own
/// (`smithy_query(query="operation", operation="CreateDBClusterParameterGroup")`).
pub(crate) const DB_CLUSTER_PARAMETER_GROUP_CALLS: ParameterGroupCalls = ParameterGroupCalls {
    resource_type: RDS_DB_CLUSTER_PARAMETER_GROUP,
    name_member: "DBClusterParameterGroupName",
    create_members: &[
        ("DBClusterParameterGroupName", QueryMember::Scalar),
        ("DBParameterGroupFamily", QueryMember::Scalar),
        ("Description", QueryMember::Scalar),
        ("Tags", QueryMember::List("Tag")),
    ],
    create: "CreateDBClusterParameterGroup",
    modify: "ModifyDBClusterParameterGroup",
    delete: "DeleteDBClusterParameterGroup",
    describe: "DescribeDBClusterParameterGroups",
    record: "DBClusterParameterGroup",
};

/// The one property of either parameter-group type whose create member is spelled
/// differently: the types' `Family` is the API's `DBParameterGroupFamily` ("the DB parameter
/// group family name"). Every other property is its member's own name.
pub(crate) const PARAMETER_GROUP_RENAMED_PROPERTIES: &[(&str, &str)] =
    &[("Family", "DBParameterGroupFamily")];

/// How many parameters one modify carries: "A maximum of 20 parameters can be modified in a
/// single request" (`ModifyDBParameterGroupMessage$Parameters`, and the cluster-level
/// request's own `Parameters`), so a declaration carrying more reaches Amazon RDS as several
/// calls rather than as one request that service would refuse.
pub(crate) const PARAMETERS_PER_MODIFY: usize = 20;

/// Create the DB instance an `AWS::RDS::DBInstance` declares.
///
/// The type's properties are `CreateDBInstance`'s own members
/// ([`CREATE_DB_INSTANCE_MEMBERS`]), so the evaluated declaration IS the request, with the
/// one value CloudFormation decides — the `DBInstanceIdentifier`, which is this resource's
/// physical id — set on the way out, and the tag list built from the declared tags plus the
/// ones CloudFormation propagates.
///
/// What the create ANSWERS replaces the identifiers CloudFormation's naming rules had
/// computed for the resource: the ARN, the `DbiResourceId` and the endpoint are Amazon
/// RDS's to assign, and the whole point of provisioning the instance is that the handle the
/// stack publishes addresses the database RDS really holds. The identifier itself is not
/// replaced — it is the one the instance was created under.
pub(crate) async fn create_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_DB_INSTANCE_MEMBERS,
        DB_INSTANCE_RENAMED_PROPERTIES,
        DB_INSTANCE_PROPERTIES_NOT_SENT,
        RDS_DB_INSTANCE,
    )?;
    parameters.push((
        "DBInstanceIdentifier".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "CreateDBInstance",
        &parameters,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "DBInstanceArn").filter(|arn| !arn.is_empty()) {
        // The type publishes one ARN, so the conventional `Arn` attribute every type
        // answers is that same value ([`super::template::conventional_attributes`]): both
        // are re-pointed at the ARN the instance really carries.
        attributes.insert("Arn".to_string(), arn.clone());
        attributes.insert("DBInstanceArn".to_string(), arn);
    }
    if let Some(resource_id) =
        xml_element(&answered, "DbiResourceId").filter(|id| !id.is_empty())
    {
        attributes.insert("DbiResourceId".to_string(), resource_id);
    }
    // `Endpoint` is a structure (`<Endpoint><Address/><Port/>…`), and the two members the
    // type publishes are read off it by name.
    let endpoint = xml_scalar_children(&answered, "Endpoint");
    for member in ["Address", "Port"] {
        if let Some(value) = endpoint.get(member).and_then(scalar_text) {
            attributes.insert(format!("Endpoint.{member}"), value);
        }
    }
    Ok(Created::attributes(attributes))
}

/// Delete the DB instance a stack created, addressed by its identifier — this resource's
/// physical id. An instance Amazon RDS no longer has (`DBInstanceNotFound`) is already in
/// the state the delete asks for.
///
/// `DeleteDBInstance` requires the caller to say what becomes of the data: "you must
/// specify the `FinalDBSnapshotIdentifier` parameter" unless the snapshot is skipped. Which
/// of the two the call carries is the resource's `DeletionPolicy`
/// ([`super::provisioning::Policies`]): `final_snapshot` is the identifier a `Snapshot`
/// policy's snapshot is taken under, and without one the call is the form CloudFormation
/// makes for the `Delete` policy, which skips the final snapshot. The one member the type's
/// own declaration carries for the delete, `DeleteAutomatedBackups`, is read off the record
/// and sent as declared.
///
/// Amazon RDS answers the delete by putting the instance in `deleting` and removing it a
/// moment later, so this arm then WAITS for it to be gone
/// ([`confirm_db_instance_deleted`]): CloudFormation deletes a stack's resources in
/// dependency order and waits for each deletion to complete before it deletes what that
/// resource sat in, and a DB subnet group whose instance is still there is still in use.
pub(crate) async fn delete_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    final_snapshot: Option<&str>,
) -> Result<(), Refusal> {
    let mut parameters = vec![(
        "DBInstanceIdentifier".to_string(),
        resource.physical_resource_id.clone(),
    )];
    match final_snapshot {
        // "The DBSnapshotIdentifier of the new DBSnapshot created when the SkipFinalSnapshot
        // parameter is disabled" — and enabling both "results in an error", so the skip is
        // not sent beside it.
        Some(snapshot_identifier) => parameters.push((
            "FinalDBSnapshotIdentifier".to_string(),
            snapshot_identifier.to_string(),
        )),
        None => parameters.push(("SkipFinalSnapshot".to_string(), "true".to_string())),
    }
    if let Some(declared) = resource
        .properties
        .get("DeleteAutomatedBackups")
        .and_then(scalar_text)
    {
        parameters.push(("DeleteAutomatedBackups".to_string(), declared));
    }
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeleteDBInstance",
        &parameters,
    )
    .await
    {
        Ok(_) => {
            confirm_db_instance_deleted(ctx, region, &resource.physical_resource_id).await;
            Ok(())
        }
        Err(refusal) if refusal.code == DB_INSTANCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon Redshift: AWS::Redshift::Cluster
// ---------------------------------------------------------------------------

/// Amazon Redshift's signing name and API version, from its model (`sigv4Name: redshift`,
/// service shape `RedshiftServiceVersion20121201`, `version: 2012-12-01`), which speaks
/// `awsQuery` — so its calls ride on [`call_query`], exactly as Amazon RDS's do.
pub(crate) const REDSHIFT_SIGNING_NAME: &str = "redshift";
pub(crate) const REDSHIFT_API_VERSION: &str = "2012-12-01";

/// The error code Amazon Redshift answers for a cluster it does not hold:
/// `ClusterNotFoundFault`'s own `awsQueryError` code, which is what an awsQuery client
/// branches on.
pub(crate) const REDSHIFT_CLUSTER_NOT_FOUND: &str = "ClusterNotFound";

/// The members `CreateCluster` takes, from Amazon Redshift's own model
/// (`smithy_query(query="operation", operation="CreateCluster")`), with each list's element
/// name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="ClusterSecurityGroupNameList,IamRoleArnList,
/// VpcSecurityGroupIdList,TagList")`).
///
/// This is the catalogue the declaration of an `AWS::Redshift::Cluster` is walked against:
/// the type's properties ARE these members, spelled identically on both sides, so a
/// property whose name is one of them is passed through under it and one the catalogue does
/// not carry is refused rather than dropped ([`query_members`]).
pub(crate) const CREATE_CLUSTER_MEMBERS: &[(&str, QueryMember)] = &[
    ("AdditionalInfo", QueryMember::Scalar),
    ("AllowVersionUpgrade", QueryMember::Scalar),
    ("AquaConfigurationStatus", QueryMember::Scalar),
    ("AutomatedSnapshotRetentionPeriod", QueryMember::Scalar),
    ("AvailabilityZone", QueryMember::Scalar),
    ("AvailabilityZoneRelocation", QueryMember::Scalar),
    ("CatalogName", QueryMember::Scalar),
    ("ClusterIdentifier", QueryMember::Scalar),
    ("ClusterParameterGroupName", QueryMember::Scalar),
    (
        "ClusterSecurityGroups",
        QueryMember::List("ClusterSecurityGroupName"),
    ),
    ("ClusterSubnetGroupName", QueryMember::Scalar),
    ("ClusterType", QueryMember::Scalar),
    ("ClusterVersion", QueryMember::Scalar),
    ("DBName", QueryMember::Scalar),
    ("DefaultIamRoleArn", QueryMember::Scalar),
    ("ElasticIp", QueryMember::Scalar),
    ("Encrypted", QueryMember::Scalar),
    ("EnhancedVpcRouting", QueryMember::Scalar),
    ("ExtraComputeForAutomaticOptimization", QueryMember::Scalar),
    ("HsmClientCertificateIdentifier", QueryMember::Scalar),
    ("HsmConfigurationIdentifier", QueryMember::Scalar),
    ("IamRoles", QueryMember::List("IamRoleArn")),
    ("IpAddressType", QueryMember::Scalar),
    ("KmsKeyId", QueryMember::Scalar),
    ("LoadSampleData", QueryMember::Scalar),
    ("MaintenanceTrackName", QueryMember::Scalar),
    ("ManageMasterPassword", QueryMember::Scalar),
    ("ManualSnapshotRetentionPeriod", QueryMember::Scalar),
    ("MasterPasswordSecretKmsKeyId", QueryMember::Scalar),
    ("MasterUserPassword", QueryMember::Scalar),
    ("MasterUsername", QueryMember::Scalar),
    ("MultiAZ", QueryMember::Scalar),
    ("NodeType", QueryMember::Scalar),
    ("NumberOfNodes", QueryMember::Scalar),
    ("Port", QueryMember::Scalar),
    ("PreferredMaintenanceWindow", QueryMember::Scalar),
    ("PubliclyAccessible", QueryMember::Scalar),
    ("RedshiftIdcApplicationArn", QueryMember::Scalar),
    ("SnapshotScheduleIdentifier", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
    (
        "VpcSecurityGroupIds",
        QueryMember::List("VpcSecurityGroupId"),
    ),
];

/// The properties of `AWS::Redshift::Cluster` the walk over the declaration does NOT turn
/// into a `CreateCluster` member, each for a reason of its own — the same split the DB
/// instance's catalogue is in ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]).
pub(crate) const CLUSTER_PROPERTIES_NOT_SENT: &[&str] = &[
    // The identifier CloudFormation decides, which is this resource's physical id ("Ref
    // returns the name of the cluster"): the arm sets the member from that id, so the walk
    // must not also push the declared spelling of it.
    "ClusterIdentifier",
    // The tag list carries the declared tags AND the ones CloudFormation propagates to the
    // resource, so the arm builds it out of both ([`tags_of`]).
    "Tags",
    // Members of a DIFFERENT Amazon Redshift call, which say nothing about the cluster a
    // create makes: `EnableLogging` takes the logging destination, `EnableSnapshotCopy` the
    // cross-Region copy settings, `ModifyClusterMaintenance` the deferral window,
    // `ModifyCluster` the in-place actions and the revision to move to,
    // `RestoreFromClusterSnapshot` the snapshot a cluster is rebuilt from, and
    // `PutResourcePolicy` the namespace's resource policy.
    "Classic",
    "DeferMaintenance",
    "DeferMaintenanceDuration",
    "DeferMaintenanceEndTime",
    "DeferMaintenanceStartTime",
    "DestinationRegion",
    "LoggingProperties",
    "NamespaceResourcePolicy",
    "OwnerAccount",
    "ResourceAction",
    "RevisionTarget",
    "RotateEncryptionKey",
    "SnapshotClusterIdentifier",
    "SnapshotCopyGrantName",
    "SnapshotCopyManual",
    "SnapshotCopyRetentionPeriod",
    "SnapshotIdentifier",
    // Read-only members CloudFormation publishes as this type's RETURN values: a create
    // neither takes them nor could be told them.
    "Endpoint",
];

/// Create the cluster an `AWS::Redshift::Cluster` declares.
///
/// The type's properties are `CreateCluster`'s own members ([`CREATE_CLUSTER_MEMBERS`]), so
/// the evaluated declaration IS the request, with the one value CloudFormation decides —
/// the `ClusterIdentifier`, which is this resource's physical id — set on the way out, and
/// the tag list built from the declared tags plus the ones CloudFormation propagates.
///
/// The cluster is created in the service that OWNS it, so Amazon Redshift's own
/// `DescribeClusters` reports the cluster the stack claims to have created. Its identity is
/// CloudFormation's ("Ref returns the name of the cluster"), so nothing here replaces the
/// physical id, and the endpoint is not an attribute this create can report: Amazon Redshift
/// publishes a cluster's endpoint only once that cluster is serving, so the resource's
/// creation is held open until it reports it `available` ([`redshift_cluster_readiness`]) and
/// the type's endpoint attributes are read off it then
/// ([`redshift_cluster_ready_attributes`]).
pub(crate) async fn create_redshift_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_CLUSTER_MEMBERS,
        // Every property of this type is its member's own name.
        &[],
        CLUSTER_PROPERTIES_NOT_SENT,
        REDSHIFT_CLUSTER,
    )?;
    parameters.push((
        "ClusterIdentifier".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "CreateCluster",
        &parameters,
    )
    .await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the cluster a stack created, addressed by its identifier — this resource's
/// physical id. A cluster Amazon Redshift no longer has (`ClusterNotFound`) is already in
/// the state the delete asks for.
///
/// `DeleteCluster` requires the caller to say what becomes of the data: a final snapshot
/// identifier, or the skip. Which one the call carries is the resource's `DeletionPolicy`,
/// exactly as it is for [`delete_db_instance`]: `final_snapshot` is the identifier a
/// `Snapshot` policy's snapshot is taken under, and without one the call skips the final
/// snapshot, which is the form CloudFormation makes for the `Delete` policy.
pub(crate) async fn delete_redshift_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    final_snapshot: Option<&str>,
) -> Result<(), Refusal> {
    let mut parameters = vec![(
        "ClusterIdentifier".to_string(),
        physical_resource_id.to_string(),
    )];
    match final_snapshot {
        // "The FinalClusterSnapshotIdentifier parameter must be specified if
        // SkipFinalClusterSnapshot is false", and the two cannot be sent together.
        Some(snapshot_identifier) => parameters.push((
            "FinalClusterSnapshotIdentifier".to_string(),
            snapshot_identifier.to_string(),
        )),
        None => parameters.push((
            "SkipFinalClusterSnapshot".to_string(),
            "true".to_string(),
        )),
    }
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DeleteCluster",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REDSHIFT_CLUSTER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The workspace's configuration as Amazon Managed Grafana reports it, for drift
/// detection: `DescribeWorkspace`'s own record, read by the id that service minted.
///
/// A workspace it no longer holds (`ResourceNotFoundException`) was deleted. The record's
/// members are that service's own (lower camel, `workspace`-prefixed for several), so only
/// a declared property spelled identically on both sides takes part in the comparison —
/// the position the Amazon ECS and AWS FIS arms above are in too, and what [`super::drift`]
/// documents: a name mismatch reports the resource as existing rather than as modified.
pub(crate) async fn describe_workspace(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_rest_json(
        ctx,
        GRAFANA_SIGNING_NAME,
        region,
        "GET",
        &grafana_workspace_uri(physical_resource_id),
        None,
    )
    .await
    {
        Ok(answer) => Ok(answer.get("workspace").cloned()),
        Err(refusal) if refusal.code == GRAFANA_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// How often this module checks whether an accepted DB instance deletion has finished, and
/// how many times.
///
/// The product is the longest a stack's deletion waits for ONE instance to go: an upper
/// bound on the wait, not a duration anything is owed. Amazon RDS reports a deleted
/// instance as `deleting` and removes it a moment later (its own transition span), so the
/// check is far shorter than that moment and the whole wait is a small multiple of it —
/// enough that a loaded machine still gets there, and bounded so a `DeleteStack` of several
/// instances cannot hold its request open.
pub(crate) const DELETION_CHECK_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(25);
pub(crate) const DELETION_CHECKS: usize = 24;

/// Wait until Amazon RDS no longer holds the instance whose deletion it has ACCEPTED.
///
/// Returns as soon as the instance is gone, and gives up after [`DELETION_CHECKS`] checks
/// — the delete itself succeeded, so a wait that runs out is not a failure of it; what
/// fails then is whatever the next delete of the same stack needs the instance gone for,
/// reported as that service's own refusal. Any other refusal of the READ ends the wait the
/// same way: it says nothing about whether the instance is there, and the delete was
/// accepted either way.
pub(crate) async fn confirm_db_instance_deleted(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) {
    for _ in 0..DELETION_CHECKS {
        match describe_db_instance(ctx, region, physical_resource_id).await {
            // The instance is gone: the deletion RDS accepted has finished.
            Ok(None) | Err(_) => return,
            Ok(Some(_)) => {}
        }
        ::tokio::time::sleep(DELETION_CHECK_INTERVAL).await;
    }
}

/// The DB instance's configuration as Amazon RDS reports it, for drift detection. An
/// instance it no longer has (`DBInstanceNotFound`) was deleted.
///
/// A DB instance is reported as a STRUCTURE whose scalar members are named exactly as this
/// type's properties are (`Engine`, `DBInstanceClass`, `AllocatedStorage`, `MultiAZ`), so
/// those members are the actual configuration the declared one is compared against. Its
/// composite members are not: `VpcSecurityGroups`, `DBParameterGroups` and `Subnets` are
/// reported as MEMBERSHIP structures carrying a status, where the properties are plain id
/// and name lists, so comparing either would need a mapping this module would have to
/// invent — and a property the actual document does not carry is not evidence of anything
/// ([`super::drift`]).
pub(crate) async fn describe_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "DBInstanceIdentifier".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBInstances",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "DBInstance",
        )))),
        Err(refusal) if refusal.code == DB_INSTANCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Apply a CHANGED declaration to the DB instance a stack already created, through
/// `ModifyDBInstance` — the call Amazon RDS changes an instance's own settings with.
///
/// Only the settings the modification TAKES are sent ([`MODIFY_DB_INSTANCE_MEMBERS`]): a
/// create-only setting is not something a modification can change, so a declaration that
/// changed one is applied as far as the modification reaches and the rest is left to the
/// replacement CloudFormation decides elsewhere. `ApplyImmediately` is sent as the
/// declaration asks, and defaults to applying the change at once, because a stack reports a
/// resource updated when the update is done rather than when the instance's next maintenance
/// window comes round.
pub(crate) async fn update_db_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let mut parameters = query_members(
        declared,
        MODIFY_DB_INSTANCE_MEMBERS,
        DB_INSTANCE_RENAMED_PROPERTIES,
        DB_INSTANCE_UPDATE_PROPERTIES_NOT_SENT,
        RDS_DB_INSTANCE,
    )?;
    parameters.push((
        "DBInstanceIdentifier".to_string(),
        resource.physical_resource_id.clone(),
    ));
    if !parameters
        .iter()
        .any(|(member, _)| member == "ApplyImmediately")
    {
        parameters.push(("ApplyImmediately".to_string(), "true".to_string()));
    }
    call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "ModifyDBInstance",
        &parameters,
    )
    .await?;
    // The instance keeps the identity it was created with — the ARN, the resource id and
    // the endpoint all follow the identifier — so a modification reports no new attribute.
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// The members `ModifyDBInstance` takes that an `AWS::RDS::DBInstance` declaration can
/// carry, from Amazon RDS's own model (`smithy_query(query="operation",
/// operation="ModifyDBInstance")`): the create's catalogue plus the one member only a
/// modification has.
const MODIFY_DB_INSTANCE_MEMBERS: &[(&str, QueryMember)] = &[
    ("AllocatedStorage", QueryMember::Scalar),
    ("ApplyImmediately", QueryMember::Scalar),
    ("AutoMinorVersionUpgrade", QueryMember::Scalar),
    ("BackupRetentionPeriod", QueryMember::Scalar),
    ("CACertificateIdentifier", QueryMember::Scalar),
    ("CopyTagsToSnapshot", QueryMember::Scalar),
    ("DBParameterGroupName", QueryMember::Scalar),
    ("DBSecurityGroups", QueryMember::List("DBSecurityGroupName")),
    ("DBSubnetGroupName", QueryMember::Scalar),
    ("DatabaseInsightsMode", QueryMember::Scalar),
    ("DedicatedLogVolume", QueryMember::Scalar),
    ("DeletionProtection", QueryMember::Scalar),
    ("Domain", QueryMember::Scalar),
    ("DomainAuthSecretArn", QueryMember::Scalar),
    ("DomainDnsIps", QueryMember::List("member")),
    ("DomainFqdn", QueryMember::Scalar),
    ("DomainIAMRoleName", QueryMember::Scalar),
    ("DomainOu", QueryMember::Scalar),
    ("EnableCustomerOwnedIp", QueryMember::Scalar),
    ("EnableIAMDatabaseAuthentication", QueryMember::Scalar),
    ("EnablePerformanceInsights", QueryMember::Scalar),
    ("Engine", QueryMember::Scalar),
    ("EngineVersion", QueryMember::Scalar),
    ("Iops", QueryMember::Scalar),
    ("LicenseModel", QueryMember::Scalar),
    ("ManageMasterUserPassword", QueryMember::Scalar),
    ("MasterUserPassword", QueryMember::Scalar),
    ("MasterUserSecretKmsKeyId", QueryMember::Scalar),
    ("MaxAllocatedStorage", QueryMember::Scalar),
    ("MonitoringInterval", QueryMember::Scalar),
    ("MonitoringRoleArn", QueryMember::Scalar),
    ("MultiAZ", QueryMember::Scalar),
    ("MultiTenant", QueryMember::Scalar),
    ("NetworkType", QueryMember::Scalar),
    ("OptionGroupName", QueryMember::Scalar),
    ("PerformanceInsightsKMSKeyId", QueryMember::Scalar),
    ("PerformanceInsightsRetentionPeriod", QueryMember::Scalar),
    ("PreferredBackupWindow", QueryMember::Scalar),
    ("PreferredMaintenanceWindow", QueryMember::Scalar),
    ("ProcessorFeatures", QueryMember::List("ProcessorFeature")),
    ("PromotionTier", QueryMember::Scalar),
    ("PubliclyAccessible", QueryMember::Scalar),
    ("StorageThroughput", QueryMember::Scalar),
    ("StorageType", QueryMember::Scalar),
    ("VpcSecurityGroupIds", QueryMember::List("VpcSecurityGroupId")),
];

/// The properties the UPDATE does not send: the create's own list, plus every property that
/// is not a member of `ModifyDBInstance` at all — where the instance was placed, what it was
/// created from and the cluster it joined, none of which that call can change.
const DB_INSTANCE_UPDATE_PROPERTIES_NOT_SENT: &[&str] = &[
    "DBInstanceIdentifier",
    "Tags",
    "AvailabilityZone",
    "BackupTarget",
    "CertificateDetails",
    "CharacterSetName",
    "CustomIamInstanceProfile",
    "CustomIAMInstanceProfile",
    "DBClusterIdentifier",
    "DBInstanceClass",
    "DBName",
    "DBSystemId",
    "DbiResourceId",
    "DeleteAutomatedBackups",
    "EnableCloudwatchLogsExports",
    "Endpoint",
    "EngineLifecycleSupport",
    "KmsKeyId",
    "MasterUserAuthenticationType",
    "MasterUserSecret",
    "MasterUsername",
    "NcharCharacterSetName",
    "Port",
    "SourceDBInstanceIdentifier",
    "SourceDbiResourceId",
    "SourceRegion",
    "StorageEncrypted",
    "TdeCredentialArn",
    "TdeCredentialPassword",
    "Timezone",
    "UseDefaultProcessorFeatures",
    "AllowMajorVersionUpgrade",
    "CertificateRotationRestart",
];

/// The `DBInstance$DBInstanceStatus` an instance SERVES in, and the ones a new instance is
/// still coming up out of ("Viewing DB instance status" in the Amazon RDS User Guide lists
/// the statuses an instance is reported in, of which these are the ones a create and a
/// modification pass through).
///
/// An instance in any other status is in one no wait brings a NEW instance out of, which is
/// a resource that failed to come up rather than one still coming.
pub(crate) const DB_INSTANCE_STATUS_AVAILABLE: &str = "available";
pub(crate) const DB_INSTANCE_STATUSES_COMING_UP: &[&str] = &[
    "creating",
    "backing-up",
    "configuring-enhanced-monitoring",
    "configuring-log-exports",
    "maintenance",
    "modifying",
    "rebooting",
    "starting",
    "upgrading",
];

/// Whether Amazon RDS reports the DB instance a stack created UP.
///
/// `CreateDBInstance` answers the instance `creating`, in which it serves no connection, so
/// the resource's creation is held open until that service reports it `available` — which is
/// what makes a stack that declares a database and something that uses it complete only once
/// the database is really there. A status no new instance comes out of, and an instance
/// Amazon RDS no longer holds, is a failure, because nothing is going to bring it up.
pub(crate) async fn db_instance_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(reported) = describe_db_instance(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon RDS holds no DB instance named {physical_resource_id}"
        )));
    };
    let status = reported
        .get("DBInstanceStatus")
        .and_then(scalar_text)
        .unwrap_or_default();
    if status == DB_INSTANCE_STATUS_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if DB_INSTANCE_STATUSES_COMING_UP.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon RDS reports the DB instance {physical_resource_id} in status {status}"
    )))
}

/// The cluster's configuration as Amazon Redshift reports it, for drift detection. A
/// cluster it no longer has (`ClusterNotFound`) was deleted.
///
/// A cluster is reported as a STRUCTURE whose scalar members are named exactly as this
/// type's properties are (`NodeType`, `ClusterType`, `NumberOfNodes`, `DBName`,
/// `PubliclyAccessible`), so those members are the actual configuration the declared one is
/// compared against — the same position the DB instance's read is in
/// ([`describe_db_instance`]), and for the same reason its composite members take no part.
pub(crate) async fn describe_redshift_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_redshift_cluster(ctx, region, physical_resource_id)
        .await?
        .map(|answered| Value::Object(xml_scalar_children(&answered, "Cluster"))))
}

/// Amazon Redshift's OWN answer for the cluster `physical_resource_id`, as that service
/// wrote it, or `None` for a cluster it no longer has (`ClusterNotFound`).
///
/// The ONE read behind the drift document ([`describe_redshift_cluster`]), the readiness
/// answer ([`redshift_cluster_readiness`]) and the endpoint attributes
/// ([`redshift_cluster_ready_attributes`]), so the three ask that service the same way and
/// each reads the part of one answer it is about — the cluster's scalar members for the
/// first two, its `Endpoint` structure for the third.
pub(crate) async fn read_redshift_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<String>, Refusal> {
    let parameters = vec![(
        "ClusterIdentifier".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DescribeClusters",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(answered)),
        Err(refusal) if refusal.code == REDSHIFT_CLUSTER_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `Cluster$ClusterStatus` a provisioned cluster SERVES in, and the ones a new cluster is
/// still coming up out of (that member's own doc lists the statuses a cluster is reported in:
/// `available`, `available, prep-for-resize`, `available, resize-cleanup`,
/// `cancelling-resize`, `creating`, `deleting`, `final-snapshot`, `hardware-failure`,
/// `incompatible-hsm`, `incompatible-network`, `incompatible-parameters`,
/// `incompatible-restore`, `modifying`, `paused`, `rebooting`, `renaming`, `resizing`,
/// `rotating-keys`, `storage-full`, `updating-hsm`).
///
/// A cluster in any other status is in one no wait brings a NEW cluster out of, which is a
/// resource that failed to come up rather than one still coming.
pub(crate) const REDSHIFT_CLUSTER_STATUS_AVAILABLE: &str = "available";
pub(crate) const REDSHIFT_CLUSTER_STATUSES_COMING_UP: &[&str] =
    &["creating", "modifying", "resizing"];

/// Whether Amazon Redshift reports the cluster a stack created UP.
///
/// `CreateCluster` answers the cluster `creating`, and in that status it serves no connection
/// and publishes no `Cluster$Endpoint` — "the connection endpoint" is reported once the
/// cluster is available — so the resource's creation is held open until that service reports
/// it there. A status no new cluster comes out of, and a cluster Amazon Redshift no longer
/// holds, is a failure, because nothing is going to bring it up.
pub(crate) async fn redshift_cluster_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let reported = read_redshift_cluster(ctx, region, physical_resource_id)
        .await?
        .map(|answered| xml_scalar_children(&answered, "Cluster"))
        .filter(|reported| !reported.is_empty());
    let Some(reported) = reported else {
        return Ok(Readiness::Failed(format!(
            "Amazon Redshift holds no cluster named {physical_resource_id}"
        )));
    };
    let status = reported
        .get("ClusterStatus")
        .and_then(scalar_text)
        .unwrap_or_default();
    if status == REDSHIFT_CLUSTER_STATUS_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if REDSHIFT_CLUSTER_STATUSES_COMING_UP.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon Redshift reports the cluster {physical_resource_id} in status {status}"
    )))
}

/// The endpoint attributes `AWS::Redshift::Cluster` publishes, read off the cluster once
/// Amazon Redshift reports it serving.
///
/// That type's published return values include the two members of its connection endpoint —
/// `Endpoint.Address`, "the connection endpoint for the Amazon Redshift cluster", and
/// `Endpoint.Port`, "the port number on which the cluster accepts connections" — and Amazon
/// Redshift reports both as `Cluster$Endpoint`'s own `Address` and `Port`
/// (`smithy_query(query="shape", shape="Endpoint")`). A cluster that is still `creating`
/// carries no endpoint at all, so the create's answer could report neither: they are read
/// HERE, off the service that owns them, so a stack output reading either carries the address
/// a client really connects to rather than no value.
pub(crate) async fn redshift_cluster_ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut attributes = BTreeMap::new();
    let Some(answered) = read_redshift_cluster(ctx, region, physical_resource_id).await? else {
        return Ok(attributes);
    };
    // `Endpoint` is a STRUCTURE of the cluster's answer, so it is read out of the same
    // document on its own ([`xml_scalar_children`] leaves a composite member out), and its
    // own composite member (`VpcEndpoints`) is left out of it the same way.
    let endpoint = xml_scalar_children(&answered, "Endpoint");
    for member in ["Address", "Port"] {
        let Some(value) = endpoint.get(member).and_then(scalar_text) else {
            continue;
        };
        attributes.insert(format!("Endpoint.{member}"), value);
    }
    Ok(attributes)
}

/// The error code Amazon Redshift answers for a cluster subnet group it does not hold:
/// `ClusterSubnetGroupNotFoundFault`'s own `awsQueryError` code, which is what an awsQuery
/// client branches on.
pub(crate) const REDSHIFT_SUBNET_GROUP_NOT_FOUND: &str = "ClusterSubnetGroupNotFoundFault";

/// The members `CreateClusterSubnetGroup` takes, from Amazon Redshift's own model
/// (`smithy_query(query="operation", operation="CreateClusterSubnetGroup")`), with each
/// list's element name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="SubnetIdentifierList,TagList")`).
pub(crate) const CREATE_CLUSTER_SUBNET_GROUP_MEMBERS: &[(&str, QueryMember)] = &[
    ("ClusterSubnetGroupName", QueryMember::Scalar),
    ("Description", QueryMember::Scalar),
    ("SubnetIds", QueryMember::List("SubnetIdentifier")),
    ("Tags", QueryMember::List("Tag")),
];

/// The properties of `AWS::Redshift::ClusterSubnetGroup` the walk does NOT turn into a
/// create member, for the same reasons the DB subnet group's are
/// ([`DB_SUBNET_GROUP_PROPERTIES_NOT_SENT`]): the type declares no name property at all —
/// the name is the one CloudFormation generated, which this arm pushes — and the tag list
/// carries the declared tags PLUS the ones CloudFormation propagates, which this arm merges.
pub(crate) const CLUSTER_SUBNET_GROUP_PROPERTIES_NOT_SENT: &[&str] = &["Tags"];

/// Create the cluster subnet group an `AWS::Redshift::ClusterSubnetGroup` declares.
///
/// The type's remaining two properties (`Description`, `SubnetIds`) are
/// `CreateClusterSubnetGroup`'s own members, spelled identically on both sides, with the
/// name CloudFormation decided — this resource's physical id, which is what "Ref returns
/// the name of the cluster subnet group" means — and the merged tag list pushed by this arm
/// the way the DB subnet group's are.
///
/// The group is what a cluster's `ClusterSubnetGroupName` names, and Amazon Redshift
/// resolves that name against the groups it HOLDS before it will create a cluster in one
/// (`ClusterSubnetGroupNotFoundFault`), so a stack that declares both has to create this
/// one for real too — the same reason [`create_db_subnet_group`] exists.
pub(crate) async fn create_cluster_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_CLUSTER_SUBNET_GROUP_MEMBERS,
        // Every property of this type is its member's own name.
        &[],
        CLUSTER_SUBNET_GROUP_PROPERTIES_NOT_SENT,
        REDSHIFT_CLUSTER_SUBNET_GROUP,
    )?;
    parameters.push((
        "ClusterSubnetGroupName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "CreateClusterSubnetGroup",
        &parameters,
    )
    .await?;
    // The group's own name is the handle every Amazon Redshift call takes it by, and it is
    // already this resource's physical id; the type publishes it as its one attribute too.
    Ok(Created::attributes(BTreeMap::from([(
        "ClusterSubnetGroupName".to_string(),
        resource.physical_resource_id.clone(),
    )])))
}

/// Delete the cluster subnet group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon Redshift no longer has
/// (`ClusterSubnetGroupNotFoundFault`) is already in the state the delete asks for.
pub(crate) async fn delete_cluster_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "ClusterSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DeleteClusterSubnetGroup",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == REDSHIFT_SUBNET_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The cluster subnet group's configuration as Amazon Redshift reports it, for drift
/// detection. A group it no longer has (`ClusterSubnetGroupNotFoundFault`) was deleted.
///
/// Its scalar members are read the same way the DB subnet group's are, which covers the one
/// property a declaration can drift in under the same name (`Description`); `SubnetIds` is
/// reported as the `Subnets` structure list, which is not that property's form, so it is not
/// compared ([`super::drift`]).
pub(crate) async fn describe_cluster_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "ClusterSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        REDSHIFT_SIGNING_NAME,
        region,
        REDSHIFT_API_VERSION,
        "DescribeClusterSubnetGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "ClusterSubnetGroup",
        )))),
        Err(refusal) if refusal.code == REDSHIFT_SUBNET_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the DB subnet group an `AWS::RDS::DBSubnetGroup` declares.
///
/// The type's four properties are `CreateDBSubnetGroup`'s own members, named identically on
/// both sides, with the name CloudFormation decides and the merged tag list pushed by this
/// arm the way the DB instance's are. The group is what a DB instance's
/// `DBSubnetGroupName` names, and Amazon RDS resolves that name before it will create an
/// instance (`DBSubnetGroupNotFoundFault`), so a stack that declares both has to create
/// this one for real too.
pub(crate) async fn create_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_DB_SUBNET_GROUP_MEMBERS,
        &[],
        DB_SUBNET_GROUP_PROPERTIES_NOT_SENT,
        RDS_DB_SUBNET_GROUP,
    )?;
    parameters.push((
        "DBSubnetGroupName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "CreateDBSubnetGroup",
        &parameters,
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "DBSubnetGroupArn").filter(|arn| !arn.is_empty())
    {
        // The group's own ARN, which is the resource's: the conventional `Arn` attribute
        // the type answers is re-pointed at the one Amazon RDS reports.
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Delete the DB subnet group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon RDS no longer has (`DBSubnetGroupNotFoundFault`) is already
/// in the state the delete asks for.
pub(crate) async fn delete_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "DBSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeleteDBSubnetGroup",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == DB_SUBNET_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The DB subnet group's configuration as Amazon RDS reports it, for drift detection. A
/// group it no longer has (`DBSubnetGroupNotFoundFault`) was deleted.
///
/// Its scalar members are read the same way the DB instance's are, which covers the one
/// property a declaration can drift in under the same name
/// (`DBSubnetGroupDescription`); `SubnetIds` is reported as the `Subnets` structure list,
/// which is not that property's form, so it is not compared.
pub(crate) async fn describe_db_subnet_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "DBSubnetGroupName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBSubnetGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            "DBSubnetGroup",
        )))),
        Err(refusal) if refusal.code == DB_SUBNET_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the parameter group a declaration of either RDS parameter-group type asks for.
///
/// The type's properties are its create's own members ([`ParameterGroupCalls`]) but for the
/// one Amazon RDS spells differently ([`PARAMETER_GROUP_RENAMED_PROPERTIES`]), with the name
/// CloudFormation decides — what this type's `Ref` returns, this resource's physical id —
/// and the merged tag list pushed by this arm the way the two types above push theirs.
///
/// The declared `Parameters` are a SECOND call ([`modify_parameters`]), the same pair a
/// client makes: a group is created carrying none of its own parameters, and the values a
/// template declares are what the describe of the group's parameters then reports for it.
pub(crate) async fn create_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
    calls: ParameterGroupCalls,
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        calls.create_members,
        PARAMETER_GROUP_RENAMED_PROPERTIES,
        &calls.properties_not_sent(),
        calls.resource_type,
    )?;
    parameters.push((
        calls.name_member.to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.create,
        &parameters,
    )
    .await?;
    modify_parameters(
        ctx,
        region,
        &resource.physical_resource_id,
        &declared_parameters(&resource.properties),
        calls,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) =
        xml_element(&answered, &format!("{}Arn", calls.record)).filter(|arn| !arn.is_empty())
    {
        // The group's own ARN, which is the resource's: the conventional `Arn` attribute the
        // type answers is re-pointed at the one Amazon RDS reports.
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Write the parameter settings a declaration carries into the group named
/// `physical_resource_id`, through the modify a client makes for them.
///
/// Either type's `Parameters` is a MAP of parameter name to value, where the modify takes
/// the `Parameter` STRUCTURE list its `ParametersList` spells, so each entry is written as
/// one element of that list. A declaration carrying more than one request admits
/// ([`PARAMETERS_PER_MODIFY`]) is sent as several calls, since one request carrying them all
/// is a request Amazon RDS refuses; a declaration carrying none makes no call at all.
///
/// How many calls that is is bounded by the declared map, which is bounded by the template
/// body the stack was created from, and Amazon RDS's own cap on how many parameters one
/// group carries refuses the rest — so neither this loop nor the group it writes grows with
/// anything a caller can choose freely.
pub(crate) async fn modify_parameters(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    declared: &Map<String, Value>,
    calls: ParameterGroupCalls,
) -> Result<(), Refusal> {
    let settings: Vec<(&String, &Value)> = declared.iter().collect();
    for batch in settings.chunks(PARAMETERS_PER_MODIFY) {
        let mut parameters = vec![(
            calls.name_member.to_string(),
            physical_resource_id.to_string(),
        )];
        for (index, (name, value)) in batch.iter().enumerate() {
            let position = index + 1;
            parameters.push((
                format!("Parameters.Parameter.{position}.ParameterName"),
                (*name).clone(),
            ));
            parameters.push((
                format!("Parameters.Parameter.{position}.ParameterValue"),
                attribute_text(value),
            ));
        }
        call_query(
            ctx,
            RDS_SIGNING_NAME,
            region,
            RDS_API_VERSION,
            calls.modify,
            &parameters,
        )
        .await?;
    }
    Ok(())
}

/// The parameter settings a declaration of either parameter-group type carries: its
/// `Parameters` MAP, or an empty one where it declares none.
pub(crate) fn declared_parameters(properties: &Value) -> Map<String, Value> {
    properties
        .get("Parameters")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The part of either parameter-group declaration that is NOT its parameter settings:
/// everything but the `Parameters` map and the tag list, which travel on their own
/// ([`modify_parameters`], [`Provisioner::retag`]).
pub(crate) fn parameter_group_configuration(properties: &Value) -> Map<String, Value> {
    properties
        .as_object()
        .map(|properties| {
            properties
                .iter()
                .filter(|(property, _)| !matches!(property.as_str(), "Parameters" | "Tags"))
                .map(|(property, value)| (property.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Apply a CHANGED declaration of either parameter-group type to the group the stack
/// already has.
///
/// A group's `Parameters` are the one part of either declaration Amazon RDS can change on a
/// group that exists, and the modify is the call that changes it — the same call the create
/// makes for the values a template declares — so a deployment that states a parameter
/// differently, or states one more, reaches that service as that call, carrying only what it
/// changed. The group keeps its name, so it keeps the handle a DB instance names it by and
/// the ARN the create reported: this reports no attribute of its own.
///
/// Two changes Amazon RDS will not make to a group that exists are REFUSED rather than
/// quietly dropped, so the stack never reports a group configured differently from its
/// declaration:
///
/// * a change to any other property — the group's family or its description, neither of
///   which is a member of either modify at all, so AWS applies such a change by creating a
///   new group and this emulator creates no replacement;
/// * a parameter the new declaration DROPS, which AWS puts back to the engine's default.
///   Taking a value off a group is the reset call of the group's own API
///   (`ResetDBParameterGroup`), which this emulator does not serve yet, so the removal is
///   answered with `NotImplemented` naming the parameter.
pub(crate) async fn update_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    calls: ParameterGroupCalls,
) -> Result<Option<Created>, Refusal> {
    if parameter_group_configuration(&resource.properties)
        != parameter_group_configuration(declared)
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not replace an existing {} whose declaration changes a \
                 property Amazon RDS cannot change on a group that exists; the resource was \
                 left as it is.",
                calls.resource_type
            ),
        });
    }

    let before = declared_parameters(&resource.properties);
    let after = declared_parameters(declared);
    if let Some(dropped) = before.keys().find(|name| !after.contains_key(*name)) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not put the {dropped} parameter of an existing {} back to \
                 the engine's default when an update no longer declares it; the resource was \
                 left as it is.",
                calls.resource_type
            ),
        });
    }
    // Only the settings this deployment states differently are written: a parameter neither
    // revision changed already carries the value the group holds.
    let changed: Map<String, Value> = after
        .iter()
        .filter(|(name, value)| before.get(*name) != Some(value))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    modify_parameters(ctx, region, &resource.physical_resource_id, &changed, calls).await?;
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Delete the parameter group a stack created, addressed by its name — this resource's
/// physical id. A group Amazon RDS no longer has (`DBParameterGroupNotFound`) is already in
/// the state the delete asks for.
///
/// Amazon RDS refuses the delete of a DB parameter group an instance is still a member of,
/// so that deletion comes first: CloudFormation deletes a stack's resources in dependency
/// order and waits for each deletion to finish, which is what
/// [`confirm_db_instance_deleted`] is for.
pub(crate) async fn delete_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    calls: ParameterGroupCalls,
) -> Result<(), Refusal> {
    let parameters = vec![(
        calls.name_member.to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.delete,
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == DB_PARAMETER_GROUP_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The parameter group's configuration as Amazon RDS reports it, for drift detection. A
/// group it no longer has (`DBParameterGroupNotFound`) was deleted.
///
/// Its scalar members are read the way the two types above read theirs, which covers the one
/// property a declaration can drift in under the same name (`Description`); `Family` is
/// reported under the API's own `DBParameterGroupFamily` and `Parameters` is not a member of
/// this record at all, so neither takes part in the comparison — the position every arm
/// whose property is spelled differently from its member is in, and what [`super::drift`]
/// documents.
pub(crate) async fn describe_parameter_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    calls: ParameterGroupCalls,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        calls.name_member.to_string(),
        physical_resource_id.to_string(),
    )];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        calls.describe,
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered,
            calls.record,
        )))),
        Err(refusal) if refusal.code == DB_PARAMETER_GROUP_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon RDS Proxy: AWS::RDS::DBProxy, AWS::RDS::DBProxyTargetGroup
// ---------------------------------------------------------------------------

/// `DBProxyNotFoundFault`'s and `DBProxyTargetNotFoundFault`'s own `awsQueryError` codes
/// (`smithy_query(query="operation", operation="DeleteDBProxy,DeregisterDBProxyTargets")`),
/// which is what an awsQuery client branches on for a proxy Amazon RDS does not hold and
/// for a database its target group does not have registered.
pub(crate) const DB_PROXY_NOT_FOUND: &str = "DBProxyNotFoundFault";
pub(crate) const DB_PROXY_TARGET_NOT_FOUND: &str = "DBProxyTargetNotFoundFault";

/// `DBProxyStatus$available` — "A status of `available` means the proxy is ready to handle
/// requests" — and the values a proxy that is still on its way to it reports. "Other values
/// indicate that you must wait for the proxy to be ready, or take some action to resolve an
/// issue", so every status outside these two sets is one no wait brings a NEW proxy out of
/// (`incompatible-network`, `insufficient-resource-limits`, `suspended`, `suspending`,
/// `deleting`; `smithy_query(query="shape", shape="DBProxyStatus")` lists them all).
pub(crate) const DB_PROXY_STATUS_AVAILABLE: &str = "available";
pub(crate) const DB_PROXY_STATUSES_COMING_UP: &[&str] = &["creating", "modifying", "reactivating"];

/// The one target group a proxy has: "Currently, this name must be `default`"
/// (`RegisterDBProxyTargets$TargetGroupName`), which is the group an
/// `AWS::RDS::DBProxyTargetGroup` that states no `TargetGroupName` means.
pub(crate) const DEFAULT_DB_PROXY_TARGET_GROUP: &str = "default";

/// How often this module checks whether a proxy has finished coming up, and how many times.
///
/// The product is the longest the deployment of a proxy's target group waits for the proxy
/// it belongs to: an upper bound on the wait, not a duration anything is owed. Amazon RDS
/// answers `CreateDBProxy` with the proxy `creating` and reports it `available` a moment
/// later (its own transition span), so the check is far shorter than that moment and the
/// whole wait is a small multiple of it — enough that a loaded machine still gets there,
/// and bounded so a stack operation cannot hold its request open.
pub(crate) const DB_PROXY_CHECK_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(25);
pub(crate) const DB_PROXY_CHECKS: usize = 24;

/// The members `CreateDBProxy` takes, from Amazon RDS's own model
/// (`smithy_query(query="operation", operation="CreateDBProxy")`), with each list's element
/// name from its list shape's member `@xmlName` — `UserAuthConfigList` and `StringList` fix
/// none, so their entries are indexed under the Smithy default `member`, and `TagList`
/// spells its own `Tag` (`smithy_query(query="shape",
/// shape="UserAuthConfigList,StringList,TagList")`).
///
/// This is the catalogue the declaration of an `AWS::RDS::DBProxy` is walked against: the
/// type's properties ARE these members, named identically on both sides, so a property
/// whose name is one of them is passed through under it.
pub(crate) const CREATE_DB_PROXY_MEMBERS: &[(&str, QueryMember)] = &[
    ("Auth", QueryMember::List("member")),
    ("DBProxyName", QueryMember::Scalar),
    ("DebugLogging", QueryMember::Scalar),
    ("DefaultAuthScheme", QueryMember::Scalar),
    ("EndpointNetworkType", QueryMember::Scalar),
    ("EngineFamily", QueryMember::Scalar),
    ("IdleClientTimeout", QueryMember::Scalar),
    ("RequireTLS", QueryMember::Scalar),
    ("RoleArn", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
    ("TargetConnectionNetworkType", QueryMember::Scalar),
    ("VpcSecurityGroupIds", QueryMember::List("member")),
    ("VpcSubnetIds", QueryMember::List("member")),
];

/// `AWS::RDS::DBProxy`'s two properties the arm itself pushes, for the same reasons the DB
/// instance's are ([`DB_INSTANCE_PROPERTIES_NOT_SENT`]): the name CloudFormation decides,
/// which is this resource's physical id ("Ref returns the DBProxyName"), and the tag list
/// it merges.
pub(crate) const DB_PROXY_PROPERTIES_NOT_SENT: &[&str] = &["DBProxyName", "Tags"];

/// The properties an `AWS::RDS::DBProxyTargetGroup` declares, each of which this arm acts
/// on: the proxy and the target group it addresses, the databases it registers there and
/// the connection pool settings it applies to the group. A declaration carrying any other
/// property is REFUSED rather than dropped, for the reason [`query_members`] records.
pub(crate) const DB_PROXY_TARGET_GROUP_PROPERTIES: &[&str] = &[
    "ConnectionPoolConfigurationInfo",
    "DBClusterIdentifiers",
    "DBInstanceIdentifiers",
    "DBProxyName",
    "TargetGroupName",
];

/// The two properties of `AWS::RDS::DBProxyTargetGroup` that name the databases the group
/// routes to, each the member `RegisterDBProxyTargets` and `DeregisterDBProxyTargets` take
/// that kind of database under — named identically on both sides.
pub(crate) const DB_PROXY_TARGET_PROPERTIES: &[&str] = &["DBInstanceIdentifiers", "DBClusterIdentifiers"];

/// Create the RDS proxy an `AWS::RDS::DBProxy` declares.
///
/// The type's properties are `CreateDBProxy`'s own members ([`CREATE_DB_PROXY_MEMBERS`]),
/// so the evaluated declaration IS the request, with the one value CloudFormation decides —
/// the `DBProxyName`, which is this resource's physical id — set on the way out and the tag
/// list built from the declared tags plus the ones CloudFormation propagates.
///
/// What the create ANSWERS replaces the identifiers CloudFormation's naming rules had
/// computed for the resource: the proxy's ARN and its endpoint are Amazon RDS's to assign,
/// and the whole point of provisioning the proxy is that the handle the stack publishes
/// addresses the proxy RDS really holds. `VpcId` is not among them — that service reports
/// none for a proxy, so nothing is re-pointed at a value it does not know.
pub(crate) async fn create_db_proxy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_DB_PROXY_MEMBERS,
        &[],
        DB_PROXY_PROPERTIES_NOT_SENT,
        RDS_DB_PROXY,
    )?;
    parameters.push((
        "DBProxyName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "CreateDBProxy",
        &parameters,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "DBProxyArn").filter(|arn| !arn.is_empty()) {
        // The type publishes one ARN, so the conventional `Arn` attribute every type
        // answers is that same value ([`super::template::conventional_attributes`]): both
        // are re-pointed at the ARN the proxy really carries.
        attributes.insert("Arn".to_string(), arn.clone());
        attributes.insert("DBProxyArn".to_string(), arn);
    }
    if let Some(endpoint) = xml_element(&answered, "Endpoint").filter(|host| !host.is_empty())
    {
        attributes.insert("Endpoint".to_string(), endpoint);
    }
    Ok(Created::attributes(attributes))
}

/// Whether the proxy a stack created is ready to handle requests yet: `available` is the
/// status Amazon RDS reports a usable proxy in, the statuses in
/// [`DB_PROXY_STATUSES_COMING_UP`] are ones a proxy comes up out of, and every other one is
/// a state no wait brings a new proxy out of. A proxy that service no longer holds is in the
/// same position: nothing is going to create it.
pub(crate) async fn db_proxy_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    db_proxy_name: &str,
) -> Result<Readiness, Refusal> {
    let Some(reported) = read_db_proxy(ctx, region, db_proxy_name).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon RDS holds no DB proxy named {db_proxy_name}"
        )));
    };
    let status = reported
        .get("Status")
        .and_then(scalar_text)
        .unwrap_or_default();
    if status == DB_PROXY_STATUS_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if DB_PROXY_STATUSES_COMING_UP.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon RDS reports the DB proxy {db_proxy_name} in status {status}"
    )))
}

/// Wait until Amazon RDS reports the proxy `db_proxy_name` out of the statuses it comes up
/// in.
///
/// Returns as soon as the proxy is no longer coming up, and gives up after
/// [`DB_PROXY_CHECKS`] checks — a wait that runs out is not a failure of its own; what fails
/// then is the call that needed the proxy ready, reported as Amazon RDS's own refusal of it
/// (`InvalidDBProxyStateFault`). Any refusal of the READ ends the wait the same way: it says
/// nothing about whether the proxy is ready.
///
/// This is what a target group's deployment needs: "Other values indicate that you must wait
/// for the target group to be ready", and the group is ready exactly when its proxy is, so
/// the registration a declaration asks for cannot be made before then.
pub(crate) async fn await_db_proxy_ready(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    db_proxy_name: &str,
) {
    for _ in 0..DB_PROXY_CHECKS {
        match db_proxy_readiness(ctx, region, db_proxy_name).await {
            Ok(Readiness::Pending(_)) => {}
            Ok(_) | Err(_) => return,
        }
        ::tokio::time::sleep(DB_PROXY_CHECK_INTERVAL).await;
    }
}

/// Delete the RDS proxy a stack created, addressed by its name — this resource's physical
/// id. A proxy Amazon RDS no longer has (`DBProxyNotFoundFault`) is already in the state the
/// delete asks for.
///
/// A proxy that has not finished coming up is one that service refuses the delete of, so the
/// delete waits for it first ([`await_db_proxy_ready`]) — the position a rollback of a stack
/// whose proxy was still `creating` is in. Amazon RDS answers the delete by putting the
/// proxy in `deleting` and removing it a moment later, so this arm then WAITS for it to be
/// gone ([`confirm_db_proxy_deleted`]): CloudFormation deletes a stack's resources in
/// dependency order and waits for each deletion to complete before it deletes what that
/// resource sat in.
pub(crate) async fn delete_db_proxy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    await_db_proxy_ready(ctx, region, physical_resource_id).await;
    let parameters = vec![("DBProxyName".to_string(), physical_resource_id.to_string())];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeleteDBProxy",
        &parameters,
    )
    .await
    {
        Ok(_) => {
            confirm_db_proxy_deleted(ctx, region, physical_resource_id).await;
            Ok(())
        }
        Err(refusal) if refusal.code == DB_PROXY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Wait until Amazon RDS no longer holds the proxy whose deletion it has ACCEPTED, bounded
/// the way [`await_db_proxy_ready`] is and for the same reasons.
pub(crate) async fn confirm_db_proxy_deleted(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) {
    for _ in 0..DB_PROXY_CHECKS {
        match read_db_proxy(ctx, region, physical_resource_id).await {
            // The proxy is gone: the deletion RDS accepted has finished.
            Ok(None) | Err(_) => return,
            Ok(Some(_)) => {}
        }
        ::tokio::time::sleep(DB_PROXY_CHECK_INTERVAL).await;
    }
}

/// The proxy's configuration as Amazon RDS reports it, for drift detection and for the
/// readiness read above. A proxy it no longer has (`DBProxyNotFoundFault`) was deleted.
///
/// A proxy is reported as one `<member>` of the answer's `DBProxies` list, whose scalar
/// members are named exactly as this type's properties are (`EngineFamily`, `RoleArn`,
/// `RequireTLS`, `IdleClientTimeout`, `DebugLogging`), so those members are the actual
/// configuration the declared one is compared against. Its composite members are not:
/// `VpcSubnetIds`, `VpcSecurityGroupIds` and `Auth` are lists, which
/// [`xml_scalar_children`] leaves out, so a declaration can drift in them without this
/// read's saying so — the position every arm whose property is not a scalar on the wire is
/// in, and what [`super::drift`] documents.
pub(crate) async fn read_db_proxy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    db_proxy_name: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("DBProxyName".to_string(), db_proxy_name.to_string())];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBProxies",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered, "member",
        )))),
        Err(refusal) if refusal.code == DB_PROXY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Register the databases an `AWS::RDS::DBProxyTargetGroup` declares in the target group of
/// the proxy it names, with the connection pool settings it asks for applied to that group.
///
/// The target group itself is NOT created here: a proxy comes up with the one target group
/// it has, so what this type declares is the group's CONTENTS and its settings — which is
/// what `RegisterDBProxyTargets` and `ModifyDBProxyTargetGroup` are, the same pair a client
/// makes. The proxy has to be ready before either is accepted ("The requested operation
/// can't be performed while the proxy is in this state"), so the deployment waits for it.
///
/// The resource's identity is the `TargetGroupArn` Amazon RDS minted for the group, which is
/// what this type's `Ref` returns, so it is read back off that service rather than computed.
pub(crate) async fn create_db_proxy_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    if let Some(properties) = resource.properties.as_object() {
        for property in properties.keys() {
            if !DB_PROXY_TARGET_GROUP_PROPERTIES.contains(&property.as_str()) {
                return Err(property_not_sent(RDS_DB_PROXY_TARGET_GROUP, property));
            }
        }
    }
    let proxy = declared_db_proxy_name(resource)?;
    let group = declared_target_group_name(resource);
    await_db_proxy_ready(ctx, region, &proxy).await;

    if let Some(declared) = resource.properties.get("ConnectionPoolConfigurationInfo") {
        // The property's members are `ConnectionPoolConfiguration`'s own
        // (`MaxConnectionsPercent`, `MaxIdleConnectionsPercent`,
        // `ConnectionBorrowTimeout`, `SessionPinningFilters`, `InitQuery`), so the declared
        // structure IS the member the modify takes it under.
        let mut parameters = vec![
            ("DBProxyName".to_string(), proxy.clone()),
            ("TargetGroupName".to_string(), group.clone()),
        ];
        push_structure(&mut parameters, "ConnectionPoolConfig", declared, 0)?;
        call_query(
            ctx,
            RDS_SIGNING_NAME,
            region,
            RDS_API_VERSION,
            "ModifyDBProxyTargetGroup",
            &parameters,
        )
        .await?;
    }
    if let Some(parameters) = db_proxy_target_parameters(resource, &proxy, &group) {
        call_query(
            ctx,
            RDS_SIGNING_NAME,
            region,
            RDS_API_VERSION,
            "RegisterDBProxyTargets",
            &parameters,
        )
        .await?;
    }

    let reported = read_db_proxy_target_group(ctx, region, &proxy, &group).await?;
    let arn = reported
        .as_ref()
        .and_then(|group| group.get("TargetGroupArn"))
        .and_then(scalar_text)
        .filter(|arn| !arn.is_empty());
    let mut attributes = BTreeMap::new();
    if let Some(arn) = &arn {
        attributes.insert("TargetGroupArn".to_string(), arn.clone());
    }
    Ok(Created {
        physical_resource_id: arn,
        attributes,
    })
}

/// Deregister the databases a stack registered in a proxy's target group.
///
/// There is no target group to delete — the group belongs to the proxy and goes away with it
/// — so what this resource's deletion undoes is exactly what its create did: the
/// registrations it made. A proxy Amazon RDS no longer holds, and a database its group no
/// longer has registered, are both already in the state the deregistration asks for.
pub(crate) async fn delete_db_proxy_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let proxy = declared_db_proxy_name(resource)?;
    let group = declared_target_group_name(resource);
    let Some(parameters) = db_proxy_target_parameters(resource, &proxy, &group) else {
        return Ok(());
    };
    await_db_proxy_ready(ctx, region, &proxy).await;
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DeregisterDBProxyTargets",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == DB_PROXY_NOT_FOUND
                || refusal.code == DB_PROXY_TARGET_NOT_FOUND =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// The target group's configuration as Amazon RDS reports it, for drift detection, read by
/// the proxy-and-group pair the declaration names. A proxy that service no longer holds
/// means the group is gone with it.
///
/// Its scalar members cover the two properties a declaration can drift in under the same
/// name (`DBProxyName`, `TargetGroupName`); `ConnectionPoolConfigurationInfo` is reported
/// under the API's own `ConnectionPoolConfig` and as a structure, and the two identifier
/// lists are not members of this record at all, so none of the three takes part in the
/// comparison — what [`super::drift`] documents.
pub(crate) async fn describe_db_proxy_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let proxy = declared_db_proxy_name(resource)?;
    let group = declared_target_group_name(resource);
    read_db_proxy_target_group(ctx, region, &proxy, &group).await
}

/// One target group of one proxy as Amazon RDS reports it, or `None` where that service
/// holds neither.
pub(crate) async fn read_db_proxy_target_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    db_proxy_name: &str,
    target_group_name: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![
        ("DBProxyName".to_string(), db_proxy_name.to_string()),
        ("TargetGroupName".to_string(), target_group_name.to_string()),
    ];
    match call_query(
        ctx,
        RDS_SIGNING_NAME,
        region,
        RDS_API_VERSION,
        "DescribeDBProxyTargetGroups",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(xml_scalar_children(
            &answered, "member",
        )))),
        Err(refusal) if refusal.code == DB_PROXY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The proxy an `AWS::RDS::DBProxyTargetGroup` names, which every call this type makes is
/// addressed by. The property is required on the type, so a declaration carrying none names
/// no proxy at all — refused rather than sent, for the reason [`property_not_sent`] records.
pub(crate) fn declared_db_proxy_name(resource: &StoredResource) -> Result<String, Refusal> {
    declared_text(&resource.properties, "DBProxyName").ok_or_else(|| Refusal {
        code: NOT_IMPLEMENTED.to_string(),
        message: format!(
            "{RDS_DB_PROXY_TARGET_GROUP} names no DBProxyName, so there is no proxy whose \
             target group this resource is"
        ),
    })
}

/// The target group an `AWS::RDS::DBProxyTargetGroup` names, or the one group every proxy
/// has where the declaration names none.
pub(crate) fn declared_target_group_name(resource: &StoredResource) -> String {
    declared_text(&resource.properties, "TargetGroupName")
        .unwrap_or_else(|| DEFAULT_DB_PROXY_TARGET_GROUP.to_string())
}

/// The registration call's parameters for the databases a declaration names, or `None`
/// where it names none.
///
/// Both identifier properties are optional, and a registration of nothing is not a
/// registration — which is the request Amazon RDS refuses ("Missing DB instance or DB
/// cluster identifier") — so a declaration that states the group and its settings alone
/// makes no registration call at all.
pub(crate) fn db_proxy_target_parameters(
    resource: &StoredResource,
    db_proxy_name: &str,
    target_group_name: &str,
) -> Option<Vec<(String, String)>> {
    let mut parameters = vec![
        ("DBProxyName".to_string(), db_proxy_name.to_string()),
        ("TargetGroupName".to_string(), target_group_name.to_string()),
    ];
    let mut named = 0usize;
    for member in DB_PROXY_TARGET_PROPERTIES {
        for (index, identifier) in declared_text_list(&resource.properties, member)
            .iter()
            .enumerate()
        {
            parameters.push((
                format!("{member}.member.{}", index + 1),
                identifier.clone(),
            ));
            named += 1;
        }
    }
    (named > 0).then_some(parameters)
}

/// The awsQuery parameters an evaluated property document is, against the member catalogue
/// of the create that takes it.
///
/// `renamed` is the properties whose member is spelled differently, and `not_sent` the ones
/// the ARM itself pushes or that belong to another call. Every other property has to be a
/// member of the catalogue: one that is not is REFUSED, because a resource created without
/// a property its declaration asked for is configured differently from what the template
/// declared — and the stack would have reported it as created.
pub(crate) fn query_members(
    properties: &Value,
    members: &[(&str, QueryMember)],
    renamed: &[(&str, &str)],
    not_sent: &[&str],
    resource_type: &str,
) -> Result<Vec<(String, String)>, Refusal> {
    let mut parameters: Vec<(String, String)> = Vec::new();
    let Some(properties) = properties.as_object() else {
        return Ok(parameters);
    };
    for (property, value) in properties {
        if not_sent.contains(&property.as_str()) {
            continue;
        }
        let member = renamed
            .iter()
            .find(|(declared, _)| *declared == property.as_str())
            .map_or(property.as_str(), |(_, member)| *member);
        let Some((_, shape)) = members.iter().find(|(name, _)| *name == member) else {
            return Err(property_not_sent(resource_type, property));
        };
        match shape {
            QueryMember::Scalar => {
                parameters.push((member.to_string(), attribute_text(value)));
            }
            QueryMember::List(element) => {
                push_list(&mut parameters, member, element, value, 0)?
            }
            QueryMember::Structure => push_structure(&mut parameters, member, value, 0)?,
        }
    }
    Ok(parameters)
}

/// The refusal a declaration carrying a property no arm maps onto the owning service's
/// request answers with: a resource created without a property its declaration asked for is
/// configured differently from what the template declared, and the stack would have reported
/// it as created.
///
/// The one wording for it, shared by [`query_members`] and by the per-member walk of a
/// structure an arm maps itself ([`security_group_rule_parameters`]), so a property left out
/// of one catalogue is refused in the same words as one left out of the other.
pub(crate) fn property_not_sent(resource_type: &str, property: &str) -> Refusal {
    Refusal {
        code: NOT_IMPLEMENTED.to_string(),
        message: format!(
            "this emulator does not send {resource_type}'s {property} property to the \
             service that owns the type, so the resource was not created"
        ),
    }
}

/// Write one LIST member into an awsQuery form body: each entry indexed from 1 under
/// `element`, with a structured entry's own members following the index.
///
/// A value the template did not write as a list is the one-entry list of itself, so a
/// single value a template spells without the sequence syntax is not dropped.
///
/// Each entry is written the way a structure member is ([`push_structure`]), so an entry
/// carrying a nested structure or list of its own — a receipt rule's action, which is one
/// structure per kind of action — is WALKED to its leaves rather than flattened into the
/// text of its JSON, which no member of the called operation would take. The walk is
/// bounded for the reason [`MAX_PROPERTY_NESTING`] records.
pub(crate) fn push_list(
    parameters: &mut Vec<(String, String)>,
    member: &str,
    element: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    let entries: Vec<&Value> = match declared {
        Value::Array(values) => values.iter().collect(),
        single => vec![single],
    };
    for (index, value) in entries.iter().enumerate() {
        let position = index + 1;
        push_structure(
            parameters,
            &format!("{member}.{element}.{position}"),
            value,
            depth + 1,
        )?;
    }
    Ok(())
}

/// Write one STRUCTURE member into an awsQuery form body: each of its declared members
/// under `<prefix>.<Member>`, nested structures the same way, and a nested LIST indexed
/// from 1 under `member` — the Smithy default element name for a list member that fixes
/// none, which is what every list inside these structures carries.
///
/// The walk is bounded by [`MAX_PROPERTY_NESTING`] for the reason that constant records:
/// the document comes from a caller-supplied template, so a document nested past the
/// bound is REFUSED rather than walked or quietly truncated.
pub(crate) fn push_structure(
    parameters: &mut Vec<(String, String)>,
    prefix: &str,
    declared: &Value,
    depth: usize,
) -> Result<(), Refusal> {
    if depth >= MAX_PROPERTY_NESTING {
        return Err(too_deeply_nested());
    }
    match declared {
        Value::Object(members) => {
            for (name, value) in members {
                push_structure(parameters, &format!("{prefix}.{name}"), value, depth + 1)?;
            }
        }
        Value::Array(_) => push_list(parameters, prefix, "member", declared, depth)?,
        scalar => parameters.push((prefix.to_string(), attribute_text(scalar))),
    }
    Ok(())
}

/// Write a tag set as the `Tag` structure list an awsQuery create takes, with the
/// `Key`/`Value` members Amazon RDS's own `Tag` shape spells.
pub(crate) fn push_tag_list(
    parameters: &mut Vec<(String, String)>,
    member: &str,
    element: &str,
    tags: &[(String, String)],
) {
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("{member}.{element}.{position}.Key"), key.clone()));
        parameters.push((
            format!("{member}.{element}.{position}.Value"),
            value.clone(),
        ));
    }
}

// ---------------------------------------------------------------------------
// Amazon EC2: AWS::EC2::VPC, AWS::EC2::Subnet and AWS::EC2::SecurityGroup
// ---------------------------------------------------------------------------

/// Amazon EC2's signing name and the API version a request names beside the action, from
/// its model (`sigv4Name: ec2`, `version: 2016-11-15`).
///
/// That model declares `ec2Query`, whose request is the same `Action`-plus-`Version` form
/// body an awsQuery one is — so [`call_query`] carries it — answered with XML. What differs
/// is only how a list is spelled (`ResourceId.1`, `Tag.1.Key`, with no `.member.`
/// indirection) and that a member's parameter name is its `ec2QueryName`, which every
/// member these arms send carries as its own name with an upper-case initial: both are what
/// the parameters below are written in.
pub(crate) const EC2_SIGNING_NAME: &str = "ec2";
pub(crate) const EC2_API_VERSION: &str = "2016-11-15";

/// The error codes Amazon EC2 answers for a VPC, a subnet and a security group it does not
/// hold (`InvalidVpcID.NotFound` / `InvalidSubnetID.NotFound` / `InvalidGroup.NotFound`, EC2
/// API error codes), which is a delete asking for a state that service is already in.
pub(crate) const INVALID_VPC_ID_NOT_FOUND: &str = "InvalidVpcID.NotFound";
pub(crate) const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";
pub(crate) const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// The `AWS::EC2::VPC` properties `CreateVpc` takes under the SAME member name.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateVpc")` — `CidrBlock`,
/// `InstanceTenancy`, `Ipv4IpamPoolId` and `Ipv4NetmaskLength` are spelled identically on
/// both sides, and every one is a SCALAR, which is why [`query_members`] can write this
/// catalogue: a LIST member would need the flattening ec2Query gives one (`Tag.1.Key`, with
/// no `.member.` indirection) and that helper writes the awsQuery spelling. EC2 takes no
/// name for a VPC on that call and assigns the `vpc-…` id itself, which is why this type's
/// identity is the owning service's
/// ([`super::template::identity_is_service_assigned`]).
pub(crate) const EC2_VPC_CREATE_MEMBERS: &[(&str, QueryMember)] = &[
    ("CidrBlock", QueryMember::Scalar),
    ("InstanceTenancy", QueryMember::Scalar),
    ("Ipv4IpamPoolId", QueryMember::Scalar),
    ("Ipv4NetmaskLength", QueryMember::Scalar),
];

/// The `AWS::EC2::VPC` properties another call carries: the two DNS switches, which are VPC
/// ATTRIBUTES rather than members of the create ([`EC2_VPC_ATTRIBUTES`]), and the tags,
/// which reach EC2 through the one call that tags a resource that exists
/// ([`tag_ec2_resource`]).
pub(crate) const EC2_VPC_PROPERTIES_NOT_SENT: &[&str] =
    &["EnableDnsHostnames", "EnableDnsSupport", "Tags"];

/// The `AWS::EC2::VPC` properties that are attributes of the VPC, each the
/// `ModifyVpcAttribute` member that writes it — named identically on both sides
/// (`smithy_query(query="operation",operation="ModifyVpcAttribute")`), and each an
/// `AttributeBooleanValue`, whose value is its `Value` member.
///
/// "You can only modify one attribute at a time" (`ModifyVpcAttribute`), so each is its own
/// call, in this order: "You cannot enable DNS hostnames if DNS support is disabled" (Amazon
/// VPC User Guide, "DNS attributes in your VPC"), so a declaration enabling both has support
/// written first.
pub(crate) const EC2_VPC_ATTRIBUTES: &[&str] = &["EnableDnsSupport", "EnableDnsHostnames"];

/// The `AWS::EC2::Subnet` properties `CreateSubnet` takes under the SAME member name, each
/// a scalar for the reason [`EC2_VPC_CREATE_MEMBERS`] records.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateSubnet")`. `VpcId` is the VPC the
/// subnet is created in — a `Ref` to the VPC of the same template resolves to the id EC2
/// assigned it ([`super::provisioning::reconcile`]) — and the subnet's own `subnet-…` id is
/// EC2's to assign, exactly as the VPC's is.
pub(crate) const EC2_SUBNET_CREATE_MEMBERS: &[(&str, QueryMember)] = &[
    ("AvailabilityZone", QueryMember::Scalar),
    ("AvailabilityZoneId", QueryMember::Scalar),
    ("CidrBlock", QueryMember::Scalar),
    ("Ipv6CidrBlock", QueryMember::Scalar),
    ("Ipv6Native", QueryMember::Scalar),
    ("OutpostArn", QueryMember::Scalar),
    ("VpcId", QueryMember::Scalar),
];

/// The `AWS::EC2::Subnet` properties another call carries: the four that are subnet
/// ATTRIBUTES ([`EC2_SUBNET_BOOLEAN_ATTRIBUTES`] and the members of
/// `PrivateDnsNameOptionsOnLaunch`, [`EC2_SUBNET_PRIVATE_DNS_ATTRIBUTES`]) and the tags.
///
/// `Ipv6CidrBlocks` is NOT one of them and is not a member of the create either, so a
/// declaration carrying it is refused: a subnet's further IPv6 blocks are each an
/// `AssociateSubnetCidrBlock` of their own, and a subnet created without the blocks its
/// declaration asked for is not the subnet the template declared.
pub(crate) const EC2_SUBNET_PROPERTIES_NOT_SENT: &[&str] = &[
    "AssignIpv6AddressOnCreation",
    "EnableDns64",
    "MapPublicIpOnLaunch",
    "PrivateDnsNameOptionsOnLaunch",
    "Tags",
];

/// The `AWS::EC2::Subnet` properties that are boolean attributes of the subnet, each the
/// `ModifySubnetAttribute` member that writes it — named identically on both sides
/// (`smithy_query(query="operation",operation="ModifySubnetAttribute")`), and each an
/// `AttributeBooleanValue`.
pub(crate) const EC2_SUBNET_BOOLEAN_ATTRIBUTES: &[&str] = &[
    "AssignIpv6AddressOnCreation",
    "EnableDns64",
    "MapPublicIpOnLaunch",
];

/// The members of the type's `PrivateDnsNameOptionsOnLaunch` property, each against the
/// `ModifySubnetAttribute` member that writes it and whether that member is a boolean
/// (`AttributeBooleanValue`) or the plain `HostnameType` string.
pub(crate) const EC2_SUBNET_PRIVATE_DNS_ATTRIBUTES: &[(&str, &str, bool)] = &[
    (
        "EnableResourceNameDnsARecord",
        "EnableResourceNameDnsARecordOnLaunch",
        true,
    ),
    (
        "EnableResourceNameDnsAAAARecord",
        "EnableResourceNameDnsAAAARecordOnLaunch",
        true,
    ),
    ("HostnameType", "PrivateDnsHostnameTypeOnLaunch", false),
];

/// Tag one EC2 resource with the set CloudFormation gives it.
///
/// Through `CreateTags` rather than the create's own `TagSpecifications`, because that set
/// carries the three `aws:cloudformation:*` keys: "Do not use the `aws:` prefix … it is
/// reserved for AWS use" binds a CALLER, and the services that provision resources on a
/// caller's behalf are what set those keys — which is the request EC2 admits them from.
/// Nothing is called for a resource with no tags at all.
pub(crate) async fn tag_ec2_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_id: &str,
    tags: &[(String, String)],
) -> Result<(), Refusal> {
    if tags.is_empty() {
        return Ok(());
    }
    let mut parameters = vec![("ResourceId.1".to_string(), resource_id.to_string())];
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("Tag.{position}.Key"), key.clone()));
        parameters.push((format!("Tag.{position}.Value"), value.clone()));
    }
    call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateTags",
        &parameters,
    )
    .await
    .map(|_| ())
}

/// One Amazon EC2 `Modify…Attribute` call: the action, and the member of it that names the
/// resource the attribute belongs to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ec2AttributeCall {
    pub(crate) action: &'static str,
    pub(crate) id_member: &'static str,
}

/// The two of them these arms write through: a VPC's attributes and a subnet's.
pub(crate) const MODIFY_VPC_ATTRIBUTE: Ec2AttributeCall = Ec2AttributeCall {
    action: "ModifyVpcAttribute",
    id_member: "VpcId",
};
pub(crate) const MODIFY_SUBNET_ATTRIBUTE: Ec2AttributeCall = Ec2AttributeCall {
    action: "ModifySubnetAttribute",
    id_member: "SubnetId",
};

/// Write ONE attribute of a VPC or a subnet: the `Modify…Attribute` call that owns the
/// attribute, carrying the one member it names.
///
/// `boolean` is whether that member is an `AttributeBooleanValue`, whose value travels as
/// its `Value` member — the shape every switch of either call takes, beside the one plain
/// string (`PrivateDnsHostnameTypeOnLaunch`).
pub(crate) async fn modify_ec2_attribute(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    call: Ec2AttributeCall,
    resource_id: &str,
    member: &str,
    value: &Value,
    boolean: bool,
) -> Result<(), Refusal> {
    let name = if boolean {
        format!("{member}.Value")
    } else {
        member.to_string()
    };
    let parameters = vec![
        (call.id_member.to_string(), resource_id.to_string()),
        (name, attribute_text(value)),
    ];
    call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        call.action,
        &parameters,
    )
    .await
    .map(|_| ())
}

/// The scalar members Amazon EC2 reports a resource with, under the names the TYPE's
/// properties and return values use.
///
/// An ec2Query answer spells a member in lower camel case (`<vpcId>`, `<cidrBlock>`) while
/// the model's `ec2QueryName` for that same member — and the CloudFormation property it
/// corresponds to — is that name with an upper-case initial, so the one transformation puts
/// the reported document in the names a declaration is written in. That is what lets drift
/// detection compare a declared property against what EC2 reports for it.
pub(crate) fn ec2_reported(document: &str, parent: &str) -> Map<String, Value> {
    xml_scalar_children(document, parent)
        .into_iter()
        .map(|(member, value)| {
            let mut characters = member.chars();
            let named = match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => member,
            };
            (named, value)
        })
        .collect()
}

/// The one resource a describe's RESULT SET carries, read the same way: the document from
/// the `<…Set>` element the answer nests its entries in, so the `<item>` read is that set's
/// own first entry rather than the first element of that name anywhere in the answer.
///
/// `set` is the list member's `@xmlName` (`vpcSet`, `subnetSet`), and `ELEMENT` the name an
/// ec2Query list's entries carry. Both describes here name ONE id, so that set holds one
/// entry; a read of several would need the entry matched to the id rather than taken by
/// position.
pub(crate) fn ec2_reported_entry(document: &str, set: &str) -> Map<String, Value> {
    /// The element name an ec2Query list wraps each of its entries in.
    const ELEMENT: &str = "item";

    let entries = document
        .find(&format!("<{set}>"))
        .map_or(document, |start| &document[start..]);
    ec2_reported(entries, ELEMENT)
}

/// Create the VPC an `AWS::EC2::VPC` declares, in Amazon EC2.
///
/// "Ref returns the ID of the VPC" — an id EC2 ASSIGNS (`vpc-` and 17 hexadecimal
/// characters), which is what this answers as the resource's physical id, so the stack's
/// `Ref`, its outputs and its per-resource reads report an id EC2 really holds a VPC for.
/// The two DNS switches are attributes of the VPC rather than members of the create, and
/// the tag set is the one call that tags a resource that exists.
pub(crate) async fn create_ec2_vpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let parameters = query_members(
        &resource.properties,
        EC2_VPC_CREATE_MEMBERS,
        &[],
        EC2_VPC_PROPERTIES_NOT_SENT,
        EC2_VPC,
    )?;
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateVpc",
        &parameters,
    )
    .await?;
    let reported = ec2_reported(&answered, "vpc");
    let vpc_id = reported
        .get("VpcId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateVpc answered no vpcId".to_string(),
        })?
        .to_string();
    for attribute in EC2_VPC_ATTRIBUTES {
        if let Some(value) = resource.properties.get(*attribute) {
            modify_ec2_attribute(
                ctx,
                region,
                MODIFY_VPC_ATTRIBUTE,
                &vpc_id,
                attribute,
                value,
                true,
            )
            .await?;
        }
    }
    tag_ec2_resource(
        ctx,
        region,
        &vpc_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The attributes the type publishes that EC2 itself decides: the id it assigned (which
    // this type's `VpcId` return value is) and the block it canonicalised the declared one
    // into. The two that are ANOTHER EC2 resource's id — the default security group and the
    // default network ACL "for this VPC", which that service creates with the VPC — are not
    // reported by this call, so each is read back from the service that holds it
    // ([`ec2_vpc_default_member`]) rather than invented or left absent.
    let mut attributes = BTreeMap::new();
    attributes.insert("VpcId".to_string(), vpc_id.clone());
    if let Some(cidr_block) = reported.get("CidrBlock").and_then(Value::as_str) {
        attributes.insert("CidrBlock".to_string(), cidr_block.to_string());
    }
    for (attribute, read) in [
        ("DefaultSecurityGroup", DEFAULT_SECURITY_GROUP_OF_VPC),
        ("DefaultNetworkAcl", DEFAULT_NETWORK_ACL_OF_VPC),
    ] {
        if let Some(id) = ec2_vpc_default_member(ctx, region, &vpc_id, read).await? {
            attributes.insert(attribute.to_string(), id);
        }
    }
    Ok(Created {
        physical_resource_id: Some(vpc_id),
        attributes,
    })
}

/// One Amazon EC2 read that answers a resource the service created WITH a VPC: the describe
/// to make, the filter that narrows it to the one the VPC has, and the element its id is
/// reported under.
///
/// A VPC's published `DefaultSecurityGroup` and `DefaultNetworkAcl` return values are the ids
/// of resources `CreateVpc` does not report — "when you create a VPC, it comes with a default
/// security group" and a default network ACL — so each is read from EC2 by the attribute that
/// makes it the VPC's default one.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ec2VpcDefaultMember {
    /// The describe that reports the resource.
    action: &'static str,
    /// The filter name and value that select the VPC's DEFAULT one, beside the `vpc-id`
    /// filter every such read carries.
    default_filter: (&'static str, &'static str),
    /// The element of the answer that carries the resource's id, as an `ec2Query` answer
    /// spells it (lower camel case).
    id_element: &'static str,
}

/// The default security group of a VPC: "the ID of the default security group for this VPC".
/// The group is the one named `default` among the groups of that VPC, which is the group EC2
/// creates with the VPC and the only one of that name it can hold
/// (`DescribeSecurityGroups`'s `group-name` filter).
pub(crate) const DEFAULT_SECURITY_GROUP_OF_VPC: Ec2VpcDefaultMember = Ec2VpcDefaultMember {
    action: "DescribeSecurityGroups",
    default_filter: ("group-name", "default"),
    id_element: "groupId",
};

/// The default network ACL of a VPC: "the ID of the default network ACL for this VPC", which
/// `DescribeNetworkAcls` reports under its own `default` filter.
pub(crate) const DEFAULT_NETWORK_ACL_OF_VPC: Ec2VpcDefaultMember = Ec2VpcDefaultMember {
    action: "DescribeNetworkAcls",
    default_filter: ("default", "true"),
    id_element: "networkAclId",
};

/// The id of the resource `read` names for the VPC `vpc_id`, as Amazon EC2 reports it, or
/// `None` where that service reports none.
///
/// A filtered describe answers an empty set rather than a refusal for a VPC that has no such
/// resource, so an absent one leaves the attribute off rather than failing the create: the
/// attribute carries only what EC2 really holds.
pub(crate) async fn ec2_vpc_default_member(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
    read: Ec2VpcDefaultMember,
) -> Result<Option<String>, Refusal> {
    let (filter_name, filter_value) = read.default_filter;
    let parameters = vec![
        ("Filter.1.Name".to_string(), "vpc-id".to_string()),
        ("Filter.1.Value.1".to_string(), vpc_id.to_string()),
        ("Filter.2.Name".to_string(), filter_name.to_string()),
        ("Filter.2.Value.1".to_string(), filter_value.to_string()),
    ];
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        read.action,
        &parameters,
    )
    .await?;
    Ok(xml_element(&answered, read.id_element).filter(|id| !id.is_empty()))
}

/// Create the subnet an `AWS::EC2::Subnet` declares, in Amazon EC2.
///
/// "Ref returns the subnet ID", again an id EC2 assigns, and the subnet is created IN the
/// VPC its `VpcId` names — the id EC2 assigned the VPC of the same template, since a
/// deployment creates each resource after the ones it depends on and re-points a reference
/// at the identifier the owning service minted.
pub(crate) async fn create_ec2_subnet(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let parameters = query_members(
        &resource.properties,
        EC2_SUBNET_CREATE_MEMBERS,
        &[],
        EC2_SUBNET_PROPERTIES_NOT_SENT,
        EC2_SUBNET,
    )?;
    let answered = call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "CreateSubnet",
        &parameters,
    )
    .await?;
    let reported = ec2_reported(&answered, "subnet");
    let subnet_id = reported
        .get("SubnetId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateSubnet answered no subnetId".to_string(),
        })?
        .to_string();
    for attribute in EC2_SUBNET_BOOLEAN_ATTRIBUTES {
        if let Some(value) = resource.properties.get(*attribute) {
            modify_ec2_attribute(
                ctx,
                region,
                MODIFY_SUBNET_ATTRIBUTE,
                &subnet_id,
                attribute,
                value,
                true,
            )
            .await?;
        }
    }
    for (property, member, boolean) in EC2_SUBNET_PRIVATE_DNS_ATTRIBUTES {
        let Some(value) = resource
            .properties
            .get("PrivateDnsNameOptionsOnLaunch")
            .and_then(|options| options.get(*property))
        else {
            continue;
        };
        modify_ec2_attribute(
            ctx,
            region,
            MODIFY_SUBNET_ATTRIBUTE,
            &subnet_id,
            member,
            value,
            *boolean,
        )
        .await?;
    }
    tag_ec2_resource(
        ctx,
        region,
        &subnet_id,
        &tags_of(&resource.properties, propagated),
    )
    .await?;
    // The attributes this type publishes that EC2 itself decides: the id it assigned, the
    // VPC the subnet really sits in, the zone it placed the subnet in (and that zone's id)
    // where the declaration named none, the canonicalised block, and the subnet's ARN. The
    // published attributes nothing here holds (`NetworkAclAssociationId`) are left out
    // rather than invented.
    let mut attributes = BTreeMap::new();
    attributes.insert("SubnetId".to_string(), subnet_id.clone());
    for attribute in [
        "VpcId",
        "AvailabilityZone",
        "AvailabilityZoneId",
        "CidrBlock",
        "OutpostArn",
    ] {
        if let Some(value) = reported
            .get(attribute)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    }
    if let Some(arn) = reported
        .get("SubnetArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
    {
        attributes.insert("Arn".to_string(), arn.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(subnet_id),
        attributes,
    })
}

/// Delete the VPC a stack created, addressed by the id EC2 assigned it — this resource's
/// physical id. A VPC EC2 no longer holds (`InvalidVpcID.NotFound`) is already in the state
/// the delete asks for.
pub(crate) async fn delete_ec2_vpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("VpcId".to_string(), vpc_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteVpc",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_VPC_ID_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete the subnet a stack created, the same way — and before the VPC that holds it, which
/// is what [`super::provisioning::delete`] walking a stack's resources newest-first gives.
pub(crate) async fn delete_ec2_subnet(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("SubnetId".to_string(), subnet_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DeleteSubnet",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == INVALID_SUBNET_ID_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The VPC's configuration as Amazon EC2 reports it, for drift detection. A VPC it no
/// longer holds (`InvalidVpcID.NotFound`) was deleted.
pub(crate) async fn describe_ec2_vpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("VpcId.1".to_string(), vpc_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeVpcs",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(&answered, "vpcSet")))),
        Err(refusal) if refusal.code == INVALID_VPC_ID_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The subnet's configuration as Amazon EC2 reports it, read the same way.
pub(crate) async fn describe_ec2_subnet(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("SubnetId.1".to_string(), subnet_id.to_string())];
    match call_query(
        ctx,
        EC2_SIGNING_NAME,
        region,
        EC2_API_VERSION,
        "DescribeSubnets",
        &parameters,
    )
    .await
    {
        Ok(answered) => Ok(Some(Value::Object(ec2_reported_entry(&answered, "subnetSet")))),
        Err(refusal) if refusal.code == INVALID_SUBNET_ID_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `AWS::EC2::SecurityGroup` properties `CreateSecurityGroup` takes, each a scalar for
/// the reason [`EC2_VPC_CREATE_MEMBERS`] records.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreateSecurityGroup")`. The description is
/// `GroupDescription` on both sides — that is the `@xmlName` of the create's `Description`
/// member, which is the parameter name an ec2Query request carries it under — and `VpcId` is
/// the VPC the group is created in, a `Ref` to the VPC of the same template resolving to the
/// id EC2 assigned it ([`super::provisioning::reconcile`]). The group's own `sg-…` id is
/// EC2's to assign, exactly as a VPC's is.
pub(crate) const EC2_SECURITY_GROUP_CREATE_MEMBERS: &[(&str, QueryMember)] = &[
    ("GroupDescription", QueryMember::Scalar),
    ("VpcId", QueryMember::Scalar),
];

/// The `AWS::EC2::SecurityGroup` properties another parameter or another call carries: the
/// NAME, which `CreateSecurityGroup` requires and the type leaves optional, so the arm
/// pushes the name CloudFormation decided ([`declared_name`]); the two inline rule lists,
/// which are not members of the create at all and are each one authorize call per rule; and
/// the tags, which reach EC2 through [`tag_ec2_resource`].
pub(crate) const EC2_SECURITY_GROUP_PROPERTIES_NOT_SENT: &[&str] = &[
    "GroupName",
    "SecurityGroupEgress",
    "SecurityGroupIngress",
    "Tags",
];

/// Where one member of an inline security group rule goes inside the `IpPermission` the
/// authorize call for its direction takes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RuleMember {
    /// A member of the permission itself (`IpProtocol`, `FromPort`, `ToPort`).
    Permission(&'static str),
    /// A member of the SOURCE the rule grants to: the permission's list that carries that
    /// kind of source, and the member of its one entry the value is (`IpRanges` `CidrIp`,
    /// `Groups` `GroupId`).
    Source(&'static str, &'static str),
}

/// The `Description` member an inline rule may carry, which is a member of the SOURCE the
/// rule references rather than of the permission ([`security_group_rule_parameters`]).
pub(crate) const RULE_DESCRIPTION: &str = "Description";

/// The published members of an `AWS::EC2::SecurityGroup` `Ingress` entry, each against the
/// `IpPermission` member `AuthorizeSecurityGroupIngress` takes it as.
///
/// Source: the type's `Ingress` property type in AWS's resource and property type reference
/// against `smithy_query(query="shape",shape="IpPermission,IpRange,Ipv6Range,PrefixListId,
/// UserIdGroupPair")`. One inline rule is ONE permission, and the source it names decides
/// which of the permission's four source lists carries it — "you must specify exactly one of
/// the following sources" — so the three members that name a source group are all members of
/// the `Groups` entry.
pub(crate) const EC2_SECURITY_GROUP_INGRESS_MEMBERS: &[(&str, RuleMember)] = &[
    ("IpProtocol", RuleMember::Permission("IpProtocol")),
    ("FromPort", RuleMember::Permission("FromPort")),
    ("ToPort", RuleMember::Permission("ToPort")),
    ("CidrIp", RuleMember::Source("IpRanges", "CidrIp")),
    ("CidrIpv6", RuleMember::Source("Ipv6Ranges", "CidrIpv6")),
    (
        "SourcePrefixListId",
        RuleMember::Source("PrefixListIds", "PrefixListId"),
    ),
    (
        "SourceSecurityGroupId",
        RuleMember::Source("Groups", "GroupId"),
    ),
    (
        "SourceSecurityGroupName",
        RuleMember::Source("Groups", "GroupName"),
    ),
    (
        "SourceSecurityGroupOwnerId",
        RuleMember::Source("Groups", "UserId"),
    ),
];

/// The published members of an `AWS::EC2::SecurityGroup` `Egress` entry, read the same way
/// against `AuthorizeSecurityGroupEgress`: an outbound rule names a DESTINATION, which is
/// the same permission structure under that type's own property names.
pub(crate) const EC2_SECURITY_GROUP_EGRESS_MEMBERS: &[(&str, RuleMember)] = &[
    ("IpProtocol", RuleMember::Permission("IpProtocol")),
    ("FromPort", RuleMember::Permission("FromPort")),
    ("ToPort", RuleMember::Permission("ToPort")),
    ("CidrIp", RuleMember::Source("IpRanges", "CidrIp")),
    ("CidrIpv6", RuleMember::Source("Ipv6Ranges", "CidrIpv6")),
    (
        "DestinationPrefixListId",
        RuleMember::Source("PrefixListIds", "PrefixListId"),
    ),
    (
        "DestinationSecurityGroupId",
        RuleMember::Source("Groups", "GroupId"),
    ),
];

/// The inline rules one of a security group's two rule properties declares, as the entries
/// each authorize call takes one of.
///
/// A declaration that writes a single rule without the sequence syntax is the one-entry list
/// of itself, exactly as [`push_list`] reads one, so such a rule is not dropped.
pub(crate) fn declared_security_group_rules<'declaration>(
    properties: &'declaration Value,
    property: &str,
) -> Vec<&'declaration Value> {
    match properties.get(property) {
        Some(Value::Array(rules)) => rules.iter().collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(single) => vec![single],
    }
}

/// ONE inline rule of a security group as the `IpPermissions.1.…` parameters the authorize
/// call for its direction takes, walked against that rule type's published members.
///
/// A member the catalogue does not carry is REFUSED, for the reason [`property_not_sent`]
/// records: a group authorized without a rule its declaration asked for is not the group the
/// template declared. The rule's `Description` is a member of the source it references
/// (`IpRange$Description`, `UserIdGroupPair$Description`, `PrefixListId$Description`), so it
/// is written onto the list that source is in — and a rule naming no source at all carries it
/// nowhere, which leaves Amazon EC2's own refusal of a rule that names none ("you must
/// specify exactly one of the following sources") as the answer.
pub(crate) fn security_group_rule_parameters(
    rule: &Value,
    members: &[(&str, RuleMember)],
) -> Result<Vec<(String, String)>, Refusal> {
    /// The index of the one permission each of these calls carries: a rule is one
    /// permission, and an ec2Query list is indexed from 1.
    const PERMISSION: &str = "IpPermissions.1";

    let mut parameters: Vec<(String, String)> = Vec::new();
    let Some(declared) = rule.as_object() else {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "a rule of {EC2_SECURITY_GROUP} is a structure of the members its rule \
                 property type publishes, so {rule} is not one this emulator can authorize"
            ),
        });
    };
    let mut source: Option<&str> = None;
    for (member, value) in declared {
        if member == RULE_DESCRIPTION {
            continue;
        }
        let Some((_, placed)) = members.iter().find(|(name, _)| *name == member.as_str()) else {
            return Err(property_not_sent(EC2_SECURITY_GROUP, member));
        };
        match placed {
            RuleMember::Permission(name) => {
                parameters.push((format!("{PERMISSION}.{name}"), attribute_text(value)));
            }
            RuleMember::Source(list, name) => {
                source = Some(list);
                parameters.push((
                    format!("{PERMISSION}.{list}.1.{name}"),
                    attribute_text(value),
                ));
            }
        }
    }
    if let (Some(description), Some(list)) = (declared.get(RULE_DESCRIPTION), source) {
        parameters.push((
            format!("{PERMISSION}.{list}.1.{RULE_DESCRIPTION}"),
            attribute_text(description),
        ));
    }
    Ok(parameters)
}

/// Amazon EC2's error code for a rule a revoke named that the group does not hold, which is
/// the answer to a revoke of the default outbound rule a group no longer carries — the state
/// that revoke asks for.
pub(crate) const INVALID_PERMISSION_NOT_FOUND: &str = "InvalidPermission.NotFound";

/// The all-protocols value of a security group rule's protocol, which the outbound rule
/// Amazon EC2 gives every new group carries.
pub(crate) const EC2_ALL_IP_PROTOCOLS: &str = "-1";

/// The destination that same rule carries: every IPv4 address.
pub(crate) const EC2_ALL_IPV4_ADDRESSES: &str = "0.0.0.0/0";

/// The one permission of the default outbound rule a new security group carries — "a
/// security group has an outbound rule that allows all outbound traffic" — as the
/// `IpPermissions.1.…` parameters a revoke of it names it by.
pub(crate) fn allow_all_egress_parameters() -> Vec<(String, String)> {
    vec![
        (
            "IpPermissions.1.IpProtocol".to_string(),
            EC2_ALL_IP_PROTOCOLS.to_string(),
        ),
        (
            "IpPermissions.1.IpRanges.1.CidrIp".to_string(),
            EC2_ALL_IPV4_ADDRESSES.to_string(),
        ),
    ]
}
