// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the certificate and DNS services: AWS Certificate
//! Manager and its Private CA, Amazon Route 53 Resolver, AWS Elastic Beanstalk, AWS
//! CodeDeploy, AWS Transfer Family and AWS Batch. One file of [`super::super`]'s per-type
//! arms, dispatched to by its `Provisioner`.

use super::*;

/// Import the certificate a declaration carries INTO the private CA it names, and leave
/// that CA in the status the declaration asks for — which is what makes a CA a stack
/// declared able to issue anything at all.
///
/// `ImportCertificateAuthorityCertificate` takes the certificate and the chain as BLOBS,
/// while this type publishes both as PEM properties — the `Certificate` one being what an
/// `Fn::GetAtt <Certificate>.Certificate` of the same stack resolves to — so each
/// property's text travels base64 as the awsJson wire carries a blob.
///
/// A CA is `ACTIVE` once it holds its certificate, so a declaration asking for that status
/// needs nothing more; one asking for any other status (`DISABLED`) is applied through
/// `UpdateCertificateAuthority`, the call that moves a CA between the statuses it can be
/// put in. The resource's identity is the CA it activated, and its published return value
/// is the complete chain that CA now presents.
pub(crate) async fn activate_certificate_authority(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let Some(ca_arn) = properties
        .get("CertificateAuthorityArn")
        .and_then(Value::as_str)
    else {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: "AWS::ACMPCA::CertificateAuthorityActivation declares no \
                      CertificateAuthorityArn to activate"
                .to_string(),
        });
    };
    let Some(certificate) = properties.get("Certificate").and_then(Value::as_str) else {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: "AWS::ACMPCA::CertificateAuthorityActivation declares no Certificate to \
                      import"
                .to_string(),
        });
    };
    let chain = properties
        .get("CertificateChain")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut request = Map::new();
    request.insert(
        "CertificateAuthorityArn".to_string(),
        Value::String(ca_arn.to_string()),
    );
    request.insert(
        "Certificate".to_string(),
        Value::String(::aws_smithy_types::base64::encode(certificate.as_bytes())),
    );
    if let Some(chain) = chain.as_deref() {
        request.insert(
            "CertificateChain".to_string(),
            Value::String(::aws_smithy_types::base64::encode(chain.as_bytes())),
        );
    }
    call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.ImportCertificateAuthorityCertificate"),
        &Value::Object(request),
    )
    .await?;
    if let Some(status) = properties.get("Status").and_then(Value::as_str) {
        if status != CERTIFICATE_AUTHORITY_ACTIVE {
            call_json_rpc(
                ctx,
                ACMPCA_SIGNING_NAME,
                region,
                AWS_JSON_1_1,
                &format!("{ACMPCA_TARGET_PREFIX}.UpdateCertificateAuthority"),
                &serde_json::json!({
                    "CertificateAuthorityArn": ca_arn,
                    "Status": status,
                }),
            )
            .await?;
        }
    }
    let mut attributes: ::std::collections::BTreeMap<String, String> =
        ::std::collections::BTreeMap::new();
    attributes.insert(
        "CompleteCertificateChain".to_string(),
        match chain {
            Some(chain) => format!("{certificate}{chain}"),
            None => certificate.to_string(),
        },
    );
    Ok(Created {
        physical_resource_id: Some(ca_arn.to_string()),
        attributes,
    })
}

/// Whether Amazon Web Services Private CA still holds the certificate a stack had one of
/// its CAs issue, for drift detection.
///
/// A certificate ARN carries the ARN of the CA that issued it as its prefix
/// (`<ca arn>/certificate/<serial>`), which is what addresses the pair `GetCertificate`
/// takes. A certificate that service no longer serves is one the stack's record is of
/// nothing. The document is otherwise EMPTY: nothing a declaration of this type states is
/// reported back, and a property the actual document does not carry is not evidence of
/// anything (`super::drift::differs`).
pub(crate) async fn issued_certificate_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    certificate_arn: &str,
) -> Result<Option<Value>, Refusal> {
    let Some((ca_arn, _)) = certificate_arn.split_once("/certificate/") else {
        return Ok(None);
    };
    let request = serde_json::json!({
        "CertificateAuthorityArn": ca_arn,
        "CertificateArn": certificate_arn,
    });
    match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.GetCertificate"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(Some(Value::Object(Map::new()))),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The status the activation of the private CA `ca_arn` names left that CA in, as Amazon
/// Web Services Private CA reports it, for drift detection: the one property of this type
/// that is also read back. A CA that service no longer holds was deleted, which takes its
/// activation with it.
pub(crate) async fn activation_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    ca_arn: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "CertificateAuthorityArn": ca_arn });
    let reported = match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.DescribeCertificateAuthority"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(authority) = reported.get("CertificateAuthority") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        authority,
        &[("Status", "Status")],
    )))
}

/// Withdraw the activation of the private CA `physical_resource_id` names: the CA keeps the
/// certificate it was activated with and stops being able to use it, which is
/// `CertificateAuthorityStatus$DISABLED` ("the private CA is disabled and cannot issue
/// certificates").
///
/// A CA that service no longer holds (`ResourceNotFoundException`) is already past this, so
/// that answer is a success. So is a CA whose status cannot be changed to `DISABLED`
/// (`InvalidStateException`) — a CA that was never activated, or one already on its way out
/// — because the activation this withdraws is not in force either way. Every other refusal
/// is that service's own and is reported.
pub(crate) async fn deactivate_certificate_authority(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "CertificateAuthorityArn": physical_resource_id,
        "Status": CERTIFICATE_AUTHORITY_DISABLED,
    });
    match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.UpdateCertificateAuthority"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == RESOURCE_NOT_FOUND
                || refusal.code == ACMPCA_INVALID_STATE =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// The `CertificateAuthorityStatus` member a CA that holds its certificate but may not use
/// it is in.
pub(crate) const CERTIFICATE_AUTHORITY_DISABLED: &str = "DISABLED";

/// The error code Amazon Web Services Private CA answers when a CA's status does not allow
/// what was asked of it.
pub(crate) const ACMPCA_INVALID_STATE: &str = "InvalidStateException";

/// Whether Amazon Web Services Private CA has finished creating the CA a stack declared.
///
/// A created CA is reported `CREATING` — "Private CA is pending creation", the state in
/// which its certificate signing request is not available yet — so that is the state the
/// resource is still coming up in, `FAILED` ("Private CA could not be created") is one no
/// wait will bring it out of, and every other status is a CA that exists and can be acted
/// on. A CA that service no longer holds is in the same position as a failed one: nothing
/// is going to create it.
pub(crate) async fn certificate_authority_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let request = serde_json::json!({ "CertificateAuthorityArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.DescribeCertificateAuthority"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => {
            return Ok(Readiness::Failed(format!(
                "Amazon Web Services Private CA holds no certificate authority for \
                 {physical_resource_id}"
            )))
        }
        Err(refusal) => return Err(refusal),
    };
    let authority = reported.get("CertificateAuthority");
    let status = authority
        .and_then(|authority| authority.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match status {
        CERTIFICATE_AUTHORITY_CREATING => Ok(Readiness::Pending(
            CERTIFICATE_AUTHORITY_CREATING.to_string(),
        )),
        CERTIFICATE_AUTHORITY_FAILED => {
            // `FailureReason` is reported for a CA that failed, so it is quoted where that
            // service gave one and the status speaks for itself otherwise.
            let reason = authority
                .and_then(|authority| authority.get("FailureReason"))
                .and_then(Value::as_str);
            Ok(Readiness::Failed(match reason {
                Some(reason) => format!(
                    "Amazon Web Services Private CA reports certificate authority \
                     {physical_resource_id} as {CERTIFICATE_AUTHORITY_FAILED}: {reason}"
                ),
                None => format!(
                    "Amazon Web Services Private CA reports certificate authority \
                     {physical_resource_id} as {CERTIFICATE_AUTHORITY_FAILED}"
                ),
            }))
        }
        _ => Ok(Readiness::Ready),
    }
}

/// Delete the private CA a stack created.
///
/// A CA that service no longer holds (`ResourceNotFoundException`) is already in the state
/// the delete asks for, so that answer is a success. Every other refusal is that service's
/// own and is reported: a CA whose status does not allow a delete is its
/// `InvalidStateException`, which is a delete that really did not happen.
///
/// `PermanentDeletionTimeInDays` is left to that service's own default — "the default value
/// is 30" — because the restoration window is not something this type's declaration states.
pub(crate) async fn delete_certificate_authority(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "CertificateAuthorityArn": physical_resource_id });
    match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.DeleteCertificateAuthority"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The CA's configuration as Amazon Web Services Private CA reports it, for drift
/// detection. An ARN that service no longer holds a CA for (`ResourceNotFoundException`)
/// was deleted.
///
/// Only the members the read reports in the SAME shape the type declares them in are
/// reported, because a property the actual document does not carry is not evidence of
/// anything (`super::drift::differs`): the CA's kind, which the type publishes as `Type`,
/// its revocation configuration and its usage mode, each read off the CA that service
/// answers with. The key algorithm, the signing algorithm and the subject are members of
/// the reported `CertificateAuthorityConfiguration` rather than properties of the type, and
/// `Tags` is not part of that read at all.
pub(crate) async fn describe_certificate_authority(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "CertificateAuthorityArn": physical_resource_id });
    let reported = match call_json_rpc(
        ctx,
        ACMPCA_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ACMPCA_TARGET_PREFIX}.DescribeCertificateAuthority"),
        &request,
    )
    .await
    {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(authority) = reported.get("CertificateAuthority") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        authority,
        &[
            (ACMPCA_CA_TYPE, "Type"),
            ("RevocationConfiguration", "RevocationConfiguration"),
            ("UsageMode", "UsageMode"),
        ],
    )))
}

// ---------------------------------------------------------------------------
// Route 53 Resolver: AWS::Route53Resolver::ResolverEndpoint / ::ResolverRule /
// ::ResolverRuleAssociation
// ---------------------------------------------------------------------------

/// Route 53 Resolver's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: route53resolver`, service shape `Route53Resolver`), which speaks
/// `awsJson1_1`.
pub(crate) const ROUTE53RESOLVER_SIGNING_NAME: &str = "route53resolver";
pub(crate) const ROUTE53RESOLVER_TARGET_PREFIX: &str = "Route53Resolver";

/// The `CreatorRequestId` CloudFormation sends with a Route 53 Resolver create.
///
/// Each of the three creates REQUIRES one ("a unique string that identifies the request
/// and that allows failed requests to be retried without the risk of running the operation
/// twice"), and no `AWS::Route53Resolver::*` type publishes a property for it, so it is
/// CloudFormation's to supply. The resource's computed physical id is what it is built from:
/// unique per stack and logical id, and the SAME string if this deployment is attempted
/// again — which is exactly the retry that member exists for, so a re-ask resolves to the
/// resource the first attempt made instead of a second one.
pub(crate) fn resolver_creator_request_id(resource: &StoredResource) -> String {
    resource.physical_resource_id.clone()
}

/// One required property of a declared resource, or the refusal a declaration missing it
/// gets — the create could not identify what to make without it. Read by the arms whose
/// create needs a property the TYPE requires (the Route 53 Resolver ones, and the Amazon
/// EKS access entry's cluster and principal).
pub(crate) fn required_property(resource: &StoredResource, name: &str) -> Result<String, Refusal> {
    property(resource, name).ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no {name}", resource.resource_type),
    })
}

/// Call one Route 53 Resolver operation in process.
pub(crate) async fn call_route53resolver(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        ROUTE53RESOLVER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{ROUTE53RESOLVER_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// The evaluated property document as the create's request MEMBERS, with the
/// `CreatorRequestId` CloudFormation decides and — for a taggable type — the tags it gives
/// the resource.
///
/// All three types' properties ARE their creates' own members, spelled identically on both
/// sides, so the document is carried through rather than mapped member by member:
/// `Direction`, `IpAddresses`, `SecurityGroupIds`, `Protocols`, `ResolverEndpointType`,
/// `OutpostArn` and `PreferredInstanceType` for an endpoint; `RuleType`, `DomainName`,
/// `TargetIps`, `ResolverEndpointId` and `DelegationRecord` for a rule; `ResolverRuleId`
/// and `VPCId` for an association — each beside the `Name` all three publish
/// (`smithy_query(operation="CreateResolverEndpoint,CreateResolverRule,\
/// AssociateResolverRule")`).
///
/// `tags` is `None` for the one type that is not taggable, whose call takes no tags at all.
/// The `CreatorRequestId` is added by the two creates that MODEL one, since
/// `AssociateResolverRule` has no such member.
pub(crate) fn resolver_request(
    resource: &StoredResource,
    tags: Option<Vec<(String, String)>>,
) -> Map<String, Value> {
    let mut request = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    match tags {
        Some(tags) if !tags.is_empty() => {
            request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
        }
        Some(_) => {
            request.remove("Tags");
        }
        None => {}
    }
    request
}

/// What one Route 53 Resolver create answered: the id that service minted for the
/// resource, and the attributes read off its own answer.
///
/// `member` is the output member carrying the created resource (`ResolverEndpoint`,
/// `ResolverRule`, `ResolverRuleAssociation`), and `attributes` pairs each member of it
/// with the `Fn::GetAtt` attribute that type publishes for it. An answer carrying no `Id`
/// is reported as a failure: nothing addresses a Resolver resource without one, so it is
/// never recorded as created under an identifier nothing minted.
pub(crate) fn resolver_created(
    reported: &Value,
    operation: &str,
    member: &str,
    attributes: &[(&str, &str)],
) -> Result<Created, Refusal> {
    let detail = reported.get(member).cloned().unwrap_or(Value::Null);
    let id = detail
        .get("Id")
        .and_then(Value::as_str)
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("{operation} answered no {member}.Id"),
        })?
        .to_string();
    let mut reported_attributes = BTreeMap::new();
    for (reported_member, attribute) in attributes {
        if let Some(value) = detail.get(*reported_member) {
            reported_attributes.insert((*attribute).to_string(), attribute_text(value));
        }
    }
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: reported_attributes,
    })
}

/// Create the Resolver endpoint an `AWS::Route53Resolver::ResolverEndpoint` declares.
///
/// `Ref` on this resource returns the endpoint's ID — the `rslvr-out-…`/`rslvr-in-…` value
/// `CreateResolverEndpoint` MINTS, which is what `GetResolverEndpoint` takes and what a
/// Resolver rule's `ResolverEndpointId` names — so the create's own answer replaces the
/// computed physical id, and the attributes the type publishes are read off that answer
/// rather than rebuilt here.
pub(crate) async fn create_resolver_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = resolver_request(resource, Some(tags_of(&resource.properties, propagated)));
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(resolver_creator_request_id(resource)),
    );
    // Required by the type and by the create alike, so a declaration missing either is
    // refused here rather than with a request the owning service would have to reject.
    required_property(resource, "Direction")?;
    let reported = call_route53resolver(
        ctx,
        region,
        "CreateResolverEndpoint",
        &Value::Object(request),
    )
    .await?;
    resolver_created(
        &reported,
        "CreateResolverEndpoint",
        "ResolverEndpoint",
        &[
            ("Arn", "Arn"),
            ("Direction", "Direction"),
            ("HostVPCId", "HostVPCId"),
            ("IpAddressCount", "IpAddressCount"),
            ("Name", "Name"),
            ("OutpostArn", "OutpostArn"),
            ("PreferredInstanceType", "PreferredInstanceType"),
            ("ResolverEndpointType", "ResolverEndpointType"),
        ],
    )
}

/// Delete the Resolver endpoint a stack created, by the id its create minted. An endpoint
/// Route 53 Resolver no longer holds (`ResourceNotFoundException`) is already in the state
/// the delete asks for.
pub(crate) async fn delete_resolver_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "ResolverEndpointId": physical_resource_id });
    match call_route53resolver(ctx, region, "DeleteResolverEndpoint", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The endpoint's configuration as Route 53 Resolver reports it, for drift detection. An
/// id it no longer holds an endpoint for (`ResourceNotFoundException`) was deleted.
///
/// Only the members `GetResolverEndpoint` reports in the SAME shape the type declares them
/// in are compared, because a property the actual document does not carry is not evidence
/// of anything ([`super::drift::differs`]): the declared `IpAddresses` and
/// `SecurityGroupIds` are not members of the reported endpoint at all.
pub(crate) async fn describe_resolver_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ResolverEndpointId": physical_resource_id });
    let reported = match call_route53resolver(ctx, region, "GetResolverEndpoint", &request).await {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(endpoint) = reported.get("ResolverEndpoint") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        endpoint,
        &[
            ("Direction", "Direction"),
            ("Name", "Name"),
            ("OutpostArn", "OutpostArn"),
            ("PreferredInstanceType", "PreferredInstanceType"),
            ("ResolverEndpointType", "ResolverEndpointType"),
        ],
    )))
}

/// Create the Resolver rule an `AWS::Route53Resolver::ResolverRule` declares.
///
/// `Ref` on this resource returns the rule's ID — the `rslvr-rr-…` value
/// `CreateResolverRule` MINTS — so that answer replaces the computed physical id, exactly
/// as it does for the endpoint above.
pub(crate) async fn create_resolver_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = resolver_request(resource, Some(tags_of(&resource.properties, propagated)));
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(resolver_creator_request_id(resource)),
    );
    required_property(resource, "RuleType")?;
    // `TargetAddress$Port` is the one INTEGER member among these types' properties, and a
    // template may write a number as a quoted scalar (`Port: "53"`), which the declaration
    // carries through as text. The create takes the integer the model declares, so each
    // declared port is read as the whole number it is ([`whole_number`]) — a value that is
    // not one is left exactly as written, for the owning service to refuse.
    if let Some(Value::Array(targets)) = request.get_mut("TargetIps") {
        for target in targets.iter_mut() {
            let Value::Object(address) = target else {
                continue;
            };
            if let Some(port) = whole_number(address.get("Port")) {
                address.insert("Port".to_string(), Value::from(port));
            }
        }
    }
    let reported =
        call_route53resolver(ctx, region, "CreateResolverRule", &Value::Object(request)).await?;
    resolver_created(
        &reported,
        "CreateResolverRule",
        "ResolverRule",
        &[
            ("Arn", "Arn"),
            ("DomainName", "DomainName"),
            ("Name", "Name"),
            ("ResolverEndpointId", "ResolverEndpointId"),
        ],
    )
}

/// Delete the Resolver rule a stack created, by the id its create minted. A rule
/// Route 53 Resolver no longer holds is already in the state the delete asks for.
///
/// "Before you can delete a Resolver rule, you must disassociate it from all the VPCs that
/// you associated the Resolver rule with": a stack that declares both deletes its
/// association FIRST, because [`super::provisioning::delete`] deletes a stack's resources
/// in the reverse of the order they were created in, and the association depends on the
/// rule.
pub(crate) async fn delete_resolver_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "ResolverRuleId": physical_resource_id });
    match call_route53resolver(ctx, region, "DeleteResolverRule", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The rule's configuration as Route 53 Resolver reports it, for drift detection.
///
/// `DomainName` is deliberately not compared: Resolver reports a rule's domain name as the
/// fully-qualified name whatever form the declaration wrote it in, so comparing it would
/// read that canonicalisation as drift. `TargetIps` is not compared either — the reported
/// addresses carry the port and protocol the service filled in, which the declaration
/// leaves out.
pub(crate) async fn describe_resolver_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ResolverRuleId": physical_resource_id });
    let reported = match call_route53resolver(ctx, region, "GetResolverRule", &request).await {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(rule) = reported.get("ResolverRule") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        rule,
        &[
            ("Name", "Name"),
            ("RuleType", "RuleType"),
            ("ResolverEndpointId", "ResolverEndpointId"),
            ("DelegationRecord", "DelegationRecord"),
        ],
    )))
}

/// Associate the rule an `AWS::Route53Resolver::ResolverRuleAssociation` declares with the
/// VPC it names.
///
/// `Ref` on this resource returns the association's ID — the `rslvr-rrassoc-…` value
/// `AssociateResolverRule` MINTS, which is what `GetResolverRuleAssociation` takes — so
/// that answer replaces the computed physical id. The type publishes no `Tags` property and
/// the call takes none, so the association carries no propagated tag.
pub(crate) async fn associate_resolver_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let request = resolver_request(resource, None);
    required_property(resource, "ResolverRuleId")?;
    required_property(resource, "VPCId")?;
    let reported = call_route53resolver(
        ctx,
        region,
        "AssociateResolverRule",
        &Value::Object(request),
    )
    .await?;
    resolver_created(
        &reported,
        "AssociateResolverRule",
        "ResolverRuleAssociation",
        &[
            ("Name", "Name"),
            ("ResolverRuleId", "ResolverRuleId"),
            ("VPCId", "VPCId"),
        ],
    )
}

/// Remove the association a stack created.
///
/// `DisassociateResolverRule` addresses an association by the PAIR it was made from rather
/// than by its own id, so this takes the resource's record: the rule id and the VPC id its
/// declaration carries — the rule id being the one Route 53 Resolver minted, since that is
/// what the declaration's reference to the rule resolved to. A pair that service holds no
/// association for is already in the state the delete asks for.
pub(crate) async fn disassociate_resolver_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "ResolverRuleId": required_property(resource, "ResolverRuleId")?,
        "VPCId": required_property(resource, "VPCId")?,
    });
    match call_route53resolver(ctx, region, "DisassociateResolverRule", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The association's configuration as Route 53 Resolver reports it, for drift detection.
/// An id it no longer holds an association for (`ResourceNotFoundException`) was deleted.
pub(crate) async fn describe_resolver_rule_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ResolverRuleAssociationId": physical_resource_id });
    let reported =
        match call_route53resolver(ctx, region, "GetResolverRuleAssociation", &request).await {
            Ok(reported) => reported,
            Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    let Some(association) = reported.get("ResolverRuleAssociation") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        association,
        &[
            ("Name", "Name"),
            ("ResolverRuleId", "ResolverRuleId"),
            ("VPCId", "VPCId"),
        ],
    )))
}

// ---------------------------------------------------------------------------
// Route 53 Resolver DNS Firewall: AWS::Route53Resolver::FirewallDomainList /
// ::FirewallRuleGroup / ::FirewallRuleGroupAssociation
// ---------------------------------------------------------------------------

/// The `AWS::Route53Resolver::FirewallDomainList` property that is no member of
/// `CreateFirewallDomainList`: the domains the list holds, which that call does not take at
/// all — it "creates an empty firewall domain list" — so they travel in the
/// `UpdateFirewallDomains` the arm makes next.
pub(crate) const FIREWALL_DOMAINS: &str = "Domains";

/// The other property that is no member of the create: a file of domains to import. The
/// import is `ImportFirewallDomains`' own, which this emulator has not written, so a
/// declaration naming one is REFUSED rather than reported as a list that silently holds
/// nothing.
pub(crate) const FIREWALL_DOMAIN_FILE_URL: &str = "DomainFileUrl";

/// The `AWS::Route53Resolver::FirewallRuleGroup` property that is no member of
/// `CreateFirewallRuleGroup`: the rules of the group, each of which is one
/// `CreateFirewallRule` against the group the create made ("you can add rules to the new
/// rule group by calling CreateFirewallRule").
pub(crate) const FIREWALL_RULES: &str = "FirewallRules";

/// The `FirewallDomainUpdateOperation` that adds the declared domains to the list,
/// `UpdateFirewallDomains`' own `ADD`: "Add the domains to the ones that are already in the
/// domain list" — a list CloudFormation has just created being empty, the ADD leaves it
/// holding exactly what the template declares.
pub(crate) const FIREWALL_DOMAINS_ADD: &str = "ADD";

/// The members of a declared `FirewallRule` that `CreateFirewallRule` takes as WHOLE
/// NUMBERS: a template may write an integer as a quoted scalar (`Priority: "100"`), which
/// the evaluated declaration carries through as text, and the create takes the integer the
/// model declares — the same transformation [`create_resolver_rule`] makes for a target's
/// port.
pub(crate) const FIREWALL_RULE_NUMBERS: &[&str] = &["Priority", "BlockOverrideTtl"];

/// Create the DNS Firewall domain list an `AWS::Route53Resolver::FirewallDomainList`
/// declares, and put the domains it declares in it.
///
/// `Ref` on this resource returns the list's ID — the `rslvr-fdl-…` value
/// `CreateFirewallDomainList` MINTS, which is what `GetFirewallDomainList` takes and what a
/// rule's `FirewallDomainListId` names — so the create's own answer replaces the computed
/// physical id. `Domains` is no member of that create, so the declared domains are added by
/// the one call that takes them; a list whose domains could not be stored is deleted again,
/// so a create the stack reports as failed leaves nothing behind.
pub(crate) async fn create_firewall_domain_list(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    if property(resource, FIREWALL_DOMAIN_FILE_URL).is_some() {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not implement the {FIREWALL_DOMAIN_FILE_URL} property of {}",
                resource.resource_type
            ),
        });
    }
    let mut request = resolver_request(resource, Some(tags_of(&resource.properties, propagated)));
    let domains = request.remove(FIREWALL_DOMAINS);
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(resolver_creator_request_id(resource)),
    );
    let reported = call_route53resolver(
        ctx,
        region,
        "CreateFirewallDomainList",
        &Value::Object(request),
    )
    .await?;
    let created = resolver_created(
        &reported,
        "CreateFirewallDomainList",
        "FirewallDomainList",
        FIREWALL_DOMAIN_LIST_ATTRIBUTES,
    )?;
    let Some(id) = created.physical_resource_id.clone() else {
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateFirewallDomainList answered no FirewallDomainList.Id".to_string(),
        });
    };
    if let Some(Value::Array(domains)) = domains {
        if !domains.is_empty() {
            let update = serde_json::json!({
                "FirewallDomainListId": id,
                "Operation": FIREWALL_DOMAINS_ADD,
                "Domains": Value::Array(domains),
            });
            if let Err(refusal) =
                call_route53resolver(ctx, region, "UpdateFirewallDomains", &update).await
            {
                let _ = delete_firewall_domain_list(ctx, region, &id).await;
                return Err(refusal);
            }
        }
    }
    Ok(created)
}

/// The members `GetFirewallDomainList` reports, paired with the `Fn::GetAtt` attribute
/// `AWS::Route53Resolver::FirewallDomainList` publishes for each: that type's attributes are
/// the reported list's own members, spelled identically.
pub(crate) const FIREWALL_DOMAIN_LIST_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "Arn"),
    ("CreationTime", "CreationTime"),
    ("CreatorRequestId", "CreatorRequestId"),
    ("DomainCount", "DomainCount"),
    ("Id", "Id"),
    ("ManagedOwnerName", "ManagedOwnerName"),
    ("ModificationTime", "ModificationTime"),
    ("Status", "Status"),
    ("StatusMessage", "StatusMessage"),
];

/// Delete the domain list a stack created, by the id its create minted. A list Route 53
/// Resolver no longer holds (`ResourceNotFoundException`) is already in the state the delete
/// asks for.
pub(crate) async fn delete_firewall_domain_list(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "FirewallDomainListId": physical_resource_id });
    match call_route53resolver(ctx, region, "DeleteFirewallDomainList", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The domain list's configuration as Route 53 Resolver reports it, for drift detection. An
/// id it no longer holds a list for (`ResourceNotFoundException`) was deleted.
///
/// Only `Name` is compared: the reported list carries no `Domains` member at all — those are
/// read with `ListFirewallDomains` — and a property the actual document does not carry is not
/// evidence of anything ([`super::drift::differs`]).
pub(crate) async fn describe_firewall_domain_list(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "FirewallDomainListId": physical_resource_id });
    let reported =
        match call_route53resolver(ctx, region, "GetFirewallDomainList", &request).await {
            Ok(reported) => reported,
            Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    let Some(domain_list) = reported.get("FirewallDomainList") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        domain_list,
        &[("Name", "Name")],
    )))
}

/// Create the DNS Firewall rule group an `AWS::Route53Resolver::FirewallRuleGroup` declares,
/// with each rule it declares.
///
/// `Ref` on this resource returns the group's ID — the `rslvr-frg-…` value
/// `CreateFirewallRuleGroup` MINTS, which is what `GetFirewallRuleGroup` takes and what an
/// association's `FirewallRuleGroupId` names — so the create's own answer replaces the
/// computed physical id. `FirewallRules` is no member of that create ("creates an EMPTY DNS
/// Firewall rule group"), so each declared rule is one `CreateFirewallRule` against the
/// group; a group whose rules could not be created is deleted again, so a create the stack
/// reports as failed leaves nothing behind.
///
/// A declared rule carries no name — the type's `FirewallRule` property publishes none — and
/// `CreateFirewallRule` requires one, so the name is CloudFormation's to supply: the group's
/// own physical id and the rule's priority, which is unique within the group, so the name is
/// unique within it too and is the SAME string if this deployment is attempted again.
pub(crate) async fn create_firewall_rule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = resolver_request(resource, Some(tags_of(&resource.properties, propagated)));
    let rules = request.remove(FIREWALL_RULES);
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(resolver_creator_request_id(resource)),
    );
    let reported = call_route53resolver(
        ctx,
        region,
        "CreateFirewallRuleGroup",
        &Value::Object(request),
    )
    .await?;
    let created = resolver_created(
        &reported,
        "CreateFirewallRuleGroup",
        "FirewallRuleGroup",
        FIREWALL_RULE_GROUP_ATTRIBUTES,
    )?;
    let Some(id) = created.physical_resource_id.clone() else {
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateFirewallRuleGroup answered no FirewallRuleGroup.Id".to_string(),
        });
    };
    if let Some(Value::Array(rules)) = rules {
        for (position, rule) in rules.iter().enumerate() {
            let request = firewall_rule_request(&id, resource, position, rule);
            if let Err(refusal) =
                call_route53resolver(ctx, region, "CreateFirewallRule", &request).await
            {
                let _ = delete_firewall_rule_group(ctx, region, &id).await;
                return Err(refusal);
            }
        }
    }
    Ok(created)
}

/// The `CreateFirewallRule` request one declared rule of a rule group makes.
///
/// The declared members ARE that call's own, spelled identically (`Action`, `BlockResponse`,
/// the three `BlockOverride*`, `FirewallDomainListId`, `FirewallDomainRedirectionAction`,
/// `Priority`, `Qtype`, `DnsThreatProtection`, `ConfidenceThreshold` and
/// `FirewallRuleType`), so the declaration is carried through with the two members
/// CloudFormation decides added: the rule's name and the `CreatorRequestId`, both built from
/// the group's physical id so a second attempt at this deployment re-asks for the same rule
/// instead of a duplicate.
pub(crate) fn firewall_rule_request(
    firewall_rule_group_id: &str,
    resource: &StoredResource,
    position: usize,
    rule: &Value,
) -> Value {
    let mut request = match rule {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    for member in FIREWALL_RULE_NUMBERS {
        if let Some(number) = whole_number(request.get(*member)) {
            request.insert((*member).to_string(), Value::from(number));
        }
    }
    let suffix = whole_number(request.get("Priority"))
        .map(|priority| priority.to_string())
        .unwrap_or_else(|| position.to_string());
    request.insert(
        "FirewallRuleGroupId".to_string(),
        Value::String(firewall_rule_group_id.to_string()),
    );
    request.insert(
        "Name".to_string(),
        Value::String(format!("{firewall_rule_group_id}-{suffix}")),
    );
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(format!(
            "{}-{suffix}",
            resolver_creator_request_id(resource)
        )),
    );
    Value::Object(request)
}

/// The members `GetFirewallRuleGroup` reports, paired with the `Fn::GetAtt` attribute
/// `AWS::Route53Resolver::FirewallRuleGroup` publishes for each: that type's attributes are
/// the reported group's own members, spelled identically.
pub(crate) const FIREWALL_RULE_GROUP_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "Arn"),
    ("CreationTime", "CreationTime"),
    ("CreatorRequestId", "CreatorRequestId"),
    ("Id", "Id"),
    ("ModificationTime", "ModificationTime"),
    ("OwnerId", "OwnerId"),
    ("RuleCount", "RuleCount"),
    ("ShareStatus", "ShareStatus"),
    ("Status", "Status"),
    ("StatusMessage", "StatusMessage"),
];

/// Delete the rule group a stack created, by the id its create minted. A group Route 53
/// Resolver no longer holds is already in the state the delete asks for.
///
/// The rules the create made in the group go FIRST: a group that still holds a rule still
/// filters DNS traffic, so Route 53 Resolver refuses to delete it (`ConflictException`) until
/// its rules are gone — the same order a client tearing a group down by hand follows. The
/// rules are read back off the group rather than off the declaration, so a rule an update
/// added is removed too.
///
/// A group a VPC is still filtered by cannot be deleted either, so a stack that declares both
/// removes the association FIRST, because [`super::provisioning::delete`] deletes a stack's
/// resources in the reverse of the order they were created in and the association depends on
/// the group.
pub(crate) async fn delete_firewall_rule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    delete_firewall_rule_group_rules(ctx, region, physical_resource_id).await?;
    let request = serde_json::json!({ "FirewallRuleGroupId": physical_resource_id });
    match call_route53resolver(ctx, region, "DeleteFirewallRuleGroup", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete every rule the rule group `physical_resource_id` holds, each by the match source
/// `DeleteFirewallRule` names one by: the group, the domain list or threat-protection id the
/// rule matches on, and the query type that tells apart two rules of the group matching the
/// same source.
///
/// The rules are walked page by page, as `ListFirewallRules` reports them; a group the service
/// no longer holds has no rules to delete, and a rule that is already gone leaves nothing to
/// do.
async fn delete_firewall_rule_group_rules(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let mut next_token: Option<String> = None;
    loop {
        let mut listing = Map::new();
        listing.insert(
            "FirewallRuleGroupId".to_string(),
            Value::String(physical_resource_id.to_string()),
        );
        // The maximum `ListFirewallRules` admits, so the walk makes as few calls as the
        // model allows.
        listing.insert("MaxResults".to_string(), Value::from(100));
        if let Some(token) = &next_token {
            listing.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let listed = match call_route53resolver(
            ctx,
            region,
            "ListFirewallRules",
            &Value::Object(listing),
        )
        .await
        {
            Ok(listed) => listed,
            Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(()),
            Err(refusal) => return Err(refusal),
        };
        for rule in rules_in_delete_order(
            listed
                .get("FirewallRules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        ) {
            let mut request = Map::new();
            request.insert(
                "FirewallRuleGroupId".to_string(),
                Value::String(physical_resource_id.to_string()),
            );
            for member in FIREWALL_RULE_MATCH_SOURCE {
                if let Some(value) = rule.get(*member) {
                    request.insert((*member).to_string(), value.clone());
                }
            }
            match call_route53resolver(ctx, region, "DeleteFirewallRule", &Value::Object(request))
                .await
            {
                Ok(_) => {}
                Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => {}
                Err(refusal) => return Err(refusal),
            }
        }
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return Ok(());
        }
    }
}

/// The members of a reported `FirewallRule` that name WHICH rule of a group a delete is
/// about: the source it matches on, and the query type that scopes it.
const FIREWALL_RULE_MATCH_SOURCE: &[&str] = &[
    "FirewallDomainListId",
    "FirewallThreatProtectionId",
    "Qtype",
];

/// `rules` in the order their deletes are issued: the rules that name a `Qtype` FIRST.
///
/// A delete names a rule by the source it matches, and the `Qtype` is what tells apart two
/// rules of one group matching the SAME source — so a delete that names no `Qtype` is only
/// unambiguous while one rule of that source is left. Taking the `Qtype`-bearing rules out
/// first leaves exactly that, whatever order the group reports its rules in.
fn rules_in_delete_order(rules: Vec<Value>) -> Vec<Value> {
    let (scoped, unscoped): (Vec<Value>, Vec<Value>) = rules
        .into_iter()
        .partition(|rule| rule.get("Qtype").is_some());
    scoped.into_iter().chain(unscoped).collect()
}

/// The rule group's configuration as Route 53 Resolver reports it, for drift detection.
///
/// Only `Name` is compared: the reported group carries no `FirewallRules` member — those are
/// read with `ListFirewallRules` — and a property the actual document does not carry is not
/// evidence of anything.
pub(crate) async fn describe_firewall_rule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "FirewallRuleGroupId": physical_resource_id });
    let reported = match call_route53resolver(ctx, region, "GetFirewallRuleGroup", &request).await {
        Ok(reported) => reported,
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(rule_group) = reported.get("FirewallRuleGroup") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        rule_group,
        &[("Name", "Name")],
    )))
}

/// Associate the rule group an `AWS::Route53Resolver::FirewallRuleGroupAssociation` declares
/// with the VPC it names.
///
/// `Ref` on this resource returns the association's ID — the `rslvr-frgassoc-…` value
/// `AssociateFirewallRuleGroup` MINTS, which is what `GetFirewallRuleGroupAssociation` and
/// `DisassociateFirewallRuleGroup` take — so that answer replaces the computed physical id.
/// The declared properties are that call's own members, spelled identically
/// (`FirewallRuleGroupId`, `VpcId`, `Priority`, `Name`, `MutationProtection`), with the
/// `CreatorRequestId` CloudFormation decides and the tags it gives the resource.
pub(crate) async fn associate_firewall_rule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = resolver_request(resource, Some(tags_of(&resource.properties, propagated)));
    if let Some(priority) = whole_number(request.get("Priority")) {
        request.insert("Priority".to_string(), Value::from(priority));
    }
    request.insert(
        "CreatorRequestId".to_string(),
        Value::String(resolver_creator_request_id(resource)),
    );
    // Required by the type and by the call alike, so a declaration missing one is refused
    // here rather than with a request the owning service would have to reject.
    required_property(resource, "FirewallRuleGroupId")?;
    required_property(resource, "VpcId")?;
    let reported = call_route53resolver(
        ctx,
        region,
        "AssociateFirewallRuleGroup",
        &Value::Object(request),
    )
    .await?;
    resolver_created(
        &reported,
        "AssociateFirewallRuleGroup",
        "FirewallRuleGroupAssociation",
        FIREWALL_RULE_GROUP_ASSOCIATION_ATTRIBUTES,
    )
}

/// The members `GetFirewallRuleGroupAssociation` reports, paired with the `Fn::GetAtt`
/// attribute `AWS::Route53Resolver::FirewallRuleGroupAssociation` publishes for each.
pub(crate) const FIREWALL_RULE_GROUP_ASSOCIATION_ATTRIBUTES: &[(&str, &str)] = &[
    ("Arn", "Arn"),
    ("CreationTime", "CreationTime"),
    ("CreatorRequestId", "CreatorRequestId"),
    ("Id", "Id"),
    ("ManagedOwnerName", "ManagedOwnerName"),
    ("ModificationTime", "ModificationTime"),
    ("Status", "Status"),
    ("StatusMessage", "StatusMessage"),
];

/// Remove the association a stack created, by the id that service minted for it. An
/// association Route 53 Resolver no longer holds is already in the state the delete asks
/// for.
pub(crate) async fn disassociate_firewall_rule_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request =
        serde_json::json!({ "FirewallRuleGroupAssociationId": physical_resource_id });
    match call_route53resolver(ctx, region, "DisassociateFirewallRuleGroup", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The association's configuration as Route 53 Resolver reports it, for drift detection. An
/// id it no longer holds an association for (`ResourceNotFoundException`) was deleted.
pub(crate) async fn describe_firewall_rule_group_association(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request =
        serde_json::json!({ "FirewallRuleGroupAssociationId": physical_resource_id });
    let reported =
        match call_route53resolver(ctx, region, "GetFirewallRuleGroupAssociation", &request).await
        {
            Ok(reported) => reported,
            Err(refusal) if refusal.code == RESOURCE_NOT_FOUND => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
    let Some(association) = reported.get("FirewallRuleGroupAssociation") else {
        return Ok(Some(Value::Object(Map::new())));
    };
    Ok(Some(reported_as_properties(
        association,
        &[
            ("FirewallRuleGroupId", "FirewallRuleGroupId"),
            ("VpcId", "VpcId"),
            ("Name", "Name"),
            ("MutationProtection", "MutationProtection"),
        ],
    )))
}

// ---------------------------------------------------------------------------
// AWS Elastic Beanstalk: AWS::ElasticBeanstalk::Application,
// AWS::ElasticBeanstalk::ApplicationVersion, AWS::ElasticBeanstalk::Environment
// ---------------------------------------------------------------------------

/// Elastic Beanstalk's signing name and API version, from its model
/// (`sigv4Name: elasticbeanstalk`, service shape `AWSElasticBeanstalkService`,
/// `version: 2010-12-01`), which speaks `awsQuery` — so its calls ride on [`call_query`]
/// rather than on [`call_json_rpc`].
pub(crate) const ELASTIC_BEANSTALK_SIGNING_NAME: &str = "elasticbeanstalk";
pub(crate) const ELASTIC_BEANSTALK_API_VERSION: &str = "2010-12-01";

/// The members `CreateApplication` takes, from Elastic Beanstalk's own model
/// (`smithy_query(query="operation", operation="CreateApplication")`).
///
/// `AWS::ElasticBeanstalk::Application` publishes `ApplicationName`, `Description` and
/// `ResourceLifecycleConfig`, which are three of those members spelled identically; the
/// name is the one value CloudFormation decides, so the arm pushes it and
/// [`APPLICATION_PROPERTIES_NOT_SENT`] keeps the walk from sending it twice. The type
/// publishes no `Tags` property, so it is not taggable and no stack tag is propagated to
/// it.
pub(crate) const CREATE_APPLICATION_MEMBERS: &[(&str, QueryMember)] = &[
    ("ApplicationName", QueryMember::Scalar),
    ("Description", QueryMember::Scalar),
    ("ResourceLifecycleConfig", QueryMember::Structure),
];

/// The `AWS::ElasticBeanstalk::Application` property the arm pushes itself: the
/// application's name is this resource's physical id.
pub(crate) const APPLICATION_PROPERTIES_NOT_SENT: &[&str] = &["ApplicationName"];

/// The members `CreateApplicationVersion` takes, from Elastic Beanstalk's own model.
///
/// `AWS::ElasticBeanstalk::ApplicationVersion` publishes `ApplicationName`, `Description`
/// and `SourceBundle` — the application the version belongs to, and the Amazon S3 archive
/// it is built from, each spelled as the create spells it. The `VersionLabel` is not a
/// property of the type at all: "Ref returns the auto-generated version label", so it is
/// the physical id CloudFormation's naming rules computed and the arm pushes it.
pub(crate) const CREATE_APPLICATION_VERSION_MEMBERS: &[(&str, QueryMember)] = &[
    ("ApplicationName", QueryMember::Scalar),
    ("Description", QueryMember::Scalar),
    ("SourceBundle", QueryMember::Structure),
];

/// The members `CreateEnvironment` takes, from Elastic Beanstalk's own model, with each
/// list's element name the Smithy default for a list member that fixes none (`member`).
///
/// `AWS::ElasticBeanstalk::Environment` publishes `ApplicationName`, `CNAMEPrefix`,
/// `Description`, `EnvironmentName`, `OperationsRole`, `OptionSettings`, `PlatformArn`,
/// `SolutionStackName`, `Tags`, `TemplateName`, `Tier` and `VersionLabel` — every one of
/// them a member of the create under the same name. The two the arm pushes itself are in
/// [`ENVIRONMENT_PROPERTIES_NOT_SENT`].
pub(crate) const CREATE_ENVIRONMENT_MEMBERS: &[(&str, QueryMember)] = &[
    ("ApplicationName", QueryMember::Scalar),
    ("CNAMEPrefix", QueryMember::Scalar),
    ("Description", QueryMember::Scalar),
    ("EnvironmentName", QueryMember::Scalar),
    ("OperationsRole", QueryMember::Scalar),
    ("OptionSettings", QueryMember::List("member")),
    ("PlatformArn", QueryMember::Scalar),
    ("SolutionStackName", QueryMember::Scalar),
    ("Tags", QueryMember::List("member")),
    ("TemplateName", QueryMember::Scalar),
    ("Tier", QueryMember::Structure),
    ("VersionLabel", QueryMember::Scalar),
];

/// The `AWS::ElasticBeanstalk::Environment` properties the arm pushes itself: the
/// environment's name, which is this resource's physical id, and its tags, which reach
/// the create merged with the ones CloudFormation gives the resource ([`tags_of`]).
pub(crate) const ENVIRONMENT_PROPERTIES_NOT_SENT: &[&str] = &["EnvironmentName", "Tags"];

/// Create the application an `AWS::ElasticBeanstalk::Application` declares.
pub(crate) async fn create_eb_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_APPLICATION_MEMBERS,
        &[],
        APPLICATION_PROPERTIES_NOT_SENT,
        ELASTIC_BEANSTALK_APPLICATION,
    )?;
    parameters.push((
        "ApplicationName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "CreateApplication",
        &parameters,
    )
    .await?;
    // "Ref returns the Elastic Beanstalk application name", which is the physical id the
    // application was created under, and the type publishes no `Fn::GetAtt` attribute.
    Ok(Created::default())
}

/// Create the application version an `AWS::ElasticBeanstalk::ApplicationVersion`
/// declares, under the version label CloudFormation decided for it.
pub(crate) async fn create_eb_application_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_APPLICATION_VERSION_MEMBERS,
        &[],
        &[],
        ELASTIC_BEANSTALK_APPLICATION_VERSION,
    )?;
    parameters.push((
        "VersionLabel".to_string(),
        resource.physical_resource_id.clone(),
    ));
    call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "CreateApplicationVersion",
        &parameters,
    )
    .await?;
    Ok(Created::default())
}

/// Create the environment an `AWS::ElasticBeanstalk::Environment` declares, through
/// Elastic Beanstalk's own `CreateEnvironment`.
///
/// The environment is created with the tags CloudFormation gives it beside the declared
/// ones, as the `Tags` structure list the create takes (`Tag` spells `Key`/`Value`), and
/// under the name CloudFormation decided — this resource's physical id, which is what
/// "Ref returns the Elastic Beanstalk environment name" resolves to. `EndpointURL` is the
/// one attribute the type publishes, so it is recorded from what Elastic Beanstalk
/// answered rather than computed; an environment that reports none carries no such
/// attribute.
pub(crate) async fn create_eb_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_ENVIRONMENT_MEMBERS,
        &[],
        ENVIRONMENT_PROPERTIES_NOT_SENT,
        ELASTIC_BEANSTALK_ENVIRONMENT,
    )?;
    parameters.push((
        "EnvironmentName".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "member",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "CreateEnvironment",
        &parameters,
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(endpoint) =
        xml_element(&answered, "EndpointURL").filter(|endpoint| !endpoint.is_empty())
    {
        attributes.insert("EndpointURL".to_string(), endpoint);
    }
    Ok(Created::attributes(attributes))
}

/// Delete the application a stack created, addressed by its name — this resource's
/// physical id.
///
/// `DeleteApplication` models no not-found error (its one modelled error is
/// `OperationInProgressException`), so "already gone" is decided by READING the
/// application back rather than off a code that is also what another refusal carries.
pub(crate) async fn delete_eb_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "ApplicationName".to_string(),
        physical_resource_id.to_string(),
    )];
    let refusal = match call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "DeleteApplication",
        &parameters,
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    match describe_eb_application(ctx, region, physical_resource_id).await {
        // Elastic Beanstalk does not have the application, which is the state the delete
        // asked for.
        Ok(None) => Ok(()),
        // It still has it, or the read said nothing about it: the refusal stands rather
        // than being reported as a deletion nothing carried out.
        _ => Err(refusal),
    }
}

/// Whether Elastic Beanstalk has finished removing the application a stack deleted.
///
/// `DeleteApplication` "deletes the specified application along with all associated versions
/// and configurations" and ANSWERS BEFORE it has: the application goes on being described
/// while the deletion runs, so the call being accepted is not evidence the name is free. The
/// application is gone only once that service's own read no longer reports it — its answer,
/// never a span this crate assumed — and the stack's deletion is held open until then
/// (`crate::state::stabilising`), because CloudFormation does not report an
/// operation complete while a resource it deleted is still there. Elastic Beanstalk reports
/// no status of its own for an application, so what the resource is still in is
/// CloudFormation's own deleting state.
pub(crate) async fn eb_application_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match describe_eb_application(ctx, region, physical_resource_id).await? {
        Some(_) => Ok(Removal::Pending(
            crate::state::resource_status::DELETE_IN_PROGRESS.to_string(),
        )),
        None => Ok(Removal::Gone),
    }
}

/// Delete the application version a stack created: its label is this resource's physical
/// id, and the application it belongs to is on the resource's own declaration.
///
/// The source bundle is NOT deleted with it — `DeleteSourceBundle` is not a property of
/// the type, so the archive the template pointed at is left where the client put it.
pub(crate) async fn delete_eb_application_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let application = eb_application_of(resource);
    let parameters = vec![
        ("ApplicationName".to_string(), application.clone()),
        (
            "VersionLabel".to_string(),
            resource.physical_resource_id.clone(),
        ),
    ];
    let refusal = match call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "DeleteApplicationVersion",
        &parameters,
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    match describe_eb_application_version(ctx, region, resource).await {
        Ok(None) => Ok(()),
        _ => Err(refusal),
    }
}

/// How often this module checks whether an accepted environment termination has finished,
/// and how many times.
///
/// The product is the longest a stack's deletion waits for ONE environment to go: an
/// upper bound on the wait, not a duration anything is owed. Elastic Beanstalk reports a
/// terminated environment as `Terminating` and removes it from the live set a moment
/// later (its own transition span), so the check is far shorter than that moment and the
/// whole wait is a small multiple of it — enough that a loaded machine still gets there
/// after that span has passed, and bounded so a `DeleteStack` of several environments
/// cannot hold its request open.
/// The wait matters because the application the environment belongs to cannot be deleted
/// while it still has a running environment, and that application is the next resource
/// the same stack deletes.
pub(crate) const ENVIRONMENT_TERMINATION_CHECK_INTERVAL: ::std::time::Duration =
    ::std::time::Duration::from_millis(25);
pub(crate) const ENVIRONMENT_TERMINATION_CHECKS: usize = 240;

/// The code a refusal carries where a resource the owning service ACCEPTED the delete of
/// had not reached the deleted state by the time the bounded wait ran out:
/// CloudFormation's own resource-provider handler code for a resource that did not
/// stabilize. It is not a code any AWS service answers, so it is never mistaken for one
/// of theirs.
pub(crate) const NOT_STABILIZED: &str = "NotStabilized";

/// Terminate the environment a stack created, addressed by its name — this resource's
/// physical id — and wait until Elastic Beanstalk no longer holds it among the live
/// environments.
///
/// `TerminateEnvironment` models no not-found error, so "already gone" is decided by
/// READING the environment back, exactly as the application's delete does.
///
/// The wait is what makes this delete USABLE by the rest of the stack's deletion: the
/// application this environment belongs to is the next resource deleted, and Elastic
/// Beanstalk refuses to delete an application that still has a running environment. So a
/// wait that runs out while the environment is still live is reported HERE, naming this
/// resource and the termination that did not finish, rather than left to surface as a
/// confusing refusal of the application's delete. Only the read that says the environment
/// is GONE ends the wait successfully: a read that answers nothing about it is not
/// evidence the termination finished.
pub(crate) async fn terminate_eb_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "EnvironmentName".to_string(),
        physical_resource_id.to_string(),
    )];
    let terminated = call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        "TerminateEnvironment",
        &parameters,
    )
    .await;
    if let Err(refusal) = terminated {
        return match describe_eb_environment(ctx, region, physical_resource_id).await {
            Ok(None) => Ok(()),
            _ => Err(refusal),
        };
    }
    // The termination was ACCEPTED; it finishes a moment later.
    for _ in 0..ENVIRONMENT_TERMINATION_CHECKS {
        if matches!(
            describe_eb_environment(ctx, region, physical_resource_id).await,
            Ok(None)
        ) {
            return Ok(());
        }
        ::tokio::time::sleep(ENVIRONMENT_TERMINATION_CHECK_INTERVAL).await;
    }
    Err(Refusal {
        code: NOT_STABILIZED.to_string(),
        message: format!(
            "the termination of environment {physical_resource_id} had not finished within the \
             wait this deletion gives it, so the environment is still running"
        ),
    })
}

/// The application an `AWS::ElasticBeanstalk::ApplicationVersion` belongs to: the
/// `ApplicationName` its declaration names, which is the other half of the pair Elastic
/// Beanstalk keys an application version by.
pub(crate) fn eb_application_of(resource: &StoredResource) -> String {
    resource
        .properties
        .get("ApplicationName")
        .map(attribute_text)
        .unwrap_or_default()
}

/// The application's configuration as Elastic Beanstalk reports it, for drift detection.
/// An application it no longer has is reported as no application at all — the describe
/// answers an empty list rather than refusing — which is the `DELETED` drift status.
pub(crate) async fn describe_eb_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "ApplicationNames.member.1".to_string(),
        physical_resource_id.to_string(),
    )];
    eb_reported_entry(ctx, region, "DescribeApplications", &parameters).await
}

/// The application version's configuration as Elastic Beanstalk reports it, for drift
/// detection. Addressed by the PAIR that identifies it: the version label (this
/// resource's physical id) and the application its declaration names.
pub(crate) async fn describe_eb_application_version(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![
        ("ApplicationName".to_string(), eb_application_of(resource)),
        (
            "VersionLabels.member.1".to_string(),
            resource.physical_resource_id.clone(),
        ),
    ];
    eb_reported_entry(ctx, region, "DescribeApplicationVersions", &parameters).await
}

/// The environment's configuration as Elastic Beanstalk reports it, for drift detection —
/// and the read that says whether a termination has finished.
///
/// A terminated environment is not one of the live environments the describe reports (it
/// is included only for `IncludeDeleted`, which this read does not ask for), so it reads
/// as absent: the `DELETED` drift status, and the state a stack's deletion asked for.
pub(crate) async fn describe_eb_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "EnvironmentNames.member.1".to_string(),
        physical_resource_id.to_string(),
    )];
    eb_reported_entry(ctx, region, "DescribeEnvironments", &parameters).await
}

/// The ONE resource an Elastic Beanstalk `Describe…` answered with: the scalar members of
/// the first entry of the list it reports, or `None` when that list is empty.
///
/// Every `Describe…` of this service answers a LIST — `<Environments><member>…` — and
/// each request above narrows it to the one resource by name, so the first entry is that
/// resource and an empty list is the service saying it does not hold it. The entry's
/// scalar members are named exactly as the declaring type's properties are
/// (`ApplicationName`, `Description`, `SolutionStackName`, `PlatformArn`, `VersionLabel`,
/// `TemplateName`), which is what makes them the actual configuration a declaration is
/// compared against; a member reported as a STRUCTURE or a list is not a scalar and is
/// left out, and a property the actual document does not carry is not evidence of
/// anything (see [`super::drift`]).
pub(crate) async fn eb_reported_entry(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    parameters: &[(String, String)],
) -> Result<Option<Value>, Refusal> {
    let answered = call_query(
        ctx,
        ELASTIC_BEANSTALK_SIGNING_NAME,
        region,
        ELASTIC_BEANSTALK_API_VERSION,
        action,
        parameters,
    )
    .await?;
    if xml_element(&answered, "member").is_none() {
        return Ok(None);
    }
    Ok(Some(Value::Object(xml_scalar_children(&answered, "member"))))
}

// ---------------------------------------------------------------------------
// CodeDeploy: AWS::CodeDeploy::Application / ::DeploymentGroup / ::DeploymentConfig
// ---------------------------------------------------------------------------

/// CodeDeploy's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: codedeploy`, service shape `CodeDeploy_20141006`), which speaks
/// `awsJson1_1`.
pub(crate) const CODEDEPLOY_SIGNING_NAME: &str = "codedeploy";
pub(crate) const CODEDEPLOY_TARGET_PREFIX: &str = "CodeDeploy_20141006";

/// The error codes CodeDeploy answers for the resources these types are, when it does not
/// hold them: `ApplicationDoesNotExistException`, `DeploymentGroupDoesNotExistException` and
/// `DeploymentConfigDoesNotExistException`, each a modelled error of the matching read.
pub(crate) const APPLICATION_DOES_NOT_EXIST: &str = "ApplicationDoesNotExistException";
pub(crate) const DEPLOYMENT_GROUP_DOES_NOT_EXIST: &str = "DeploymentGroupDoesNotExistException";
pub(crate) const DEPLOYMENT_CONFIG_DOES_NOT_EXIST: &str = "DeploymentConfigDoesNotExistException";

/// How one declared property reaches the CodeDeploy create that takes it — the one thing
/// the properties of these types differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeDeployMember {
    /// The value as the template wrote it: the property is a scalar or a list of scalars,
    /// or the members inside it are spelled by CodeDeploy exactly as the property spells
    /// them (its `EC2TagFilter` and `TagFilter` shapes both spell `Key`, `Value` and
    /// `Type`).
    Verbatim,
    /// The value with every member name inside it lower-camel cased, which is how
    /// CodeDeploy spells the members a property names in upper camel
    /// (`DeploymentStyle: {DeploymentType}` is `deploymentStyle: {deploymentType}`).
    LowerCamel,
    /// A WRAPPER structure whose own member is lower-camel cased while what it holds is
    /// carried verbatim: an `Ec2TagSet`'s `Ec2TagSetList` is `ec2TagSetList`, and the
    /// `EC2TagFilter` groups inside it keep their upper-camel members.
    WrappedVerbatim,
}

/// The `AWS::CodeDeploy::Application` properties `CreateApplication` takes, as
/// `(property, member, rule)`.
///
/// Source: that type's CloudFormation resource reference (`ApplicationName`,
/// `ComputePlatform`, `Tags`) against `CreateApplicationInput`. The two the arm pushes
/// itself are in [`CODEDEPLOY_APPLICATION_PROPERTIES_NOT_SENT`].
pub(crate) const CODEDEPLOY_APPLICATION_MEMBERS: &[(&str, &str, CodeDeployMember)] =
    &[("ComputePlatform", "computePlatform", CodeDeployMember::Verbatim)];

/// The `AWS::CodeDeploy::Application` properties the arm pushes itself: the application's
/// name, which is this resource's physical id, and its tags, which reach the create merged
/// with the ones CloudFormation gives the resource ([`tags_of`]).
pub(crate) const CODEDEPLOY_APPLICATION_PROPERTIES_NOT_SENT: &[&str] = &["ApplicationName", "Tags"];

/// The `AWS::CodeDeploy::DeploymentGroup` properties `CreateDeploymentGroup` takes.
///
/// Source: that type's CloudFormation resource reference against
/// `CreateDeploymentGroupInput`, member by member. Every one of them is the property with
/// its initial letter lower-cased except `ECSServices`, whose member CodeDeploy spells
/// `ecsServices`; the rule beside each says how far INTO the value that re-casing goes
/// ([`CodeDeployMember`]).
pub(crate) const CODEDEPLOY_DEPLOYMENT_GROUP_MEMBERS: &[(&str, &str, CodeDeployMember)] = &[
    (
        "AlarmConfiguration",
        "alarmConfiguration",
        CodeDeployMember::LowerCamel,
    ),
    (
        "AutoRollbackConfiguration",
        "autoRollbackConfiguration",
        CodeDeployMember::LowerCamel,
    ),
    (
        "AutoScalingGroups",
        "autoScalingGroups",
        CodeDeployMember::Verbatim,
    ),
    (
        "BlueGreenDeploymentConfiguration",
        "blueGreenDeploymentConfiguration",
        CodeDeployMember::LowerCamel,
    ),
    (
        "DeploymentConfigName",
        "deploymentConfigName",
        CodeDeployMember::Verbatim,
    ),
    (
        "DeploymentStyle",
        "deploymentStyle",
        CodeDeployMember::LowerCamel,
    ),
    ("ECSServices", "ecsServices", CodeDeployMember::LowerCamel),
    (
        "Ec2TagFilters",
        "ec2TagFilters",
        CodeDeployMember::Verbatim,
    ),
    (
        "Ec2TagSet",
        "ec2TagSet",
        CodeDeployMember::WrappedVerbatim,
    ),
    (
        "LoadBalancerInfo",
        "loadBalancerInfo",
        CodeDeployMember::LowerCamel,
    ),
    (
        "OnPremisesInstanceTagFilters",
        "onPremisesInstanceTagFilters",
        CodeDeployMember::Verbatim,
    ),
    (
        "OnPremisesTagSet",
        "onPremisesTagSet",
        CodeDeployMember::WrappedVerbatim,
    ),
    (
        "OutdatedInstancesStrategy",
        "outdatedInstancesStrategy",
        CodeDeployMember::Verbatim,
    ),
    (
        "ServiceRoleArn",
        "serviceRoleArn",
        CodeDeployMember::Verbatim,
    ),
    (
        "TerminationHookEnabled",
        "terminationHookEnabled",
        CodeDeployMember::Verbatim,
    ),
    (
        "TriggerConfigurations",
        "triggerConfigurations",
        CodeDeployMember::LowerCamel,
    ),
];

/// The `AWS::CodeDeploy::DeploymentGroup` properties the arm does not send to
/// `CreateDeploymentGroup`: the two that make the group's identity (the application it
/// belongs to and its own name, which is this resource's physical id), its tags — which
/// reach the create merged with CloudFormation's ([`tags_of`]) — and `Deployment`, which is
/// a `CreateDeployment` of its own ([`create_codedeploy_deployment`]).
pub(crate) const CODEDEPLOY_DEPLOYMENT_GROUP_PROPERTIES_NOT_SENT: &[&str] = &[
    "ApplicationName",
    "DeploymentGroupName",
    "Deployment",
    "Tags",
];

/// The members of a deployment group's `Deployment` property, as `CreateDeployment` takes
/// them.
///
/// Source: that property's own reference (`Description`,
/// `IgnoreApplicationStopFailures`, `Revision`) against `CreateDeploymentInput`. A
/// revision's members are CodeDeploy's `RevisionLocation` in lower camel all the way down
/// (`S3Location.ETag` is `s3Location.eTag`), so the whole value is re-cased.
pub(crate) const CODEDEPLOY_DEPLOYMENT_MEMBERS: &[(&str, &str, CodeDeployMember)] = &[
    ("Description", "description", CodeDeployMember::Verbatim),
    (
        "IgnoreApplicationStopFailures",
        "ignoreApplicationStopFailures",
        CodeDeployMember::Verbatim,
    ),
    ("Revision", "revision", CodeDeployMember::LowerCamel),
];

/// The `AWS::CodeDeploy::DeploymentConfig` properties `CreateDeploymentConfig` takes.
///
/// Source: that type's CloudFormation resource reference against
/// `CreateDeploymentConfigInput`. `MinimumHealthyHosts`, `TrafficRoutingConfig` and
/// `ZonalConfig` are structures CodeDeploy spells in lower camel all the way down
/// (`MinimumHealthyHosts: {Type, Value}` is `minimumHealthyHosts: {type, value}`).
pub(crate) const CODEDEPLOY_DEPLOYMENT_CONFIG_MEMBERS: &[(&str, &str, CodeDeployMember)] = &[
    (
        "ComputePlatform",
        "computePlatform",
        CodeDeployMember::Verbatim,
    ),
    (
        "MinimumHealthyHosts",
        "minimumHealthyHosts",
        CodeDeployMember::LowerCamel,
    ),
    (
        "TrafficRoutingConfig",
        "trafficRoutingConfig",
        CodeDeployMember::LowerCamel,
    ),
    ("ZonalConfig", "zonalConfig", CodeDeployMember::LowerCamel),
];

/// The one `AWS::CodeDeploy::DeploymentConfig` property the arm pushes itself: the
/// configuration's name, which is this resource's physical id.
pub(crate) const CODEDEPLOY_DEPLOYMENT_CONFIG_PROPERTIES_NOT_SENT: &[&str] = &["DeploymentConfigName"];

/// Create the application an `AWS::CodeDeploy::Application` declares, so a client that
/// deployed the stack and then calls `codedeploy GetApplication` with the name the stack
/// publishes reads the application back rather than an `ApplicationDoesNotExistException`.
///
/// "Ref returns the application name", which is this resource's physical id, so the
/// application is created under the name CloudFormation decided and the type publishes no
/// `Fn::GetAtt` attribute for the arm to adopt. It carries the tags the declaration asks
/// for plus the ones CloudFormation gives the resource ([`tags_of`]), as the `[{Key,
/// Value}]` list CodeDeploy's own `Tag` shape spells.
pub(crate) async fn create_codedeploy_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = codedeploy_request(
        &resource.properties,
        CODEDEPLOY_APPLICATION_MEMBERS,
        CODEDEPLOY_APPLICATION_PROPERTIES_NOT_SENT,
        CODEDEPLOY_APPLICATION,
    )?;
    request.insert(
        "applicationName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    call_codedeploy(ctx, region, "CreateApplication", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Create the deployment group an `AWS::CodeDeploy::DeploymentGroup` declares, under the
/// application its declaration names.
///
/// "Ref returns the deployment group name", which is this resource's physical id, so the
/// group is created under the name CloudFormation decided. A group is addressed by the pair
/// `(applicationName, deploymentGroupName)` and the application half is a required property
/// of the type: a declaration carrying none describes no group that could be created, so it
/// is refused here rather than sent as a request CodeDeploy would have to reject
/// ([`codedeploy_application_of`]).
///
/// The group's `Deployment` property is the one thing `CreateDeploymentGroup` has no member
/// for: it is an application revision to deploy to the group once it exists, so it is a
/// `CreateDeployment` on the group just created ([`create_codedeploy_deployment`]) — the
/// same second call a client makes.
pub(crate) async fn create_codedeploy_deployment_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let application = codedeploy_application_of(resource)?;
    let mut request = codedeploy_request(
        &resource.properties,
        CODEDEPLOY_DEPLOYMENT_GROUP_MEMBERS,
        CODEDEPLOY_DEPLOYMENT_GROUP_PROPERTIES_NOT_SENT,
        CODEDEPLOY_DEPLOYMENT_GROUP,
    )?;
    request.insert(
        "applicationName".to_string(),
        Value::String(application.clone()),
    );
    request.insert(
        "deploymentGroupName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    call_codedeploy(
        ctx,
        region,
        "CreateDeploymentGroup",
        &Value::Object(request),
    )
    .await?;
    if let Some(declared) = resource.properties.get("Deployment") {
        create_codedeploy_deployment(
            ctx,
            region,
            &application,
            &resource.physical_resource_id,
            declared,
        )
        .await?;
    }
    // The type's identity is the pair the stack already records (its physical id and its
    // declared `ApplicationName`), so there is no identifier to adopt from the answer.
    Ok(Created::attributes(BTreeMap::new()))
}

/// Deploy the revision a deployment group's `Deployment` property declares to the group the
/// stack has just created, through CodeDeploy's own `CreateDeployment`.
///
/// The property's members are that call's own ([`CODEDEPLOY_DEPLOYMENT_MEMBERS`]) beside the
/// two that say WHERE the deployment goes, which are the group's identity rather than
/// anything the property states.
pub(crate) async fn create_codedeploy_deployment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    application: &str,
    deployment_group: &str,
    declared: &Value,
) -> Result<(), Refusal> {
    let mut request = codedeploy_request(
        declared,
        CODEDEPLOY_DEPLOYMENT_MEMBERS,
        &[],
        CODEDEPLOY_DEPLOYMENT_GROUP,
    )?;
    request.insert(
        "applicationName".to_string(),
        Value::String(application.to_string()),
    );
    request.insert(
        "deploymentGroupName".to_string(),
        Value::String(deployment_group.to_string()),
    );
    call_codedeploy(ctx, region, "CreateDeployment", &Value::Object(request)).await?;
    Ok(())
}

/// Create the deployment configuration an `AWS::CodeDeploy::DeploymentConfig` declares, so
/// a deployment group of the same stack can name it and `codedeploy GetDeploymentConfig`
/// resolves it.
///
/// "Ref returns the deployment configuration name", which is this resource's physical id.
pub(crate) async fn create_codedeploy_deployment_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = codedeploy_request(
        &resource.properties,
        CODEDEPLOY_DEPLOYMENT_CONFIG_MEMBERS,
        CODEDEPLOY_DEPLOYMENT_CONFIG_PROPERTIES_NOT_SENT,
        CODEDEPLOY_DEPLOYMENT_CONFIG,
    )?;
    request.insert(
        "deploymentConfigName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_codedeploy(
        ctx,
        region,
        "CreateDeploymentConfig",
        &Value::Object(request),
    )
    .await?;
    Ok(Created::default())
}

/// Delete the application a stack created, addressed by its name — this resource's physical
/// id.
///
/// `DeleteApplication` models no does-not-exist error at all (`ApplicationNameRequired`,
/// `InvalidApplicationName` and `InvalidRole` are its whole error list), so an application
/// CodeDeploy no longer holds is already in the state the delete asks for and the call says
/// so itself.
pub(crate) async fn delete_codedeploy_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "applicationName": physical_resource_id });
    call_codedeploy(ctx, region, "DeleteApplication", &request).await?;
    Ok(())
}

/// Delete the deployment group a stack created, addressed by the pair that identifies it:
/// the application its declaration names and its own name, which is this resource's
/// physical id.
///
/// `DeleteDeploymentGroup` models no does-not-exist error either, for the application or
/// for the group, so one CodeDeploy no longer holds is already in the state the delete asks
/// for.
pub(crate) async fn delete_codedeploy_deployment_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "applicationName": codedeploy_application_of(resource)?,
        "deploymentGroupName": resource.physical_resource_id.clone(),
    });
    call_codedeploy(ctx, region, "DeleteDeploymentGroup", &request).await?;
    Ok(())
}

/// Delete the deployment configuration a stack created, addressed by its name — this
/// resource's physical id.
///
/// `DeleteDeploymentConfig` models no does-not-exist error (`DeploymentConfigInUse`,
/// `DeploymentConfigNameRequired`, `InvalidDeploymentConfigName` and `InvalidOperation` are
/// its whole error list), so one CodeDeploy no longer holds is already in the state the
/// delete asks for; one a deployment group still uses is NOT deleted, and that refusal is
/// what the stack reports.
pub(crate) async fn delete_codedeploy_deployment_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "deploymentConfigName": physical_resource_id });
    call_codedeploy(ctx, region, "DeleteDeploymentConfig", &request).await?;
    Ok(())
}

/// The application's configuration as CodeDeploy reports it, for drift detection. One it no
/// longer holds (`ApplicationDoesNotExistException`, judged on the CODE and never on a
/// message that carries the request's own name) was deleted.
///
/// `GetApplication` answers an `ApplicationInfo` whose `applicationName` and
/// `computePlatform` are the two properties of this type CodeDeploy reports; the rest of
/// that structure is the application's own history (its id, its creation time, its GitHub
/// link), which no property declares.
pub(crate) async fn describe_codedeploy_application(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "applicationName": physical_resource_id });
    match call_codedeploy(ctx, region, "GetApplication", &request).await {
        Ok(answer) => {
            let Some(application) = answer.get("application") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                application,
                &[
                    ("ApplicationName", "applicationName"),
                    ("ComputePlatform", "computePlatform"),
                ],
            )))
        }
        Err(refusal) if refusal.code == APPLICATION_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The deployment group's configuration as CodeDeploy reports it, for drift detection,
/// addressed by the pair that identifies it. A group it no longer holds — and an
/// application it no longer holds, which cannot hold the group either — was deleted.
///
/// The properties reported are the SCALAR members `GetDeploymentGroup` answers under a name
/// this type declares its properties with. Its instance-selection lists are deliberately
/// not among them: CodeDeploy answers an `autoScalingGroups` list of structures where the
/// property is a list of names, and a property the read does not report is not evidence of
/// anything ([`super::drift`]).
pub(crate) async fn describe_codedeploy_deployment_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "applicationName": codedeploy_application_of(resource)?,
        "deploymentGroupName": resource.physical_resource_id.clone(),
    });
    match call_codedeploy(ctx, region, "GetDeploymentGroup", &request).await {
        Ok(answer) => {
            let Some(group) = answer.get("deploymentGroupInfo") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                group,
                &[
                    ("ApplicationName", "applicationName"),
                    ("DeploymentGroupName", "deploymentGroupName"),
                    ("DeploymentConfigName", "deploymentConfigName"),
                    ("ServiceRoleArn", "serviceRoleArn"),
                    ("OutdatedInstancesStrategy", "outdatedInstancesStrategy"),
                    ("TerminationHookEnabled", "terminationHookEnabled"),
                ],
            )))
        }
        Err(refusal)
            if refusal.code == DEPLOYMENT_GROUP_DOES_NOT_EXIST
                || refusal.code == APPLICATION_DOES_NOT_EXIST =>
        {
            Ok(None)
        }
        Err(refusal) => Err(refusal),
    }
}

/// The deployment configuration as CodeDeploy reports it, for drift detection. One it no
/// longer holds (`DeploymentConfigDoesNotExistException`) was deleted.
///
/// The two properties reported are the scalar members `GetDeploymentConfig` answers under a
/// name this type declares: its name and its compute platform. Its
/// `minimumHealthyHosts`, `trafficRoutingConfig` and `zonalConfig` are answered with
/// CodeDeploy's own lower-camel member names inside them, which the type's properties spell
/// in upper camel, so comparing the two spellings would report drift where there is none.
pub(crate) async fn describe_codedeploy_deployment_config(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "deploymentConfigName": physical_resource_id });
    match call_codedeploy(ctx, region, "GetDeploymentConfig", &request).await {
        Ok(answer) => {
            let Some(configuration) = answer.get("deploymentConfigInfo") else {
                return Ok(None);
            };
            Ok(Some(reported_as_properties(
                configuration,
                &[
                    ("DeploymentConfigName", "deploymentConfigName"),
                    ("ComputePlatform", "computePlatform"),
                ],
            )))
        }
        Err(refusal) if refusal.code == DEPLOYMENT_CONFIG_DOES_NOT_EXIST => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The application a deployment group's declaration names — the other half of the group's
/// identity.
///
/// `ApplicationName` is a required property of the type, so a declaration carrying none is
/// refused here, naming that property, rather than sent as a request CodeDeploy would
/// reject.
pub(crate) fn codedeploy_application_of(resource: &StoredResource) -> Result<String, Refusal> {
    property(resource, "ApplicationName").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!("{} declares no ApplicationName", resource.resource_type),
    })
}

/// The request one evaluated property document is, against the member catalogue of the
/// CodeDeploy call that takes it.
///
/// `not_sent` are the properties the ARM itself pushes or that belong to another call.
/// Every other property has to be in `members`: one that is not is REFUSED, for the reason
/// [`query_members`] records — a resource created without a property its declaration asked
/// for is configured differently from what the template declared, and the stack would have
/// reported it as created.
pub(crate) fn codedeploy_request(
    properties: &Value,
    members: &[(&str, &str, CodeDeployMember)],
    not_sent: &[&str],
    resource_type: &str,
) -> Result<Map<String, Value>, Refusal> {
    let mut request = Map::new();
    let Some(properties) = properties.as_object() else {
        return Ok(request);
    };
    for (property, value) in properties {
        if not_sent.contains(&property.as_str()) {
            continue;
        }
        let Some((_, member, rule)) = members
            .iter()
            .find(|(declared, _, _)| *declared == property.as_str())
        else {
            return Err(Refusal {
                code: NOT_IMPLEMENTED.to_string(),
                message: format!(
                    "this emulator does not send {resource_type}'s {property} property to \
                     the service that owns the type, so the resource was not created"
                ),
            });
        };
        let carried = match rule {
            CodeDeployMember::Verbatim => value.clone(),
            CodeDeployMember::LowerCamel => lower_camel_members(value, 0)?,
            CodeDeployMember::WrappedVerbatim => wrapped_verbatim(value),
        };
        request.insert((*member).to_string(), carried);
    }
    Ok(request)
}

/// One wrapper structure under the member names its API spells — the property's initial
/// letter lower-cased — with each value carried as the template wrote it.
///
/// That is what an `Ec2TagSet` and an `OnPremisesTagSet` need: the wrapper's own member is
/// `ec2TagSetList`/`onPremisesTagSetList`, while the tag-filter groups inside it keep the
/// `Key`/`Value`/`Type` members CodeDeploy's own filter shapes spell.
pub(crate) fn wrapped_verbatim(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(name, inner)| (lower_initial(name), inner.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Call one CodeDeploy operation on the awsJson1_1 wire its model declares.
pub(crate) async fn call_codedeploy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        CODEDEPLOY_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{CODEDEPLOY_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Transfer Family: AWS::Transfer::Server
// ---------------------------------------------------------------------------

/// AWS Transfer Family's signing name and `X-Amz-Target` prefix, from its model
/// (`sigv4Name: transfer`, service shape `TransferService`), which speaks `awsJson1_1`.
pub(crate) const TRANSFER_SIGNING_NAME: &str = "transfer";
pub(crate) const TRANSFER_TARGET_PREFIX: &str = "TransferService";

/// The error code AWS Transfer Family answers for a server it does not hold.
pub(crate) const TRANSFER_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The properties of `AWS::Transfer::Server` that are reported back as the server's ACTUAL
/// configuration, as `(property, member)` pairs: the members `DescribeServer` answers
/// under the property's own name AND in the property's own shape.
///
/// `Protocols`, `Domain`, `EndpointType`, `IdentityProviderType`, `IpAddressType`,
/// `SecurityPolicyName`, `LoggingRole`, `Certificate` and the two login banners are each
/// one member of `DescribedServer` spelled identically to the property. The structured
/// members a server also reports (`EndpointDetails`, `IdentityProviderDetails`,
/// `ProtocolDetails`, `S3StorageOptions`, `WorkflowDetails`, `Tags`) are reported with
/// every documented default already materialised, so comparing them against a declaration
/// that names only some of their members would read as drift the declaration never asked
/// for; a property the actual document does not carry is not evidence of anything
/// ([`super::drift`]), so they are left out.
pub(crate) const TRANSFER_SERVER_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Certificate", "Certificate"),
    ("Domain", "Domain"),
    ("EndpointType", "EndpointType"),
    ("IdentityProviderType", "IdentityProviderType"),
    ("IpAddressType", "IpAddressType"),
    ("LoggingRole", "LoggingRole"),
    ("PostAuthenticationLoginBanner", "PostAuthenticationLoginBanner"),
    ("PreAuthenticationLoginBanner", "PreAuthenticationLoginBanner"),
    ("Protocols", "Protocols"),
    ("SecurityPolicyName", "SecurityPolicyName"),
];

/// Create the server an `AWS::Transfer::Server` declares, so a client that deployed the
/// stack and then calls `transfer describe-server` with the `ServerId` the stack publishes
/// reads the server back rather than a `ResourceNotFoundException`.
///
/// The type's properties ARE `CreateServer`'s own members — `Certificate`, `Domain`,
/// `EndpointDetails`, `EndpointType`, `IdentityProviderDetails`, `IdentityProviderType`,
/// `IpAddressType`, `LoggingRole`, `PostAuthenticationLoginBanner`,
/// `PreAuthenticationLoginBanner`, `ProtocolDetails`, `Protocols`, `S3StorageOptions`,
/// `SecurityPolicyName`, `StructuredLogDestinations`, `Tags` and `WorkflowDetails` are
/// named and shaped identically on both sides — so the evaluated property document IS the
/// request, passed through verbatim. `Tags` is the one member rebuilt rather than carried:
/// the server carries the tags CloudFormation gives it beside the declared ones
/// ([`tags_of`]), as the `[{Key, Value}]` list that model's own `Tag` shape spells.
///
/// The server's identity is Transfer Family's OWN: `CreateServer` takes no identifier and
/// answers the `ServerId` it assigned, and "Ref returns the Amazon Resource Name (ARN) of
/// the server" — which carries that id. So the id that service assigned is reported back
/// as this resource's physical id and as its `ServerId` attribute, and the stack publishes
/// the handle Transfer Family really created rather than one these naming rules computed.
pub(crate) async fn create_transfer_server(
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
            request.insert(property.clone(), value.clone());
        }
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let answered = call_transfer(ctx, region, "CreateServer", &Value::Object(request)).await?;
    let server_id = answered
        .get("ServerId")
        .and_then(Value::as_str)
        .filter(|server_id| !server_id.is_empty())
        .ok_or_else(|| Refusal {
            code: "InternalFailure".to_string(),
            message: "AWS Transfer Family answered CreateServer without a ServerId".to_string(),
        })?;
    // A Transfer Family server's ARN is a total function of the Region and the assigned
    // id, built from the one account id this emulator mints — the same ARN that service
    // keyed the server by and reports for it — so the handle below is the created
    // server's own, not a guess at one.
    let arn = ::emulator_server::emulator_arn(
        TRANSFER_SIGNING_NAME,
        region,
        &format!("server/{server_id}"),
    );
    let mut attributes = BTreeMap::new();
    attributes.insert("Arn".to_string(), arn.clone());
    attributes.insert("ServerId".to_string(), server_id.to_string());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the server a stack created, addressed by the `ServerId` Transfer Family assigned
/// it.
///
/// A server that service no longer resolves the id to is already in the state the delete
/// asks for, judged on its not-found CODE and never on a message.
pub(crate) async fn delete_transfer_server(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "ServerId": transfer_server_id(resource) });
    match call_transfer(ctx, region, "DeleteServer", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == TRANSFER_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The server's configuration as Transfer Family reports it, for drift detection. One it
/// no longer holds was deleted, which is the `DELETED` drift status.
pub(crate) async fn describe_transfer_server(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "ServerId": transfer_server_id(resource) });
    match call_transfer(ctx, region, "DescribeServer", &request).await {
        Ok(answered) => Ok(Some(reported_as_properties(
            answered.get("Server").unwrap_or(&Value::Null),
            TRANSFER_SERVER_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == TRANSFER_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The `ServerId` a resource record is addressed by: the attribute the create reported,
/// which is the id Transfer Family assigned.
///
/// A record with no such attribute (one whose attribute budget was spent) falls back to
/// the last segment of its physical id, which is that same id — the physical id is the
/// server's ARN, whose resource path is `server/<ServerId>`.
pub(crate) fn transfer_server_id(resource: &StoredResource) -> String {
    if let Some(server_id) = resource.attributes.get("ServerId") {
        return server_id.clone();
    }
    resource
        .physical_resource_id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Call one AWS Transfer Family operation on the awsJson1_1 wire its model declares.
pub(crate) async fn call_transfer(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        TRANSFER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{TRANSFER_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// AWS Batch: AWS::Batch::ComputeEnvironment, AWS::Batch::JobQueue,
//            AWS::Batch::ServiceEnvironment
// ---------------------------------------------------------------------------

/// AWS Batch's signing name, from its model (`sigv4Name: batch`). It speaks `restJson1`,
/// so each call below is a method and a URI rather than an `X-Amz-Target`.
pub(crate) const BATCH_SIGNING_NAME: &str = "batch";

/// The URIs the calls these three types need are bound to, from the Batch model's own
/// `@http` traits.
pub(crate) const BATCH_CREATE_COMPUTE_ENVIRONMENT_URI: &str = "/v1/createcomputeenvironment";
pub(crate) const BATCH_UPDATE_COMPUTE_ENVIRONMENT_URI: &str = "/v1/updatecomputeenvironment";
pub(crate) const BATCH_DELETE_COMPUTE_ENVIRONMENT_URI: &str = "/v1/deletecomputeenvironment";
pub(crate) const BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI: &str = "/v1/describecomputeenvironments";
pub(crate) const BATCH_CREATE_JOB_QUEUE_URI: &str = "/v1/createjobqueue";
pub(crate) const BATCH_UPDATE_JOB_QUEUE_URI: &str = "/v1/updatejobqueue";
pub(crate) const BATCH_DELETE_JOB_QUEUE_URI: &str = "/v1/deletejobqueue";
pub(crate) const BATCH_DESCRIBE_JOB_QUEUES_URI: &str = "/v1/describejobqueues";
pub(crate) const BATCH_CREATE_SERVICE_ENVIRONMENT_URI: &str = "/v1/createserviceenvironment";
pub(crate) const BATCH_UPDATE_SERVICE_ENVIRONMENT_URI: &str = "/v1/updateserviceenvironment";
pub(crate) const BATCH_DELETE_SERVICE_ENVIRONMENT_URI: &str = "/v1/deleteserviceenvironment";
pub(crate) const BATCH_DESCRIBE_SERVICE_ENVIRONMENTS_URI: &str = "/v1/describeserviceenvironments";

/// The `JQState`/`CEState` a resource has to be in before AWS Batch will delete it:
/// `DeleteJobQueue`'s own doc ("you must first disable submissions for a queue with the
/// UpdateJobQueue operation") and `DeleteComputeEnvironment`'s ("Before you can delete a
/// compute environment, you must set its state to DISABLED").
pub(crate) const BATCH_STATE_DISABLED: &str = "DISABLED";

/// The `CEStatus`/`JQStatus` of a compute environment or job queue AWS Batch has finished
/// creating, and the one it reports for a configuration it could not accept.
pub(crate) const BATCH_STATUS_VALID: &str = "VALID";
pub(crate) const BATCH_STATUS_INVALID: &str = "INVALID";

/// The `AWS::Batch::ComputeEnvironment` properties `CreateComputeEnvironment` takes whose
/// value goes through unchanged under its lower-camel member name.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateComputeEnvironmentRequest` shape. `ComputeEnvironmentName` is not here because it
/// is the name CloudFormation decides, and `Tags` is not because the type spells it as a
/// map of tag keys the template chose. `ReplaceComputeEnvironment` and `UpdatePolicy` are
/// not members of the CREATE at all — they govern how a later deployment applies a change
/// — so they are not sent.
pub(crate) const BATCH_COMPUTE_ENVIRONMENT_MEMBERS: &[(&str, &str)] = &[
    ("Type", "type"),
    ("State", "state"),
    ("ServiceRole", "serviceRole"),
    ("UnmanagedvCpus", "unmanagedvCpus"),
    ("Context", "context"),
];

/// The `AWS::Batch::JobQueue` properties `CreateJobQueue` takes whose value goes through
/// unchanged under its lower-camel member name.
///
/// Source: the same pairing of the type's resource reference with
/// `CreateJobQueueRequest`. `JobQueueName` is the name CloudFormation decides and `Tags` is
/// a map of the template's own keys, so neither is here; the three LIST-valued properties
/// are mapped entry by entry below, their own members being modelled structures.
pub(crate) const BATCH_JOB_QUEUE_MEMBERS: &[(&str, &str)] = &[
    ("Priority", "priority"),
    ("State", "state"),
    ("SchedulingPolicyArn", "schedulingPolicyArn"),
    ("JobQueueType", "jobQueueType"),
];

/// The members of one `ComputeEnvironmentOrder` entry of a job queue's
/// `ComputeEnvironmentOrder`.
pub(crate) const BATCH_COMPUTE_ENVIRONMENT_ORDER_MEMBERS: &[(&str, &str)] =
    &[("ComputeEnvironment", "computeEnvironment"), ("Order", "order")];

/// The members of one `ServiceEnvironmentOrder` entry.
pub(crate) const BATCH_SERVICE_ENVIRONMENT_ORDER_MEMBERS: &[(&str, &str)] =
    &[("ServiceEnvironment", "serviceEnvironment"), ("Order", "order")];

/// The members of one `JobStateTimeLimitAction` entry.
pub(crate) const BATCH_JOB_STATE_TIME_LIMIT_ACTION_MEMBERS: &[(&str, &str)] = &[
    ("Action", "action"),
    ("MaxTimeSeconds", "maxTimeSeconds"),
    ("Reason", "reason"),
    ("State", "state"),
];

/// The `AWS::Batch::ServiceEnvironment` properties `CreateServiceEnvironment` takes whose
/// value goes through unchanged under its lower-camel member name.
///
/// `ServiceEnvironmentName` is the name CloudFormation decides, `CapacityLimits` is a list
/// of modelled structures mapped entry by entry, and `Tags` is a map of the template's own
/// keys, so none of the three is here.
pub(crate) const BATCH_SERVICE_ENVIRONMENT_MEMBERS: &[(&str, &str)] =
    &[("ServiceEnvironmentType", "serviceEnvironmentType"), ("State", "state")];

/// The members of one `CapacityLimit` entry of a service environment's `CapacityLimits`.
pub(crate) const BATCH_CAPACITY_LIMIT_MEMBERS: &[(&str, &str)] =
    &[("MaxCapacity", "maxCapacity"), ("CapacityUnit", "capacityUnit")];

/// How long to wait between re-reads while a Batch resource the stack asked to delete is
/// still on its way out, and how many of those re-reads to make.
///
/// A deleted job queue leaves AWS Batch a moment after `DeleteJobQueue` answers, and a
/// compute environment cannot be deleted while a queue still orders it
/// (`DeleteComputeEnvironment`: "you must first remove it from a job queue"). The stack
/// deletes its resources newest first, so the queue's delete is asked for before the
/// environment's: this is what lets the queue actually go before the environment is asked
/// for, rather than reporting a stack deletion as failed over an ordering the caller has
/// no part in.
pub(crate) const BATCH_DELETION_CHECK_INTERVAL: ::std::time::Duration =
    ::std::time::Duration::from_millis(50);
pub(crate) const BATCH_DELETION_CHECKS: usize = 40;

/// The resource NAME an `arn:aws:batch:…:<kind>/<name>` names.
///
/// Both types' `Ref` is the resource's ARN, so that is the record's physical id — and the
/// name AWS Batch keys the resource by is its last segment, whether the template declared
/// the name or CloudFormation generated it.
pub(crate) fn batch_resource_name(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// Create the compute environment an `AWS::Batch::ComputeEnvironment` declares, so a
/// client that reads the stack's output can call AWS Batch with it
/// (`DescribeComputeEnvironments`, `CreateJobQueue`) rather than being handed an ARN Batch
/// never minted.
///
/// `computeEnvironmentName` is the name CloudFormation decided — the declared
/// `ComputeEnvironmentName`, or the generated one — read back off the ARN that is this
/// resource's physical id. `computeResources` and `eksConfiguration` are the create's own
/// nested structures with every member's initial letter lower-cased ([`camel_cased`]),
/// which is the whole difference between the type's spelling and the API's; the TAG MAPS
/// inside them are lifted out first, their keys being the template's own.
pub(crate) async fn create_compute_environment(
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
        "computeEnvironmentName".to_string(),
        Value::String(batch_resource_name(&resource.physical_resource_id).to_string()),
    );
    copy_members(&properties, BATCH_COMPUTE_ENVIRONMENT_MEMBERS, &mut request);
    if let Some(resources) = properties.get("ComputeResources") {
        request.insert(
            "computeResources".to_string(),
            batch_nested(resources, "Tags")?,
        );
    }
    if let Some(configuration) = properties.get("EksConfiguration") {
        request.insert("eksConfiguration".to_string(), camel_cased(configuration, 0)?);
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_CREATE_COMPUTE_ENVIRONMENT_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("computeEnvironmentArn")
        .and_then(Value::as_str)
    {
        attributes.insert("ComputeEnvironmentArn".to_string(), arn.to_string());
    }
    // AWS Batch returns a fresh environment `CREATING`, and the resource is not created
    // until it is `VALID`: "Compute environments must be created and valid before
    // attaching them to a job queue" is what the NEXT resource of this stack depends on,
    // so the wait belongs here rather than in the queue that orders it.
    batch_await_valid(
        ctx,
        region,
        BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI,
        "computeEnvironments",
        &resource.physical_resource_id,
    )
    .await?;
    Ok(Created {
        physical_resource_id: None,
        attributes,
    })
}

/// Create the job queue an `AWS::Batch::JobQueue` declares, so the queue the stack's
/// output names is one `batch describe-job-queues` and `batch submit-job` find.
///
/// `jobQueueName` is the name CloudFormation decided, as above. The three list-valued
/// properties are modelled structures of their own, so each entry is mapped member by
/// member rather than passed through.
pub(crate) async fn create_job_queue(
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
        "jobQueueName".to_string(),
        Value::String(batch_resource_name(&resource.physical_resource_id).to_string()),
    );
    copy_members(&properties, BATCH_JOB_QUEUE_MEMBERS, &mut request);
    for (property, member, members) in [
        (
            "ComputeEnvironmentOrder",
            "computeEnvironmentOrder",
            BATCH_COMPUTE_ENVIRONMENT_ORDER_MEMBERS,
        ),
        (
            "ServiceEnvironmentOrder",
            "serviceEnvironmentOrder",
            BATCH_SERVICE_ENVIRONMENT_ORDER_MEMBERS,
        ),
        (
            "JobStateTimeLimitActions",
            "jobStateTimeLimitActions",
            BATCH_JOB_STATE_TIME_LIMIT_ACTION_MEMBERS,
        ),
    ] {
        if let Some(Value::Array(entries)) = properties.get(property) {
            let entries: Vec<Value> = entries
                .iter()
                .map(|entry| mapped_members(entry, members))
                .collect();
            request.insert(member.to_string(), Value::Array(entries));
        }
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_CREATE_JOB_QUEUE_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported.get("jobQueueArn").and_then(Value::as_str) {
        attributes.insert("JobQueueArn".to_string(), arn.to_string());
    }
    // A fresh queue is `CREATING` too, and a client that submits into it as soon as the
    // stack completes needs it `VALID`, so the resource is not created until it is.
    batch_await_valid(
        ctx,
        region,
        BATCH_DESCRIBE_JOB_QUEUES_URI,
        "jobQueues",
        &resource.physical_resource_id,
    )
    .await?;
    Ok(Created {
        physical_resource_id: None,
        attributes,
    })
}

/// One nested structure of a Batch declaration as the create takes it: every member's
/// initial letter lower-cased, with the MAP member `keyed` (a tag map, whose keys are the
/// template's own) carried through exactly as the template wrote it.
pub(crate) fn batch_nested(declared: &Value, keyed: &str) -> Result<Value, Refusal> {
    let kept = declared.get(keyed).cloned();
    let mut mapped = camel_cased(declared, 0)?;
    if let (Some(kept), Value::Object(members)) = (kept, &mut mapped) {
        members.remove(&lower_initial(keyed));
        members.insert(lower_initial(keyed), kept);
    }
    Ok(mapped)
}

/// Create the service environment an `AWS::Batch::ServiceEnvironment` declares, so the
/// environment a `SAGEMAKER_TRAINING` job queue of the same stack orders is one AWS Batch
/// really holds — a queue cannot attach one that does not exist ("Service environments must
/// be created and valid before attaching them to a job queue").
///
/// The type's properties are `CreateServiceEnvironment`'s own members spelled with an
/// upper-case initial, including the `CapacityLimits` entries, so the declared document is
/// re-cased rather than mapped member by member; its `Tags` map is the create's own member,
/// beside the tags CloudFormation propagates.
pub(crate) async fn create_service_environment(
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
        "serviceEnvironmentName".to_string(),
        Value::String(batch_resource_name(&resource.physical_resource_id).to_string()),
    );
    copy_members(&properties, BATCH_SERVICE_ENVIRONMENT_MEMBERS, &mut request);
    if let Some(Value::Array(limits)) = properties.get("CapacityLimits") {
        let limits: Vec<Value> = limits
            .iter()
            .map(|limit| mapped_members(limit, BATCH_CAPACITY_LIMIT_MEMBERS))
            .collect();
        request.insert("capacityLimits".to_string(), Value::Array(limits));
    }
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let reported = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_CREATE_SERVICE_ENVIRONMENT_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let mut attributes = BTreeMap::new();
    if let Some(arn) = reported
        .get("serviceEnvironmentArn")
        .and_then(Value::as_str)
    {
        attributes.insert("ServiceEnvironmentArn".to_string(), arn.to_string());
    }
    Ok(Created {
        physical_resource_id: None,
        attributes,
    })
}

/// Delete the service environment a stack created, by the ARN that is its physical id —
/// the same two steps the job queue below takes ("set it to DISABLED with
/// UpdateServiceEnvironment first").
pub(crate) async fn delete_batch_service_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    if batch_described(
        ctx,
        region,
        BATCH_DESCRIBE_SERVICE_ENVIRONMENTS_URI,
        "serviceEnvironments",
        physical_resource_id,
    )
    .await?
    .is_none()
    {
        return Ok(());
    }
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_SERVICE_ENVIRONMENT_URI,
        Some(&serde_json::json!({
            "serviceEnvironment": physical_resource_id,
            "state": BATCH_STATE_DISABLED,
        })),
    )
    .await?;
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DELETE_SERVICE_ENVIRONMENT_URI,
        Some(&serde_json::json!({"serviceEnvironment": physical_resource_id})),
    )
    .await?;
    batch_await_gone(
        ctx,
        region,
        BATCH_DESCRIBE_SERVICE_ENVIRONMENTS_URI,
        "serviceEnvironments",
        physical_resource_id,
    )
    .await
}

/// Delete the job queue a stack created, by the ARN that is its physical id.
///
/// AWS Batch takes a queue out of service in two steps, and a client has to make both:
/// "you must first disable submissions for a queue with the UpdateJobQueue operation",
/// then `DeleteJobQueue`. The deletion is then not instantaneous, so this waits for the
/// queue to actually go — the compute environment the stack deletes NEXT cannot be deleted
/// while a queue still orders it.
pub(crate) async fn delete_batch_job_queue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    if batch_described(
        ctx,
        region,
        BATCH_DESCRIBE_JOB_QUEUES_URI,
        "jobQueues",
        physical_resource_id,
    )
    .await?
    .is_none()
    {
        return Ok(());
    }
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_JOB_QUEUE_URI,
        Some(&serde_json::json!({
            "jobQueue": physical_resource_id,
            "state": BATCH_STATE_DISABLED,
        })),
    )
    .await?;
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DELETE_JOB_QUEUE_URI,
        Some(&serde_json::json!({"jobQueue": physical_resource_id})),
    )
    .await?;
    batch_await_gone(
        ctx,
        region,
        BATCH_DESCRIBE_JOB_QUEUES_URI,
        "jobQueues",
        physical_resource_id,
    )
    .await
}

/// Delete the compute environment a stack created, by the ARN that is its physical id —
/// the same two steps the queue above takes ("Before you can delete a compute environment,
/// you must set its state to DISABLED").
pub(crate) async fn delete_batch_compute_environment(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    if batch_described(
        ctx,
        region,
        BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI,
        "computeEnvironments",
        physical_resource_id,
    )
    .await?
    .is_none()
    {
        return Ok(());
    }
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_UPDATE_COMPUTE_ENVIRONMENT_URI,
        Some(&serde_json::json!({
            "computeEnvironment": physical_resource_id,
            "state": BATCH_STATE_DISABLED,
        })),
    )
    .await?;
    call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        BATCH_DELETE_COMPUTE_ENVIRONMENT_URI,
        Some(&serde_json::json!({"computeEnvironment": physical_resource_id})),
    )
    .await?;
    batch_await_gone(
        ctx,
        region,
        BATCH_DESCRIBE_COMPUTE_ENVIRONMENTS_URI,
        "computeEnvironments",
        physical_resource_id,
    )
    .await
}

/// Wait, bounded, until a Batch `Describe*` reports `identifier` as `VALID`.
///
/// AWS Batch returns a compute environment and a job queue in a transient status
/// (`CREATING`), and CloudFormation reports a resource CREATE_COMPLETE only once its
/// service has really created it — which for both of these types is `VALID`. An `INVALID`
/// status is that service's own refusal of the configuration the template declared, and is
/// reported as the failure it is rather than waited out.
pub(crate) async fn batch_await_valid(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    member: &str,
    identifier: &str,
) -> Result<(), Refusal> {
    for _ in 0..BATCH_DELETION_CHECKS {
        let status = batch_described(ctx, region, uri, member, identifier)
            .await?
            .as_ref()
            .and_then(|record| record.get("status"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        match status.as_deref() {
            Some(BATCH_STATUS_VALID) => return Ok(()),
            Some(BATCH_STATUS_INVALID) => {
                return Err(Refusal {
                    code: "ClientException".to_string(),
                    message: format!("{identifier} is INVALID"),
                })
            }
            _ => ::tokio::time::sleep(BATCH_DELETION_CHECK_INTERVAL).await,
        }
    }
    Err(Refusal {
        code: "ClientException".to_string(),
        message: format!("{identifier} did not become {BATCH_STATUS_VALID}"),
    })
}

/// The ONE record a Batch `Describe*` answers for `identifier`, or `None` when it holds
/// none.
///
/// Batch's readers take a LIST and answer the records they found, so a resource the service
/// no longer holds comes back as an empty list rather than as a modelled not-found error:
/// that empty list is what "no longer there" is read off here.
pub(crate) async fn batch_described(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    member: &str,
    identifier: &str,
) -> Result<Option<Value>, Refusal> {
    // Each of these readers names its list identically on the way in and on the way out
    // (`DescribeJobQueues$jobQueues`, `DescribeComputeEnvironments$computeEnvironments`,
    // `DescribeServiceEnvironments$serviceEnvironments`), so one member name does both.
    let mut request = Map::new();
    request.insert(
        member.to_string(),
        Value::Array(vec![Value::String(identifier.to_string())]),
    );
    let answer = call_rest_json(
        ctx,
        BATCH_SIGNING_NAME,
        region,
        "POST",
        uri,
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(answer
        .get(member)
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .cloned())
}

/// Wait, bounded, until a Batch `Describe*` holds no record for `identifier`.
///
/// A deletion AWS Batch accepted is owed rather than immediate, and the stack's next
/// delete depends on this one having completed, so the wait is here rather than left to
/// the caller — bounded by [`BATCH_DELETION_CHECKS`], after which the resource is reported
/// as still there and the stack says so.
pub(crate) async fn batch_await_gone(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    member: &str,
    identifier: &str,
) -> Result<(), Refusal> {
    for _ in 0..BATCH_DELETION_CHECKS {
        if batch_described(ctx, region, uri, member, identifier)
            .await?
            .is_none()
        {
            return Ok(());
        }
        ::tokio::time::sleep(BATCH_DELETION_CHECK_INTERVAL).await;
    }
    Err(Refusal {
        code: "ResourceInUseException".to_string(),
        message: format!("{identifier} is still being deleted"),
    })
}

// ---------------------------------------------------------------------------
// Amazon EKS: AWS::EKS::AccessEntry
// ---------------------------------------------------------------------------

/// Amazon EKS's signing name, from its model (`sigv4Name: eks`). That service speaks
/// `restJson1`, so its operations are bound to a METHOD and a URI rather than to an
/// `X-Amz-Target` header: `CreateAccessEntry` is
/// `POST /clusters/{clusterName}/access-entries`, `AssociateAccessPolicy` is
/// `POST /clusters/{clusterName}/access-entries/{principalArn}/access-policies`,
/// `DescribeAccessEntry` is `GET /clusters/{clusterName}/access-entries/{principalArn}`
/// and `DeleteAccessEntry` is the `DELETE` of that same URI
/// (`smithy_query(query="operation", operation="CreateAccessEntry,AssociateAccessPolicy,\
/// DescribeAccessEntry,DeleteAccessEntry")` on that service's model).
pub(crate) const EKS_SIGNING_NAME: &str = "eks";

/// The URI prefix every operation on a cluster's access entries hangs off.
pub(crate) const EKS_CLUSTERS_URI: &str = "/clusters";

/// The error code Amazon EKS answers for a cluster or an access entry it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of `DescribeAccessEntry` and
/// `DeleteAccessEntry`).
pub(crate) const EKS_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `AWS::EKS::AccessEntry` properties `CreateAccessEntry` takes under its own member
/// name, as (property, member) pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against the
/// `CreateAccessEntryRequest` shape. The Amazon EKS model spells its members in lower
/// camel case and renames none of these, so the only difference is the first character's
/// case. `ClusterName` is not here because that create binds it to its URI, `Tags` is not
/// because the call takes a tag MAP while the type publishes a tag LIST, and
/// `AccessPolicies` is not because it is no member of that create at all.
pub(crate) const EKS_ACCESS_ENTRY_MEMBERS: &[(&str, &str)] = &[
    ("KubernetesGroups", "kubernetesGroups"),
    ("PrincipalArn", "principalArn"),
    ("Type", "type"),
    ("Username", "username"),
];

/// The members of one `AccessScope` an `AccessPolicies` entry declares, as
/// `AssociateAccessPolicy`'s `accessScope` takes them (`AccessScope`'s own members are
/// `type` and `namespaces`).
pub(crate) const EKS_ACCESS_SCOPE_MEMBERS: &[(&str, &str)] = &[("Type", "type"), ("Namespaces", "namespaces")];

/// The members `DescribeAccessEntry` reports under this type's own property names, as
/// `(property, member)` pairs, for drift detection.
///
/// `AccessPolicies` is not among them: the policies associated to an entry are answered by
/// `ListAssociatedAccessPolicies`, a read of its own, and a property the read does not
/// report is not evidence of anything ([`super::drift`]). `Tags` is left out for the same
/// reason as everywhere else here — the tags a resource carries change independently of its
/// declaration.
pub(crate) const EKS_ACCESS_ENTRY_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("ClusterName", "clusterName"),
    ("KubernetesGroups", "kubernetesGroups"),
    ("PrincipalArn", "principalArn"),
    ("Type", "type"),
    ("Username", "username"),
];
