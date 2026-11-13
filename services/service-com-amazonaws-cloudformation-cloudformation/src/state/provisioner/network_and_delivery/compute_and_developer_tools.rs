// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the compute and developer-tool services: Amazon EKS,
//! Amazon API Gateway, AWS CodeBuild, AWS CodeCommit, AWS CodeArtifact and AWS KMS. One file
//! of [`super::super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;

/// Create the access entry an `AWS::EKS::AccessEntry` declares, and associate every access
/// policy its `AccessPolicies` lists to it — so a client that deployed the stack and then
/// calls `eks describe-access-entry` for the declared principal reads the entry back, and
/// `eks list-associated-access-policies` lists the declared policies, rather than a
/// `ResourceNotFoundException` for an entry nothing created.
///
/// The entry's own properties are that create's members ([`EKS_ACCESS_ENTRY_MEMBERS`]),
/// with the cluster in the URI and the declared tags beside the ones CloudFormation gives
/// the resource ([`tags_of`]) as the tag MAP `CreateAccessEntry` takes. Each
/// `AccessPolicies` entry is then one `AssociateAccessPolicy` on the entry just created:
/// that is a call of its own, exactly as it is for a client.
///
/// An association Amazon EKS refuses leaves NO entry behind: the entry this arm created is
/// deleted before the refusal is reported, so the resource fails — and with it the stack —
/// with nothing half-configured in Amazon EKS for a resource the stack does not record.
///
/// The ARN Amazon EKS minted for the entry is reported back as the `AccessEntryArn`
/// attribute the type publishes, so `Fn::GetAtt` reads the handle that service really
/// created.
pub(crate) async fn create_access_entry(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let (cluster_name, principal_arn) = access_entry_identity(resource)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    copy_members(&properties, EKS_ACCESS_ENTRY_MEMBERS, &mut request);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        EKS_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{EKS_CLUSTERS_URI}/{}/access-entries",
            percent_encoded(&cluster_name)
        ),
        Some(&Value::Object(request)),
    )
    .await?;
    let arn = answered
        .get("accessEntry")
        .and_then(|entry| entry.get("accessEntryArn"))
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon EKS answered CreateAccessEntry without an accessEntryArn"
                .to_string(),
        })?
        .to_string();
    for declared in declared_list(&resource.properties, "AccessPolicies") {
        let associated = match access_policy_request(resource, declared) {
            Ok(request) => {
                call_rest_json(
                    ctx,
                    EKS_SIGNING_NAME,
                    region,
                    "POST",
                    &format!(
                        "{}/access-policies",
                        access_entry_uri(&cluster_name, &principal_arn)
                    ),
                    Some(&request),
                )
                .await
                .map(|_| ())
            }
            Err(refusal) => Err(refusal),
        };
        if let Err(refusal) = associated {
            // The entry exists but not with the access it was declared with, and the stack
            // will record no resource for it, so it is removed before the failure is
            // reported — a refused delete changes nothing about the refusal that caused it.
            let _ = delete_access_entry(ctx, region, resource).await;
            return Err(refusal);
        }
    }
    let mut attributes = BTreeMap::new();
    attributes.insert("AccessEntryArn".to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// Delete the access entry a stack created, addressed by the cluster-and-principal pair
/// Amazon EKS keys one by.
///
/// An entry that service no longer holds — and a cluster it no longer holds, which took
/// the entry with it — answers `ResourceNotFoundException`, judged on the CODE and never on
/// a message, and is already in the state the delete asks for. Deleting the entry takes its
/// associated access policies with it, so nothing disassociates them first.
pub(crate) async fn delete_access_entry(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (cluster_name, principal_arn) = access_entry_identity(resource)?;
    let uri = access_entry_uri(&cluster_name, &principal_arn);
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The access entry's configuration as Amazon EKS reports it, for drift detection:
/// `DescribeAccessEntry`'s own record, read by the same pair the delete uses. One that
/// service no longer holds (`ResourceNotFoundException`) was deleted.
pub(crate) async fn describe_access_entry(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (cluster_name, principal_arn) = access_entry_identity(resource)?;
    let uri = access_entry_uri(&cluster_name, &principal_arn);
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            answer.get("accessEntry").unwrap_or(&Value::Null),
            EKS_ACCESS_ENTRY_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The pair that identifies one access entry: the cluster holding it and the IAM principal
/// it is for, both of them required properties of the type and both read off the resource's
/// own declaration. A declaration missing either names no entry that could be created or
/// addressed, so it is refused here rather than with a request Amazon EKS would reject.
pub(crate) fn access_entry_identity(resource: &StoredResource) -> Result<(String, String), Refusal> {
    Ok((
        required_property(resource, "ClusterName")?,
        required_property(resource, "PrincipalArn")?,
    ))
}

/// The URI that addresses ONE access entry: the two `@httpLabel`s of every operation on it,
/// each percent-encoded, as a non-greedy label is sent — a principal ARN carries both `:`
/// and `/`, so it reaches that service as one value rather than as extra URI structure.
pub(crate) fn access_entry_uri(cluster_name: &str, principal_arn: &str) -> String {
    format!(
        "{EKS_CLUSTERS_URI}/{}/access-entries/{}",
        percent_encoded(cluster_name),
        percent_encoded(principal_arn)
    )
}

/// One entry of the type's `AccessPolicies` property as `AssociateAccessPolicy` takes it:
/// the policy ARN it names and the scope that policy is associated under.
///
/// Both are required members of that call and of the type's own `AccessPolicy` property, so
/// an entry carrying neither describes no association that could be made and is refused
/// here.
pub(crate) fn access_policy_request(resource: &StoredResource, declared: &Value) -> Result<Value, Refusal> {
    let missing = |property: &str| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "{} declares an AccessPolicies entry with no {property}",
            resource.resource_type
        ),
    };
    let policy_arn = declared
        .get("PolicyArn")
        .and_then(scalar_text)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| missing("PolicyArn"))?;
    let scope = declared
        .get("AccessScope")
        .ok_or_else(|| missing("AccessScope"))?;
    let mut request = Map::new();
    request.insert("policyArn".to_string(), Value::String(policy_arn));
    request.insert(
        "accessScope".to_string(),
        mapped_members(scope, EKS_ACCESS_SCOPE_MEMBERS),
    );
    Ok(Value::Object(request))
}

// ---------------------------------------------------------------------------
// Amazon EKS: AWS::EKS::Cluster and AWS::EKS::PodIdentityAssociation
// ---------------------------------------------------------------------------

/// The URI Amazon EKS binds a cluster's pod identity associations to, as that model's
/// `@http` traits declare it: `POST /clusters/{clusterName}/pod-identity-associations`
/// creates one and `DELETE /clusters/{clusterName}/pod-identity-associations/{associationId}`
/// removes it
/// (`smithy_query(query="operation",operation="CreatePodIdentityAssociation,\
/// DeletePodIdentityAssociation")` on that service's model).
pub(crate) const EKS_POD_IDENTITY_ASSOCIATIONS_PATH: &str = "pod-identity-associations";

/// The `Cluster$status` a cluster is usable in: "you must wait for the cluster status to
/// become `ACTIVE` before you can add nodes" (`CreateCluster`).
pub(crate) const EKS_CLUSTER_ACTIVE: &str = "ACTIVE";

/// The `Cluster$status` a created cluster starts in, which that create answers with.
pub(crate) const EKS_CLUSTER_CREATING: &str = "CREATING";

/// The `AWS::EKS::Cluster` properties `CreateCluster` takes under its own member name, as
/// (property, member) pairs — each a SCALAR of the declaration.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateClusterRequest` shape. The Amazon EKS model spells its members in lower camel
/// case and renames none of these, so the only difference is the first character's case.
/// `Name` is not here because the cluster's name is its physical resource id, which the
/// create below sends; `Tags` is not because the call takes a tag MAP while the type
/// publishes a tag LIST.
pub(crate) const EKS_CLUSTER_MEMBERS: &[(&str, &str)] = &[
    ("BootstrapSelfManagedAddons", "bootstrapSelfManagedAddons"),
    ("DeletionProtection", "deletionProtection"),
    ("RoleArn", "roleArn"),
    ("Version", "version"),
];

/// The members of the type's `ResourcesVpcConfig` property as `CreateCluster`'s
/// `resourcesVpcConfig` (a `VpcConfigRequest`) takes them.
pub(crate) const EKS_VPC_CONFIG_MEMBERS: &[(&str, &str)] = &[
    ("EndpointPrivateAccess", "endpointPrivateAccess"),
    ("EndpointPublicAccess", "endpointPublicAccess"),
    ("PublicAccessCidrs", "publicAccessCidrs"),
    ("SecurityGroupIds", "securityGroupIds"),
    ("SubnetIds", "subnetIds"),
];

/// The members of the type's `AccessConfig` property as that create's `accessConfig` (a
/// `CreateAccessConfigRequest`) takes them.
pub(crate) const EKS_ACCESS_CONFIG_MEMBERS: &[(&str, &str)] = &[
    ("AuthenticationMode", "authenticationMode"),
    (
        "BootstrapClusterCreatorAdminPermissions",
        "bootstrapClusterCreatorAdminPermissions",
    ),
];

/// The members of the type's `KubernetesNetworkConfig` property as that create's
/// `kubernetesNetworkConfig` (a `KubernetesNetworkConfigRequest`) takes them.
pub(crate) const EKS_NETWORK_CONFIG_MEMBERS: &[(&str, &str)] = &[
    ("IpFamily", "ipFamily"),
    ("ServiceIpv4Cidr", "serviceIpv4Cidr"),
];

/// The members of the type's `UpgradePolicy` property as that create's `upgradePolicy` (an
/// `UpgradePolicyRequest`) takes them.
pub(crate) const EKS_UPGRADE_POLICY_MEMBERS: &[(&str, &str)] = &[("SupportType", "supportType")];

/// The members of the type's `ZonalShiftConfig` property as that create's
/// `zonalShiftConfig` (a `ZonalShiftConfigRequest`) takes them.
pub(crate) const EKS_ZONAL_SHIFT_MEMBERS: &[(&str, &str)] = &[("Enabled", "enabled")];

/// The members of one `EncryptionConfig` entry as that create's `encryptionConfig` list
/// takes them: the resources it covers, and the provider whose one member is the KMS key.
pub(crate) const EKS_ENCRYPTION_CONFIG_MEMBERS: &[(&str, &str)] = &[("Resources", "resources")];

/// The members of an `EncryptionConfig` entry's `Provider` as that list's `provider` (a
/// `Provider`) takes them.
pub(crate) const EKS_ENCRYPTION_PROVIDER_MEMBERS: &[(&str, &str)] = &[("KeyArn", "keyArn")];

/// Every `AWS::EKS::Cluster` property this arm maps onto `CreateCluster`, so a declared
/// property it does NOT map is refused rather than dropped
/// ([`refuse_unmapped_properties`]): a cluster created without what its declaration asked
/// for is not the cluster the template declared.
///
/// `ComputeConfig`, `StorageConfig`, `RemoteNetworkConfig`, `OutpostConfig` and `Logging`
/// are the properties no arm maps: each is a nested configuration of a cluster this
/// emulator holds no state for (auto mode's compute and storage, a hybrid remote network,
/// an Outpost control plane, control-plane log export to Amazon CloudWatch Logs), so a
/// declaration that asks for one is refused rather than silently created without it.
pub(crate) const EKS_CLUSTER_MAPPED_PROPERTIES: &[&str] = &[
    "AccessConfig",
    "BootstrapSelfManagedAddons",
    "DeletionProtection",
    "EncryptionConfig",
    "KubernetesNetworkConfig",
    "Name",
    "ResourcesVpcConfig",
    "RoleArn",
    "Tags",
    "UpgradePolicy",
    "Version",
    "ZonalShiftConfig",
];

/// The members `DescribeCluster` reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// Only the scalars that are a property of the declaration: the version in force, the role
/// the control plane assumes and the name itself. The nested configurations that service
/// reports back are its own `…Response` shapes rather than the request shapes a declaration
/// states, so they are not evidence of a declared value ([`super::drift`]), and `Tags` is
/// left out for the same reason as everywhere else here.
pub(crate) const EKS_CLUSTER_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "name"),
    ("RoleArn", "roleArn"),
    ("Version", "version"),
];

/// The `AWS::EKS::PodIdentityAssociation` properties `CreatePodIdentityAssociation` takes
/// under its own member name. `ClusterName` is not here because that create binds it to its
/// URI, and `Tags` is not because the call takes a tag MAP.
pub(crate) const EKS_POD_IDENTITY_MEMBERS: &[(&str, &str)] = &[
    ("DisableSessionTags", "disableSessionTags"),
    ("Namespace", "namespace"),
    ("RoleArn", "roleArn"),
    ("ServiceAccount", "serviceAccount"),
    ("TargetRoleArn", "targetRoleArn"),
];

/// Every `AWS::EKS::PodIdentityAssociation` property this arm maps, on the terms
/// [`EKS_CLUSTER_MAPPED_PROPERTIES`] records.
pub(crate) const EKS_POD_IDENTITY_MAPPED_PROPERTIES: &[&str] = &[
    "ClusterName",
    "DisableSessionTags",
    "Namespace",
    "RoleArn",
    "ServiceAccount",
    "Tags",
    "TargetRoleArn",
];

/// Create the cluster an `AWS::EKS::Cluster` declares, in Amazon EKS — so a client that
/// deployed the stack and then calls `eks describe-cluster` for the declared name reads the
/// cluster back, and finds it in `eks list-clusters`, rather than a
/// `ResourceNotFoundException` for a cluster nothing created.
///
/// The cluster's name is the resource's physical id (the `Name` the declaration states, or
/// the name CloudFormation computed for it), and its own properties are that create's
/// members — the scalars of [`EKS_CLUSTER_MEMBERS`] and the nested configurations mapped
/// member by member — with the declared tags beside the ones CloudFormation gives the
/// resource as the tag MAP the call takes.
///
/// What Amazon EKS answers is what the stack reports: the ARN it minted, the API server
/// endpoint, the cluster's certificate authority data, the OIDC issuer URL of its identity
/// provider and the security group it made for the control plane are each an attribute the
/// type publishes that only that service can produce, so each is read off the created
/// cluster rather than computed here. The create leaves the cluster `CREATING`, and
/// [`eks_cluster_readiness`] is what the resource's creation is held open on.
pub(crate) async fn create_eks_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, EKS_CLUSTER_MAPPED_PROPERTIES)?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, EKS_CLUSTER_MEMBERS, &mut request);
    for (property, member, pairs) in [
        (
            "ResourcesVpcConfig",
            "resourcesVpcConfig",
            EKS_VPC_CONFIG_MEMBERS,
        ),
        ("AccessConfig", "accessConfig", EKS_ACCESS_CONFIG_MEMBERS),
        (
            "KubernetesNetworkConfig",
            "kubernetesNetworkConfig",
            EKS_NETWORK_CONFIG_MEMBERS,
        ),
        ("UpgradePolicy", "upgradePolicy", EKS_UPGRADE_POLICY_MEMBERS),
        ("ZonalShiftConfig", "zonalShiftConfig", EKS_ZONAL_SHIFT_MEMBERS),
    ] {
        if let Some(declared) = properties.get(property) {
            request.insert(member.to_string(), mapped_members(declared, pairs));
        }
    }
    // "The encryption configuration for the cluster" is a LIST of at most one entry, each
    // naming the resources it covers and the KMS key provider that encrypts them, so every
    // entry is mapped the same way its own members are.
    let encryption: Vec<Value> = declared_list(&resource.properties, "EncryptionConfig")
        .iter()
        .map(|declared| {
            let mut entry = mapped_members(declared, EKS_ENCRYPTION_CONFIG_MEMBERS);
            if let (Some(provider), Value::Object(members)) =
                (declared.get("Provider"), &mut entry)
            {
                members.insert(
                    "provider".to_string(),
                    mapped_members(provider, EKS_ENCRYPTION_PROVIDER_MEMBERS),
                );
            }
            entry
        })
        .collect();
    if !encryption.is_empty() {
        request.insert("encryptionConfig".to_string(), Value::Array(encryption));
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        EKS_SIGNING_NAME,
        region,
        "POST",
        EKS_CLUSTERS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let cluster = answered.get("cluster").cloned().unwrap_or(Value::Null);
    let name = cluster
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon EKS answered CreateCluster without a cluster name".to_string(),
        })?
        .to_string();
    Ok(Created {
        // "Ref returns the name of the cluster": the handle every Amazon EKS operation on
        // one takes, as that service reported it.
        physical_resource_id: Some(name),
        attributes: eks_cluster_attributes(&cluster),
    })
}

/// The attributes an `AWS::EKS::Cluster` publishes that only Amazon EKS can produce, read
/// off the cluster record that service reported.
///
/// One reading for both the create and the readiness read, so the attributes a stack
/// reports for a cluster cannot depend on which call last looked at it. An attribute that
/// service does not report is left out rather than invented: the stand-in the naming rules
/// computed stays in place, which is what says the value is not available.
pub(crate) fn eks_cluster_attributes(cluster: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let mut insert = |attribute: &str, value: Option<&str>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            attributes.insert(attribute.to_string(), value.to_string());
        }
    };
    insert("Arn", cluster.get("arn").and_then(Value::as_str));
    insert("Endpoint", cluster.get("endpoint").and_then(Value::as_str));
    insert(
        "CertificateAuthorityData",
        cluster
            .get("certificateAuthority")
            .and_then(|authority| authority.get("data"))
            .and_then(Value::as_str),
    );
    insert(
        "OpenIdConnectIssuerUrl",
        cluster
            .get("identity")
            .and_then(|identity| identity.get("oidc"))
            .and_then(|oidc| oidc.get("issuer"))
            .and_then(Value::as_str),
    );
    insert(
        "ClusterSecurityGroupId",
        cluster
            .get("resourcesVpcConfig")
            .and_then(|config| config.get("clusterSecurityGroupId"))
            .and_then(Value::as_str),
    );
    insert(
        "EncryptionConfigKeyArn",
        cluster
            .get("encryptionConfig")
            .and_then(Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("provider"))
            .and_then(|provider| provider.get("keyArn"))
            .and_then(Value::as_str),
    );
    attributes
}

/// Whether Amazon EKS has finished bringing up the cluster a stack created.
///
/// A created cluster is reported [`EKS_CLUSTER_CREATING`] and "you must wait for the
/// cluster status to become `ACTIVE` before you can … add nodes" (`CreateCluster`), so
/// `ACTIVE` is the state the resource's creation is finished in, `CREATING` the state it is
/// still coming up in, and every other status is one no wait will bring a cluster out of: a
/// cluster that failed, one being deleted or updated out of this stack's reach. A cluster
/// that service no longer holds is in the same position, nothing being left to create it.
pub(crate) async fn eks_cluster_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(cluster) = read_eks_cluster(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon EKS holds no cluster {physical_resource_id}"
        )));
    };
    match cluster.get("status").and_then(Value::as_str).unwrap_or_default() {
        EKS_CLUSTER_ACTIVE => Ok(Readiness::Ready),
        EKS_CLUSTER_CREATING => Ok(Readiness::Pending(EKS_CLUSTER_CREATING.to_string())),
        other => Ok(Readiness::Failed(format!(
            "Amazon EKS reports cluster {physical_resource_id} as {other}"
        ))),
    }
}

/// The cluster Amazon EKS holds under `name`, or `None` when it holds none
/// (`ResourceNotFoundException`, judged on the CODE and never on a message).
pub(crate) async fn read_eks_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!("{EKS_CLUSTERS_URI}/{}", percent_encoded(name));
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(answer.get("cluster").cloned()),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The cluster's configuration as Amazon EKS reports it, for drift detection:
/// `DescribeCluster`'s own record, read by the name the stack published. One that service no
/// longer holds was deleted.
pub(crate) async fn describe_eks_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    Ok(read_eks_cluster(ctx, region, physical_resource_id)
        .await?
        .map(|cluster| reported_as_properties(&cluster, EKS_CLUSTER_REPORTED_PROPERTIES)))
}

/// Delete the cluster a stack created, addressed by the name Amazon EKS knows it by. A
/// cluster that service no longer holds is already in the state the delete asks for.
pub(crate) async fn delete_eks_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!("{EKS_CLUSTERS_URI}/{}", percent_encoded(physical_resource_id));
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Create the association an `AWS::EKS::PodIdentityAssociation` declares, in Amazon EKS —
/// so a client that deployed the stack reads the association back through
/// `eks describe-pod-identity-association`, and a pod of the declared service account gets
/// the declared role's credentials, rather than nothing having been associated at all.
///
/// The cluster is the create's URI label and the rest of the declaration its members
/// ([`EKS_POD_IDENTITY_MEMBERS`]), with the declared tags beside the ones CloudFormation
/// gives the resource as the tag MAP the call takes. The ARN and the id Amazon EKS minted
/// are the type's two published attributes and its `Ref`, so both are read off the
/// association that service created.
pub(crate) async fn create_pod_identity_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, EKS_POD_IDENTITY_MAPPED_PROPERTIES)?;
    let cluster_name = required_property(resource, "ClusterName")?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    copy_members(&properties, EKS_POD_IDENTITY_MEMBERS, &mut request);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        EKS_SIGNING_NAME,
        region,
        "POST",
        &format!(
            "{EKS_CLUSTERS_URI}/{}/{EKS_POD_IDENTITY_ASSOCIATIONS_PATH}",
            percent_encoded(&cluster_name)
        ),
        Some(&Value::Object(request)),
    )
    .await?;
    let association = answered.get("association").cloned().unwrap_or(Value::Null);
    let arn = association
        .get("associationArn")
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "Amazon EKS answered CreatePodIdentityAssociation without an \
                      associationArn"
                .to_string(),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert("AssociationArn".to_string(), arn.clone());
    if let Some(association_id) = association
        .get("associationId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        attributes.insert("AssociationId".to_string(), association_id.to_string());
    }
    Ok(Created {
        // "Ref returns the association ARN", which is this type's primary identifier.
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the pod identity association a stack created, addressed by the cluster that holds
/// it and the id Amazon EKS assigned it — the two `@httpLabel`s of that delete.
///
/// The id is the one the create reported, kept on the resource's own `AssociationId`
/// attribute; a record that carries none names no association that could be deleted, so it
/// is refused here rather than with a request that service would reject. An association
/// Amazon EKS no longer holds — and a cluster it no longer holds, which took the
/// association with it — is already in the state the delete asks for.
pub(crate) async fn delete_pod_identity_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let cluster_name = required_property(resource, "ClusterName")?;
    let association_id = resource
        .attributes
        .get("AssociationId")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "{} carries no AssociationId to delete",
                resource.resource_type
            ),
        })?;
    let uri = format!(
        "{EKS_CLUSTERS_URI}/{}/{EKS_POD_IDENTITY_ASSOCIATIONS_PATH}/{}",
        percent_encoded(&cluster_name),
        percent_encoded(association_id)
    );
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The members `DescribePodIdentityAssociation` reports under this type's own property
/// names, as `(property, member)` pairs, for drift detection — the scalars of the
/// declaration, `Tags` left out for the reason it is everywhere else here.
pub(crate) const EKS_POD_IDENTITY_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("ClusterName", "clusterName"),
    ("DisableSessionTags", "disableSessionTags"),
    ("Namespace", "namespace"),
    ("RoleArn", "roleArn"),
    ("ServiceAccount", "serviceAccount"),
    ("TargetRoleArn", "targetRoleArn"),
];

/// The association's configuration as Amazon EKS reports it, for drift detection:
/// `DescribePodIdentityAssociation`'s own record, read by the pair that addresses one. One
/// that service no longer holds — or whose cluster it no longer holds — was deleted.
///
/// A record carrying no `AssociationId` names no association to read, which is the same
/// answer as one that service does not hold: there is nothing to compare a declaration
/// against.
pub(crate) async fn describe_pod_identity_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let cluster_name = required_property(resource, "ClusterName")?;
    let Some(association_id) = resource
        .attributes
        .get("AssociationId")
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let uri = format!(
        "{EKS_CLUSTERS_URI}/{}/{EKS_POD_IDENTITY_ASSOCIATIONS_PATH}/{}",
        percent_encoded(&cluster_name),
        percent_encoded(association_id)
    );
    match call_rest_json(ctx, EKS_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            answer.get("association").unwrap_or(&Value::Null),
            EKS_POD_IDENTITY_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == EKS_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon API Gateway (REST): AWS::ApiGateway::DomainName
// ---------------------------------------------------------------------------

/// The URI the REST control plane binds its custom-domain-name collection to, as that
/// model's `@http` traits declare it: `POST /domainnames` creates one and
/// `GET|DELETE /domainnames/{domainName}` reads and deletes it. The calls ride on
/// [`API_GATEWAY_SIGNING_NAME`], the signing name both API Gateway models share.
pub(crate) const APIGATEWAY_DOMAIN_NAMES_URI: &str = "/domainnames";

/// The property of this type whose value is a POLICY DOCUMENT: "a stringified JSON policy
/// document" on the wire, while a template declares it as the JSON object it is.
pub(crate) const APIGATEWAY_DOMAIN_NAME_POLICY: &str = "Policy";

/// The attributes an `AWS::ApiGateway::DomainName` publishes that only API Gateway can
/// answer, as `(attribute, answered member)`: the CloudFront distribution an
/// edge-optimized domain is served by and that distribution's region-agnostic Route 53
/// zone, the host a regional one is served on, and the ARN API Gateway names the custom
/// domain by.
///
/// Each is read off the create's own answer, so what the stack publishes for the resource
/// is the endpoint that service really assigned — never a host or an ARN computed here,
/// which is a value a caller could put in a real DNS alias record. `RegionalHostedZoneId`
/// is not among them for that same reason: it names a Route 53 zone this emulator neither
/// owns nor can look up.
pub(crate) const APIGATEWAY_DOMAIN_NAME_ATTRIBUTES: &[(&str, &str)] = &[
    ("DistributionDomainName", "distributionDomainName"),
    ("DistributionHostedZoneId", "distributionHostedZoneId"),
    ("DomainNameArn", "domainNameArn"),
    ("RegionalDomainName", "regionalDomainName"),
];

/// The properties of this type API Gateway's own read reports, as `(property, member)`
/// pairs, which is what drift is judged on.
///
/// The SCALAR configuration of the domain name: `Tags` are left out for the reason they
/// are everywhere here (the tags a resource carries change independently of its
/// declaration), the two structured properties (`EndpointConfiguration`,
/// `MutualTlsAuthentication`) are left out because that read spells their members in the
/// model's own case while a declaration spells them in CloudFormation's, and `Policy` is
/// left out because the read reports the TEXT of a document a template declares as an
/// object — a property the read does not report is not evidence of anything
/// ([`super::drift`]), while one reported in a different SHAPE would be evidence of a
/// change nobody made.
pub(crate) const APIGATEWAY_DOMAIN_NAME_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("CertificateArn", "certificateArn"),
    ("CertificateName", "certificateName"),
    ("DomainName", "domainName"),
    (
        "OwnershipVerificationCertificateArn",
        "ownershipVerificationCertificateArn",
    ),
    ("RegionalCertificateArn", "regionalCertificateArn"),
    ("RegionalCertificateName", "regionalCertificateName"),
    ("RoutingMode", "routingMode"),
    ("SecurityPolicy", "securityPolicy"),
];

/// Create the custom domain name an `AWS::ApiGateway::DomainName` declares, so a client
/// that deployed the stack and then calls `apigateway GetDomainName` for the declared name
/// reads it back — with the configuration the template stated and the `domainNameStatus`
/// that service reports it under — rather than a `NotFoundException` for a domain nothing
/// created, and can map a base path of it onto a REST API with a direct
/// `CreateBasePathMapping`.
///
/// The type's properties ARE `CreateDomainName`'s own members under the same names with a
/// lower-cased initial (`CertificateArn`, `CertificateName`, `EndpointConfiguration` and
/// its own members, `MutualTlsAuthentication`, `OwnershipVerificationCertificateArn`,
/// `RegionalCertificateArn`, `RegionalCertificateName`, `RoutingMode`, `SecurityPolicy`),
/// so the declaration is the request once re-cased ([`api_gateway_request`]), with the tags
/// CloudFormation gives the resource carried as that create's tag map. "Ref returns the
/// domain name", which is the name the template stated and the physical id the naming rules
/// already computed ([`super::template::declared_name_property`]), so nothing replaces it.
pub(crate) async fn create_apigateway_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    // The policy travels as the document's TEXT rather than re-cased member by member like
    // every other property: the keys inside a policy document are the policy language's own
    // (`Version`, `Statement`), not member names.
    let mut request =
        api_gateway_request(resource, propagated, &[], &[APIGATEWAY_DOMAIN_NAME_POLICY])?;
    if let Some(members) = request.as_object_mut() {
        // The name the resource is created under is CloudFormation's to decide: it is this
        // resource's physical id, which is the declared `DomainName` itself.
        members.insert(
            "domainName".to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        APIGATEWAY_DOMAIN_NAMES_URI,
        Some(&request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    for (attribute, member) in APIGATEWAY_DOMAIN_NAME_ATTRIBUTES {
        if let Some(value) = answered.get(*member).and_then(Value::as_str) {
            attributes.insert((*attribute).to_string(), value.to_string());
        }
    }
    Ok(Created::attributes(attributes))
}

/// Delete the custom domain name a stack created, addressed by the name it was created
/// under — this resource's physical id, which is what `Ref` returns for the type. One API
/// Gateway no longer holds (`NotFoundException`, judged on the CODE) is already in the
/// state the delete asks for.
pub(crate) async fn delete_apigateway_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    api_gateway_deleted(ctx, region, &apigateway_domain_name_uri(physical_resource_id)).await
}

/// The domain name's configuration as API Gateway reports it, for drift detection:
/// `GetDomainName`'s own record, under this type's property names. One that service no
/// longer holds (`NotFoundException`) was deleted.
pub(crate) async fn describe_apigateway_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = apigateway_domain_name_uri(physical_resource_id);
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            APIGATEWAY_DOMAIN_NAME_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `DomainNameStatus` a custom domain name is usable in: "AVAILABLE" — the status of
/// the domain-name migration having finished — out of the `DomainNameStatus` enum that
/// model declares.
pub(crate) const DOMAIN_NAME_AVAILABLE: &str = "AVAILABLE";

/// The `DomainNameStatus` values a custom domain name is still COMING UP in: an
/// edge-optimized domain is `PENDING` until the CloudFront distribution serving it is in
/// place, `UPDATING` is the status of a migration that is not complete ("If the status is
/// UPDATING, you cannot update the domain name"), and the two `PENDING_…` values are
/// waiting on a certificate re-import and on the ownership verification of the domain. Each
/// is a state the domain leaves on its own, so the resource's creation is held open in it;
/// `FAILED`, and a domain API Gateway no longer holds, are not.
pub(crate) const DOMAIN_NAME_COMING_UP: &[&str] = &[
    "UPDATING",
    "PENDING",
    "PENDING_CERTIFICATE_REIMPORT",
    "PENDING_OWNERSHIP_VERIFICATION",
];

/// Whether API Gateway has finished bringing up the custom domain name a stack created.
///
/// `DomainName$domainNameStatus` is "the status of the DomainName migration" and the member
/// documents that a domain name in `UPDATING` cannot be updated — an edge-optimized domain
/// is served by a CloudFront distribution that is not in place when the create answers — so
/// `AVAILABLE` is the state the resource's creation is finished in, the statuses in
/// [`DOMAIN_NAME_COMING_UP`] are the ones it is still coming up in, and anything else is a
/// state no wait brings the domain out of.
pub(crate) async fn domain_name_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let uri = apigateway_domain_name_uri(physical_resource_id);
    let answered = match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None)
        .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => {
            return Ok(Readiness::Failed(format!(
                "API Gateway holds no custom domain name {physical_resource_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    let status = answered
        .get("domainNameStatus")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status == DOMAIN_NAME_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if DOMAIN_NAME_COMING_UP.contains(&status) {
        return Ok(Readiness::Pending(status.to_string()));
    }
    // `domainNameStatusMessage` is "an optional text message containing detailed
    // information about status of the DomainName migration", which is the detail that
    // service has about the state it reports; the status speaks for itself otherwise.
    let detail = answered
        .get("domainNameStatusMessage")
        .and_then(Value::as_str);
    Ok(Readiness::Failed(match detail {
        Some(detail) => format!(
            "API Gateway reports custom domain name {physical_resource_id} as {status}: \
             {detail}"
        ),
        None => format!(
            "API Gateway reports custom domain name {physical_resource_id} as {status}"
        ),
    }))
}

/// The URI that addresses ONE custom domain name: the `@httpLabel` every read and delete of
/// it binds, which is the name it was created under.
pub(crate) fn apigateway_domain_name_uri(domain_name: &str) -> String {
    format!(
        "{APIGATEWAY_DOMAIN_NAMES_URI}/{}",
        percent_encoded(domain_name)
    )
}

// ---------------------------------------------------------------------------
// Amazon API Gateway (REST): AWS::ApiGateway::BasePathMapping
// ---------------------------------------------------------------------------

/// The segment the base-path mappings of ONE custom domain name live under, as every
/// operation on them binds it: `POST /domainnames/{domainName}/basepathmappings` creates one
/// and `GET|DELETE /domainnames/{domainName}/basepathmappings/{basePath}` reads and deletes
/// it.
pub(crate) const APIGATEWAY_BASE_PATH_MAPPINGS_SEGMENT: &str = "basepathmappings";

/// The property naming the custom domain name the mapping belongs to, which addresses the
/// call rather than travelling in its body (`CreateBasePathMapping$domainName` is that
/// operation's `@httpLabel`).
pub(crate) const APIGATEWAY_MAPPING_DOMAIN_NAME: &str = "DomainName";

/// The property naming WHICH record of that domain name the mapping belongs to, which
/// travels as the call's own `@httpQuery` parameter rather than in its body
/// (`CreateBasePathMapping$domainNameId`).
pub(crate) const APIGATEWAY_MAPPING_DOMAIN_NAME_ID: &str = "DomainNameId";

/// The property carrying the base path a caller reaches the mapped API under.
pub(crate) const APIGATEWAY_MAPPING_BASE_PATH: &str = "BasePath";

/// The base path of the mapping reached by naming the domain name and nothing after it:
/// "leave this blank if you do not want callers to specify a base path name after the domain
/// name" is the declaration of that mapping, and API Gateway addresses it by this spelling
/// (`GetBasePathMapping$basePath`: "specify '(none)' if you do not want callers to specify
/// any base path name after the domain name"). So a declaration stating no base path is the
/// one the mapping is read and deleted under.
pub(crate) const APIGATEWAY_BASE_PATH_NONE: &str = "(none)";

/// The properties of this type API Gateway's own read reports, as `(property, member)`
/// pairs, which is what drift is judged on: the whole `BasePathMapping` record — the base
/// path it answers on, the API it maps to and that API's stage.
pub(crate) const APIGATEWAY_BASE_PATH_MAPPING_REPORTED: &[(&str, &str)] = &[
    ("BasePath", "basePath"),
    ("RestApiId", "restApiId"),
    ("Stage", "stage"),
];

/// Create the base-path mapping an `AWS::ApiGateway::BasePathMapping` declares, so a client
/// that deployed the stack and then calls `apigateway GetBasePathMapping` for the declared
/// domain name and base path reads the mapping back — with the API and stage the template
/// named — rather than a `NotFoundException` for a mapping nothing created.
///
/// The type's properties ARE `CreateBasePathMapping`'s own members under the same names with
/// a lower-cased initial (`BasePath`, `RestApiId`, `Stage`), so the declaration is the
/// request once re-cased ([`api_gateway_request`]), less the two that address the call. The
/// type is not taggable, so CloudFormation propagates no tag to a mapping and that create
/// takes none.
///
/// The mapping is addressed by the PAIR API Gateway keys one by — the domain name and the
/// base path — and both come off this resource's own declaration
/// ([`apigateway_base_path_mapping_uri`]), so nothing it reports is an identifier this
/// emulator invented.
pub(crate) async fn create_apigateway_base_path_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = api_gateway_request(resource, &[], &[], &[])?;
    if let Some(members) = request.as_object_mut() {
        for property in [
            APIGATEWAY_MAPPING_DOMAIN_NAME,
            APIGATEWAY_MAPPING_DOMAIN_NAME_ID,
        ] {
            members.remove(&lower_initial(property));
        }
    }
    let _answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &apigateway_base_path_mappings_uri(resource)?,
        Some(&request),
    )
    .await?;
    // The type publishes no attribute of its own: everything a client reads a mapping back
    // by is what the declaration already states.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the base-path mapping a stack created, addressed by the same pair its own read is.
/// One API Gateway no longer holds (`NotFoundException`, judged on the CODE), including one a
/// deleted domain name took with it, is already in the state the delete asks for.
pub(crate) async fn delete_apigateway_base_path_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = apigateway_base_path_mapping_uri(resource)?;
    api_gateway_deleted(ctx, region, &uri).await
}

/// The mapping's configuration as API Gateway reports it, for drift detection:
/// `GetBasePathMapping`'s own record, under this type's property names. One that service no
/// longer holds (`NotFoundException`) was deleted.
pub(crate) async fn describe_apigateway_base_path_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = apigateway_base_path_mapping_uri(resource)?;
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            APIGATEWAY_BASE_PATH_MAPPING_REPORTED,
        ))),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI of the base-path-mapping collection of ONE custom domain name, with the record of
/// that domain name the declaration names carried as the call's own query parameter where it
/// names one.
pub(crate) fn apigateway_base_path_mappings_uri(
    resource: &StoredResource,
) -> Result<String, Refusal> {
    let domain_name = required_property(resource, APIGATEWAY_MAPPING_DOMAIN_NAME)?;
    let mut uri = format!(
        "{}/{APIGATEWAY_BASE_PATH_MAPPINGS_SEGMENT}",
        apigateway_domain_name_uri(&domain_name)
    );
    if let Some(domain_name_id) = property(resource, APIGATEWAY_MAPPING_DOMAIN_NAME_ID) {
        uri.push_str(&format!(
            "?domainNameId={}",
            percent_encoded(&domain_name_id)
        ));
    }
    Ok(uri)
}

/// The URI that addresses ONE base-path mapping: the two `@httpLabel`s every operation on
/// one binds, the domain name and the base path, both from the resource's own declaration.
///
/// A declaration that states no base path, and one that states it BLANK ("leave this blank
/// if you do not want callers to specify a base path name after the domain name"), both
/// declare the mapping API Gateway holds under [`APIGATEWAY_BASE_PATH_NONE`] — that create
/// reads an absent and an empty `basePath` alike — and [`property`] reads a blank
/// declaration as one stating nothing, so both are addressed under that spelling.
pub(crate) fn apigateway_base_path_mapping_uri(
    resource: &StoredResource,
) -> Result<String, Refusal> {
    let domain_name = required_property(resource, APIGATEWAY_MAPPING_DOMAIN_NAME)?;
    let base_path = property(resource, APIGATEWAY_MAPPING_BASE_PATH)
        .unwrap_or_else(|| APIGATEWAY_BASE_PATH_NONE.to_string());
    let mut uri = format!(
        "{}/{APIGATEWAY_BASE_PATH_MAPPINGS_SEGMENT}/{}",
        apigateway_domain_name_uri(&domain_name),
        percent_encoded(&base_path)
    );
    if let Some(domain_name_id) = property(resource, APIGATEWAY_MAPPING_DOMAIN_NAME_ID) {
        uri.push_str(&format!(
            "?domainNameId={}",
            percent_encoded(&domain_name_id)
        ));
    }
    Ok(uri)
}

// ---------------------------------------------------------------------------
// Amazon API Gateway v2: AWS::ApiGatewayV2::Api / ::DomainName / ::ApiMapping
// ---------------------------------------------------------------------------

/// API Gateway's signing name, from the models of BOTH its control planes (`sigv4Name:
/// apigateway`, service shapes `BackplaneControlService` and `ApiGatewayV2`), each of which
/// speaks `restJson1` — so their calls ride on [`call_rest_json`], each on the URI its own
/// operation's `@http` binding declares. Every operation of the v2 model is bound under
/// [`APIGATEWAYV2_URI_PREFIX`], which is what separates the two surfaces that share this
/// one signing name.
pub(crate) const API_GATEWAY_SIGNING_NAME: &str = "apigateway";

/// The URIs the API Gateway v2 operations a stack makes are bound to, as that model's
/// `@http` traits declare them: `POST|PUT /v2/apis`, `DELETE|GET /v2/apis/{ApiId}`,
/// `POST /v2/apis/{ApiId}/{integrations|routes|stages|deployments|authorizers}`,
/// `POST /v2/domainnames`, `DELETE|GET /v2/domainnames/{DomainName}` and
/// `POST /v2/domainnames/{DomainName}/apimappings`. Every operation of this model is bound
/// below [`APIGATEWAYV2_URI_PREFIX`], which is what separates it from the Amazon API
/// Gateway control plane it shares its signing name with.
pub(crate) const APIGATEWAYV2_URI_PREFIX: &str = "/v2";
pub(crate) const APIGATEWAYV2_APIS_URI: &str = "/v2/apis";
pub(crate) const APIGATEWAYV2_DOMAIN_NAMES_URI: &str = "/v2/domainnames";
pub(crate) const APIGATEWAYV2_API_MAPPINGS_SEGMENT: &str = "apimappings";

/// The error code either API Gateway control plane answers for a resource it does not hold
/// (`NotFoundException`, the modelled 404 of every read and delete of both models).
pub(crate) const API_GATEWAY_NOT_FOUND: &str = "NotFoundException";

/// The `AWS::ApiGatewayV2::Api` properties that are NOT members of `CreateApi`.
///
/// They are `ImportApi`'s: that operation takes an OpenAPI definition as its payload
/// (`Body`/`BodyS3Location`, with `BasePath` and `FailOnWarnings` governing how it is
/// read), which is a different call on a different URI from the one [`create_apigatewayv2_api`]
/// makes — so a declaration carrying a definition is made with THAT call
/// ([`import_apigatewayv2_api`]) instead.
pub(crate) const APIGATEWAYV2_IMPORT_PROPERTIES: &[&str] = &[
    APIGATEWAYV2_BODY,
    APIGATEWAYV2_BODY_S3_LOCATION,
    APIGATEWAYV2_BASE_PATH,
    APIGATEWAYV2_FAIL_ON_WARNINGS,
];

/// The property that carries the OpenAPI definition an api is IMPORTED from, inline: "the
/// OpenAPI definition. Supported only for HTTP APIs" (`AWS::ApiGatewayV2::Api.Body`), and
/// "to import an HTTP API, you must specify a Body or BodyS3Location".
pub(crate) const APIGATEWAYV2_BODY: &str = "Body";

/// The property that carries that definition as an Amazon S3 OBJECT instead. The object has
/// to be FETCHED from S3 before any api can be imported from it, which nothing here does, so
/// a declaration stating it is still refused rather than created without its definition.
pub(crate) const APIGATEWAYV2_BODY_S3_LOCATION: &str = "BodyS3Location";

/// The two properties that govern how a definition is READ, which are `ImportApi`'s own
/// query parameters (`basepath`, `failOnWarnings`).
pub(crate) const APIGATEWAYV2_BASE_PATH: &str = "BasePath";
pub(crate) const APIGATEWAYV2_FAIL_ON_WARNINGS: &str = "FailOnWarnings";

/// The `ImportApi` query parameters [`APIGATEWAYV2_BASE_PATH`] and
/// [`APIGATEWAYV2_FAIL_ON_WARNINGS`] are sent as, as that operation's `@httpQuery` traits
/// name them.
pub(crate) const APIGATEWAYV2_IMPORT_QUERY: &[(&str, &str)] = &[
    (APIGATEWAYV2_BASE_PATH, "basepath"),
    (APIGATEWAYV2_FAIL_ON_WARNINGS, "failOnWarnings"),
];

/// The `ImportApi` member the definition itself travels as.
pub(crate) const APIGATEWAYV2_BODY_MEMBER: &str = "body";

/// Create the api an `AWS::ApiGatewayV2::Api` declares, so a client that deployed the
/// stack and then calls `apigatewayv2 GetApi` with the id the stack publishes reads the
/// api back rather than a `NotFoundException` for an api nothing created.
///
/// Every other property of the type IS a `CreateApi` member under the same name with its
/// initial letter lower-cased — `Name`, `ProtocolType`, `Description`, `Version`,
/// `RouteSelectionExpression`, `ApiKeySelectionExpression`, `CorsConfiguration` (whose own
/// members re-case the same way), `CredentialsArn`, `DisableExecuteApiEndpoint`,
/// `DisableSchemaValidation`, `IpAddressType`, `RouteKey` and `Target` — so the evaluated
/// property document is the request once its member names are re-cased
/// ([`lower_camel_members`]) and the declared tags are carried beside the ones
/// CloudFormation gives the resource ([`tags_of`]) as the tag MAP that create takes.
///
/// "Ref returns the API ID", which only that service produces, so the `ApiId` it answers
/// replaces the stand-in the naming rules computed, and the `ApiEndpoint` it reports is
/// the attribute the type publishes — the endpoint a client really invokes the api on.
///
/// A declaration that carries a DEFINITION is a different call: see
/// [`import_apigatewayv2_api`].
pub(crate) async fn create_apigatewayv2_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    if let Some(definition) = resource.properties.get(APIGATEWAYV2_BODY) {
        return import_apigatewayv2_api(ctx, region, resource, definition).await;
    }
    let request = api_gateway_request(resource, propagated, APIGATEWAYV2_IMPORT_PROPERTIES, &[])?;
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        APIGATEWAYV2_APIS_URI,
        Some(&request),
    )
    .await?;
    apigatewayv2_api_created(&answered, "CreateApi")
}

/// Import the api an `AWS::ApiGatewayV2::Api` declares a DEFINITION for, so a client that
/// deployed such a stack reads the api back — with the routes and the integration behind
/// each of them that its definition declares — rather than a `NotFoundException` for an api
/// nothing created.
///
/// "To import an HTTP API, you must specify a Body or BodyS3Location", and the call that
/// reads a definition is `ImportApi` (`PUT /v2/apis`), not `CreateApi`: the definition IS
/// the api — its name, its protocol, its routes and their integrations all come out of the
/// document — so the declared `Body` travels as that operation's own `Body` member, with
/// `BasePath` and `FailOnWarnings` on its query ([`APIGATEWAYV2_IMPORT_QUERY`]). An inline
/// definition is declared as the JSON object it is and sent as the text that operation
/// reads.
///
/// Every remaining property of the type is one only `CreateApi` takes, and this type's own
/// rule is that a declaration carrying a definition states none of them ("If you specify a
/// Body or BodyS3Location, don't specify CorsConfiguration, CredentialsArn,
/// DisableExecuteApiEndpoint, Name, ProtocolType, RouteKey, RouteSelectionExpression, Tags,
/// Target, or Version"), so one that does is REFUSED rather than imported with that property
/// silently dropped — including `BodyS3Location`, whose object nothing here fetches.
///
/// The api's identity is that service's exactly as it is for a create: `ImportApi` answers
/// the `ApiId` it assigned and the `ApiEndpoint` it serves the api on.
pub(crate) async fn import_apigatewayv2_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    definition: &Value,
) -> Result<Created, Refusal> {
    let mut query: Vec<String> = Vec::new();
    for (property, parameter) in APIGATEWAYV2_IMPORT_QUERY {
        if let Some(value) = resource.properties.get(*property) {
            query.push(format!(
                "{parameter}={}",
                percent_encoded(&attribute_text(value))
            ));
        }
    }
    if let Some(properties) = resource.properties.as_object() {
        for property in properties.keys() {
            if property == APIGATEWAYV2_BODY
                || APIGATEWAYV2_IMPORT_QUERY
                    .iter()
                    .any(|(declared, _)| declared == property)
            {
                continue;
            }
            return Err(Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "{} declares a {APIGATEWAYV2_BODY}, so the api is imported from that \
                     definition, and {property} is no member of that import; the resource was \
                     not created.",
                    resource.resource_type
                ),
            });
        }
    }
    let mut payload = Map::new();
    payload.insert(
        APIGATEWAYV2_BODY_MEMBER.to_string(),
        Value::String(attribute_text(definition)),
    );
    let payload = Value::Object(payload);
    let uri = if query.is_empty() {
        APIGATEWAYV2_APIS_URI.to_string()
    } else {
        format!("{APIGATEWAYV2_APIS_URI}?{}", query.join("&"))
    };
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "PUT",
        &uri,
        Some(&payload),
    )
    .await?;
    apigatewayv2_api_created(&answered, "ImportApi")
}

/// The api an answered create or import reports: the `ApiId` that service assigned, which
/// is this resource's identity, and the `ApiEndpoint` it serves the api on, which is the
/// attribute the type publishes. One spelling for both calls, which answer the same api.
pub(crate) fn apigatewayv2_api_created(answered: &Value, operation: &str) -> Result<Created, Refusal> {
    let api_id = api_gateway_answered(answered, "apiId", operation)?;
    let mut attributes = BTreeMap::new();
    if let Some(endpoint) = answered.get("apiEndpoint").and_then(Value::as_str) {
        attributes.insert("ApiEndpoint".to_string(), endpoint.to_string());
    }
    Ok(Created {
        physical_resource_id: Some(api_id),
        attributes,
    })
}

/// Delete the api a stack created, addressed by the `ApiId` API Gateway v2 assigned it —
/// this resource's physical id, which is what `Ref` returns for the type. An api that
/// service no longer holds (`NotFoundException`, judged on the CODE and never on a message
/// that carries the request's own identifier) is already in the state the delete asks for.
pub(crate) async fn delete_apigatewayv2_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{APIGATEWAYV2_APIS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    api_gateway_deleted(ctx, region, &uri).await
}

/// The api's configuration as API Gateway v2 reports it, for drift detection: `GetApi`'s
/// own record. One it no longer holds (`NotFoundException`) was deleted.
///
/// Every member that read reports under a name this type publishes as a property is
/// compared; `Tags` is left out for the reason it is everywhere here (the tags a resource
/// carries change independently of its declaration), and a property the read does not
/// report is not evidence of anything ([`super::drift`]).
pub(crate) async fn describe_apigatewayv2_api(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{APIGATEWAYV2_APIS_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            APIGATEWAYV2_API_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The members `GetApi` reports under this type's own property names, as
/// `(property, member)` pairs.
pub(crate) const APIGATEWAYV2_API_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("ApiKeySelectionExpression", "apiKeySelectionExpression"),
    ("CorsConfiguration", "corsConfiguration"),
    ("Description", "description"),
    ("DisableExecuteApiEndpoint", "disableExecuteApiEndpoint"),
    ("DisableSchemaValidation", "disableSchemaValidation"),
    ("IpAddressType", "ipAddressType"),
    ("Name", "name"),
    ("ProtocolType", "protocolType"),
    ("RouteSelectionExpression", "routeSelectionExpression"),
    ("Version", "version"),
];

/// Create the custom domain name an `AWS::ApiGatewayV2::DomainName` declares, so a client
/// that deployed the stack and then calls `apigatewayv2 GetDomainName` for the declared
/// name reads it back — with the configuration the template stated — rather than a
/// `NotFoundException` for a domain nothing created, and can map an api onto it with a
/// direct `CreateApiMapping`.
///
/// The type's properties ARE `CreateDomainName`'s own members under the same names with a
/// lower-cased initial (`DomainName`, `DomainNameConfigurations` and the members of each
/// entry, `MutualTlsAuthentication`, `RoutingMode`), so the declaration is the request
/// once re-cased, with the tags CloudFormation gives the resource carried as that create's
/// tag map. "Ref returns the domain name", which is the name the template stated and the
/// physical id the naming rules already computed, so nothing replaces it.
pub(crate) async fn create_apigatewayv2_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = api_gateway_request(resource, propagated, &[], &[])?;
    // The name the resource is created under is CloudFormation's to decide: it is this
    // resource's physical id, which is the declared `DomainName` itself.
    if let Some(members) = request.as_object_mut() {
        members.insert(
            "domainName".to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        APIGATEWAYV2_DOMAIN_NAMES_URI,
        Some(&request),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    // "Fn::GetAtt DomainNameArn: the ARN of the domain name" — the handle API Gateway
    // itself names the custom domain by, so the stack publishes the ARN that service
    // minted rather than one CloudFormation computed.
    if let Some(arn) = answered.get("domainNameArn").and_then(Value::as_str) {
        attributes.insert("DomainNameArn".to_string(), arn.to_string());
    }
    // "Fn::GetAtt RegionalDomainName: the domain name associated with the regional
    // endpoint for this custom domain name" — the host API Gateway serves it on, which
    // that service reports as the endpoint's `ApiGatewayDomainName`. The sibling
    // `RegionalHostedZoneId` is not among the attributes this type publishes here
    // ([`super::template::published_attribute_names`]): it names a Route 53 zone this
    // emulator neither owns nor can look up, and an invented zone id is one a caller could
    // put in a real DNS alias record.
    if let Some(endpoint) = apigatewayv2_regional_endpoint(&answered) {
        if let Some(host) = endpoint.get("apiGatewayDomainName").and_then(Value::as_str) {
            attributes.insert("RegionalDomainName".to_string(), host.to_string());
        }
    }
    Ok(Created::attributes(attributes))
}

/// The REGIONAL endpoint of an answered domain name, out of the
/// `DomainNameConfigurations` list that answer carries: the entry whose `EndpointType` is
/// `REGIONAL` ("the endpoint type", `EndpointType` being `REGIONAL | EDGE`), and otherwise
/// the first entry the domain carries — a domain configured with one endpoint is served on
/// that one.
pub(crate) fn apigatewayv2_regional_endpoint(answered: &Value) -> Option<&Value> {
    let configurations = answered.get("domainNameConfigurations")?.as_array()?;
    configurations
        .iter()
        .find(|endpoint| {
            endpoint
                .get("endpointType")
                .and_then(Value::as_str)
                .is_some_and(|endpoint_type| endpoint_type == APIGATEWAYV2_REGIONAL_ENDPOINT)
        })
        .or_else(|| configurations.first())
}

/// The `EndpointType` whose endpoint the `RegionalDomainName` attribute names, out of the
/// `REGIONAL | EDGE` enum that model declares.
pub(crate) const APIGATEWAYV2_REGIONAL_ENDPOINT: &str = "REGIONAL";

/// Delete the custom domain name a stack created, addressed by the name it was created
/// under. One API Gateway v2 no longer holds (`NotFoundException`, judged on the CODE) is
/// already in the state the delete asks for.
pub(crate) async fn delete_apigatewayv2_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = format!(
        "{APIGATEWAYV2_DOMAIN_NAMES_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    api_gateway_deleted(ctx, region, &uri).await
}

/// The domain name's configuration as API Gateway v2 reports it, for drift detection:
/// `GetDomainName`'s own record. One it no longer holds (`NotFoundException`) was deleted.
pub(crate) async fn describe_apigatewayv2_domain_name(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = format!(
        "{APIGATEWAYV2_DOMAIN_NAMES_URI}/{}",
        percent_encoded(physical_resource_id)
    );
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[
                ("DomainName", "domainName"),
                ("MutualTlsAuthentication", "mutualTlsAuthentication"),
                ("RoutingMode", "routingMode"),
            ],
        ))),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the api mapping an `AWS::ApiGatewayV2::ApiMapping` declares, which is what makes
/// the api it names reachable on the custom domain it names.
///
/// `CreateApiMapping` binds the domain to its URI
/// (`POST /v2/domainnames/{DomainName}/apimappings`) and takes the rest of the type's
/// properties as members under their lower-cased initial (`ApiId`, `Stage`,
/// `ApiMappingKey`), so the declaration is the request with the domain lifted out. The
/// domain and the api it names are the resources the stack created before this one, so the
/// values carried here are the identifiers those services really minted.
///
/// "Ref returns the API mapping ID", a value only that service produces, so the
/// `ApiMappingId` it answers replaces the stand-in the naming rules computed and is
/// reported as the type's one published attribute — the id a client reads the mapping back
/// with through `GetApiMapping`.
pub(crate) async fn create_apigatewayv2_api_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let domain_name = required_property(resource, "DomainName")?;
    let mut request = api_gateway_request(resource, &[], &[], &[])?;
    if let Some(members) = request.as_object_mut() {
        // Bound to the create's URI rather than sent as a member of its body.
        members.remove("domainName");
    }
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &apigatewayv2_api_mappings_uri(&domain_name),
        Some(&request),
    )
    .await?;
    let api_mapping_id = api_gateway_answered(&answered, "apiMappingId", "CreateApiMapping")?;
    let mut attributes = BTreeMap::new();
    attributes.insert("ApiMappingId".to_string(), api_mapping_id.clone());
    Ok(Created {
        physical_resource_id: Some(api_mapping_id),
        attributes,
    })
}

/// Delete the api mapping a stack created, addressed by the PAIR API Gateway v2 keys one
/// by: the `ApiMappingId` it minted — this resource's physical id — and the domain name
/// holding it, which only the resource's own declaration carries. A mapping that service no
/// longer holds (`NotFoundException`, judged on the CODE), including one a deleted domain
/// took with it, is already in the state the delete asks for.
pub(crate) async fn delete_apigatewayv2_api_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = apigatewayv2_api_mapping_uri(resource)?;
    api_gateway_deleted(ctx, region, &uri).await
}

/// The api mapping's configuration as API Gateway v2 reports it, for drift detection:
/// `GetApiMapping`'s own record, read by the same pair the delete uses. One that service no
/// longer holds (`NotFoundException`) was deleted.
pub(crate) async fn describe_apigatewayv2_api_mapping(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = apigatewayv2_api_mapping_uri(resource)?;
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &answer,
            &[
                ("ApiId", "apiId"),
                ("ApiMappingKey", "apiMappingKey"),
                ("Stage", "stage"),
            ],
        ))),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI of the api-mapping collection of ONE custom domain name.
pub(crate) fn apigatewayv2_api_mappings_uri(domain_name: &str) -> String {
    format!(
        "{APIGATEWAYV2_DOMAIN_NAMES_URI}/{}/{APIGATEWAYV2_API_MAPPINGS_SEGMENT}",
        percent_encoded(domain_name)
    )
}

/// The URI that addresses ONE api mapping: the two `@httpLabel`s of every operation on it,
/// the domain from the resource's own declaration and the id that service minted.
pub(crate) fn apigatewayv2_api_mapping_uri(resource: &StoredResource) -> Result<String, Refusal> {
    let domain_name = required_property(resource, "DomainName")?;
    Ok(format!(
        "{}/{}",
        apigatewayv2_api_mappings_uri(&domain_name),
        percent_encoded(&resource.physical_resource_id)
    ))
}

// ---------------------------------------------------------------------------
// Amazon API Gateway v2: the resource types that live INSIDE an api
// ---------------------------------------------------------------------------

/// One API Gateway v2 resource type whose resources live inside an api — an integration, a
/// route, a stage, a deployment, an authorizer — as the three calls a stack makes about one
/// of them.
///
/// Every one of these types is in the SAME shape, which is why they are a table rather than
/// an arm apiece: the resource is created in a collection addressed by the api that holds
/// it (`POST /v2/apis/{ApiId}/<collection>`), every property of the type other than the ones
/// that address that collection is a member of the create under the same name with its
/// initial letter lower-cased, the create answers the identifier API Gateway v2 ASSIGNED,
/// and the read and the delete address the resource as that collection plus that identifier.
///
/// Without these, a stack that declared an api and the integration, route and stage that
/// make it serve created only the api: the rest were recorded as declared and nothing in
/// API Gateway v2 existed for them, so the api a completed stack published answered nothing.
pub(crate) struct ApiGatewayV2Child {
    /// The properties that address the collection the resource lives in, each with the URI
    /// segment it is written under, outermost first — `("ApiId", "apis")` for a child of an
    /// api, with the parent's own identifier beside it for a child of one of those children.
    pub(crate) path: &'static [(&'static str, &'static str)],
    /// The last segment of the collection's URI, as that create's `@http` uri spells it.
    pub(crate) collection: &'static str,
    /// The member of the create's answer carrying the identifier API Gateway v2 ASSIGNED,
    /// with the operation's name for the failure a create that answered WITHOUT it is
    /// reported as. `None` for a type created under a name CloudFormation decides, whose
    /// physical id is that name rather than anything that service minted.
    pub(crate) assigned_id: Option<(&'static str, &'static str)>,
    /// The property the resource is created UNDER where CloudFormation decides its name (a
    /// stage's `StageName`): the create is sent this resource's physical id for it, which is
    /// the name the declaration asked for where it asked for one.
    pub(crate) named_by: Option<&'static str>,
    /// The properties that are MAPS whose keys are the caller's own data rather than
    /// modelled member names — a stage's route settings are keyed by route key (`GET
    /// /items`), an integration's request parameters and templates by parameter name and
    /// content type. Those keys travel EXACTLY as declared, while the values under them are
    /// re-cased like every other member ([`lower_camel_members`]).
    pub(crate) data_keys: &'static [&'static str],
    /// Whether the create takes the tags CloudFormation gives the resource: true only where
    /// the type's own property set publishes `Tags`, because CloudFormation propagates no
    /// tags to a resource type that is not taggable.
    pub(crate) taggable: bool,
    /// The properties of the type the owning service's own READ reports, which is what
    /// drift is judged on. Each is read off the member that answer spells it with — the
    /// property with its initial letter lower-cased, like every member here.
    pub(crate) reported: &'static [&'static str],
}

/// `AWS::ApiGatewayV2::Integration`: `POST /v2/apis/{ApiId}/integrations`, whose answer
/// carries the `IntegrationId` that service assigned ("Ref returns the integration ID").
pub(crate) const APIGATEWAYV2_INTEGRATION_CHILD: ApiGatewayV2Child = ApiGatewayV2Child {
    path: APIGATEWAYV2_API_PATH,
    collection: "integrations",
    assigned_id: Some(("integrationId", "CreateIntegration")),
    named_by: None,
    data_keys: &["RequestParameters", "RequestTemplates", "ResponseParameters"],
    taggable: false,
    reported: &[
        "ConnectionId",
        "ConnectionType",
        "ContentHandlingStrategy",
        "CredentialsArn",
        "Description",
        "IntegrationMethod",
        "IntegrationSubtype",
        "IntegrationType",
        "IntegrationUri",
        "PassthroughBehavior",
        "PayloadFormatVersion",
        "RequestParameters",
        "RequestTemplates",
        "ResponseParameters",
        "TemplateSelectionExpression",
        "TimeoutInMillis",
        "TlsConfig",
    ],
};

/// `AWS::ApiGatewayV2::Route`: `POST /v2/apis/{ApiId}/routes`, whose answer carries the
/// `RouteId` that service assigned ("Ref returns the route ID"). A route's `Target` names
/// the integration it is wired to as `integrations/<IntegrationId>`, so the identifier that
/// service minted for the integration has to have reached this declaration before the route
/// is created ([`super::provisioning::repoint_references`]).
pub(crate) const APIGATEWAYV2_ROUTE_CHILD: ApiGatewayV2Child = ApiGatewayV2Child {
    path: APIGATEWAYV2_API_PATH,
    collection: "routes",
    assigned_id: Some(("routeId", "CreateRoute")),
    named_by: None,
    data_keys: &["RequestModels", "RequestParameters"],
    taggable: false,
    reported: &[
        "ApiKeyRequired",
        "AuthorizationScopes",
        "AuthorizationType",
        "AuthorizerId",
        "ModelSelectionExpression",
        "OperationName",
        "RequestModels",
        "RequestParameters",
        "RouteKey",
        "RouteResponseSelectionExpression",
        "Target",
    ],
};

/// `AWS::ApiGatewayV2::Stage`: `POST /v2/apis/{ApiId}/stages`. "Ref returns the stage name",
/// which is the name the declaration states and the physical id the naming rules already
/// computed, so nothing that service answers replaces it — and the stage is created UNDER
/// that name. The type publishes `Tags`, so the stage carries the tags CloudFormation gives
/// it as `CreateStage`'s own tag map.
pub(crate) const APIGATEWAYV2_STAGE_CHILD: ApiGatewayV2Child = ApiGatewayV2Child {
    path: APIGATEWAYV2_API_PATH,
    collection: "stages",
    assigned_id: None,
    named_by: Some("StageName"),
    data_keys: &["RouteSettings", "StageVariables"],
    taggable: true,
    reported: &[
        "AccessLogSettings",
        "AutoDeploy",
        "ClientCertificateId",
        "DefaultRouteSettings",
        "DeploymentId",
        "Description",
        "RouteSettings",
        "StageVariables",
    ],
};

/// `AWS::ApiGatewayV2::Deployment`: `POST /v2/apis/{ApiId}/deployments`, whose answer
/// carries the `DeploymentId` that service assigned ("Ref returns the deployment ID").
pub(crate) const APIGATEWAYV2_DEPLOYMENT_CHILD: ApiGatewayV2Child = ApiGatewayV2Child {
    path: APIGATEWAYV2_API_PATH,
    collection: "deployments",
    assigned_id: Some(("deploymentId", "CreateDeployment")),
    named_by: None,
    data_keys: &[],
    taggable: false,
    // `GetDeployment` reports the deployment's own record — its description and the status
    // of the release — and not the stage a create released it to.
    reported: &["Description"],
};

/// `AWS::ApiGatewayV2::Authorizer`: `POST /v2/apis/{ApiId}/authorizers`, whose answer
/// carries the `AuthorizerId` that service assigned ("Ref returns the authorizer ID") — the
/// id a route's `AuthorizerId` names it by, which the `Name` the declaration states is not.
pub(crate) const APIGATEWAYV2_AUTHORIZER_CHILD: ApiGatewayV2Child = ApiGatewayV2Child {
    path: APIGATEWAYV2_API_PATH,
    collection: "authorizers",
    assigned_id: Some(("authorizerId", "CreateAuthorizer")),
    named_by: None,
    data_keys: &[],
    taggable: false,
    reported: &[
        "AuthorizerCredentialsArn",
        "AuthorizerPayloadFormatVersion",
        "AuthorizerResultTtlInSeconds",
        "AuthorizerType",
        "AuthorizerUri",
        "EnableSimpleResponses",
        "IdentitySource",
        "IdentityValidationExpression",
        "JwtConfiguration",
        "Name",
    ],
};

/// The collection path of a resource that lives directly inside an api: the `ApiId` its
/// declaration states, under the `apis` segment every operation of that api is bound below.
pub(crate) const APIGATEWAYV2_API_PATH: &[(&str, &str)] = &[("ApiId", "apis")];

/// The URI of the collection one of these resources lives in, built from the properties
/// that address it — the api the resource belongs to, which its own declaration states.
pub(crate) fn apigatewayv2_collection_uri(
    resource: &StoredResource,
    child: &ApiGatewayV2Child,
) -> Result<String, Refusal> {
    let mut uri = APIGATEWAYV2_URI_PREFIX.to_string();
    for (property, segment) in child.path {
        let value = required_property(resource, property)?;
        uri.push('/');
        uri.push_str(segment);
        uri.push('/');
        uri.push_str(&percent_encoded(&value));
    }
    uri.push('/');
    uri.push_str(child.collection);
    Ok(uri)
}

/// The URI that addresses ONE of these resources: its collection plus the identifier the
/// resource carries as its physical id — the one that service assigned where it assigns
/// one, and otherwise the name it was created under.
pub(crate) fn apigatewayv2_child_uri(
    resource: &StoredResource,
    child: &ApiGatewayV2Child,
) -> Result<String, Refusal> {
    Ok(format!(
        "{}/{}",
        apigatewayv2_collection_uri(resource, child)?,
        percent_encoded(&resource.physical_resource_id)
    ))
}

/// Create one resource that lives inside an api, so a client that deployed the stack reads
/// it back through that service's own read — and, for the integration, route and stage an
/// api needs to serve at all, so the api the stack published answers requests.
///
/// The properties that address the collection are the URI; every other property is a member
/// of the create ([`apigatewayv2_child_request`]). Where that service ASSIGNS the
/// resource's identifier, the id it answers is this resource's physical id — what `Ref`
/// resolves to and what the stack publishes — exactly as it is for the api itself.
pub(crate) async fn create_apigatewayv2_child(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
    child: &ApiGatewayV2Child,
) -> Result<Created, Refusal> {
    let uri = apigatewayv2_collection_uri(resource, child)?;
    let request = apigatewayv2_child_request(resource, propagated, child)?;
    let answered = call_rest_json(
        ctx,
        API_GATEWAY_SIGNING_NAME,
        region,
        "POST",
        &uri,
        Some(&request),
    )
    .await?;
    let Some((member, operation)) = child.assigned_id else {
        // Created under the name CloudFormation decided, which the resource already
        // carries as its physical id.
        return Ok(Created::attributes(BTreeMap::new()));
    };
    Ok(Created {
        physical_resource_id: Some(api_gateway_answered(&answered, member, operation)?),
        attributes: BTreeMap::new(),
    })
}

/// One declared child resource as the request that creates it: every property that is a
/// member of that create, under the member name the model spells it with.
///
/// The properties that address the collection are left out — they are the URI — and so is
/// the name property where CloudFormation decides the name, which is sent as the physical id
/// the naming rules computed instead. A property named in [`ApiGatewayV2Child::data_keys`]
/// is a map of the caller's own keys and keeps them; everything else is re-cased all the way
/// down.
pub(crate) fn apigatewayv2_child_request(
    resource: &StoredResource,
    propagated: &[(String, String)],
    child: &ApiGatewayV2Child,
) -> Result<Value, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            let addresses_the_collection = child
                .path
                .iter()
                .any(|(parent, _)| parent == &property.as_str());
            if property == "Tags"
                || addresses_the_collection
                || child.named_by == Some(property.as_str())
            {
                continue;
            }
            let member = lower_initial(property);
            if child.data_keys.contains(&property.as_str()) {
                request.insert(member, apigatewayv2_data_keyed_map(value)?);
            } else {
                request.insert(member, lower_camel_members(value, 0)?);
            }
        }
    }
    if let Some(named_by) = child.named_by {
        request.insert(
            lower_initial(named_by),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    if child.taggable {
        let tags = tags_as_map(&tags_of(&resource.properties, propagated));
        if !tags.is_empty() {
            request.insert("tags".to_string(), Value::Object(tags));
        }
    }
    Ok(Value::Object(request))
}

/// One declared MAP whose KEYS are the caller's own data: each key travels exactly as the
/// declaration spells it — a route key (`GET /items`) or a content type is not a member name
/// to re-case — while the value under it is re-cased like any other member.
pub(crate) fn apigatewayv2_data_keyed_map(value: &Value) -> Result<Value, Refusal> {
    let Some(entries) = value.as_object() else {
        return lower_camel_members(value, 0);
    };
    let mut mapped = Map::new();
    for (key, entry) in entries {
        mapped.insert(key.clone(), lower_camel_members(entry, 1)?);
    }
    Ok(Value::Object(mapped))
}

/// Delete one resource a stack created inside an api, addressed exactly as its own read is.
/// One that service no longer holds (`NotFoundException`, judged on the CODE) is already in
/// the state the delete asks for — which is also what deleting the API that held it leaves
/// behind, since an api owns its integrations, routes, stages and deployments.
pub(crate) async fn delete_apigatewayv2_child(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    child: &ApiGatewayV2Child,
) -> Result<(), Refusal> {
    let uri = apigatewayv2_child_uri(resource, child)?;
    api_gateway_deleted(ctx, region, &uri).await
}

/// The configuration one of these resources has as API Gateway v2 reports it, for drift
/// detection: that resource's own read, under the type's property names
/// ([`ApiGatewayV2Child::reported`]). One that service no longer holds
/// (`NotFoundException`) was deleted.
pub(crate) async fn describe_apigatewayv2_child(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    child: &ApiGatewayV2Child,
) -> Result<Option<Value>, Refusal> {
    let uri = apigatewayv2_child_uri(resource, child)?;
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answer) => {
            let mut reported = Map::new();
            for property in child.reported {
                if let Some(value) = answer.get(lower_initial(property)) {
                    reported.insert((*property).to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One declared API Gateway resource as the request that creates it: every property
/// under the member name that model spells it with — the property with a lower-cased
/// initial, all the way down ([`lower_camel_members`]) — with the declared tags beside the
/// ones CloudFormation gives the resource, as the tag MAP these creates take.
///
/// Shared by both API Gateway control planes, whose types are in the same shape: an
/// `AWS::ApiGatewayV2::*` property and an `AWS::ApiGateway::DomainName` one are alike their
/// create's own member with a lower-cased initial.
///
/// `unmapped` names the properties that are NOT members of this create: one of those is
/// REFUSED rather than dropped, so a stack never reports a resource configured differently
/// from its declaration.
///
/// `documents` names the properties whose member takes the TEXT of a document a template
/// declares as an object (a resource policy): such a value travels as that text
/// ([`attribute_text`]) rather than being walked member by member, because the keys inside
/// it are the declared document's own and not modelled member names.
pub(crate) fn api_gateway_request(
    resource: &StoredResource,
    propagated: &[(String, String)],
    unmapped: &[&str],
    documents: &[&str],
) -> Result<Value, Refusal> {
    let mut request = Map::new();
    if let Some(properties) = resource.properties.as_object() {
        for (property, value) in properties {
            if property == "Tags" {
                continue;
            }
            if unmapped.contains(&property.as_str()) {
                return Err(Refusal {
                    code: NOT_IMPLEMENTED.to_string(),
                    message: format!(
                        "this emulator does not create a {} from its {property} property; \
                         the resource was not created.",
                        resource.resource_type
                    ),
                });
            }
            if documents.contains(&property.as_str()) {
                request.insert(lower_initial(property), Value::String(attribute_text(value)));
                continue;
            }
            request.insert(lower_initial(property), lower_camel_members(value, 0)?);
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    Ok(Value::Object(request))
}

/// One identifier an API Gateway create answered, or the failure a create that answered 2xx
/// WITHOUT it is reported as: nothing addresses the resource without that id, so it is never
/// recorded with a physical id nothing minted.
///
/// Shared by both API Gateway control planes, which each answer the id they assigned under a
/// member of their own ([`apigatewayv2_api_created`],
/// [`crate::state::provisioning::apigateway_rest_api`]).
pub(crate) fn api_gateway_answered(
    answered: &Value,
    member: &str,
    operation: &str,
) -> Result<String, Refusal> {
    answered
        .get(member)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("API Gateway answered {operation} without a {member}"),
        })
}

/// One API Gateway delete, on the URI that addresses the resource: a resource that service
/// no longer holds is already in the state the delete asks for, judged on its modelled
/// not-found CODE and never on a message, which carries the request's own identifier.
///
/// Shared by both API Gateway control planes: each models the same `NotFoundException` for
/// a resource it does not hold ([`API_GATEWAY_NOT_FOUND`]).
pub(crate) async fn api_gateway_deleted(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
) -> Result<(), Refusal> {
    match call_rest_json(ctx, API_GATEWAY_SIGNING_NAME, region, "DELETE", uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == API_GATEWAY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// AWS CodeBuild: AWS::CodeBuild::ReportGroup
// ---------------------------------------------------------------------------

/// CodeBuild's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codebuild`, service shape `CodeBuild_20161006`), which speaks
/// `awsJson1_1`.
pub(crate) const CODEBUILD_SIGNING_NAME: &str = "codebuild";
pub(crate) const CODEBUILD_TARGET_PREFIX: &str = "CodeBuild_20161006";

/// The `AWS::CodeBuild::ReportGroup` properties `CreateReportGroup` takes under its own
/// member name, as `(property, member)` pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against that call's own
/// members — the same member under a lower-case initial. `Name` is not here because it is
/// the one value CloudFormation decides (this resource's physical id), `ExportConfig` is
/// not because it is a nested structure whose members are mapped in turn
/// ([`report_export_config`]), `Tags` is not because the tags CloudFormation gives the
/// resource travel beside the declared ones ([`tags_of`]), and `DeleteReports` is not a
/// member of that create at all — it governs the DELETE
/// ([`CODEBUILD_DELETE_REPORTS_PROPERTY`]).
pub(crate) const CODEBUILD_REPORT_GROUP_MEMBERS: &[(&str, &str)] = &[("Type", "type")];

/// The members of the type's `ExportConfig` property, as `ReportExportConfig`'s own
/// members: each the property with its initial letter lower-cased. `S3Destination` is
/// mapped separately, since it is a structure of its own.
pub(crate) const CODEBUILD_EXPORT_CONFIG_MEMBERS: &[(&str, &str)] =
    &[("ExportConfigType", "exportConfigType")];

/// The members of that configuration's `S3Destination`, as `S3ReportExportConfig`'s own
/// members. A property no pair names is no member of that shape, so it is not sent.
pub(crate) const CODEBUILD_S3_DESTINATION_MEMBERS: &[(&str, &str)] = &[
    ("Bucket", "bucket"),
    ("BucketOwner", "bucketOwner"),
    ("Path", "path"),
    ("Packaging", "packaging"),
    ("EncryptionKey", "encryptionKey"),
    ("EncryptionDisabled", "encryptionDisabled"),
];

/// The property that says whether the reports held in the group go with it when the stack
/// removes it, which is `DeleteReportGroup`'s own `deleteReports` member.
pub(crate) const CODEBUILD_DELETE_REPORTS_PROPERTY: &str = "DeleteReports";

/// The `ReportGroup` members CodeBuild reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// `ExportConfig` is not among them: it is a nested structure spelled differently on each
/// side, and a property the read does not report is not evidence of anything
/// ([`super::drift`]) — the same position `AWS::CodeCommit::Repository`'s `Triggers` is
/// in. `Tags` is left out for the reason it is everywhere here: the tags a resource
/// carries change independently of its declaration.
pub(crate) const CODEBUILD_REPORT_GROUP_REPORTED_PROPERTIES: &[(&str, &str)] =
    &[("Name", "name"), ("Type", "type")];

/// Create the report group an `AWS::CodeBuild::ReportGroup` declares — so a client that
/// deployed the stack and then calls `codebuild BatchGetReportGroups` with the ARN the
/// stack publishes reads the group back, and `PutResourcePolicy` against that ARN has a
/// resource to attach a policy to, rather than CodeBuild reporting the ARN its own stack
/// just completed in `reportGroupsNotFound`.
///
/// "Ref returns the report group name", which is this resource's physical id and the one
/// value `CreateReportGroup` takes to key the group by, so the group is created under it
/// and the id the stack already computed stays in force. The ARN that create ANSWERS with
/// is recorded as the type's published `Arn` attribute, so `Fn::GetAtt` reads the ARN
/// CodeBuild really keyed the group by.
///
/// The group carries the tags the declaration asks for plus the stack's own
/// ([`tags_of`]), as the `[{key, value}]` list CodeBuild's own `Tag` shape spells.
pub(crate) async fn create_codebuild_report_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    copy_members(&properties, CODEBUILD_REPORT_GROUP_MEMBERS, &mut request);
    if let Some(declared) = properties.get("ExportConfig") {
        request.insert(
            "exportConfig".to_string(),
            report_export_config(declared),
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }

    let answered =
        call_codebuild(ctx, region, "CreateReportGroup", &Value::Object(request)).await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported_member(&answered, "reportGroup")
        .get("arn")
        .and_then(scalar_text)
    {
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// The declared `ExportConfig` as `CreateReportGroup` takes it: the configuration's own
/// member plus the S3 destination it may carry, each mapped member by member.
pub(crate) fn report_export_config(declared: &Value) -> Value {
    let mut mapped = mapped_members(declared, CODEBUILD_EXPORT_CONFIG_MEMBERS);
    let Value::Object(members) = &mut mapped else {
        return mapped;
    };
    if let Some(destination) = declared.get("S3Destination") {
        members.insert(
            "s3Destination".to_string(),
            mapped_members(destination, CODEBUILD_S3_DESTINATION_MEMBERS),
        );
    }
    mapped
}

/// Delete the report group a stack created, addressed by its ARN — the handle
/// `DeleteReportGroup` takes.
///
/// `DeleteReportGroup` models no does-not-exist error: a group CodeBuild no longer holds
/// is already in the state the delete asks for, and that call says so itself. Whether the
/// reports held in the group go with it is the declaration's `DeleteReports`, which is
/// that call's own `deleteReports` member, and it is sent only where the declaration
/// states it — the member carries a documented default of its own.
pub(crate) async fn delete_codebuild_report_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert(
        "arn".to_string(),
        Value::String(codebuild_report_group_arn(resource, region)),
    );
    if let Some(delete_reports) = resource
        .properties
        .get(CODEBUILD_DELETE_REPORTS_PROPERTY)
        .and_then(Value::as_bool)
    {
        request.insert("deleteReports".to_string(), Value::Bool(delete_reports));
    }
    call_codebuild(ctx, region, "DeleteReportGroup", &Value::Object(request)).await?;
    Ok(())
}

/// The report group's configuration as CodeBuild reports it, for drift detection.
///
/// `BatchGetReportGroups` reports an ARN it holds no group for in `reportGroupsNotFound`
/// rather than refusing the read, so an answer carrying no group is that service saying it
/// no longer holds the resource — the `DELETED` drift status.
pub(crate) async fn describe_codebuild_report_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "reportGroupArns": [codebuild_report_group_arn(resource, region)],
    });
    let answered = call_codebuild(ctx, region, "BatchGetReportGroups", &request).await?;
    let Some(group) = answered
        .get("reportGroups")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
    else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        group,
        CODEBUILD_REPORT_GROUP_REPORTED_PROPERTIES,
    )))
}

/// The ARN of the report group a resource record describes: the `Arn` attribute its
/// create reported, which is the ARN CodeBuild keyed the group by.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to
/// the ARN that service forms for the group's NAME, which is this resource's physical id —
/// the `report-group/<name>` path under the one account id this emulator mints, so the two
/// are the same ARN.
pub(crate) fn codebuild_report_group_arn(resource: &StoredResource, region: &str) -> String {
    if let Some(arn) = resource.attributes.get("Arn") {
        return arn.clone();
    }
    ::emulator_server::emulator_arn(
        super::template::CODEBUILD_ARN_NAMESPACE,
        region,
        &format!("report-group/{}", resource.physical_resource_id),
    )
}

// ---------------------------------------------------------------------------
// AWS CodeBuild: AWS::CodeBuild::Project
// ---------------------------------------------------------------------------

/// The properties of an `AWS::CodeBuild::Project` that are NOT members of
/// `CreateProject`, and so never travel in its request.
///
/// `Name` is the one value CloudFormation decides (this resource's physical id) and is set
/// on the way out; `Tags` is the create's own member beside the project, carrying the tags
/// CloudFormation gives the resource as well ([`tags_of`]); and the last three are each
/// another call's business — a webhook (`CreateWebhook`) for `Triggers`, the project's
/// visibility (`UpdateProjectVisibility`) for `Visibility` and the role that call takes for
/// `ResourceAccessRole`. Everything else the type publishes IS a `CreateProject` member
/// under the same name with a lower-case initial, so the evaluated document is carried
/// through with that one transformation ([`camel_cased`]).
pub(crate) const CODEBUILD_PROJECT_PROPERTIES_NOT_SENT: &[&str] = &[
    "name",
    "tags",
    "triggers",
    "visibility",
    "resourceAccessRole",
];

/// The one `ProjectSource` member whose property name is not its API member under a
/// lower-case initial: a template declares `BuildSpec` and the model spells it
/// `buildspec` (`ProjectSource$buildspec`, "the buildspec file declaration to use for the
/// builds in this build project").
///
/// `(as camel_cased leaves it, as the model spells it)`, applied to the primary `Source`
/// and to every entry of `SecondarySources`, which are the same shape.
pub(crate) const CODEBUILD_BUILDSPEC_MEMBER: (&str, &str) = ("buildSpec", "buildspec");

/// The `Project` members CodeBuild reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// The nested structures are not among them, for the reason the report group's catalogue
/// gives: a property the read reports differently is not evidence of anything
/// ([`super::drift`]). `Tags` is left out because the tags a resource carries change
/// independently of its declaration.
pub(crate) const CODEBUILD_PROJECT_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Name", "name"),
    ("Description", "description"),
    ("ServiceRole", "serviceRole"),
    ("SourceVersion", "sourceVersion"),
    ("EncryptionKey", "encryptionKey"),
    ("TimeoutInMinutes", "timeoutInMinutes"),
    ("QueuedTimeoutInMinutes", "queuedTimeoutInMinutes"),
    ("ConcurrentBuildLimit", "concurrentBuildLimit"),
    ("BadgeEnabled", "badgeEnabled"),
];

/// Create the build project an `AWS::CodeBuild::Project` declares — so a client that
/// deployed the stack and then calls `codebuild start-build` or `batch-get-projects` for
/// the project the stack reports gets the project, rather than CodeBuild refusing a
/// resource its own stack just reported created.
///
/// "Ref returns the project name", which is this resource's physical id and the one value
/// `CreateProject` takes to key the project by, so the project is created under it and the
/// id the stack already computed stays in force. The ARN that create ANSWERS with is
/// recorded as the type's published `Arn` attribute, so `Fn::GetAtt` reads the ARN
/// CodeBuild really keyed the project by.
///
/// The project carries the tags the declaration asks for plus the stack's own
/// ([`tags_of`]), as the `[{key, value}]` list CodeBuild's own `Tag` shape spells.
pub(crate) async fn create_codebuild_project(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = match camel_cased(&resource.properties, 0)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    for property in CODEBUILD_PROJECT_PROPERTIES_NOT_SENT {
        request.remove(*property);
    }
    // The one member spelled differently on the two sides, in the primary source and in
    // each secondary one.
    if let Some(source) = request.get_mut("source") {
        respell_buildspec(source);
    }
    for source in request
        .get_mut("secondarySources")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        respell_buildspec(source);
    }
    // The one member CloudFormation itself decides.
    request.insert(
        "name".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }

    let answered = call_codebuild(ctx, region, "CreateProject", &Value::Object(request)).await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported_member(&answered, "project")
        .get("arn")
        .and_then(scalar_text)
    {
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Move a source's `buildSpec` member — what [`camel_cased`] makes of the declared
/// `BuildSpec` — to the name the model spells it under
/// ([`CODEBUILD_BUILDSPEC_MEMBER`]). A source that declares none is left as it is.
fn respell_buildspec(source: &mut Value) {
    let (as_cased, as_modelled) = CODEBUILD_BUILDSPEC_MEMBER;
    let Some(members) = source.as_object_mut() else {
        return;
    };
    if let Some(buildspec) = members.remove(as_cased) {
        members.insert(as_modelled.to_string(), buildspec);
    }
}

/// Delete the project a stack created, addressed by the NAME it was created under — the
/// handle `DeleteProject` takes.
///
/// `DeleteProject` models no does-not-exist error: a project CodeBuild no longer holds is
/// already in the state the delete asks for, and that call says so itself.
pub(crate) async fn delete_codebuild_project(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    call_codebuild(
        ctx,
        region,
        "DeleteProject",
        &serde_json::json!({ "name": physical_resource_id }),
    )
    .await?;
    Ok(())
}

/// The project's configuration as CodeBuild reports it, for drift detection.
///
/// `BatchGetProjects` reports a name it holds no project for in `projectsNotFound` rather
/// than refusing the read, so an answer carrying no project is that service saying it no
/// longer holds the resource — the `DELETED` drift status.
pub(crate) async fn describe_codebuild_project(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "names": [physical_resource_id] });
    let answered = call_codebuild(ctx, region, "BatchGetProjects", &request).await?;
    let Some(project) = answered
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
    else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        project,
        CODEBUILD_PROJECT_REPORTED_PROPERTIES,
    )))
}

/// Call one CodeBuild operation on the awsJson1_1 wire its model declares.
pub(crate) async fn call_codebuild(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        CODEBUILD_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEBUILD_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS CodeCommit: AWS::CodeCommit::Repository
// ---------------------------------------------------------------------------

/// CodeCommit's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codecommit`, service shape `CodeCommit_20150413`), which speaks
/// `awsJson1_1`.
pub(crate) const CODECOMMIT_SIGNING_NAME: &str = "codecommit";
pub(crate) const CODECOMMIT_TARGET_PREFIX: &str = "CodeCommit_20150413";

/// The error code CodeCommit answers for a repository name no repository answers to
/// (`RepositoryDoesNotExistException`, the modelled refusal of `GetRepository`).
pub(crate) const CODECOMMIT_REPOSITORY_DOES_NOT_EXIST: &str = "RepositoryDoesNotExistException";

/// The `AWS::CodeCommit::Repository` properties `CreateRepository` takes under its own
/// member name, as `(property, member)` pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against that call's
/// `repositoryDescription` and `kmsKeyId` members — the same members under a lower-case
/// initial. `RepositoryName` is not here because it is the one value CloudFormation
/// decides, `Tags` is not because the create takes a tag MAP while the type publishes a tag
/// LIST, and `Triggers` is not because it is no member of that create at all.
pub(crate) const CODECOMMIT_REPOSITORY_MEMBERS: &[(&str, &str)] = &[
    ("RepositoryDescription", "repositoryDescription"),
    ("KmsKeyId", "kmsKeyId"),
];

/// The property that asks for CODE to be committed into the repository a stack creates: an
/// Amazon S3 bundle and the branch to commit it on. It is mapped onto no call here, so a
/// declaration that states it is refused rather than deployed as a repository holding
/// nothing.
pub(crate) const CODECOMMIT_CODE_PROPERTY: &str = "Code";

/// The property whose list IS the repository's trigger configuration, which
/// `PutRepositoryTriggers` writes.
pub(crate) const CODECOMMIT_TRIGGERS_PROPERTY: &str = "Triggers";

/// The members of one declared `Triggers` entry, as `RepositoryTrigger`'s own members: each
/// one the property with its initial letter lower-cased. A member no pair names is no member
/// of that shape, so it is not sent.
pub(crate) const CODECOMMIT_TRIGGER_MEMBERS: &[(&str, &str)] = &[
    ("Name", "name"),
    ("DestinationArn", "destinationArn"),
    ("CustomData", "customData"),
    ("Branches", "branches"),
    ("Events", "events"),
];

/// The `RepositoryMetadata` members CodeCommit answers a create with, under the attribute
/// names this type's published return values give them (`Arn`, `CloneUrlHttp`,
/// `CloneUrlSsh`, `KmsKeyId`, `Name`).
///
/// They are reported back as the resource's attributes rather than computed here: the clone
/// URLs name the endpoint this emulator serves the repository's git transport on, which no
/// pattern could mint, and the ARN is the one that service itself keyed the repository by.
pub(crate) const CODECOMMIT_REPORTED_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "Arn"),
    ("CloneUrlHttp", "cloneUrlHttp"),
    ("CloneUrlSsh", "cloneUrlSsh"),
    ("KmsKeyId", "kmsKeyId"),
    ("Name", "repositoryName"),
];

/// The members `GetRepository` reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// `Triggers` is not among them: a repository's trigger configuration is answered by
/// `GetRepositoryTriggers`, a read of its own, and a property the read does not report is
/// not evidence of anything ([`super::drift`]). `Tags` is left out for the same reason as
/// everywhere else here — the tags a resource carries change independently of its
/// declaration.
pub(crate) const CODECOMMIT_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("RepositoryName", "repositoryName"),
    ("RepositoryDescription", "repositoryDescription"),
    ("KmsKeyId", "kmsKeyId"),
];

/// Create the repository an `AWS::CodeCommit::Repository` declares, and write the trigger
/// configuration it declares on it — so a client that deployed the stack and then calls
/// `codecommit GetRepository` or `GetRepositoryTriggers` with the name the stack publishes
/// reads the repository and its triggers back rather than a
/// `RepositoryDoesNotExistException`.
///
/// "Ref returns the repository name", which is this resource's physical id and the
/// identifier every CodeCommit operation addresses a repository by, so the repository is
/// created under it and the id the stack already computed stays in force.
///
/// `Triggers` is no member of the create: "Replaces all triggers for a repository" is what
/// writes a repository's trigger configuration, so the declared list reaches CodeCommit
/// through that call on the repository just created — the same second call a client makes.
/// The resource the template declares is the repository AND those triggers, so a
/// configuration CodeCommit refuses fails the resource, and the repository this arm already
/// created is removed before that refusal propagates (the stack records no physical id for a
/// resource whose create failed, so nothing else would ever delete it).
///
/// The repository carries the tags the declaration asks for plus the stack's own
/// ([`tags_of`]), as `CreateRepository`'s own tag MAP.
pub(crate) async fn create_codecommit_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if properties.contains_key(CODECOMMIT_CODE_PROPERTY) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not implement the {CODECOMMIT_CODE_PROPERTY} property of {}",
                resource.resource_type
            ),
        });
    }

    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    request.insert("repositoryName".to_string(), Value::String(name.clone()));
    copy_members(&properties, CODECOMMIT_REPOSITORY_MEMBERS, &mut request);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered =
        call_codecommit(ctx, region, "CreateRepository", &Value::Object(request)).await?;
    let metadata = reported_member(&answered, "repositoryMetadata");

    let triggers = declared_triggers(&properties);
    if !triggers.is_empty() {
        let request = serde_json::json!({
            "repositoryName": name,
            "triggers": Value::Array(triggers),
        });
        if let Err(refusal) =
            call_codecommit(ctx, region, "PutRepositoryTriggers", &request).await
        {
            let removal = delete_codecommit_repository(ctx, region, &name).await;
            return Err(rolled_back(refusal, removal, "repository"));
        }
    }

    Ok(Created::attributes(reported_attributes(
        &metadata,
        CODECOMMIT_REPORTED_ATTRIBUTES,
    )))
}

/// The triggers a declaration asks for, as the `RepositoryTriggersList`
/// `PutRepositoryTriggers` takes: each declared entry under that shape's own member names
/// ([`CODECOMMIT_TRIGGER_MEMBERS`]).
///
/// Empty for a declaration that states no `Triggers` at all, which is a repository with no
/// trigger configuration — nothing is asked of CodeCommit for one.
pub(crate) fn declared_triggers(properties: &Map<String, Value>) -> Vec<Value> {
    properties
        .get(CODECOMMIT_TRIGGERS_PROPERTY)
        .and_then(Value::as_array)
        .map(|triggers| {
            triggers
                .iter()
                .map(|trigger| mapped_members(trigger, CODECOMMIT_TRIGGER_MEMBERS))
                .collect()
        })
        .unwrap_or_default()
}

/// Delete the repository a stack created, addressed by its NAME — this resource's physical
/// id, which is what `Ref` returns for the type.
///
/// `DeleteRepository` models no does-not-exist error at all ("If a specified repository was
/// already deleted, a null repository ID is returned"), so a repository CodeCommit no longer
/// holds is already in the state the delete asks for and the call says so itself.
pub(crate) async fn delete_codecommit_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "repositoryName": physical_resource_id });
    call_codecommit(ctx, region, "DeleteRepository", &request).await?;
    Ok(())
}

/// The repository's configuration as CodeCommit reports it, for drift detection. One it no
/// longer holds (`RepositoryDoesNotExistException`, judged on the CODE and never on a
/// message that carries the request's own name) was deleted.
pub(crate) async fn describe_codecommit_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "repositoryName": physical_resource_id });
    match call_codecommit(ctx, region, "GetRepository", &request).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &reported_member(&answer, "repositoryMetadata"),
            CODECOMMIT_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == CODECOMMIT_REPOSITORY_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Call one CodeCommit operation on the awsJson1_1 wire its model declares.
pub(crate) async fn call_codecommit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        CODECOMMIT_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODECOMMIT_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS CodeArtifact: AWS::CodeArtifact::Domain / AWS::CodeArtifact::Repository
// ---------------------------------------------------------------------------

/// CodeArtifact's signing name, from its model (`sigv4Name: codeartifact`, service shape
/// `CodeArtifactControlPlaneService`), which speaks `restJson1` — so its calls ride on
/// [`call_rest_json`], each on the URI its own operation's `@http` binding declares.
pub(crate) const CODEARTIFACT_SIGNING_NAME: &str = "codeartifact";

/// The URIs the CodeArtifact operations a stack makes are bound to, as that model's `@http`
/// traits declare them. Each of them names the resource it addresses in QUERY parameters
/// rather than in the path (`domain`, `domain-owner`, `repository`, `external-connection`),
/// which is what [`codeartifact_uri`] writes.
pub(crate) const CODEARTIFACT_DOMAIN_URI: &str = "/v1/domain";
/// See [`CODEARTIFACT_DOMAIN_URI`].
pub(crate) const CODEARTIFACT_REPOSITORY_URI: &str = "/v1/repository";
/// See [`CODEARTIFACT_DOMAIN_URI`].
pub(crate) const CODEARTIFACT_EXTERNAL_CONNECTION_URI: &str = "/v1/repository/external-connection";
/// See [`CODEARTIFACT_DOMAIN_URI`]. `PutDomainPermissionsPolicy` binds NO member to the
/// query string — its `domain` travels in the body with the document.
pub(crate) const CODEARTIFACT_DOMAIN_POLICY_URI: &str = "/v1/domain/permissions/policy";
/// See [`CODEARTIFACT_DOMAIN_URI`].
pub(crate) const CODEARTIFACT_REPOSITORY_POLICY_URI: &str = "/v1/repository/permissions/policy";

/// The error code CodeArtifact answers for a domain or a repository it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its reads and of `DeleteRepository`).
pub(crate) const CODEARTIFACT_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::CodeArtifact::Domain` properties `CreateDomain` takes under its own member
/// name, as `(property, member)` pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against that call's
/// `encryptionKey` member. `DomainName` is not here because it is the one value
/// CloudFormation decides and that call binds to its `domain` query parameter, `Tags` is not
/// because the member spells a tag's halves in lower camel case, and
/// `PermissionsPolicyDocument` is not because it is no member of that create at all.
pub(crate) const CODEARTIFACT_DOMAIN_MEMBERS: &[(&str, &str)] = &[("EncryptionKey", "encryptionKey")];

/// The `AWS::CodeArtifact::Repository` properties `CreateRepository` takes under its own
/// member name.
///
/// `RepositoryName`, `DomainName` and `DomainOwner` are not here because that call binds
/// each of them to a query parameter; `Upstreams` is not because the type publishes a list
/// of repository NAMES while the member is a list of `UpstreamRepository` structures
/// ([`declared_upstreams`]); `ExternalConnections` and `PermissionsPolicyDocument` are not
/// because neither is a member of that create at all.
pub(crate) const CODEARTIFACT_REPOSITORY_MEMBERS: &[(&str, &str)] = &[("Description", "description")];

/// The property whose document IS the resource's own permissions policy, which
/// `PutDomainPermissionsPolicy` / `PutRepositoryPermissionsPolicy` writes.
pub(crate) const CODEARTIFACT_POLICY_PROPERTY: &str = "PermissionsPolicyDocument";

/// The property whose list names the repositories a declaration puts UPSTREAM of its own.
pub(crate) const CODEARTIFACT_UPSTREAMS_PROPERTY: &str = "Upstreams";

/// The property whose list names the external connections to associate with the repository,
/// which `AssociateExternalConnection` adds one at a time.
pub(crate) const CODEARTIFACT_EXTERNAL_CONNECTIONS_PROPERTY: &str = "ExternalConnections";

/// The property naming the DOMAIN a repository is declared in — half of the pair every
/// CodeArtifact call on a repository addresses it by.
pub(crate) const CODEARTIFACT_DOMAIN_PROPERTY: &str = "DomainName";

/// The property naming the account that owns that domain, which a declaration states only
/// for a domain of another account.
pub(crate) const CODEARTIFACT_DOMAIN_OWNER_PROPERTY: &str = "DomainOwner";

/// The `DomainDescription` members CodeArtifact answers a create with, under the attribute
/// names this type's published return values give them (`Arn`, `Name`, `Owner`).
///
/// They are reported back as the resource's attributes rather than kept as computed: the ARN
/// is the one that service itself keyed the domain by, and the owner is the account it
/// recorded for it.
pub(crate) const CODEARTIFACT_DOMAIN_REPORTED_ATTRIBUTES: &[(&str, &str)] =
    &[("Arn", "arn"), ("Name", "name"), ("Owner", "owner")];

/// The `RepositoryDescription` members it answers a repository create with, under that
/// type's published attribute names (`Arn`, `DomainOwner`, `Name`).
pub(crate) const CODEARTIFACT_REPOSITORY_REPORTED_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "arn"),
    ("DomainOwner", "domainOwner"),
    ("Name", "name"),
];

/// The members `DescribeDomain` reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// `Tags` is left out for the reason it is everywhere else here — the tags a resource
/// carries change independently of its declaration — and `PermissionsPolicyDocument` because
/// a domain's policy is answered by `GetDomainPermissionsPolicy`, a read of its own, and a
/// property the read does not report is not evidence of anything ([`super::drift`]).
pub(crate) const CODEARTIFACT_DOMAIN_REPORTED_PROPERTIES: &[(&str, &str)] =
    &[("DomainName", "name"), ("EncryptionKey", "encryptionKey")];

/// The members `DescribeRepository` reports under this type's own property names.
///
/// `Upstreams` and `ExternalConnections` are left out: the read reports each as a list of
/// structures (`UpstreamRepositoryInfo`, `RepositoryExternalConnectionInfo`) while the type
/// publishes a list of plain names, so the two documents could not be compared without
/// inventing a reading of one of them.
pub(crate) const CODEARTIFACT_REPOSITORY_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("RepositoryName", "name"),
    ("DomainName", "domainName"),
    ("DomainOwner", "domainOwner"),
    ("Description", "description"),
];

/// One CodeArtifact URI: the operation's path with the resource it addresses in the query
/// parameters that operation's model binds those members to, each value percent-encoded
/// ([`percent_encoded`]) so a name reaches the called operation as one value.
///
/// A pair whose value is empty is left out, which is how a member a declaration states
/// nothing for (`domain-owner`) is not sent at all.
pub(crate) fn codeartifact_uri(path: &str, parameters: &[(&str, &str)]) -> String {
    let query: Vec<String> = parameters
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| format!("{name}={}", percent_encoded(value)))
        .collect();
    if query.is_empty() {
        return path.to_string();
    }
    format!("{path}?{}", query.join("&"))
}

/// One declared property of a CodeArtifact resource as the text its calls carry: the value
/// the record's resolved properties hold ([`property`], this module's one spelling rule for
/// reading a declared scalar), else the attribute the same declaration gave the resource,
/// else `""` for a declaration that states none — which is how a member a declaration
/// states nothing for (`domain-owner`) is left out of the request.
pub(crate) fn codeartifact_property(resource: &StoredResource, name: &str) -> String {
    property(resource, name)
        .or_else(|| resource.attributes.get(name).cloned())
        .unwrap_or_default()
}

/// The permissions policy a declaration attaches, as the `policyDocument` STRING those two
/// calls take: the type publishes the property as a policy DOCUMENT, while the member is a
/// string — so [`document_text`]'s rule applies, the one every other policy arm here reads a
/// declared document by. `None` for a declaration that attaches none.
pub(crate) fn declared_permissions_policy(properties: &Map<String, Value>) -> Option<String> {
    properties
        .get(CODEARTIFACT_POLICY_PROPERTY)
        .and_then(document_text)
        .filter(|document| !document.is_empty())
}

/// The upstream repositories a declaration asks for, as the `UpstreamRepositoryList`
/// `CreateRepository` takes: the type publishes a list of repository NAMES
/// ([`declared_text_list`]), while that member is a list of
/// `UpstreamRepository{repositoryName}`.
pub(crate) fn declared_upstreams(properties: &Value) -> Vec<Value> {
    declared_text_list(properties, CODEARTIFACT_UPSTREAMS_PROPERTY)
        .into_iter()
        .map(|name| serde_json::json!({ "repositoryName": name }))
        .collect()
}

/// Create the domain an `AWS::CodeArtifact::Domain` declares, and attach the permissions
/// policy it declares to it — so a client that deployed the stack and then calls
/// `codeartifact describe-domain` with the name the stack publishes reads the domain back
/// rather than a `ResourceNotFoundException`.
///
/// "Ref returns the Amazon Resource Name (ARN) of the domain", so the resource's physical id
/// is NOT the handle CodeArtifact addresses a domain by: the domain is created under the NAME
/// the declaration asked for, which is this type's published `Name` attribute and the value
/// `CreateDomain` binds to its `domain` query parameter.
///
/// `PermissionsPolicyDocument` is no member of that create: "Sets a resource policy on a
/// domain" is what attaches one, so the declared document reaches CodeArtifact through that
/// call on the domain just created — the same second call a client makes. The resource the
/// template declares is the domain AND that policy, so a document CodeArtifact refuses fails
/// the resource, and the domain this arm already created is removed before that refusal
/// propagates (the stack records no physical id for a resource whose create failed, so
/// nothing else would ever delete it).
///
/// The domain carries the tags the declaration asks for plus the stack's own ([`tags_of`]),
/// as the `[{key, value}]` list that model's own `Tag` shape spells.
pub(crate) async fn create_codeartifact_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let domain = declared_name(resource);
    let mut request = Map::new();
    copy_members(&properties, CODEARTIFACT_DOMAIN_MEMBERS, &mut request);
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let uri = codeartifact_uri(CODEARTIFACT_DOMAIN_URI, &[("domain", &domain)]);
    let answered =
        call_codeartifact(ctx, region, "POST", &uri, Some(&Value::Object(request))).await?;

    if let Some(document) = declared_permissions_policy(&properties) {
        let policy = serde_json::json!({ "domain": domain, "policyDocument": document });
        if let Err(refusal) = call_codeartifact(
            ctx,
            region,
            "PUT",
            CODEARTIFACT_DOMAIN_POLICY_URI,
            Some(&policy),
        )
        .await
        {
            let removal = delete_codeartifact_domain(ctx, region, &domain).await;
            return Err(rolled_back(refusal, removal, "domain"));
        }
    }

    Ok(Created::attributes(reported_attributes(
        &reported_member(&answered, "domain"),
        CODEARTIFACT_DOMAIN_REPORTED_ATTRIBUTES,
    )))
}

/// Delete the domain a stack created, addressed by its NAME.
///
/// `DeleteDomain` models no does-not-exist error at all, and a domain CodeArtifact no longer
/// holds is already in the state the delete asks for, so that call says so itself. A domain
/// that still holds repositories is its own refusal, which is reported as the failure it is:
/// a stack deletes a repository before the domain that holds it, so the only way to reach it
/// is a repository something else created there.
pub(crate) async fn delete_codeartifact_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    domain: &str,
) -> Result<(), Refusal> {
    let uri = codeartifact_uri(CODEARTIFACT_DOMAIN_URI, &[("domain", domain)]);
    call_codeartifact(ctx, region, "DELETE", &uri, None).await?;
    Ok(())
}

/// The domain's configuration as CodeArtifact reports it, for drift detection. One it no
/// longer holds (`ResourceNotFoundException`, judged on the CODE and never on a message that
/// carries the request's own name) was deleted.
pub(crate) async fn describe_codeartifact_domain(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = codeartifact_uri(
        CODEARTIFACT_DOMAIN_URI,
        &[("domain", &declared_name(resource))],
    );
    match call_codeartifact(ctx, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &reported_member(&answer, "domain"),
            CODEARTIFACT_DOMAIN_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the repository an `AWS::CodeArtifact::Repository` declares, in the domain its
/// declaration names, and associate every external connection it declares with it.
///
/// "Ref returns the Amazon Resource Name (ARN) of the repository", so — as for a domain — the
/// physical id is not the handle CodeArtifact addresses one by: a repository is addressed by
/// the PAIR its create fixes, the domain and the repository's own name, both of which
/// `CreateRepository` binds to query parameters.
///
/// `ExternalConnections` and `PermissionsPolicyDocument` are no members of that create:
/// "Adds an existing external connection to a repository" and "Sets the resource policy on a
/// repository" are what apply them, so each declared connection and the declared document
/// reach CodeArtifact through those calls on the repository just created — the same further
/// calls a client makes. The resource the template declares is the repository AND those, so
/// one CodeArtifact refuses fails the resource and the repository this arm already created is
/// removed before that refusal propagates.
///
/// The repository carries the tags the declaration asks for plus the stack's own, and the
/// attributes it reports are the ones CodeArtifact answered with — the ARN that service keyed
/// the repository by and the account it recorded as the owner of its domain.
pub(crate) async fn create_codeartifact_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let repository = declared_name(resource);
    let domain = codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY);
    let owner = codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY);
    let addressed: &[(&str, &str)] = &[
        ("domain", &domain),
        ("domain-owner", &owner),
        ("repository", &repository),
    ];

    let mut request = Map::new();
    copy_members(&properties, CODEARTIFACT_REPOSITORY_MEMBERS, &mut request);
    let upstreams = declared_upstreams(&resource.properties);
    if !upstreams.is_empty() {
        request.insert("upstreams".to_string(), Value::Array(upstreams));
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let uri = codeartifact_uri(CODEARTIFACT_REPOSITORY_URI, addressed);
    let answered =
        call_codeartifact(ctx, region, "POST", &uri, Some(&Value::Object(request))).await?;
    // Each further call answers with the repository as it stands, so the attributes the
    // resource reports are the ones the LAST of them described.
    let mut described = reported_member(&answered, "repository");

    for connection in declared_text_list(
        &resource.properties,
        CODEARTIFACT_EXTERNAL_CONNECTIONS_PROPERTY,
    ) {
        let mut parameters = addressed.to_vec();
        parameters.push(("external-connection", &connection));
        let uri = codeartifact_uri(CODEARTIFACT_EXTERNAL_CONNECTION_URI, &parameters);
        // That operation's input is entirely in the query string, so the body it takes is
        // an empty document rather than nothing.
        let empty = Value::Object(Map::new());
        match call_codeartifact(ctx, region, "POST", &uri, Some(&empty)).await {
            Ok(answer) => described = reported_member(&answer, "repository"),
            Err(refusal) => {
                let removal = delete_codeartifact_repository(
                    ctx, region, &domain, &owner, &repository,
                )
                .await;
                return Err(rolled_back(refusal, removal, "repository"));
            }
        }
    }

    if let Some(document) = declared_permissions_policy(&properties) {
        let uri = codeartifact_uri(CODEARTIFACT_REPOSITORY_POLICY_URI, addressed);
        let policy = serde_json::json!({ "policyDocument": document });
        if let Err(refusal) = call_codeartifact(ctx, region, "PUT", &uri, Some(&policy)).await {
            let removal =
                delete_codeartifact_repository(ctx, region, &domain, &owner, &repository).await;
            return Err(rolled_back(refusal, removal, "repository"));
        }
    }

    Ok(Created::attributes(reported_attributes(
        &described,
        CODEARTIFACT_REPOSITORY_REPORTED_ATTRIBUTES,
    )))
}

/// Delete the repository a stack created, addressed by the domain and the name it was
/// created under. One CodeArtifact no longer holds (`ResourceNotFoundException`, judged on
/// the CODE) is already in the state the delete asks for.
pub(crate) async fn delete_codeartifact_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    domain: &str,
    owner: &str,
    repository: &str,
) -> Result<(), Refusal> {
    let uri = codeartifact_uri(
        CODEARTIFACT_REPOSITORY_URI,
        &[
            ("domain", domain),
            ("domain-owner", owner),
            ("repository", repository),
        ],
    );
    match call_codeartifact(ctx, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The repository's configuration as CodeArtifact reports it, for drift detection. One it no
/// longer holds — or one whose DOMAIN it no longer holds, which answers the same code — was
/// deleted.
pub(crate) async fn describe_codeartifact_repository(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = codeartifact_uri(
        CODEARTIFACT_REPOSITORY_URI,
        &[
            (
                "domain",
                &codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY),
            ),
            (
                "domain-owner",
                &codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY),
            ),
            ("repository", &declared_name(resource)),
        ],
    );
    match call_codeartifact(ctx, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &reported_member(&answer, "repository"),
            CODEARTIFACT_REPOSITORY_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Call one CodeArtifact operation on the restJson1 wire its model declares.
pub(crate) async fn call_codeartifact(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    uri: &str,
    request: Option<&Value>,
) -> Result<Value, Refusal> {
    call_rest_json(ctx, CODEARTIFACT_SIGNING_NAME, region, method, uri, request).await
}

// ---------------------------------------------------------------------------
// KMS: AWS::KMS::Key
// ---------------------------------------------------------------------------

/// KMS's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: kms`, service
/// shape `TrentService`), which speaks `awsJson1_1`.
pub(crate) const KMS_SIGNING_NAME: &str = "kms";
pub(crate) const KMS_TARGET_PREFIX: &str = "TrentService";

/// The error code KMS answers for a key identifier it resolves to no key.
pub(crate) const KMS_NOT_FOUND: &str = "NotFoundException";

/// The error code KMS answers for a request the key's own state does not admit — among
/// them scheduling the deletion of a key that is already awaiting it.
pub(crate) const KMS_INVALID_STATE: &str = "KMSInvalidStateException";

/// The `KeyState` of a key whose deletion is already scheduled, which is the state a
/// delete of the resource asks for.
pub(crate) const KMS_KEY_STATE_PENDING_DELETION: &str = "PendingDeletion";

/// A KMS tag's member names: `Tag{TagKey, TagValue}`, which is neither of the two
/// spellings the other services here use (`smithy_query(shape="Tag")` on the KMS model).
pub(crate) const KMS_TAG: (&str, &str) = ("TagKey", "TagValue");

/// The properties of `AWS::KMS::Key` that are members of `CreateKey`, each with the member
/// it is. Every one of them is named identically on both sides — `Description`, `KeySpec`,
/// `KeyUsage`, `MultiRegion`, `Origin` and `BypassPolicyLockoutSafetyCheck` are properties
/// of the type and members of `smithy_query(operation="CreateKey")` — and their VALUES are
/// what the template declared, so KMS's own validation is what judges them.
pub(crate) const KMS_KEY_CREATE_MEMBERS: &[(&str, &str)] = &[
    (
        "BypassPolicyLockoutSafetyCheck",
        "BypassPolicyLockoutSafetyCheck",
    ),
    ("Description", "Description"),
    ("KeySpec", "KeySpec"),
    ("KeyUsage", "KeyUsage"),
    ("MultiRegion", "MultiRegion"),
    ("Origin", "Origin"),
];

/// The property carrying the key's own key policy. It is the one property whose NAME
/// differs from the member that takes it (`CreateKey$Policy`, `PutKeyPolicy$Policy`), and
/// the member is the document's TEXT, so a template that spells the policy as an object
/// has it rendered ([`document_text`]).
pub(crate) const KMS_KEY_POLICY_PROPERTY: &str = "KeyPolicy";

/// The property that says whether the key is enabled. A key `CreateKey` mints is enabled,
/// so only a declaration asking for a DISABLED key has anything called for it — KMS's own
/// `DisableKey`, there being no member of the create for it.
pub(crate) const KMS_KEY_ENABLED_PROPERTY: &str = "Enabled";

/// The property that turns automatic rotation of the key's material on, and the one that
/// says how long a rotation period is: `EnableKeyRotation` takes both and no member of the
/// create does.
pub(crate) const KMS_KEY_ROTATION_PROPERTY: &str = "EnableKeyRotation";
pub(crate) const KMS_KEY_ROTATION_PERIOD_PROPERTY: &str = "RotationPeriodInDays";

/// The property that says how long KMS waits before it deletes the key, which is a member
/// of `ScheduleKeyDeletion` rather than of any create or update call.
pub(crate) const KMS_KEY_PENDING_WINDOW_PROPERTY: &str = "PendingWindowInDays";

/// Every property `AWS::KMS::Key` publishes, which is what this arm carries to KMS: the
/// create's own members, the four that reach it through a call of their own, and the tag
/// list. A declaration naming anything else is refused rather than deployed with the
/// property dropped.
pub(crate) const KMS_KEY_PROPERTIES: &[&str] = &[
    "BypassPolicyLockoutSafetyCheck",
    "Description",
    KMS_KEY_ENABLED_PROPERTY,
    KMS_KEY_ROTATION_PROPERTY,
    KMS_KEY_POLICY_PROPERTY,
    "KeySpec",
    "KeyUsage",
    "MultiRegion",
    "Origin",
    KMS_KEY_PENDING_WINDOW_PROPERTY,
    KMS_KEY_ROTATION_PERIOD_PROPERTY,
    "Tags",
];

/// Create the KMS key an `AWS::KMS::Key` declares, in KMS itself.
///
/// The key's IDENTITY is KMS's own to mint: `CreateKey` takes no identifier and answers
/// the `KeyMetadata` carrying the `KeyId` it assigned and the `Arn` it built from it. Both
/// are reported back — the key id as the resource's physical id, which is what `Ref`
/// resolves to, and the pair as the type's two published `Fn::GetAtt` attributes — so a
/// queue, a bucket or a stream whose declaration hands the stack's `Fn::GetAtt
/// ['Key','Arn']` to its own service resolves it in the SAME store KMS's handlers wrote,
/// rather than naming a key nothing answers to.
///
/// Three properties are not members of the create and reach KMS through the call that owns
/// each: the rotation the key's material is on (`EnableKeyRotation`, with the period the
/// declaration asks for), and a declaration asking for a DISABLED key (`DisableKey`).
/// Rotation is turned on first, because KMS does not take that request for a key that is
/// disabled. A key whose follow-up call KMS refuses is not left behind half-configured: it
/// is scheduled for deletion again, and the refusal is what the deployment reports.
pub(crate) async fn create_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if let Some(unmapped) = kms_key_unmapped_property(&properties) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an {KMS_KEY} to \
                 KMS; the resource was not created."
            ),
        });
    }
    let mut request = Map::new();
    copy_members(&properties, KMS_KEY_CREATE_MEMBERS, &mut request);
    if let Some(policy) = properties.get(KMS_KEY_POLICY_PROPERTY).and_then(document_text) {
        request.insert("Policy".to_string(), Value::String(policy));
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, KMS_TAG));
    }
    let answered = call_kms(ctx, region, "CreateKey", &Value::Object(request)).await?;
    let metadata = reported_member(&answered, "KeyMetadata");
    let (Some(key_id), Some(arn)) = (
        metadata.get("KeyId").and_then(scalar_text),
        metadata.get("Arn").and_then(scalar_text),
    ) else {
        // The create answered 2xx without the identity its output models. Nothing addresses
        // a key without one, so this is reported as a failure rather than recorded as a
        // resource whose identifier nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateKey answered no KeyMetadata.KeyId or KeyMetadata.Arn".to_string(),
        });
    };
    if let Err(refusal) = configure_kms_key(ctx, region, &key_id, &properties).await {
        return match schedule_kms_key_deletion(ctx, region, &key_id, &properties).await {
            Ok(()) => Err(refusal),
            Err(cleanup) => Err(Refusal {
                code: refusal.code,
                message: format!(
                    "{}; the key it created could not be scheduled for deletion again: \
                     {cleanup}",
                    refusal.message
                ),
            }),
        };
    }
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn);
    attributes.insert("KeyId".to_string(), key_id.clone());
    Ok(Created {
        physical_resource_id: Some(key_id),
        attributes,
    })
}

/// The first declared property of an `AWS::KMS::Key` this arm does not carry to KMS, or
/// `None` for a declaration whose every property it does ([`KMS_KEY_PROPERTIES`]).
pub(crate) fn kms_key_unmapped_property(properties: &Map<String, Value>) -> Option<String> {
    unmapped_property(properties, KMS_KEY_PROPERTIES)
}

/// Put the key the create just minted into the state the declaration asks for, through the
/// KMS calls that own the two switches no member of `CreateKey` carries.
///
/// Nothing is called where the declaration asks for what a new key already is: rotation is
/// off and the key is enabled, so a declaration that states neither switch — or states
/// each at its default — leaves KMS with one call made about the key.
pub(crate) async fn configure_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
    properties: &Map<String, Value>,
) -> Result<(), Refusal> {
    if declared_flag(properties, KMS_KEY_ROTATION_PROPERTY) == Some(true) {
        let mut request = Map::new();
        request.insert("KeyId".to_string(), Value::String(key_id.to_string()));
        if let Some(period) = whole_number(properties.get(KMS_KEY_ROTATION_PERIOD_PROPERTY)) {
            request.insert(
                "RotationPeriodInDays".to_string(),
                Value::from(period),
            );
        }
        call_kms(ctx, region, "EnableKeyRotation", &Value::Object(request)).await?;
    }
    // "Enabled ... Default: true" for the type, so only a declaration asking for the key
    // to be disabled has anything called for it — and it comes last, rotation not being a
    // request KMS takes for a disabled key.
    if declared_flag(properties, KMS_KEY_ENABLED_PROPERTY) == Some(false) {
        call_kms(
            ctx,
            region,
            "DisableKey",
            &serde_json::json!({ "KeyId": key_id }),
        )
        .await?;
    }
    Ok(())
}

/// A boolean property in either spelling a template gives one: JSON's boolean, or the
/// text a JSON template spells one as, read case-insensitively exactly as every other
/// template boolean is ([`super::template`]'s own reading of one). `None` is a property
/// the declaration does not state, or states as something that is not a boolean at all —
/// which the owning service's own validation is what judges.
pub(crate) fn declared_flag(properties: &Map<String, Value>, property: &str) -> Option<bool> {
    match properties.get(property)? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// Bring the tags of the key a resource record names to the ones this deployment gives it,
/// through KMS's own `TagResource` and `UntagResource`.
///
/// The set is the declaration's own `Tags` beside the ones CloudFormation propagates
/// ([`tags_of`]) — the same set the CREATE applied — so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value
/// is re-valued ("adds or edits tags on a customer managed KMS key", `TagResource`). Where
/// the two sets already agree nothing is called at all, which is what makes this safe to ask
/// on every deployment.
///
/// A key KMS no longer resolves the id to has no tags to reconcile.
pub(crate) async fn retag_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let key_id = resource.physical_resource_id.clone();
    if describe_kms_key_metadata(ctx, region, &key_id).await?.is_none() {
        return Ok(());
    }
    let desired = tags_of(declared, propagated);
    let held = kms_key_tags(
        &call_kms(
            ctx,
            region,
            "ListResourceTags",
            &serde_json::json!({ "KeyId": &key_id }),
        )
        .await?,
    );
    let (added, removed) = tag_delta(&held, &desired);
    if !added.is_empty() {
        call_kms(
            ctx,
            region,
            "TagResource",
            &serde_json::json!({
                "KeyId": &key_id,
                "Tags": tags_as_list(&added, KMS_TAG),
            }),
        )
        .await?;
    }
    if !removed.is_empty() {
        call_kms(
            ctx,
            region,
            "UntagResource",
            &serde_json::json!({
                "KeyId": &key_id,
                "TagKeys": tag_keys_as_list(&removed),
            }),
        )
        .await?;
    }
    Ok(())
}

/// The tags a KMS tag read answers with, as the pairs this module reconciles: KMS spells a
/// tag `{TagKey, TagValue}` ([`KMS_TAG`]), which is neither of the two spellings
/// [`declared_tags`] reads.
pub(crate) fn kms_key_tags(answer: &Value) -> Vec<(String, String)> {
    answer
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag.get(KMS_TAG.0).and_then(Value::as_str)?.to_string(),
                        tag.get(KMS_TAG.1)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Apply a changed `AWS::KMS::Key` declaration to the key that already exists.
///
/// Each property that can change on a key has a KMS call that changes it, and that is what
/// this calls: the description through `UpdateKeyDescription`, the key policy through
/// `PutKeyPolicy`, the enabled switch through `EnableKey`/`DisableKey` and the rotation
/// switch through `EnableKeyRotation`/`DisableKeyRotation`. A property neither revision
/// changed is not sent at all, and nothing is called where nothing changed.
///
/// The key keeps its identity, so it keeps the id and the ARN everything that references it
/// resolves through. A change to a property KMS cannot make to an existing key — the key
/// spec, its usage, its origin or whether it is multi-Region, all of which AWS answers by
/// REPLACING the key — is refused rather than quietly dropped, so the stack never reports a
/// key configured differently from its declaration.
///
/// This is the key's CONFIGURATION alone. The tags CloudFormation gives it travel
/// separately, through [`retag_kms_key`], because they change independently of any
/// declaration.
pub(crate) async fn update_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let before = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    let after = match declared {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if let Some(unmapped) = kms_key_unmapped_property(&after) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an {KMS_KEY} to \
                 KMS; the resource was left as it is."
            ),
        });
    }
    // The create-only properties: AWS replaces the key when one of them changes, which is a
    // new key under a new identity — not a change this arm can make to the key the stack
    // has.
    if let Some(replaced) = KMS_KEY_REPLACEMENT_PROPERTIES
        .iter()
        .find(|property| before.get(**property) != after.get(**property))
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not change the {replaced} property of an existing \
                 {KMS_KEY}, which KMS answers by replacing the key; the resource was left \
                 as it is."
            ),
        });
    }

    let key_id = resource.physical_resource_id.clone();
    if before.get("Description") != after.get("Description") {
        // A description the update no longer declares is the empty one, which is what a key
        // created without the property carries.
        let description = after
            .get("Description")
            .and_then(scalar_text)
            .unwrap_or_default();
        call_kms(
            ctx,
            region,
            "UpdateKeyDescription",
            &serde_json::json!({ "KeyId": key_id, "Description": description }),
        )
        .await?;
    }
    let policy_before = before.get(KMS_KEY_POLICY_PROPERTY).and_then(document_text);
    let policy_after = after.get(KMS_KEY_POLICY_PROPERTY).and_then(document_text);
    if policy_before != policy_after {
        let Some(policy) = policy_after else {
            // There is no way to ask KMS for the default key policy back, and restating it
            // here would be a second copy of its documentation.
            return Err(Refusal {
                code: NOT_IMPLEMENTED.to_string(),
                message: format!(
                    "this emulator does not reset the {KMS_KEY_POLICY_PROPERTY} property of \
                     an existing {KMS_KEY} that an update no longer declares to KMS's \
                     default key policy; the resource was left as it is."
                ),
            });
        };
        call_kms(
            ctx,
            region,
            "PutKeyPolicy",
            &serde_json::json!({ "KeyId": key_id, "Policy": policy }),
        )
        .await?;
    }
    // Rotation is changed before the enabled switch for the same reason the create turns it
    // on first: KMS does not take a rotation request for a key that is disabled.
    let rotation_before = declared_flag(&before, KMS_KEY_ROTATION_PROPERTY).unwrap_or(false);
    let rotation_after = declared_flag(&after, KMS_KEY_ROTATION_PROPERTY).unwrap_or(false);
    let period_changed = whole_number(before.get(KMS_KEY_ROTATION_PERIOD_PROPERTY))
        != whole_number(after.get(KMS_KEY_ROTATION_PERIOD_PROPERTY));
    if rotation_after && (!rotation_before || period_changed) {
        let mut request = Map::new();
        request.insert("KeyId".to_string(), Value::String(key_id.clone()));
        if let Some(period) = whole_number(after.get(KMS_KEY_ROTATION_PERIOD_PROPERTY)) {
            request.insert("RotationPeriodInDays".to_string(), Value::from(period));
        }
        call_kms(ctx, region, "EnableKeyRotation", &Value::Object(request)).await?;
    } else if rotation_before && !rotation_after {
        call_kms(
            ctx,
            region,
            "DisableKeyRotation",
            &serde_json::json!({ "KeyId": key_id }),
        )
        .await?;
    }
    let enabled_before = declared_flag(&before, KMS_KEY_ENABLED_PROPERTY).unwrap_or(true);
    let enabled_after = declared_flag(&after, KMS_KEY_ENABLED_PROPERTY).unwrap_or(true);
    if enabled_before != enabled_after {
        let operation = if enabled_after {
            "EnableKey"
        } else {
            "DisableKey"
        };
        call_kms(
            ctx,
            region,
            operation,
            &serde_json::json!({ "KeyId": key_id }),
        )
        .await?;
    }
    // The key's identity is untouched, so the id and the ARN the create reported still
    // address it: this arm reports no attribute of its own.
    Ok(Created::default())
}
