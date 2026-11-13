// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of the platform services: AWS KMS keys and aliases, Amazon
//! SES, Amazon Route 53, AWS Backup, AWS Glue, AWS X-Ray and Amazon Bedrock AgentCore. One
//! file of [`super`]'s per-type arms, dispatched to by its `Provisioner`.

use super::*;

/// The properties of `AWS::KMS::Key` that no KMS call changes on an existing key: the kind
/// of key material it holds, what it may be used for, where that material came from and
/// whether it is multi-Region. Each is a property AWS's resource reference marks as
/// requiring replacement.
pub(super) const KMS_KEY_REPLACEMENT_PROPERTIES: &[&str] = &["KeySpec", "KeyUsage", "MultiRegion", "Origin"];

/// Delete the key a stack created, which for a KMS key is scheduling its deletion: KMS has
/// no call that deletes a key at once — "you can schedule the deletion of a KMS key" — and
/// the waiting period is the one the declaration asks for, else the one KMS defaults to.
///
/// A key KMS no longer resolves the id to (`NotFoundException`) is already in the state the
/// delete asks for. So is one whose deletion is already scheduled, which KMS answers with
/// its invalid-state refusal: that is confirmed by READING the key back, so a refusal
/// caused by any other state is reported rather than passed off as a deletion.
pub(super) async fn schedule_kms_key_deletion(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
    properties: &Map<String, Value>,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert("KeyId".to_string(), Value::String(key_id.to_string()));
    if let Some(window) = whole_number(properties.get(KMS_KEY_PENDING_WINDOW_PROPERTY)) {
        request.insert("PendingWindowInDays".to_string(), Value::from(window));
    }
    let refusal = match call_kms(
        ctx,
        region,
        "ScheduleKeyDeletion",
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) if refusal.code == KMS_NOT_FOUND => return Ok(()),
        Err(refusal) => refusal,
    };
    if refusal.code != KMS_INVALID_STATE {
        return Err(refusal);
    }
    match describe_kms_key_metadata(ctx, region, key_id).await {
        // The key is already awaiting deletion, which is the state the delete asks for.
        Ok(Some(metadata))
            if metadata.get("KeyState").and_then(Value::as_str)
                == Some(KMS_KEY_STATE_PENDING_DELETION) =>
        {
            Ok(())
        }
        // KMS no longer resolves the id at all: nothing is left to delete.
        Ok(None) => Ok(()),
        // The key is in some other state, or the confirming read itself failed and says
        // nothing about the key: the delete really was refused.
        Ok(Some(_)) | Err(_) => Err(refusal),
    }
}

/// The key's configuration as KMS reports it, for drift detection. A key KMS no longer
/// resolves the id to was deleted.
///
/// Only the properties `DescribeKey` reports in the SAME shape the type declares them in
/// are reported, because a property the actual document does not carry is not evidence of
/// anything (`super::drift::differs`): the `KeyMetadata` members named identically to the
/// type's properties. The key POLICY is not among them — it is read through
/// `GetKeyPolicy`, as the document's text, while the property is a JSON document — and
/// neither is the rotation switch, which has a status call of its own.
pub(super) async fn describe_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let Some(metadata) = describe_kms_key_metadata(ctx, region, physical_resource_id).await?
    else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &metadata,
        KMS_KEY_REPORTED_PROPERTIES,
    )))
}

/// The `KeyMetadata` members `DescribeKey` reports that ARE properties of the type, each
/// under the property's name — which for every one of them is the member's own name.
pub(super) const KMS_KEY_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    ("Description", "Description"),
    (KMS_KEY_ENABLED_PROPERTY, "Enabled"),
    ("KeySpec", "KeySpec"),
    ("KeyUsage", "KeyUsage"),
    ("MultiRegion", "MultiRegion"),
    ("Origin", "Origin"),
];

/// The `KeyMetadata` KMS answers for a key, or `None` for an identifier it resolves to no
/// key (`NotFoundException`, judged on the CODE and never on a message that carries the
/// request's own identifier).
pub(super) async fn describe_kms_key_metadata(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_kms(
        ctx,
        region,
        "DescribeKey",
        &serde_json::json!({ "KeyId": key_id }),
    )
    .await
    {
        Ok(answer) => Ok(Some(reported_member(&answer, "KeyMetadata"))),
        Err(refusal) if refusal.code == KMS_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Call one KMS operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_kms(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        KMS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{KMS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

// ---------------------------------------------------------------------------
// KMS: AWS::KMS::Alias
// ---------------------------------------------------------------------------

/// The property naming the key an alias is bound to, which is the member that carries it on
/// both of the calls that bind one: `CreateAlias$TargetKeyId` and `UpdateAlias$TargetKeyId`
/// (`smithy_query(operation="CreateAlias,UpdateAlias")`), spelled identically on both sides.
pub(super) const KMS_ALIAS_TARGET_PROPERTY: &str = "TargetKeyId";

/// The property carrying the alias's own name, which is this resource's physical id: "Ref
/// returns the alias name", and [`super::template::physical_name`] takes a resource's
/// physical name from exactly this property.
pub(super) const KMS_ALIAS_NAME_PROPERTY: &str = "AliasName";

/// Every property `AWS::KMS::Alias` publishes, which is what this arm carries to KMS: the
/// alias's name and the key it points at. A declaration naming anything else is refused
/// rather than deployed with the property dropped, exactly as a key's is
/// ([`KMS_KEY_PROPERTIES`]).
pub(super) const KMS_ALIAS_PROPERTIES: &[&str] = &[KMS_ALIAS_NAME_PROPERTY, KMS_ALIAS_TARGET_PROPERTY];

/// Create the alias an `AWS::KMS::Alias` declares, in KMS itself, so a client that deployed
/// the stack resolves the key by that friendly name afterwards — `DescribeKey` with
/// `alias/<name>` (or the alias ARN) answers the target key's metadata — rather than KMS's
/// `NotFoundException` for an alias nothing created.
///
/// The alias's identity is CloudFormation's: the type requires an `AliasName`, and that
/// property's value IS the resource's physical id, so the alias is created under the very
/// name the stack publishes for the resource. The type publishes no `Fn::GetAtt` attribute,
/// so there is nothing to adopt from the answer — `CreateAlias` "does not return a
/// response".
///
/// The `TargetKeyId` a declaration states is normally a reference to a key of the same
/// template, which by the time this runs carries the identifier KMS minted for that key
/// ([`super::provisioning`]'s re-pointing), so the alias is bound to the key the stack
/// really created. A target KMS resolves to no key is that service's own refusal, and it is
/// what this answers with: the resource's create FAILED, which is what makes the stack roll
/// back instead of reporting a deployment whose alias does not exist.
pub(super) async fn create_kms_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let properties = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if let Some(unmapped) = kms_alias_unmapped_property(&properties) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an {KMS_ALIAS} to \
                 KMS; the resource was not created."
            ),
        });
    }
    // Both members are required by `CreateAlias` and by the type, so a declaration missing
    // either names no alias that could be created and is refused here rather than with a
    // request KMS would have to reject.
    let alias_name = required_property(resource, KMS_ALIAS_NAME_PROPERTY)?;
    let target_key_id = required_property(resource, KMS_ALIAS_TARGET_PROPERTY)?;
    call_kms(
        ctx,
        region,
        "CreateAlias",
        &serde_json::json!({ "AliasName": alias_name, "TargetKeyId": target_key_id }),
    )
    .await?;
    Ok(Created::default())
}

/// The first declared property of an `AWS::KMS::Alias` this arm does not carry to KMS, or
/// `None` for a declaration whose every property it does ([`KMS_ALIAS_PROPERTIES`]).
pub(super) fn kms_alias_unmapped_property(properties: &Map<String, Value>) -> Option<String> {
    unmapped_property(properties, KMS_ALIAS_PROPERTIES)
}

/// Apply a changed `AWS::KMS::Alias` declaration to the alias that already exists, through
/// the KMS call that owns the one property of it that can change: `UpdateAlias`
/// "associates an existing KMS alias with a different KMS key".
///
/// The alias's NAME cannot change here — "you cannot use `UpdateAlias` to change an alias
/// name" — and it does not have to: the name is the resource's physical id, so a declaration
/// that renames the alias is a different resource, which the deployment creates and whose
/// predecessor it deletes rather than updating ([`super::provisioning::is_same_resource`]).
/// So what is left for this arm is the target key, and nothing is called where the
/// declaration points the alias at the key it already points at.
pub(super) async fn update_kms_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let after_properties = match declared {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if let Some(unmapped) = kms_alias_unmapped_property(&after_properties) {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not carry the {unmapped} property of an {KMS_ALIAS} to \
                 KMS; the resource was left as it is."
            ),
        });
    }
    let before = resource
        .properties
        .get(KMS_ALIAS_TARGET_PROPERTY)
        .and_then(scalar_text);
    let after = after_properties
        .get(KMS_ALIAS_TARGET_PROPERTY)
        .and_then(scalar_text)
        .filter(|target| !target.is_empty());
    let Some(target_key_id) = after else {
        // An alias points at ONE key, and `UpdateAlias` has no way to unbind it: a
        // declaration that states no target describes no alias that could exist, so it is
        // refused rather than applied as a change that dropped the caller's property.
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!("{KMS_ALIAS} declares no {KMS_ALIAS_TARGET_PROPERTY}"),
        });
    };
    if before.as_deref() == Some(target_key_id.as_str()) {
        return Ok(Some(Created::default()));
    }
    call_kms(
        ctx,
        region,
        "UpdateAlias",
        &serde_json::json!({
            "AliasName": resource.physical_resource_id.clone(),
            "TargetKeyId": target_key_id,
        }),
    )
    .await?;
    // The alias keeps its name, which is the identity everything that resolves it addresses
    // it by: this arm reports no attribute of its own.
    Ok(Some(Created::default()))
}

/// Delete the alias a stack created. An alias is deleted at once — there is no waiting
/// period, unlike the key it points at — and deleting it leaves that key exactly as it is
/// ("These operations don't affect the underlying KMS key").
///
/// An alias KMS no longer holds (`NotFoundException`, judged on the CODE and never on a
/// message that carries the request's own name) is already in the state the delete asks for.
pub(super) async fn delete_kms_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    alias_name: &str,
) -> Result<(), Refusal> {
    match call_kms(
        ctx,
        region,
        "DeleteAlias",
        &serde_json::json!({ "AliasName": alias_name }),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == KMS_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether KMS still holds the alias a stack created, for drift detection: an alias IS an
/// identifier of a key, so asking KMS about the key the alias names resolves one that exists
/// and answers `NotFoundException` for one that does not — which is the `DELETED` drift
/// status.
///
/// No property of the type is reported with it. `AliasName` is the resource's own identity,
/// and the `TargetKeyId` a read could report is the key's canonical id while the declaration
/// may name that same key by its ARN: the drift comparison is textual
/// (`super::drift::differs`), so reporting it would read a difference in the identifier's
/// FORM as a changed target. A property the read does not report is not compared, which is
/// what keeps this check to what it really establishes.
pub(super) async fn describe_kms_alias(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    alias_name: &str,
) -> Result<Option<Value>, Refusal> {
    if describe_kms_key_metadata(ctx, region, alias_name)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    Ok(Some(Value::Object(Map::new())))
}

// ---------------------------------------------------------------------------
// Amazon SES: AWS::SES::ConfigurationSet
// ---------------------------------------------------------------------------

/// Amazon SES's signing name and API version, from its model (`sigv4Name: ses`, service
/// shape `SimpleEmailService`, `version: 2010-12-01`), which speaks `awsQuery` — so its
/// calls ride on [`call_query`] rather than on [`call_json_rpc`].
pub(super) const SES_SIGNING_NAME: &str = "ses";
pub(super) const SES_API_VERSION: &str = "2010-12-01";

/// The error code Amazon SES answers for a configuration set it does not hold:
/// `ConfigurationSetDoesNotExistException`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="DeleteConfigurationSet")`), which is what an
/// awsQuery client branches on.
pub(super) const CONFIGURATION_SET_DOES_NOT_EXIST: &str = "ConfigurationSetDoesNotExist";

/// The `AWS::SES::ConfigurationSet` properties this arm maps onto a call of the owning
/// service, each as the awsQuery member of the call that takes it.
///
/// Source: that type's CloudFormation resource reference against
/// `smithy_query(operation="CreateConfigurationSet,CreateConfigurationSetTrackingOptions")`.
/// `TrackingOptions` is the only one, because `CreateConfigurationSet` takes nothing but the
/// set's `Name` — which CloudFormation decides, and which
/// [`CONFIGURATION_SET_PROPERTIES_NOT_SENT`] therefore lifts out — and the custom
/// open-and-click domain declared beside it is a second call on the set. Every other
/// published property of the type (`DeliveryOptions`, `ReputationOptions`, `SendingOptions`,
/// `SuppressionOptions`, `VdmOptions`) reaches no operation this emulator implements YET —
/// the first three are modelled operations that are still scaffolds, and the last two belong
/// to the v2 API — so [`query_members`] REFUSES a declaration carrying one rather than
/// reporting a set created without it.
pub(super) const CONFIGURATION_SET_PROPERTIES: &[(&str, QueryMember)] =
    &[("TrackingOptions", QueryMember::Structure)];

/// The `AWS::SES::ConfigurationSet` property the arm pushes itself: the set's name, which is
/// this resource's physical id ("Ref returns the configuration set name").
pub(super) const CONFIGURATION_SET_PROPERTIES_NOT_SENT: &[&str] = &["Name"];

/// The one member of a declared `TrackingOptions` that is a member of this service's own
/// `TrackingOptions` shape (`smithy_query(query="shape", shape="TrackingOptions")`), as the
/// awsQuery parameter [`push_structure`] writes it under. The type's other published member
/// of that structure reaches no operation of this service, so it is refused rather than sent
/// to be dropped.
pub(super) const CONFIGURATION_SET_TRACKING_MEMBER: &str = "TrackingOptions.CustomRedirectDomain";

/// The attribute group `DescribeConfigurationSet` has to be ASKED for to report the set's
/// tracking options: a value of its own `ConfigurationSetAttribute` enum
/// (`smithy_query(query="shape", shape="ConfigurationSetAttribute")`).
pub(super) const CONFIGURATION_SET_TRACKING_ATTRIBUTE: &str = "trackingOptions";

/// Create the configuration set an `AWS::SES::ConfigurationSet` declares, under the name
/// CloudFormation decided for it — this resource's physical id, which is what `Ref` returns
/// for the type.
///
/// The declared `TrackingOptions` is no member of that create at all: a set's custom
/// open-and-click domain is written by the `CreateConfigurationSetTrackingOptions` a client
/// makes next, so the arm makes that second call on the set it just created. A domain the
/// owning service refuses fails the resource with the set removed again, so the failed
/// resource leaves no configuration set behind that carries none of the tracking options its
/// declaration asked for.
pub(super) async fn create_ses_configuration_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let tracking = query_members(
        &resource.properties,
        CONFIGURATION_SET_PROPERTIES,
        &[],
        CONFIGURATION_SET_PROPERTIES_NOT_SENT,
        SES_CONFIGURATION_SET,
    )?;
    if let Some((member, _)) = tracking
        .iter()
        .find(|(member, _)| member != CONFIGURATION_SET_TRACKING_MEMBER)
    {
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not send {SES_CONFIGURATION_SET}'s {member} property to \
                 the service that owns the type, so the resource was not created"
            ),
        });
    }
    let name = &resource.physical_resource_id;
    call_ses(
        ctx,
        region,
        "CreateConfigurationSet",
        &[("ConfigurationSet.Name".to_string(), name.clone())],
    )
    .await?;
    if !tracking.is_empty() {
        let mut parameters = vec![("ConfigurationSetName".to_string(), name.clone())];
        parameters.extend(tracking);
        if let Err(refusal) =
            call_ses(ctx, region, "CreateConfigurationSetTrackingOptions", &parameters).await
        {
            // The set this arm created a moment ago is removed before the refusal is
            // reported, exactly as the Amazon EKS access-entry arm removes the entry whose
            // policy association was refused.
            let _ = delete_ses_configuration_set(ctx, region, name).await;
            return Err(refusal);
        }
    }
    // "Ref returns the configuration set name", which is the physical id the set was created
    // under, and the type publishes no `Fn::GetAtt` attribute.
    Ok(Created::default())
}

/// Delete the configuration set a stack created, addressed by its NAME — this resource's
/// physical id.
pub(super) async fn delete_ses_configuration_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![(
        "ConfigurationSetName".to_string(),
        physical_resource_id.to_string(),
    )];
    match call_ses(ctx, region, "DeleteConfigurationSet", &parameters).await {
        Ok(_) => Ok(()),
        // A set the owning service no longer holds is already in the state the delete asks
        // for, judged on the CODE its modelled does-not-exist error carries and never on a
        // message, which interpolates the request's own name.
        Err(refusal) if refusal.code == CONFIGURATION_SET_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The configuration set as Amazon SES reports it NOW, for drift detection, keyed by the
/// property names the declaring type spells them with. One it no longer holds was deleted.
///
/// `DescribeConfigurationSet` reports an attribute group only where the request NAMES it, so
/// the read asks for the one group a declaration of this type can carry
/// ([`CONFIGURATION_SET_PROPERTIES`]). A group the set holds no association for is reported
/// absent, and a property the actual document does not carry is not evidence of anything
/// (see [`super::drift`]).
pub(super) async fn describe_ses_configuration_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![
        (
            "ConfigurationSetName".to_string(),
            physical_resource_id.to_string(),
        ),
        (
            "ConfigurationSetAttributeNames.member.1".to_string(),
            CONFIGURATION_SET_TRACKING_ATTRIBUTE.to_string(),
        ),
    ];
    let answered = match call_ses(ctx, region, "DescribeConfigurationSet", &parameters).await {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == CONFIGURATION_SET_DOES_NOT_EXIST => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let mut reported = Map::new();
    reported.extend(xml_scalar_children(&answered, "ConfigurationSet"));
    let tracking = xml_scalar_children(&answered, "TrackingOptions");
    if !tracking.is_empty() {
        reported.insert("TrackingOptions".to_string(), Value::Object(tracking));
    }
    Ok(Some(Value::Object(reported)))
}

/// Call one Amazon SES operation on the awsQuery wire its model declares.
pub(super) async fn call_ses(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    action: &str,
    parameters: &[(String, String)],
) -> Result<String, Refusal> {
    call_query(
        ctx,
        SES_SIGNING_NAME,
        region,
        SES_API_VERSION,
        action,
        parameters,
    )
    .await
}

// ---------------------------------------------------------------------------
// Amazon SES: AWS::SES::ReceiptRuleSet and AWS::SES::ReceiptRule
// ---------------------------------------------------------------------------

/// The error code Amazon SES answers for a receipt rule SET it does not hold:
/// `RuleSetDoesNotExistException`'s own `awsQueryError` code
/// (`smithy_query(query="operation", operation="DescribeReceiptRuleSet")`), which is what an
/// awsQuery client branches on.
pub(super) const RULE_SET_DOES_NOT_EXIST: &str = "RuleSetDoesNotExist";

/// The error code it answers for a receipt RULE a set it does hold carries no entry for
/// (`RuleDoesNotExistException`'s own code, on `DescribeReceiptRule`).
pub(super) const RULE_DOES_NOT_EXIST: &str = "RuleDoesNotExist";

/// The `AWS::SES::ReceiptRuleSet` properties this arm maps onto a call of the owning
/// service: NONE of them, because `CreateReceiptRuleSet` takes nothing but the set's
/// `RuleSetName` — which CloudFormation decides, and which
/// [`RECEIPT_RULE_SET_PROPERTIES_NOT_SENT`] therefore lifts out.
///
/// Source: that type's CloudFormation resource reference against
/// `smithy_query(operation="CreateReceiptRuleSet")`. The type publishes that one property,
/// so an unpublished one reaches [`query_members`] against this empty catalogue and is
/// REFUSED rather than reported as a set created without it.
pub(super) const RECEIPT_RULE_SET_PROPERTIES: &[(&str, QueryMember)] = &[];

/// The `AWS::SES::ReceiptRuleSet` property the arm pushes itself: the set's name, which is
/// this resource's physical id ("Ref returns the rule set name").
pub(super) const RECEIPT_RULE_SET_PROPERTIES_NOT_SENT: &[&str] = &["RuleSetName"];

/// The `AWS::SES::ReceiptRule` properties `CreateReceiptRule` takes under the SAME member
/// name, each as the awsQuery member of that call.
///
/// Source: that type's CloudFormation resource reference against
/// `smithy_query(operation="CreateReceiptRule")` — the set the rule belongs to, the rule
/// the new one is placed after, and the rule itself, spelled identically on both sides.
pub(super) const RECEIPT_RULE_PROPERTIES: &[(&str, QueryMember)] = &[
    ("After", QueryMember::Scalar),
    ("RuleSetName", QueryMember::Scalar),
];

/// The `AWS::SES::ReceiptRule` property the arm pushes itself: the rule, whose `Name`
/// member CloudFormation decides ([`create_ses_receipt_rule`]).
pub(super) const RECEIPT_RULE_PROPERTIES_NOT_SENT: &[&str] = &["Rule"];

/// The member of a declared `Rule` that carries the rule's own name, which is this
/// resource's physical id ("Ref returns the receipt rule name") and a REQUIRED member of
/// `CreateReceiptRule`'s `Rule` (`smithy_query(query="shape", shape="ReceiptRule")`).
pub(super) const RECEIPT_RULE_NAME_MEMBER: &str = "Name";

/// The property that carries the rule the arm sends, and the awsQuery prefix its members
/// are written under.
pub(super) const RECEIPT_RULE_MEMBER: &str = "Rule";

/// Create the receipt rule set an `AWS::SES::ReceiptRuleSet` declares, under the name
/// CloudFormation decided for it — this resource's physical id, which is what `Ref` returns
/// for the type.
///
/// "Creates an empty receipt rule set": the rules of such a set are the
/// `AWS::SES::ReceiptRule` resources declared against it, each its own resource of its own
/// type, so this call carries none.
pub(super) async fn create_ses_receipt_rule_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    query_members(
        &resource.properties,
        RECEIPT_RULE_SET_PROPERTIES,
        &[],
        RECEIPT_RULE_SET_PROPERTIES_NOT_SENT,
        SES_RECEIPT_RULE_SET,
    )?;
    call_ses(
        ctx,
        region,
        "CreateReceiptRuleSet",
        &[(
            "RuleSetName".to_string(),
            resource.physical_resource_id.clone(),
        )],
    )
    .await?;
    // "Ref returns the rule set name", which is the physical id the set was created under,
    // and the type publishes no `Fn::GetAtt` attribute.
    Ok(Created::default())
}

/// Delete the receipt rule set a stack created, addressed by its NAME — this resource's
/// physical id.
///
/// `DeleteReceiptRuleSet` models no does-not-exist error at all ("deletes the specified
/// receipt rule set and all of the receipt rules it contains"), so a set the owning service
/// no longer holds is already in the state this delete asks for and that service says so
/// itself. The one refusal it does model — the set email receiving is served by cannot be
/// deleted — is reported as the refusal it is.
pub(super) async fn delete_ses_receipt_rule_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let parameters = vec![("RuleSetName".to_string(), physical_resource_id.to_string())];
    call_ses(ctx, region, "DeleteReceiptRuleSet", &parameters).await?;
    Ok(())
}

/// The receipt rule set as Amazon SES reports it NOW, for drift detection, keyed by the one
/// property the declaring type spells. One it no longer holds was deleted.
///
/// `DescribeReceiptRuleSet` reports the set's name under its `Metadata`, so the reported
/// document carries that name under the property the declaration states it as. The set's
/// rules are not part of it: each is an `AWS::SES::ReceiptRule` resource of its own, with
/// its own declaration and its own drift read.
pub(super) async fn describe_ses_receipt_rule_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![("RuleSetName".to_string(), physical_resource_id.to_string())];
    let answered = match call_ses(ctx, region, "DescribeReceiptRuleSet", &parameters).await {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == RULE_SET_DOES_NOT_EXIST => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let mut reported = Map::new();
    if let Some(name) = xml_scalar_children(&answered, "Metadata").get("Name") {
        reported.insert("RuleSetName".to_string(), name.clone());
    }
    Ok(Some(Value::Object(reported)))
}

/// Create the receipt rule an `AWS::SES::ReceiptRule` declares, in the rule set its own
/// declaration names.
///
/// The type's properties ARE `CreateReceiptRule`'s own members, so the evaluated document is
/// the request — with the one member CloudFormation decides set on the way out: the rule's
/// `Name` is a REQUIRED member of that call, so a declaration stating none has this
/// resource's own computed name pushed for it rather than reaching the owning service as a
/// rule with no name. That name is what the type's `Ref` returns, so it is answered as the
/// resource's physical id. A declaration carrying no `Rule` structure at all describes no
/// rule and is refused rather than sent as a rule carrying only that name.
pub(super) async fn create_ses_receipt_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        RECEIPT_RULE_PROPERTIES,
        &[],
        RECEIPT_RULE_PROPERTIES_NOT_SENT,
        SES_RECEIPT_RULE,
    )?;
    let declared = resource
        .properties
        .get(RECEIPT_RULE_MEMBER)
        .cloned()
        .unwrap_or(Value::Null);
    let name = declared
        .get(RECEIPT_RULE_NAME_MEMBER)
        .and_then(scalar_text)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| resource.physical_resource_id.clone());
    // `Rule` is the STRUCTURE the create asks for, and the type requires it: a declaration
    // carrying no such structure describes no rule, so it is REFUSED here rather than sent
    // as a rule carrying nothing but the name CloudFormation decided — the same reason
    // [`query_members`] refuses a property this emulator sends nowhere.
    let Value::Object(mut rule) = declared else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "{} declares no {RECEIPT_RULE_MEMBER}",
                resource.resource_type
            ),
        });
    };
    rule.insert(
        RECEIPT_RULE_NAME_MEMBER.to_string(),
        Value::String(name.clone()),
    );
    push_structure(
        &mut parameters,
        RECEIPT_RULE_MEMBER,
        &Value::Object(rule),
        0,
    )?;
    call_ses(ctx, region, "CreateReceiptRule", &parameters).await?;
    Ok(Created {
        physical_resource_id: Some(name),
        attributes: BTreeMap::new(),
    })
}

/// Delete the receipt rule a stack created, addressed by the PAIR Amazon SES holds one
/// under: its own name — this resource's physical id — and the set that carries it, which
/// only the resource's own declaration names.
///
/// `DeleteReceiptRule` models one error, for the SET: a set the owning service no longer
/// holds carries no rules either, so that refusal is a request asking for a state that
/// service is already in. A rule the set itself no longer carries is already in that state
/// too, which `DeleteReceiptRule` answers as the success it is.
pub(super) async fn delete_ses_receipt_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let parameters = vec![
        (
            "RuleSetName".to_string(),
            required_property(resource, "RuleSetName")?,
        ),
        (
            "RuleName".to_string(),
            resource.physical_resource_id.clone(),
        ),
    ];
    match call_ses(ctx, region, "DeleteReceiptRule", &parameters).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == RULE_SET_DOES_NOT_EXIST => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The receipt rule as Amazon SES reports it NOW, for drift detection: the set it is held
/// in, read back by the same pair the delete addresses it by. A rule — or a set — the owning
/// service no longer holds was deleted.
///
/// What is reported is the rule's PLACE, not its configuration: `DescribeReceiptRule`
/// answers the rule as the nested action structures and recipient list of its own shape,
/// which the XML readers here carry no document for, and reporting only the members they do
/// carry would read as a REMOVAL of every member left out (see [`super::drift`]). So the
/// declared `Rule` is compared against nothing rather than against a document missing most
/// of it, while the rule's existence and the set it belongs to are reported.
pub(super) async fn describe_ses_receipt_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let rule_set_name = required_property(resource, "RuleSetName")?;
    let parameters = vec![
        ("RuleSetName".to_string(), rule_set_name.clone()),
        (
            "RuleName".to_string(),
            resource.physical_resource_id.clone(),
        ),
    ];
    match call_ses(ctx, region, "DescribeReceiptRule", &parameters).await {
        Ok(_) => {
            let mut reported = Map::new();
            reported.insert(
                "RuleSetName".to_string(),
                Value::String(rule_set_name),
            );
            Ok(Some(Value::Object(reported)))
        }
        Err(refusal)
            if refusal.code == RULE_DOES_NOT_EXIST || refusal.code == RULE_SET_DOES_NOT_EXIST =>
        {
            Ok(None)
        }
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon Route 53: AWS::Route53::HostedZone and AWS::Route53::RecordSet
// ---------------------------------------------------------------------------

/// Route 53's signing name, from its model (`sigv4Name: route53`), which speaks `restXml`.
pub(super) const ROUTE53_SIGNING_NAME: &str = "route53";

/// The API-version prefix every Route 53 operation's URI carries, from the HTTP bindings
/// its model declares (`POST /2013-04-01/hostedzone`).
pub(super) const ROUTE53_URI_PREFIX: &str = "/2013-04-01";

/// The namespace a Route 53 request document carries: that model's own
/// `xmlNamespace.uri`.
pub(super) const ROUTE53_XML_NAMESPACE: &str = "https://route53.amazonaws.com/doc/2013-04-01/";

/// The error code Route 53 answers for a hosted zone it does not hold: "No hosted zone
/// exists with the ID that you specified" (`NoSuchHostedZone`).
pub(super) const NO_SUCH_HOSTED_ZONE: &str = "NoSuchHostedZone";

/// The prefix Route 53 wraps a hosted zone id in where it reports one
/// (`HostedZone$Id` comes back as `/hostedzone/Z…`), and which every operation that TAKES a
/// zone id accepts beside the bare form.
pub(super) const HOSTED_ZONE_ID_PATH_PREFIX: &str = "/hostedzone/";

/// The `ResourceType` a tagging call names a hosted zone and a health check under: "the
/// resource type for hosted zones is `hostedzone`" and "the resource type for health checks
/// is `healthcheck`" (`ChangeTagsForResource$ResourceType`).
pub(super) const HOSTED_ZONE_TAG_RESOURCE_TYPE: &str = "hostedzone";
pub(super) const HEALTH_CHECK_TAG_RESOURCE_TYPE: &str = "healthcheck";

/// The most tags one `ChangeTagsForResource` carries: its `AddTags` list is
/// `@length(min: 1, max: 10)`, so a larger set is applied in that many calls.
pub(super) const MAX_TAGS_PER_TAG_CHANGE: usize = 10;

/// The two change-batch actions this module asks for: "CREATE: Creates a resource record
/// set that has the specified values" and "DELETE: Deletes a existing resource record set"
/// (`Change$Action`).
pub(super) const RECORD_SET_CREATE: &str = "CREATE";
pub(super) const RECORD_SET_DELETE: &str = "DELETE";

/// The properties of `AWS::Route53::HostedZone` this arm maps onto `CreateHostedZone` and
/// the calls beside it.
///
/// Source: AWS's CloudFormation resource reference for the type. `Name` is the zone's DNS
/// name, `HostedZoneConfig` carries the zone's comment, `VPCs` are the Amazon VPCs a private
/// zone answers for, and `HostedZoneTags` are the zone's own tags. The one published
/// property NOT here is `QueryLoggingConfig`, which is a `CreateQueryLoggingConfig` of its
/// own that this emulator does not serve yet — a declaration carrying it is REFUSED rather
/// than reported as a zone created without the query logging it asked for.
pub(super) const HOSTED_ZONE_PROPERTIES: &[&str] = &["Name", "HostedZoneConfig", "VPCs", "HostedZoneTags"];

/// Call one Route 53 operation in process, on the URI its model binds that operation to.
pub(super) async fn call_route53(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&str>,
) -> Result<String, Refusal> {
    call_rest_xml(
        ctx,
        ROUTE53_SIGNING_NAME,
        region,
        method,
        &format!("{ROUTE53_URI_PREFIX}{uri}"),
        body,
        // Route 53 says everything it says about a resource in the answer's body: no header
        // of its answers addresses the next call, so none is sent and none is read.
        &[],
        &[],
    )
    .await
    .map(|(document, _headers)| document)
}

/// One Route 53 request document: the operation's input shape as the root element, under
/// the namespace that model declares, wrapping the members already rendered.
pub(super) fn route53_request(root: &str, members: &str) -> String {
    format!("<{root} xmlns=\"{ROUTE53_XML_NAMESPACE}\">{members}</{root}>")
}

/// A hosted zone id in the BARE form every `ResourceId` member is bounded for: the
/// `/hostedzone/` prefix a Route 53 answer wraps one in, stripped.
///
/// A value that does not carry the prefix is already bare, which is what a template that
/// states a zone id literally writes.
pub(super) fn bare_hosted_zone_id(id: &str) -> &str {
    id.strip_prefix(HOSTED_ZONE_ID_PATH_PREFIX).unwrap_or(id)
}

/// A DNS name in the fully-qualified form Route 53 compares names in: lower case with a
/// trailing dot, since "Route 53 treats `www.example.com` (without a trailing dot) and
/// `www.example.com.` (with a trailing dot) as identical" (`ResourceRecordSet$Name`).
pub(super) fn fully_qualified(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    if name.ends_with('.') {
        name
    } else {
        format!("{name}.")
    }
}

/// The refusal for a declared property no arm maps onto the owning API's request, naming
/// which property it was: refusing is what keeps a resource from being reported as created
/// with less than its declaration asked for.
pub(super) fn refuse_unmapped_properties(
    resource: &StoredResource,
    mapped: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = &resource.properties else {
        return Ok(());
    };
    match unmapped_property(properties, mapped) {
        Some(property) => Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {}",
                resource.resource_type
            ),
        }),
        None => Ok(()),
    }
}

/// The first property of a declared document that `mapped` does not name, or `None` for a
/// document whose every property it does.
///
/// The ONE place that rule is written: an arm that refuses such a property outright goes
/// through [`refuse_unmapped_properties`], and an arm whose refusal says more than that
/// ([`create_kms_key`]'s, which says whether the resource was created) asks this directly.
pub(super) fn unmapped_property(properties: &Map<String, Value>, mapped: &[&str]) -> Option<String> {
    properties
        .keys()
        .find(|property| !mapped.contains(&property.as_str()))
        .cloned()
}

/// The `CallerReference` CloudFormation sends with a Route 53 create that requires one.
///
/// `CreateHostedZone$CallerReference` and `CreateHealthCheck$CallerReference` are both
/// REQUIRED ("a unique string that identifies the request and that allows failed
/// CreateHostedZone requests to be retried without the risk of executing the operation
/// twice") and neither resource type publishes a property for it, so it is
/// CloudFormation's to supply. It is derived from the stack this resource belongs to and its
/// place in it — the two tags CloudFormation records on every resource it provisions — so it
/// is unique per stack and logical id, and the SAME string if this deployment is attempted
/// again, which is exactly the retry that member exists for. The digest keeps it inside the
/// shorter of the two bounds (`Nonce` is 1..128 and `HealthCheckNonce` 1..64) whatever the
/// stack is called, and `salt` keeps the reference of one stack's zone apart from its health
/// check's.
pub(super) fn route53_caller_reference(
    salt: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> String {
    let stack = propagated
        .iter()
        .find(|(key, _)| key == super::provisioning::STACK_ID_TAG)
        .map(|(_, value)| value.as_str())
        .unwrap_or_default();
    super::template::content_digest(salt, &format!("{stack}/{}", resource.logical_id), 32)
}

/// The salt each Route 53 type's caller reference is derived under.
pub(super) const HOSTED_ZONE_CALLER_REFERENCE_SALT: &str = "route53-hosted-zone";
pub(super) const HEALTH_CHECK_CALLER_REFERENCE_SALT: &str = "route53-health-check";

/// Each Amazon VPC a hosted zone's declaration names, as the `VPC` element `CreateHostedZone`
/// and `AssociateVPCWithHostedZone` both take.
///
/// `VPCs` is a list of `{VPCId, VPCRegion}`, spelled exactly as that shape's own members
/// are, so each entry is rendered member by member. A declaration whose `VPCs` is not a list
/// of such structures describes no VPC association and is refused.
pub(super) fn hosted_zone_vpcs(resource: &StoredResource) -> Result<Vec<String>, Refusal> {
    let Some(declared) = resource.properties.get("VPCs") else {
        return Ok(Vec::new());
    };
    let Value::Array(entries) = declared else {
        return Err(malformed_property("VPCs"));
    };
    let mut elements = Vec::with_capacity(entries.len());
    for entry in entries {
        let Value::Object(members) = entry else {
            return Err(malformed_property("VPCs"));
        };
        let mut rendered = String::new();
        // In the order the `VPC` shape declares them.
        for member in ["VPCRegion", "VPCId"] {
            if let Some(value) = members.get(member).and_then(scalar_text) {
                rendered.push_str(&xml_leaf(member, &value));
            }
        }
        elements.push(format!("<VPC>{rendered}</VPC>"));
    }
    Ok(elements)
}

/// Create the hosted zone an `AWS::Route53::HostedZone` declares, so a client reading the
/// stack's outputs and then calling Route 53 with that value — `GetHostedZone`,
/// `TestDNSAnswer`, a record set's own `HostedZoneId` — reaches the zone the template asked
/// for.
///
/// `Ref` on this resource returns the hosted zone ID, which `CreateHostedZone` MINTS and
/// reports in its `/hostedzone/`-prefixed form, so the bare id that answer carries replaces
/// the computed physical id and the type's `Id` attribute is read off it
/// ([`super::provisioning::adopt_minted_identity`]).
///
/// A private zone's first declared VPC is carried on the create — "if you are associating a
/// VPC with a hosted zone with this request, the paramaters VPCId and VPCRegion are also
/// required" — and each further one is an `AssociateVPCWithHostedZone` of its own on the zone
/// just created, the same second call a client makes. The zone carries the tags its
/// declaration asks for plus the ones CloudFormation gives the resource ([`tags_of`]),
/// applied through `ChangeTagsForResource`, which is how Route 53 tags a zone:
/// `CreateHostedZone` takes no tags at all.
pub(super) async fn create_hosted_zone(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, HOSTED_ZONE_PROPERTIES)?;
    // Required by the type and by the create alike: a declaration with no name describes no
    // zone that could be created, so it is refused here rather than with a request Route 53
    // would have to reject.
    let name = property(resource, "Name").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property Name cannot be empty for resource of type {}",
            resource.resource_type
        ),
    })?;
    let vpcs = hosted_zone_vpcs(resource)?;
    // In the order `CreateHostedZoneRequest` declares its members.
    let mut members = xml_leaf("Name", &name);
    if let Some(first) = vpcs.first() {
        members.push_str(first);
    }
    members.push_str(&xml_leaf(
        "CallerReference",
        &route53_caller_reference(HOSTED_ZONE_CALLER_REFERENCE_SALT, resource, propagated),
    ));
    // The type's `HostedZoneConfig` publishes `Comment` alone; whether the zone is private
    // follows from the VPCs it names, which is what `CreateHostedZone` reads it from.
    if let Some(comment) = resource
        .properties
        .get("HostedZoneConfig")
        .and_then(|config| config.get("Comment"))
        .and_then(scalar_text)
    {
        members.push_str(&format!(
            "<HostedZoneConfig>{}</HostedZoneConfig>",
            xml_leaf("Comment", &comment)
        ));
    }
    let answered = call_route53(
        ctx,
        region,
        "POST",
        "/hostedzone",
        Some(&route53_request("CreateHostedZoneRequest", &members)),
    )
    .await?;
    let reported = xml_scalar_children(&answered, "HostedZone");
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .map(bare_hosted_zone_id)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateHostedZone answered no HostedZone.Id".to_string(),
        })?
        .to_string();
    for vpc in vpcs.iter().skip(1) {
        call_route53(
            ctx,
            region,
            "POST",
            &format!("/hostedzone/{}/associatevpc", percent_encoded(&id)),
            Some(&route53_request(
                "AssociateVPCWithHostedZoneRequest",
                vpc,
            )),
        )
        .await?;
    }
    change_route53_tags(
        ctx,
        region,
        HOSTED_ZONE_TAG_RESOURCE_TYPE,
        &id,
        &route53_tags(resource, "HostedZoneTags", propagated),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: hosted_zone_name_servers(&answered),
    })
}

/// The `NameServers` the zone's delegation set reports, as this type's list-valued
/// `Fn::GetAtt` attribute: "`NameServers` — returns the set of name servers for the specific
/// hosted zone".
///
/// Route 53 ASSIGNS them — `CreateHostedZone` answers "a complex type that describes the name
/// servers for this hosted zone" — so they are read off that answer rather than computed, and
/// carried element by element because the attribute map is string-valued
/// ([`list_attribute_element`]). A zone whose answer reports none carries no such attribute at
/// all, rather than an empty list standing in for the delegation Route 53 made.
fn hosted_zone_name_servers(answered: &str) -> BTreeMap<String, String> {
    xml_elements(answered, "NameServer")
        .into_iter()
        .enumerate()
        .map(|(index, server)| (list_attribute_element("NameServers", index), server))
        .collect()
}

/// The tags a Route 53 resource is to carry: the ones its declaration asks for plus the
/// ones CloudFormation gives the resource.
///
/// Neither Route 53 type spells its tag property `Tags` — a zone's are `HostedZoneTags` and
/// a health check's `HealthCheckTags` — so the declared list is read out of `property` and
/// offered under the name [`tags_of`] looks for, that being the one place the declared and
/// the propagated halves are merged.
pub(super) fn route53_tags(
    resource: &StoredResource,
    property: &str,
    propagated: &[(String, String)],
) -> Vec<(String, String)> {
    let declared = resource
        .properties
        .get(property)
        .cloned()
        .unwrap_or(Value::Null);
    let mut properties = Map::new();
    properties.insert("Tags".to_string(), declared);
    tags_of(&Value::Object(properties), propagated)
}

/// Add `tags` to the Route 53 resource `id` of `resource_type`, in the batches
/// `ChangeTagsForResource` takes ([`MAX_TAGS_PER_TAG_CHANGE`]). A resource to carry no tags
/// at all has nothing called about it: that call's `AddTags` is `@length(min: 1, …)`.
pub(super) async fn change_route53_tags(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_type: &str,
    id: &str,
    tags: &[(String, String)],
) -> Result<(), Refusal> {
    for batch in tags.chunks(MAX_TAGS_PER_TAG_CHANGE) {
        let mut added = String::new();
        for (key, value) in batch {
            added.push_str(&format!(
                "<Tag>{}{}</Tag>",
                xml_leaf("Key", key),
                xml_leaf("Value", value)
            ));
        }
        call_route53(
            ctx,
            region,
            "POST",
            &format!(
                "/tags/{}/{}",
                percent_encoded(resource_type),
                percent_encoded(id)
            ),
            Some(&route53_request(
                "ChangeTagsForResourceRequest",
                &format!("<AddTags>{added}</AddTags>"),
            )),
        )
        .await?;
    }
    Ok(())
}

/// Delete the hosted zone a stack created, by the id its create minted. A zone Route 53 no
/// longer holds (`NoSuchHostedZone`) is already in the state the delete asks for.
///
/// "You can delete a hosted zone only if it contains no resource record sets other than the
/// default SOA record and NS resource record sets": a stack that declares record sets in its
/// own zone deletes them FIRST, because [`super::provisioning::delete`] deletes a stack's
/// resources in the reverse of the order they were created in, and a record set depends on
/// the zone that holds it.
pub(super) async fn delete_hosted_zone(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_route53(
        ctx,
        region,
        "DELETE",
        &format!("/hostedzone/{}", percent_encoded(physical_resource_id)),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The hosted zone's configuration as Route 53 reports it, for drift detection. An id it no
/// longer holds a zone for (`NoSuchHostedZone`) was deleted.
///
/// Only the members `GetHostedZone` reports in the same shape the type declares them in are
/// compared: the zone's `Name`, which is the one property of this type that is also a scalar
/// member of the reported zone. The declared `VPCs` and `HostedZoneTags` are not members of
/// it at all, and a property the actual document does not carry is not evidence of anything
/// ([`super::drift::differs`]).
pub(super) async fn describe_hosted_zone(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_route53(
        ctx,
        region,
        "GET",
        &format!("/hostedzone/{}", percent_encoded(physical_resource_id)),
        None,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let reported = Value::Object(xml_scalar_children(&answered, "HostedZone"));
    Ok(Some(reported_as_properties(&reported, &[("Name", "Name")])))
}

/// What one published property of an Amazon Route 53 resource type is on the shape the
/// operation that deploys it carries — the `ResourceRecordSet` of a change batch, or the
/// `HealthCheckConfig` of a health-check create.
#[derive(Debug, Clone, Copy)]
pub(super) enum Route53Member {
    /// A scalar member, written as its own element carrying the declared text.
    Scalar,
    /// A LIST member: a list of scalars in a template, and on the wire one `entry` element
    /// per value — carrying the value itself (`ChildHealthChecks` is a list of
    /// `ChildHealthCheck` elements), or its one named member where the modelled entry is a
    /// structure (a record's VALUES are a list of `ResourceRecord` structures, "`Value` —
    /// the current or new DNS record value").
    Entries {
        entry: &'static str,
        member: Option<&'static str>,
    },
    /// A nested structure, whose own members are these.
    Structure(&'static [(&'static str, Route53Member)]),
}

/// `GeoLocation`'s members, spelled identically on the type and in the model.
pub(super) const GEO_LOCATION_MEMBERS: &[(&str, Route53Member)] = &[
    ("ContinentCode", Route53Member::Scalar),
    ("CountryCode", Route53Member::Scalar),
    ("SubdivisionCode", Route53Member::Scalar),
];

/// `Coordinates`' members, the latitude and longitude a geoproximity rule is measured from.
pub(super) const COORDINATES_MEMBERS: &[(&str, Route53Member)] = &[
    ("Latitude", Route53Member::Scalar),
    ("Longitude", Route53Member::Scalar),
];

/// `GeoProximityLocation`'s members.
pub(super) const GEO_PROXIMITY_MEMBERS: &[(&str, Route53Member)] = &[
    ("AWSRegion", Route53Member::Scalar),
    ("LocalZoneGroup", Route53Member::Scalar),
    ("Coordinates", Route53Member::Structure(COORDINATES_MEMBERS)),
    ("Bias", Route53Member::Scalar),
];

/// `AliasTarget`'s members — the zone and the DNS name an alias routes to, and whether it
/// inherits that target's health.
pub(super) const ALIAS_TARGET_MEMBERS: &[(&str, Route53Member)] = &[
    ("HostedZoneId", Route53Member::Scalar),
    ("DNSName", Route53Member::Scalar),
    ("EvaluateTargetHealth", Route53Member::Scalar),
];

/// `CidrRoutingConfig`'s members.
pub(super) const CIDR_ROUTING_MEMBERS: &[(&str, Route53Member)] = &[
    ("CollectionId", Route53Member::Scalar),
    ("LocationName", Route53Member::Scalar),
];

/// Every published property of `AWS::Route53::RecordSet` that is a member of the
/// `ResourceRecordSet` the change batch carries, in the order the model declares those
/// members.
///
/// The type's properties ARE that shape's members, spelled identically on both sides, with
/// one exception: `ResourceRecords` is a list of record VALUES in a template and a list of
/// `ResourceRecord` structures on the wire. The three published properties that are NOT
/// members of it are handled by the arm itself — `HostedZoneId` and `HostedZoneName` say
/// which zone the record belongs to (the URI label the operation binds), and `Comment` is
/// the `ChangeBatch`'s own.
pub(super) const RECORD_SET_MEMBERS: &[(&str, Route53Member)] = &[
    ("Name", Route53Member::Scalar),
    ("Type", Route53Member::Scalar),
    ("SetIdentifier", Route53Member::Scalar),
    ("Weight", Route53Member::Scalar),
    ("Region", Route53Member::Scalar),
    ("GeoLocation", Route53Member::Structure(GEO_LOCATION_MEMBERS)),
    ("Failover", Route53Member::Scalar),
    ("MultiValueAnswer", Route53Member::Scalar),
    ("TTL", Route53Member::Scalar),
    (
        "ResourceRecords",
        Route53Member::Entries {
            entry: "ResourceRecord",
            member: Some("Value"),
        },
    ),
    ("AliasTarget", Route53Member::Structure(ALIAS_TARGET_MEMBERS)),
    ("HealthCheckId", Route53Member::Scalar),
    ("CidrRoutingConfig", Route53Member::Structure(CIDR_ROUTING_MEMBERS)),
    (
        "GeoProximityLocation",
        Route53Member::Structure(GEO_PROXIMITY_MEMBERS),
    ),
];

/// The properties of `AWS::Route53::RecordSet` this arm maps onto a change batch: the
/// members of the record set itself, plus the three that are not members of it.
pub(super) fn record_set_properties() -> Vec<&'static str> {
    let mut mapped: Vec<&'static str> = RECORD_SET_MEMBERS.iter().map(|(name, _)| *name).collect();
    mapped.extend(["HostedZoneId", "HostedZoneName", "Comment"]);
    mapped
}

/// One member of a Route 53 shape as the request carries it, or `None` for a property the
/// declaration does not state.
pub(super) fn route53_member(
    name: &str,
    member: Route53Member,
    declared: &Value,
) -> Result<Option<String>, Refusal> {
    match member {
        Route53Member::Scalar => Ok(scalar_text(declared)
            .filter(|text| !text.is_empty())
            .map(|text| xml_leaf(name, &text))),
        Route53Member::Entries { entry, member } => {
            let Value::Array(values) = declared else {
                return Err(malformed_property(name));
            };
            let mut entries = String::new();
            for value in values {
                let value = scalar_text(value).ok_or_else(|| malformed_property(name))?;
                let held = match member {
                    Some(member) => xml_leaf(member, &value),
                    None => ::quick_xml::escape::escape(&value).into_owned(),
                };
                entries.push_str(&format!("<{entry}>{held}</{entry}>"));
            }
            Ok(Some(format!("<{name}>{entries}</{name}>")))
        }
        Route53Member::Structure(members) => {
            let Value::Object(declared) = declared else {
                return Err(malformed_property(name));
            };
            let mut rendered = String::new();
            for (member_name, member) in members {
                if let Some(value) = declared.get(*member_name) {
                    if let Some(written) = route53_member(member_name, *member, value)? {
                        rendered.push_str(&written);
                    }
                }
            }
            // A member of the structure the catalogue does not name is no member of the
            // shape: refused rather than dropped, so a resource is never created with
            // less than its declaration asked for.
            for member_name in declared.keys() {
                if !members.iter().any(|(known, _)| known == member_name) {
                    return Err(malformed_property(&format!("{name}.{member_name}")));
                }
            }
            Ok(Some(format!("<{name}>{rendered}</{name}>")))
        }
    }
}

/// The `ResourceRecordSet` element one `AWS::Route53::RecordSet` declares.
pub(super) fn record_set_element(resource: &StoredResource) -> Result<String, Refusal> {
    record_set_element_of(&resource.resource_type, &resource.properties)
}

/// The `ResourceRecordSet` element one DECLARED record set states, whether that declaration
/// is a resource of its own (`AWS::Route53::RecordSet`) or one entry of a group's
/// `RecordSets` — the same property document in both places, so the element is rendered
/// once.
///
/// `resource_type` is the type the refusal names, which is the type that DECLARED the
/// document.
pub(super) fn record_set_element_of(
    resource_type: &str,
    declared: &Value,
) -> Result<String, Refusal> {
    let mapped = record_set_properties();
    if let Value::Object(properties) = declared {
        if let Some(property) = unmapped_property(properties, &mapped) {
            return Err(Refusal {
                code: NOT_IMPLEMENTED.to_string(),
                message: format!(
                    "this emulator does not apply the {property} property of {resource_type}"
                ),
            });
        }
    }
    for required in ["Name", "Type"] {
        if declared.get(required).and_then(scalar_text).is_none() {
            return Err(Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "Property {required} cannot be empty for resource of type {resource_type}"
                ),
            });
        }
    }
    let mut rendered = String::new();
    for (name, member) in RECORD_SET_MEMBERS {
        let Some(value) = declared.get(*name) else {
            continue;
        };
        if let Some(written) = route53_member(name, *member, value)? {
            rendered.push_str(&written);
        }
    }
    Ok(format!("<ResourceRecordSet>{rendered}</ResourceRecordSet>"))
}

/// The hosted zone a record set's declaration names, as the id Route 53 addresses it by.
///
/// A declaration states one of the two published properties: `HostedZoneId`, which IS that
/// id — and is what a reference to a zone of the same template resolves to — or
/// `HostedZoneName`, the zone's DNS name, which `ListHostedZonesByName` resolves to the id
/// the same way a client does. A declaration that states neither names no zone to write the
/// record in and is refused.
pub(super) async fn record_set_zone_id(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<String, Refusal> {
    if let Some(id) = property(resource, "HostedZoneId") {
        return Ok(bare_hosted_zone_id(&id).to_string());
    }
    let Some(name) = property(resource, "HostedZoneName") else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "{} declares neither HostedZoneId nor HostedZoneName",
                resource.resource_type
            ),
        });
    };
    let wanted = fully_qualified(&name);
    let answered = call_route53(
        ctx,
        region,
        "GET",
        &format!(
            "/hostedzonesbyname?dnsname={}&maxitems=1",
            percent_encoded(&wanted)
        ),
        None,
    )
    .await?;
    // The listing starts at the name asked for, so the first zone it reports is the one
    // that name belongs to — if the account holds it at all.
    let reported = xml_scalar_children(&answered, "HostedZone");
    let found = reported
        .get("Name")
        .and_then(Value::as_str)
        .filter(|held| fully_qualified(held) == wanted)
        .and_then(|_| reported.get("Id"))
        .and_then(Value::as_str)
        .map(bare_hosted_zone_id);
    found.map(ToString::to_string).ok_or_else(|| Refusal {
        code: NO_SUCH_HOSTED_ZONE.to_string(),
        message: format!("No hosted zone found with DNS name {wanted}"),
    })
}

/// Apply one change to the record set an `AWS::Route53::RecordSet` declares, in the zone its
/// declaration names.
pub(super) async fn change_record_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    action: &str,
) -> Result<(), Refusal> {
    let zone = record_set_zone_id(ctx, region, resource).await?;
    let record = record_set_element(resource)?;
    // "Optional: Any comments you want to include about a change batch request"
    // (`ChangeBatch$Comment`), which is the type's own `Comment` property.
    let comment = property(resource, "Comment")
        .map(|comment| xml_leaf("Comment", &comment))
        .unwrap_or_default();
    let batch = format!(
        "<ChangeBatch>{comment}<Changes><Change>{}{record}</Change></Changes></ChangeBatch>",
        xml_leaf("Action", action)
    );
    call_route53(
        ctx,
        region,
        "POST",
        &format!("/hostedzone/{}/rrset", percent_encoded(&zone)),
        Some(&route53_request("ChangeResourceRecordSetsRequest", &batch)),
    )
    .await?;
    Ok(())
}

/// Create the record set an `AWS::Route53::RecordSet` declares, so a DNS query for that name
/// — `TestDNSAnswer`, or a resolver reading the zone — is answered with the values the
/// template declared.
///
/// `Ref` on this resource returns the record's name, which is the `Name` its declaration
/// states and the physical id CloudFormation's naming rules already computed, so the create
/// answers no identity of its own. The type publishes no `Tags` property — Route 53 tags a
/// hosted zone, not a record set — so no tags are propagated to one.
pub(super) async fn create_record_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    change_record_set(ctx, region, resource, RECORD_SET_CREATE).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the record set a stack created, by the same change batch that created it: Route 53
/// removes a record set by its own values ("if the current value does not match the actual
/// value, an error is returned"), so the delete carries the declaration the create was made
/// from.
///
/// A record set Route 53 no longer holds is already in the state the delete asks for, and
/// that is decided by READING it back — Route 53 models no not-found error for a record set,
/// reporting a change batch it cannot apply as one refusal class whatever the problem was.
/// A zone the service no longer holds holds no record set either.
pub(super) async fn delete_record_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    match describe_record_set(ctx, region, resource).await {
        Ok(None) => return Ok(()),
        Ok(Some(_)) => {}
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(()),
        Err(refusal) => return Err(refusal),
    }
    match change_record_set(ctx, region, resource, RECORD_SET_DELETE).await {
        Ok(()) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// Amazon Route 53 record set groups: AWS::Route53::RecordSetGroup
// ---------------------------------------------------------------------------

/// The properties of `AWS::Route53::RecordSetGroup` this arm maps onto a change batch.
///
/// Source: AWS's CloudFormation resource reference for the type. `RecordSets` are the record
/// sets the group holds — each one the same property document an `AWS::Route53::RecordSet`
/// declares — `HostedZoneId` and `HostedZoneName` say which zone they belong to, and
/// `Comment` is the `ChangeBatch`'s own, exactly as for a single record set.
pub(super) const RECORD_SET_GROUP_PROPERTIES: &[&str] =
    &["Comment", "HostedZoneId", "HostedZoneName", "RecordSets"];

/// The `Change` elements one `AWS::Route53::RecordSetGroup` declares, carrying `action` over
/// every record set the group holds.
///
/// "A complex type that contains one `RecordSet` element for each record set that you want to
/// create": the group IS those record sets, so a declaration stating none describes no change
/// to apply and is refused rather than reported as a group created with nothing in it.
pub(super) fn record_set_group_changes(
    resource: &StoredResource,
    action: &str,
) -> Result<String, Refusal> {
    refuse_unmapped_properties(resource, RECORD_SET_GROUP_PROPERTIES)?;
    let declared = resource.properties.get("RecordSets");
    let Some(Value::Array(record_sets)) = declared else {
        return Err(match declared {
            Some(_) => malformed_property("RecordSets"),
            None => Refusal {
                code: MISSING_PROPERTY_CODE.to_string(),
                message: format!(
                    "Property RecordSets cannot be empty for resource of type {}",
                    resource.resource_type
                ),
            },
        });
    };
    if record_sets.is_empty() {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property RecordSets cannot be empty for resource of type {}",
                resource.resource_type
            ),
        });
    }
    let mut changes = String::new();
    for declared in record_sets {
        let record = record_set_element_of(&resource.resource_type, declared)?;
        changes.push_str(&format!(
            "<Change>{}{record}</Change>",
            xml_leaf("Action", action)
        ));
    }
    Ok(changes)
}

/// Apply one change batch over every record set an `AWS::Route53::RecordSetGroup` declares,
/// in the zone the GROUP names.
///
/// One batch rather than one call per record set, which is what the type is for: "if you want
/// to create or update multiple record sets, you can specify them in a single
/// `ChangeResourceRecordSets` request" — so either every record set the group declares is
/// applied or none is.
pub(super) async fn change_record_set_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    action: &str,
) -> Result<(), Refusal> {
    let zone = record_set_zone_id(ctx, region, resource).await?;
    let changes = record_set_group_changes(resource, action)?;
    let comment = property(resource, "Comment")
        .map(|comment| xml_leaf("Comment", &comment))
        .unwrap_or_default();
    let batch = format!("<ChangeBatch>{comment}<Changes>{changes}</Changes></ChangeBatch>");
    call_route53(
        ctx,
        region,
        "POST",
        &format!("/hostedzone/{}/rrset", percent_encoded(&zone)),
        Some(&route53_request("ChangeResourceRecordSetsRequest", &batch)),
    )
    .await?;
    Ok(())
}

/// Create every record set an `AWS::Route53::RecordSetGroup` declares, so a DNS query for any
/// of those names is answered with the values the template declared.
///
/// `Ref` on this resource returns the physical id CloudFormation's naming rules computed for
/// it — the group is not a resource of Route 53's at all, so that service mints no identity
/// for it and the create answers none. The type publishes no `Tags` property.
pub(super) async fn create_record_set_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    change_record_set_group(ctx, region, resource, RECORD_SET_CREATE).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete every record set the group created, by the same change batch that created them:
/// Route 53 removes a record set by its own values, so the delete carries the declarations the
/// create was made from. A zone the service no longer holds holds none of them either.
pub(super) async fn delete_record_set_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    match change_record_set_group(ctx, region, resource, RECORD_SET_DELETE).await {
        Ok(()) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => Ok(()),
        // Route 53 models no not-found error for a record set and reports a change batch it
        // cannot apply as one refusal class whatever the problem was, so a batch it will not
        // apply is checked against what the zone still HOLDS: a group whose record sets are
        // all gone is already in the state the delete asks for.
        Err(refusal) => match describe_record_set_group(ctx, region, resource).await {
            Ok(None) => Ok(()),
            _ => Err(refusal),
        },
    }
}

/// The group's configuration as Route 53 reports it, for drift detection: the record sets it
/// declares, read back one by one, and `None` where the zone holds none of them any more —
/// including where it holds no such ZONE.
///
/// Only the record sets' own names and types are compared, which are the members
/// `ListResourceRecordSets` reports in the same shape the declaration states them in; the
/// group has no configuration of its own beyond them.
pub(super) async fn describe_record_set_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let zone = match record_set_zone_id(ctx, region, resource).await {
        Ok(zone) => zone,
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(Value::Array(record_sets)) = resource.properties.get("RecordSets") else {
        return Ok(None);
    };
    let mut reported = Vec::new();
    for declared in record_sets {
        let (Some(name), Some(record_type)) = (
            declared.get("Name").and_then(scalar_text),
            declared.get("Type").and_then(scalar_text),
        ) else {
            continue;
        };
        let held = match read_record_set(ctx, region, &zone, &name, &record_type, None).await {
            Ok(held) => held,
            Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(None),
            Err(refusal) => return Err(refusal),
        };
        if let Some(held) = held {
            reported.push(held);
        }
    }
    if reported.is_empty() {
        return Ok(None);
    }
    let mut group = Map::new();
    group.insert("RecordSets".to_string(), Value::Array(reported));
    Ok(Some(Value::Object(group)))
}

/// The record set's configuration as Route 53 reports it, for drift detection, and `None`
/// where the zone no longer holds a record set of that name and type — including where it
/// holds no such ZONE any more (`NoSuchHostedZone`), since a zone that is gone holds no
/// record set either and that is the `DELETED` drift status rather than a failed check.
///
/// `ListResourceRecordSets` reports the record sets of a zone from the position the request
/// names ("the results begin with the first resource record set in the list whose name is
/// greater than or equal to Name, and whose type is greater than or equal to Type"), so the
/// read starts at this record's own name, type and set identifier and takes the first — which
/// is this record set when the zone still holds it.
///
/// Only the scalar members the answer reports in the same shape the type declares them in
/// are compared; the declared `ResourceRecords` and the nested routing structures are not
/// scalars of the reported record set, and a property the actual document does not carry is
/// not evidence of anything ([`super::drift::differs`]).
pub(super) async fn describe_record_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let zone = match record_set_zone_id(ctx, region, resource).await {
        Ok(zone) => zone,
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let (Some(name), Some(record_type)) = (
        property(resource, "Name"),
        property(resource, "Type"),
    ) else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "{} declares no Name and Type to read the record set back by",
                resource.resource_type
            ),
        });
    };
    let mut uri = format!(
        "/hostedzone/{}/rrset?name={}&type={}&maxitems=1",
        percent_encoded(&zone),
        percent_encoded(&name),
        percent_encoded(&record_type)
    );
    if let Some(identifier) = property(resource, "SetIdentifier") {
        uri.push_str(&format!("&identifier={}", percent_encoded(&identifier)));
    }
    let answered = match call_route53(ctx, region, "GET", &uri, None).await {
        Ok(answered) => answered,
        // The zone itself is gone, so neither is the record set in it.
        Err(refusal) if refusal.code == NO_SUCH_HOSTED_ZONE => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    Ok(record_set_as_properties(&answered, &name, &record_type))
}

/// The record set of `name` and `record_type` the zone `zone` holds, as the properties a
/// declaration of it states, or `None` where it holds no such record set.
///
/// The ONE read of one record set, so a record set declared on its own and one declared as an
/// entry of a group are read back the same way.
pub(super) async fn read_record_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    zone: &str,
    name: &str,
    record_type: &str,
    set_identifier: Option<&str>,
) -> Result<Option<Value>, Refusal> {
    let mut uri = format!(
        "/hostedzone/{}/rrset?name={}&type={}&maxitems=1",
        percent_encoded(zone),
        percent_encoded(name),
        percent_encoded(record_type)
    );
    if let Some(identifier) = set_identifier {
        uri.push_str(&format!("&identifier={}", percent_encoded(identifier)));
    }
    let answered = call_route53(ctx, region, "GET", &uri, None).await?;
    Ok(record_set_as_properties(&answered, name, record_type))
}

/// The `ResourceRecordSet` a listing answered, as the properties a declaration of it states,
/// or `None` where the answer reports no record set of that name and type.
///
/// Only the scalar members the answer reports in the same shape the type declares them in are
/// carried, for the reason [`describe_record_set`] records.
fn record_set_as_properties(answered: &str, name: &str, record_type: &str) -> Option<Value> {
    let reported = xml_scalar_children(answered, "ResourceRecordSet");
    let holds_this_record = reported
        .get("Name")
        .and_then(Value::as_str)
        .is_some_and(|held| fully_qualified(held) == fully_qualified(name))
        && reported.get("Type").and_then(Value::as_str) == Some(record_type);
    if !holds_this_record {
        return None;
    }
    Some(reported_as_properties(
        &Value::Object(reported),
        &[
            ("Name", "Name"),
            ("Type", "Type"),
            ("TTL", "TTL"),
            ("SetIdentifier", "SetIdentifier"),
            ("Weight", "Weight"),
            ("Region", "Region"),
            ("Failover", "Failover"),
            ("MultiValueAnswer", "MultiValueAnswer"),
            ("HealthCheckId", "HealthCheckId"),
        ],
    ))
}

// ---------------------------------------------------------------------------
// Amazon Route 53 health checks: AWS::Route53::HealthCheck
// ---------------------------------------------------------------------------

/// The error code Route 53 answers for a health check it does not hold: "no health check
/// exists with the specified ID" (`NoSuchHealthCheck`).
pub(super) const NO_SUCH_HEALTH_CHECK: &str = "NoSuchHealthCheck";

/// `AlarmIdentifier`'s members — the Region the CloudWatch alarm lives in and its name —
/// spelled identically on the type and in the model.
pub(super) const ALARM_IDENTIFIER_MEMBERS: &[(&str, Route53Member)] = &[
    ("Region", Route53Member::Scalar),
    ("Name", Route53Member::Scalar),
];

/// Every member of the `HealthCheckConfig` a health check is created with, in the order the
/// model declares them.
///
/// The type's `HealthCheckConfig` property states those members under the very same names,
/// so each declared value goes through as the template wrote it. The two list members carry
/// one element per value, under the element name the model gives the list's member
/// (`ChildHealthChecks` a `ChildHealthCheck` each, `Regions` a `Region` each), and
/// `AlarmIdentifier` is a structure of its own.
pub(super) const HEALTH_CHECK_CONFIG_MEMBERS: &[(&str, Route53Member)] = &[
    ("IPAddress", Route53Member::Scalar),
    ("Port", Route53Member::Scalar),
    ("Type", Route53Member::Scalar),
    ("ResourcePath", Route53Member::Scalar),
    ("FullyQualifiedDomainName", Route53Member::Scalar),
    ("SearchString", Route53Member::Scalar),
    ("RequestInterval", Route53Member::Scalar),
    ("FailureThreshold", Route53Member::Scalar),
    ("MeasureLatency", Route53Member::Scalar),
    ("Inverted", Route53Member::Scalar),
    ("Disabled", Route53Member::Scalar),
    ("HealthThreshold", Route53Member::Scalar),
    (
        "ChildHealthChecks",
        Route53Member::Entries {
            entry: "ChildHealthCheck",
            member: None,
        },
    ),
    ("EnableSNI", Route53Member::Scalar),
    (
        "Regions",
        Route53Member::Entries {
            entry: "Region",
            member: None,
        },
    ),
    (
        "AlarmIdentifier",
        Route53Member::Structure(ALARM_IDENTIFIER_MEMBERS),
    ),
    ("InsufficientDataHealthStatus", Route53Member::Scalar),
    ("RoutingControlArn", Route53Member::Scalar),
];

/// The properties of `AWS::Route53::HealthCheck` this arm maps onto `CreateHealthCheck` and
/// the calls beside it.
///
/// Source: AWS's CloudFormation resource reference for the type, which publishes exactly
/// two: the `HealthCheckConfig` the check is created with, and `HealthCheckTags`, the
/// check's own tags.
pub(super) const HEALTH_CHECK_PROPERTIES: &[&str] = &["HealthCheckConfig", "HealthCheckTags"];

/// The members of a `HealthCheckConfig` that no `UpdateHealthCheck` carries, so a change to
/// one cannot be applied to a health check that already exists.
///
/// `Type`, `RequestInterval` and `MeasureLatency` each state that you can't change their
/// value after you create a health check, and `RoutingControlArn` is not a member of that
/// operation's input at all.
pub(super) const UNCHANGEABLE_HEALTH_CHECK_MEMBERS: &[&str] = &[
    "Type",
    "RequestInterval",
    "MeasureLatency",
    "RoutingControlArn",
];

/// The elements `UpdateHealthCheck$ResetElements` can put back to their default, which are
/// the only ones a declaration can stop stating and have the health check follow: the
/// `ResettableElementName` enum's own value set.
pub(super) const RESETTABLE_HEALTH_CHECK_MEMBERS: &[&str] = &[
    "ChildHealthChecks",
    "FullyQualifiedDomainName",
    "Regions",
    "ResourcePath",
];

/// The `HealthCheckConfig` a declaration states, as the member document it is.
///
/// That property is REQUIRED by the resource type, `CreateHealthCheck$HealthCheckConfig` is
/// `@required` and so is its own `Type` ("the type of health check that you want to create,
/// which indicates how Amazon Route 53 determines whether an endpoint is healthy"), so a
/// declaration missing either describes no health check that could be created and is refused
/// here rather than with a request Route 53 would have to reject. A member the catalogue does
/// not name is no member of the shape, and is refused rather than dropped.
pub(super) fn health_check_config<'a>(
    resource_type: &str,
    declared: &'a Value,
) -> Result<&'a Map<String, Value>, Refusal> {
    let missing = |member: &str| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property {member} cannot be empty for resource of type {resource_type}"
        ),
    };
    let Some(config) = declared.get("HealthCheckConfig") else {
        return Err(missing("HealthCheckConfig"));
    };
    let Value::Object(config) = config else {
        return Err(malformed_property("HealthCheckConfig"));
    };
    for member in config.keys() {
        if !HEALTH_CHECK_CONFIG_MEMBERS
            .iter()
            .any(|(known, _)| known == member)
        {
            return Err(malformed_property(&format!("HealthCheckConfig.{member}")));
        }
    }
    if config
        .get("Type")
        .and_then(scalar_text)
        .is_none_or(|kind| kind.is_empty())
    {
        return Err(missing("HealthCheckConfig.Type"));
    }
    Ok(config)
}

/// The `HealthCheckConfig` element one declaration renders to, member by member in the
/// order the model declares them.
pub(super) fn health_check_config_element(
    resource_type: &str,
    declared: &Value,
) -> Result<String, Refusal> {
    let config = health_check_config(resource_type, declared)?;
    let mut rendered = String::new();
    for (name, member) in HEALTH_CHECK_CONFIG_MEMBERS {
        if let Some(value) = config.get(*name) {
            if let Some(written) = route53_member(name, *member, value)? {
                rendered.push_str(&written);
            }
        }
    }
    Ok(format!("<HealthCheckConfig>{rendered}</HealthCheckConfig>"))
}

/// Create the health check an `AWS::Route53::HealthCheck` declares, so a client reading the
/// stack's output and then calling Route 53 with that value — `GetHealthCheck`,
/// `GetHealthCheckStatus`, a record set's own `HealthCheckId` — reaches the health check the
/// template asked for.
///
/// `Ref` on this resource returns the health check ID, which `CreateHealthCheck` MINTS, so
/// the id that answer carries replaces the computed physical id and the type's
/// `HealthCheckId` attribute is read off it
/// ([`super::provisioning::adopt_minted_identity`]). The health check carries the tags its
/// declaration asks for plus the ones CloudFormation gives the resource ([`tags_of`]),
/// applied through `ChangeTagsForResource`, which is how Route 53 tags a health check:
/// `CreateHealthCheck` takes no tags at all.
pub(super) async fn create_health_check(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, HEALTH_CHECK_PROPERTIES)?;
    // In the order `CreateHealthCheckRequest` declares its members.
    let members = format!(
        "{}{}",
        xml_leaf(
            "CallerReference",
            &route53_caller_reference(HEALTH_CHECK_CALLER_REFERENCE_SALT, resource, propagated)
        ),
        health_check_config_element(&resource.resource_type, &resource.properties)?
    );
    let answered = call_route53(
        ctx,
        region,
        "POST",
        "/healthcheck",
        Some(&route53_request("CreateHealthCheckRequest", &members)),
    )
    .await?;
    let reported = xml_scalar_children(&answered, "HealthCheck");
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateHealthCheck answered no HealthCheck.Id".to_string(),
        })?
        .to_string();
    change_route53_tags(
        ctx,
        region,
        HEALTH_CHECK_TAG_RESOURCE_TYPE,
        &id,
        &route53_tags(resource, "HealthCheckTags", propagated),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: BTreeMap::new(),
    })
}

/// Apply a CHANGED `AWS::Route53::HealthCheck` declaration to the health check the stack
/// already has, through Route 53's own `UpdateHealthCheck` — whose members are exactly the
/// settings of a live health check that are editable, so the check keeps the id every
/// reference to it resolves to and its `HealthCheckVersion` advances.
///
/// `Ok(None)` is a change that call cannot make, so the deployment reports it as not applied
/// rather than as a success that dropped it:
///
/// * a declaration that changes a member no `UpdateHealthCheck` carries
///   ([`UNCHANGEABLE_HEALTH_CHECK_MEMBERS`]);
/// * a declaration that no longer states a member that call cannot unset — only the four
///   elements `ResetElements` names go back to their default, and a member the call simply
///   leaves out keeps its stored value;
/// * a declaration that changes the health check's TAGS, which reach Route 53 through
///   `ChangeTagsForResource` rather than through this call.
pub(super) async fn update_health_check(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let held = health_check_config(&resource.resource_type, &resource.properties)?;
    let wanted = health_check_config(&resource.resource_type, declared)?;
    if resource.properties.get("HealthCheckTags") != declared.get("HealthCheckTags") {
        return Ok(None);
    }
    for member in UNCHANGEABLE_HEALTH_CHECK_MEMBERS {
        if held.get(*member) != wanted.get(*member) {
            return Ok(None);
        }
    }
    // Dropping the four unchangeable members above is already settled, so what is left is
    // the editable ones: the declared value of each, and `ResetElements` for each the
    // declaration no longer states.
    let mut members = String::new();
    let mut reset = String::new();
    for (name, member) in HEALTH_CHECK_CONFIG_MEMBERS {
        if UNCHANGEABLE_HEALTH_CHECK_MEMBERS.contains(name) {
            continue;
        }
        match wanted.get(*name) {
            Some(value) => {
                if let Some(written) = route53_member(name, *member, value)? {
                    members.push_str(&written);
                }
            }
            None if held.contains_key(*name) => {
                if !RESETTABLE_HEALTH_CHECK_MEMBERS.contains(name) {
                    return Ok(None);
                }
                reset.push_str(&xml_leaf("ResettableElementName", name));
            }
            None => {}
        }
    }
    if !reset.is_empty() {
        members.push_str(&format!("<ResetElements>{reset}</ResetElements>"));
    }
    call_route53(
        ctx,
        region,
        "POST",
        &format!(
            "/healthcheck/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        Some(&route53_request("UpdateHealthCheckRequest", &members)),
    )
    .await?;
    // The type publishes `HealthCheckId` alone, which is the identity the stack already
    // records, so the update reports no attribute of its own.
    Ok(Some(Created::attributes(BTreeMap::new())))
}

/// Delete the health check a stack created, by the id its create minted. One Route 53 no
/// longer holds (`NoSuchHealthCheck`) is already in the state the delete asks for.
///
/// "Amazon Route 53 does not prevent you from deleting a health check even if the health
/// check is associated with one or more resource record sets", so a stack that declares a
/// record set referencing its own health check has nothing to undo first.
pub(super) async fn delete_health_check(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_route53(
        ctx,
        region,
        "DELETE",
        &format!("/healthcheck/{}", percent_encoded(physical_resource_id)),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_HEALTH_CHECK => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The health check's configuration as Route 53 reports it, for drift detection. An id it
/// no longer holds a health check for (`NoSuchHealthCheck`) was deleted.
///
/// Only the SCALAR members `GetHealthCheck` reports inside the health check's configuration
/// are compared, and the property is left out of the reported document altogether where the
/// declaration states a member that read does not carry as one — a child-check list, the
/// alarm identifier — because a value the owning service did not report is not evidence of
/// anything ([`super::drift::differs`]). The declared `HealthCheckTags` are no part of the
/// reported health check at all.
pub(super) async fn describe_health_check(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_route53(
        ctx,
        region,
        "GET",
        &format!(
            "/healthcheck/{}",
            percent_encoded(&resource.physical_resource_id)
        ),
        None,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NO_SUCH_HEALTH_CHECK => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let reported = xml_scalar_children(&answered, "HealthCheckConfig");
    let mut properties = Map::new();
    if let Ok(declared) = health_check_config(&resource.resource_type, &resource.properties) {
        if declared.keys().all(|member| reported.contains_key(member)) {
            properties.insert("HealthCheckConfig".to_string(), Value::Object(reported));
        }
    }
    Ok(Some(Value::Object(properties)))
}

// ---------------------------------------------------------------------------
// AWS Backup: AWS::Backup::BackupVault, ::BackupPlan, ::BackupSelection
// ---------------------------------------------------------------------------

/// AWS Backup's signing name, from its model (`sigv4Name: backup`), which speaks
/// `restJson1`: every operation binds to a method and a URI of its own rather than to an
/// `X-Amz-Target` header.
pub(super) const BACKUP_SIGNING_NAME: &str = "backup";

/// The error code AWS Backup answers for a resource it does not hold
/// (`ResourceNotFoundException`: "a resource that is required for the action doesn't
/// exist", the modelled refusal of `GetBackupPlan`, `DescribeBackupVault`,
/// `GetBackupSelection` and each of their deletes).
pub(super) const BACKUP_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The URI prefixes AWS Backup binds its vault and plan operations to
/// (`PUT /backup-vaults/{BackupVaultName}`, `PUT /backup/plans`).
pub(super) const BACKUP_VAULTS_URI: &str = "/backup-vaults";
pub(super) const BACKUP_PLANS_URI: &str = "/backup/plans";

/// Every property `AWS::Backup::BackupVault` publishes that this arm carries to AWS
/// Backup: the name the vault is created under, the tags the create takes, the KMS key it
/// encrypts the vault with, and the vault lock a second call applies
/// ([`BACKUP_VAULT_LOCK_PROPERTY`]). A declaration naming anything else — `AccessPolicy`
/// and `Notifications`, whose `PutBackupVaultAccessPolicy` and
/// `PutBackupVaultNotifications` this emulator does not serve yet — is REFUSED rather than
/// deployed with the property dropped.
pub(super) const BACKUP_VAULT_PROPERTIES: &[&str] = &[
    "BackupVaultName",
    "BackupVaultTags",
    "EncryptionKeyArn",
    BACKUP_VAULT_LOCK_PROPERTY,
];

/// The property carrying the vault's lock. It is no member of `CreateBackupVault` at all:
/// it is the `PutBackupVaultLockConfiguration` a client makes next on the vault just
/// created, whose three members this property's own
/// (`MinRetentionDays`, `MaxRetentionDays`, `ChangeableForDays`) are named identically to.
pub(super) const BACKUP_VAULT_LOCK_PROPERTY: &str = "LockConfiguration";

/// The members of that lock call, as (property, member) pairs — identical on both sides.
pub(super) const BACKUP_VAULT_LOCK_MEMBERS: &[(&str, &str)] = &[
    ("MinRetentionDays", "MinRetentionDays"),
    ("MaxRetentionDays", "MaxRetentionDays"),
    ("ChangeableForDays", "ChangeableForDays"),
];

/// The property of `AWS::Backup::BackupVault` and of `AWS::Backup::BackupPlan` that
/// carries the resource's tags: each type publishes its own tag MAP rather than the `Tags`
/// list most types do, and each create takes that map under the same name.
pub(super) const BACKUP_VAULT_TAGS_PROPERTY: &str = "BackupVaultTags";
pub(super) const BACKUP_PLAN_TAGS_PROPERTY: &str = "BackupPlanTags";

/// Every property `AWS::Backup::BackupPlan` publishes: the plan body and the tag map,
/// both of which are members of `CreateBackupPlan` under the same names.
pub(super) const BACKUP_PLAN_PROPERTIES: &[&str] = &["BackupPlan", BACKUP_PLAN_TAGS_PROPERTY];

/// Every property `AWS::Backup::BackupSelection` publishes: the plan the selection is
/// assigned to, which `CreateBackupSelection` binds to its URI, and the selection body,
/// which is that call's own member of the same name.
pub(super) const BACKUP_SELECTION_PROPERTIES: &[&str] = &["BackupPlanId", "BackupSelection"];

/// The members of the plan body `AWS::Backup::BackupPlan` declares, as the
/// `BackupPlanInput` `CreateBackupPlan` takes spells them.
///
/// Source: AWS's CloudFormation resource reference for `BackupPlanResourceType` against
/// `smithy_query(shape="BackupPlanInput")`. The plan NAME and the advanced settings are
/// named identically on both sides; the rule list is the one member CloudFormation spells
/// differently (`BackupPlanRule` against the API's `Rules`).
pub(super) const BACKUP_PLAN_MEMBERS: &[(&str, &str)] = &[
    ("BackupPlanName", "BackupPlanName"),
    ("AdvancedBackupSettings", "AdvancedBackupSettings"),
];

/// The property carrying the plan's rules, and the member of the plan body that takes them.
pub(super) const BACKUP_PLAN_RULES_PROPERTY: &str = "BackupPlanRule";
pub(super) const BACKUP_PLAN_RULES_MEMBER: &str = "Rules";

/// The members of ONE declared rule, as the `BackupRuleInput` a plan body carries spells
/// them.
///
/// Source: AWS's `BackupRuleResourceType` against `smithy_query(shape="BackupRuleInput")`.
/// Every member is named identically on both sides except the destination vault, which the
/// type publishes as `TargetBackupVault` and the API takes as `TargetBackupVaultName`; the
/// nested `Lifecycle`, `CopyActions` and `IndexActions` structures spell their own members
/// identically too, so their values are carried through as the template wrote them.
pub(super) const BACKUP_RULE_MEMBERS: &[(&str, &str)] = &[
    ("RuleName", "RuleName"),
    ("TargetBackupVault", "TargetBackupVaultName"),
    ("ScheduleExpression", "ScheduleExpression"),
    ("ScheduleExpressionTimezone", "ScheduleExpressionTimezone"),
    ("StartWindowMinutes", "StartWindowMinutes"),
    ("CompletionWindowMinutes", "CompletionWindowMinutes"),
    ("EnableContinuousBackup", "EnableContinuousBackup"),
    ("Lifecycle", "Lifecycle"),
    ("RecoveryPointTags", "RecoveryPointTags"),
    ("CopyActions", "CopyActions"),
    ("IndexActions", "IndexActions"),
];

/// Create the backup vault an `AWS::Backup::BackupVault` declares, in AWS Backup itself.
///
/// "Ref returns the backup vault name", which is the name the declaration states (or the
/// one CloudFormation generated for it) and the `@httpLabel` `CreateBackupVault` creates
/// the vault under — so the create carries no identity of its own and the vault a client
/// then calls `DescribeBackupVault` with is the one the stack reports.
///
/// The vault carries the tags CloudFormation gives it beside the declared ones
/// ([`tags_of`]), as the tag MAP that create takes under this type's own
/// `BackupVaultTags`. A declared `LockConfiguration` is no member of the create: it is the
/// `PutBackupVaultLockConfiguration` a client makes next on the vault just created, so a
/// vault declared with one is locked rather than created unlocked.
///
/// The ARN AWS Backup minted is reported back as the `BackupVaultArn` attribute the type
/// publishes, beside the `BackupVaultName` it answered with.
pub(super) async fn create_backup_vault(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, BACKUP_VAULT_PROPERTIES)?;
    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    if let Some(key) = property(resource, "EncryptionKeyArn") {
        request.insert("EncryptionKeyArn".to_string(), Value::String(key));
    }
    let tags = tags_as_map(&tags_of(
        &backup_declared_tags(resource, BACKUP_VAULT_TAGS_PROPERTY),
        propagated,
    ));
    if !tags.is_empty() {
        request.insert(
            BACKUP_VAULT_TAGS_PROPERTY.to_string(),
            Value::Object(tags),
        );
    }
    let answered = call_rest_json(
        ctx,
        BACKUP_SIGNING_NAME,
        region,
        "PUT",
        &backup_vault_uri(&name),
        Some(&Value::Object(request)),
    )
    .await?;
    if let Some(declared) = resource.properties.get(BACKUP_VAULT_LOCK_PROPERTY) {
        let locked = call_rest_json(
            ctx,
            BACKUP_SIGNING_NAME,
            region,
            "PUT",
            &format!("{}/vault-lock", backup_vault_uri(&name)),
            Some(&mapped_members(declared, BACKUP_VAULT_LOCK_MEMBERS)),
        )
        .await;
        if let Err(refusal) = locked {
            // The vault exists but not with the lock it was declared with, and the stack
            // will record no resource for it, so it is removed before the failure is
            // reported — a refused delete changes nothing about the refusal that caused it.
            let _ = delete_backup_vault(ctx, region, &name).await;
            return Err(refusal);
        }
    }
    let mut attributes = BTreeMap::new();
    attributes.insert(
        "BackupVaultName".to_string(),
        reported_text(&answered, "BackupVaultName").unwrap_or(name),
    );
    if let Some(arn) = reported_text(&answered, "BackupVaultArn") {
        attributes.insert("BackupVaultArn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Delete the backup vault a stack created, addressed by its name — this resource's
/// physical id, which is what `Ref` returns for the type.
///
/// A vault AWS Backup no longer holds is already in the state the delete asks for, judged
/// on the CODE and never on a message.
pub(super) async fn delete_backup_vault(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = backup_vault_uri(physical_resource_id);
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The vault's configuration as AWS Backup reports it, for drift detection:
/// `DescribeBackupVault`'s own record, read by the name the delete uses. A vault that
/// service no longer holds was deleted.
///
/// Only the two published properties that read answers under the same names are reported —
/// the vault's name and the key it is encrypted with — and a property the read does not
/// report is not evidence of anything ([`super::drift::differs`]).
pub(super) async fn describe_backup_vault(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = backup_vault_uri(physical_resource_id);
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answered) => Ok(Some(reported_as_properties(
            &answered,
            &[
                ("BackupVaultName", "BackupVaultName"),
                ("EncryptionKeyArn", "EncryptionKeyArn"),
            ],
        ))),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE backup vault: the `@httpLabel` every vault operation binds
/// the name to, percent-encoded as a non-greedy label is sent.
pub(super) fn backup_vault_uri(name: &str) -> String {
    format!("{BACKUP_VAULTS_URI}/{}", percent_encoded(name))
}

/// Create the backup plan an `AWS::Backup::BackupPlan` declares, in AWS Backup itself — so
/// a client that deployed the stack and then calls `backup get-backup-plan` with the id the
/// stack published reads the plan back, rather than a `ResourceNotFoundException` for a
/// plan nothing created.
///
/// The declared plan body IS `CreateBackupPlan`'s own `BackupPlan` member, with the rule
/// list spelled differently on the two sides ([`BACKUP_PLAN_MEMBERS`],
/// [`BACKUP_RULE_MEMBERS`]) and a rule's destination vault published as
/// `TargetBackupVault` where the API takes `TargetBackupVaultName`. A member the
/// declaration leaves out is absent from the request, so the plan AWS Backup creates
/// carries the service's own defaults for it exactly as a client's own create would.
///
/// The plan's IDENTITY is that service's to mint: `CreateBackupPlan` takes no identifier
/// and answers the `BackupPlanId` it assigned, the `BackupPlanArn` it built around it and
/// the `VersionId` of the version it created. The id is reported back as the resource's
/// physical id — which is what `Ref` resolves to and what a stack output carrying it
/// publishes — and all three as the attributes the type publishes.
pub(super) async fn create_backup_plan(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, BACKUP_PLAN_PROPERTIES)?;
    let declared = resource.properties.get("BackupPlan").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property BackupPlan cannot be empty for resource of type {}",
            resource.resource_type
        ),
    })?;
    let mut plan = Map::new();
    if let Some(members) = declared.as_object() {
        copy_members(members, BACKUP_PLAN_MEMBERS, &mut plan);
    }
    let rules: Vec<Value> = declared
        .get(BACKUP_PLAN_RULES_PROPERTY)
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .map(|rule| mapped_members(rule, BACKUP_RULE_MEMBERS))
                .collect()
        })
        .unwrap_or_default();
    plan.insert(BACKUP_PLAN_RULES_MEMBER.to_string(), Value::Array(rules));
    let mut request = Map::new();
    request.insert("BackupPlan".to_string(), Value::Object(plan));
    let tags = tags_as_map(&tags_of(
        &backup_declared_tags(resource, BACKUP_PLAN_TAGS_PROPERTY),
        propagated,
    ));
    if !tags.is_empty() {
        request.insert(BACKUP_PLAN_TAGS_PROPERTY.to_string(), Value::Object(tags));
    }
    let answered = call_rest_json(
        ctx,
        BACKUP_SIGNING_NAME,
        region,
        "PUT",
        BACKUP_PLANS_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(plan_id) = reported_text(&answered, "BackupPlanId") else {
        // The create answered 2xx without the id its output models. Nothing addresses a
        // plan without one, so this is reported as a failure rather than recorded as a
        // resource with a physical id nothing minted.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateBackupPlan answered no BackupPlanId".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("BackupPlanId".to_string(), plan_id.clone());
    for (attribute, member) in [
        ("BackupPlanArn", "BackupPlanArn"),
        ("VersionId", "VersionId"),
    ] {
        if let Some(value) = reported_text(&answered, member) {
            attributes.insert(attribute.to_string(), value);
        }
    }
    Ok(Created {
        physical_resource_id: Some(plan_id),
        attributes,
    })
}

/// Delete the backup plan a stack created, addressed by the id AWS Backup minted for it —
/// this resource's physical id.
///
/// A plan that service no longer holds is already in the state the delete asks for, judged
/// on the CODE and never on a message. The selections assigned to the plan are resources of
/// their own: a stack that declared them deletes each of them in its own right, which the
/// deletion pass does before this (it deletes a stack's resources in the reverse of the
/// order they were created in), and AWS Backup refuses to delete a plan that still holds
/// one.
pub(super) async fn delete_backup_plan(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = backup_plan_uri(physical_resource_id);
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether AWS Backup still holds the plan a stack created, for drift detection:
/// `GetBackupPlan`'s own record, read by the id the delete uses. A plan that service no
/// longer holds — or one it reports a `DeletionDate` for, a plan whose deletion it has
/// recorded — was deleted.
///
/// No property is reported for a plan it still holds. Every member that read answers for
/// one carries what the SERVICE decided about the plan — a `RuleId` per rule, and the
/// backup windows it filled in where the declaration stated none — so the body it reports
/// is not the body a template declares, and a property the actual document does not carry
/// is not evidence of anything ([`super::drift::differs`]).
pub(super) async fn describe_backup_plan(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let uri = backup_plan_uri(physical_resource_id);
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answered) if answered.get("DeletionDate").is_some() => Ok(None),
        Ok(_) => Ok(Some(Value::Object(Map::new()))),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE backup plan: the `@httpLabel` every plan operation binds the
/// id to, percent-encoded as a non-greedy label is sent.
pub(super) fn backup_plan_uri(plan_id: &str) -> String {
    format!("{BACKUP_PLANS_URI}/{}", percent_encoded(plan_id))
}

/// Create the resource selection an `AWS::Backup::BackupSelection` declares, in the plan
/// its declaration names — so `backup list-backup-selections` and
/// `backup get-backup-selection` on the id the stack published answer with the selection
/// the template declared.
///
/// The type's two properties are `CreateBackupSelection`'s own: the plan, which that call
/// binds to its URI, and the selection body, whose members (`SelectionName`, `IamRoleArn`,
/// `Resources`, `NotResources`, `ListOfTags`, `Conditions`) are named identically on both
/// sides all the way down, so the evaluated document is carried through verbatim. The type
/// publishes no `Tags` property — AWS Backup tags a plan and a vault, not a selection — so
/// no tags are propagated to one.
///
/// The selection's IDENTITY is AWS Backup's to mint: that create takes no identifier and
/// answers the `SelectionId` it assigned, which is reported back as the resource's physical
/// id and as the attribute of that name beside the plan the selection belongs to.
pub(super) async fn create_backup_selection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, BACKUP_SELECTION_PROPERTIES)?;
    let plan_id = required_property(resource, "BackupPlanId")?;
    let declared = resource
        .properties
        .get("BackupSelection")
        .cloned()
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property BackupSelection cannot be empty for resource of type {}",
                resource.resource_type
            ),
        })?;
    let mut request = Map::new();
    request.insert("BackupSelection".to_string(), declared);
    let answered = call_rest_json(
        ctx,
        BACKUP_SIGNING_NAME,
        region,
        "PUT",
        &format!("{}/selections", backup_plan_uri(&plan_id)),
        Some(&Value::Object(request)),
    )
    .await?;
    let Some(selection_id) = reported_text(&answered, "SelectionId") else {
        // The create answered 2xx without the id its output models, so there is no
        // identifier to address the selection by: not something this stack can report as a
        // created resource.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateBackupSelection answered no SelectionId".to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("SelectionId".to_string(), selection_id.clone());
    attributes.insert("BackupPlanId".to_string(), plan_id);
    Ok(Created {
        physical_resource_id: Some(selection_id),
        attributes,
    })
}

/// Delete the resource selection a stack created, addressed by the PAIR AWS Backup keys one
/// by: the plan its declaration names and the `SelectionId` that service minted, which is
/// this resource's physical id.
///
/// A selection that service no longer holds — including one whose PLAN it no longer holds,
/// which took the selection with it — is already in the state the delete asks for, judged
/// on the CODE and never on a message.
pub(super) async fn delete_backup_selection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let uri = backup_selection_uri(resource)?;
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The selection's configuration as AWS Backup reports it, for drift detection:
/// `GetBackupSelection`'s own record, read by the same pair the delete uses. A selection
/// that service no longer holds was deleted.
///
/// Both published properties are reported: that read answers the plan the selection belongs
/// to and the selection body it was created with, verbatim — a selection has no member the
/// service fills — so the two sides of the comparison are the same document.
pub(super) async fn describe_backup_selection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = backup_selection_uri(resource)?;
    match call_rest_json(ctx, BACKUP_SIGNING_NAME, region, "GET", &uri, None).await {
        Ok(answered) => Ok(Some(reported_as_properties(
            &answered,
            &[
                ("BackupPlanId", "BackupPlanId"),
                ("BackupSelection", "BackupSelection"),
            ],
        ))),
        Err(refusal) if refusal.code == BACKUP_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The URI that addresses ONE resource selection: the two `@httpLabel`s of every operation
/// on one, each percent-encoded. The plan half comes from the resource's own declaration —
/// the only place it is recorded — and a declaration naming no plan addresses no selection
/// at all, so it is refused here rather than with a request AWS Backup would reject.
pub(super) fn backup_selection_uri(resource: &StoredResource) -> Result<String, Refusal> {
    let plan_id = required_property(resource, "BackupPlanId")?;
    Ok(format!(
        "{}/selections/{}",
        backup_plan_uri(&plan_id),
        percent_encoded(&resource.physical_resource_id)
    ))
}

/// The tag MAP an AWS Backup resource's declaration asks for, under the `Tags` property
/// name [`tags_of`] merges the propagated tags into.
///
/// Both taggable types here publish their tags as a MAP under a name of their own
/// (`BackupVaultTags`, `BackupPlanTags`) rather than as the `Tags` list most types publish,
/// and [`declared_tags`] already reads either SHAPE — so the declared map is handed to it
/// under the one property name it looks for, and the merge rule a resource's declared tags
/// and the stack's follow is the same one every other type here applies.
pub(super) fn backup_declared_tags(resource: &StoredResource, property: &str) -> Value {
    let mut tags = Map::new();
    if let Some(declared) = resource.properties.get(property) {
        tags.insert("Tags".to_string(), declared.clone());
    }
    Value::Object(tags)
}

// ---------------------------------------------------------------------------
// AWS Glue: AWS::Glue::Job
// ---------------------------------------------------------------------------

/// AWS Glue's signing name and `X-Amz-Target` prefix, from its model (`sigv4Name: glue`,
/// service shape `AWSGlue`), which speaks `awsJson1_1`.
pub(super) const GLUE_SIGNING_NAME: &str = "glue";
pub(super) const GLUE_TARGET_PREFIX: &str = "AWSGlue";

/// The error code AWS Glue answers for a name it holds no entity under
/// (`EntityNotFoundException`, the modelled refusal of `GetJob`).
pub(super) const GLUE_ENTITY_NOT_FOUND: &str = "EntityNotFoundException";

/// The one `AWS::Glue::Job` property that is not a member of `JobUpdate`: the job's NAME,
/// which is its identity — `UpdateJob` takes it as the request's own `JobName` — so a
/// declaration that changes it asks for a different job rather than for a change to this
/// one.
pub(super) const GLUE_JOB_NAME_PROPERTY: &str = "Name";

/// The `AWS::Glue::Job` property that is no member of `JobUpdate` either: a job's tags are
/// AWS Glue's own `TagResource`/`UntagResource`, not part of the definition that call
/// overwrites.
pub(super) const GLUE_JOB_TAGS_PROPERTY: &str = "Tags";

/// Create the job definition an `AWS::Glue::Job` declares, so a client that deployed the
/// stack and then calls `glue GetJob` (or `StartJobRun`, or `UpdateJob`) with the name the
/// stack publishes reads the job back rather than an `EntityNotFoundException`.
///
/// The type's properties ARE `CreateJob`'s own members — `Role`, `Command`, `GlueVersion`,
/// `DefaultArguments`, `Connections`, `ExecutionProperty`, `NotificationProperty`,
/// `WorkerType`, `NumberOfWorkers`, `MaxCapacity`, `MaxRetries`, `Timeout`,
/// `SecurityConfiguration`, `ExecutionClass`, `JobMode`, `JobRunQueuingEnabled`,
/// `MaintenanceWindow`, `Description`, `LogUri`, `AllocatedCapacity` and
/// `NonOverridableArguments` are named identically on both sides (AWS's resource reference
/// for the type against `CreateJobRequest`) — so the resolved property document IS the
/// request, with the one member CloudFormation decides set on the way out: "Ref returns the
/// job name", which is this resource's physical id and the identifier every AWS Glue
/// operation addresses a job definition by.
///
/// `Tags` is the one member not carried verbatim: the job carries the tags the declaration
/// asks for beside the ones CloudFormation gives the resource ([`tags_of`]), as the tag MAP
/// `CreateJob`'s own `TagsMap` member takes — whichever of the two spellings the
/// declaration wrote them in.
///
/// That tag set reaches AWS Glue on the CREATE alone. A job's tags are that service's own
/// `TagResource`/`UntagResource` rather than part of the definition an update overwrites, so
/// this type has no [`Provisioner::retag`] arm and a later deployment that changes only the
/// tags leaves the job carrying the set it was created with — the state a type with no
/// re-tagging arm is documented to be in. Writing one waits on that service's own untagging
/// call, which is still one of this emulator's scaffold stubs: an arm built on it could only
/// fail the deployment it is meant to carry.
pub(super) async fn create_glue_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    request.insert(
        GLUE_JOB_NAME_PROPERTY.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if tags.is_empty() {
        request.remove(GLUE_JOB_TAGS_PROPERTY);
    } else {
        request.insert(GLUE_JOB_TAGS_PROPERTY.to_string(), Value::Object(tags));
    }
    call_glue(ctx, region, "CreateJob", &Value::Object(request)).await?;
    // `CreateJob` answers with the name it stored, which is the physical id the stack
    // already computed, and the type publishes no `Fn::GetAtt` attribute — so there is
    // nothing the owning service decided for this resource beyond the job now existing.
    Ok(Created::default())
}

/// Apply a changed `AWS::Glue::Job` declaration to the job the stack created.
///
/// `UpdateJob` takes the whole definition — "the previous job definition is completely
/// overwritten by this information" (`JobUpdate`) — and every property of the type but its
/// name and its tags is a member of that shape, so the declaration this deployment states
/// IS the `JobUpdate`, against the `JobName` the resource is held under.
pub(super) async fn update_glue_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut update = declared.as_object().cloned().unwrap_or_else(Map::new);
    update.remove(GLUE_JOB_NAME_PROPERTY);
    update.remove(GLUE_JOB_TAGS_PROPERTY);
    let request = serde_json::json!({
        "JobName": resource.physical_resource_id,
        "JobUpdate": Value::Object(update),
    });
    call_glue(ctx, region, "UpdateJob", &request).await?;
    Ok(Created::default())
}

/// Delete the job definition a stack created, addressed by its NAME — this resource's
/// physical id.
///
/// `DeleteJob` models no not-found error at all ("if the job definition is not found, no
/// exception is thrown"), so a job AWS Glue no longer holds is already in the state the
/// delete asks for and the call says so itself.
pub(super) async fn delete_glue_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "JobName": physical_resource_id });
    call_glue(ctx, region, "DeleteJob", &request).await?;
    Ok(())
}

/// The job definition as AWS Glue reports it, for drift detection: the `Job` document its
/// own read answers with, whose members are spelled exactly as the type's properties are.
/// A name it no longer holds a job under (`EntityNotFoundException`, judged on the CODE and
/// never on a message that carries the request's own name) was deleted.
pub(super) async fn describe_glue_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "JobName": physical_resource_id });
    match call_glue(ctx, region, "GetJob", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "Job"))),
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

// ---------------------------------------------------------------------------
// AWS Glue: AWS::Glue::Database, AWS::Glue::Table, AWS::Glue::Crawler
// ---------------------------------------------------------------------------

/// The property an `AWS::Glue::Database` states the database's definition in, which is the
/// `DatabaseInput` member `CreateDatabase` and `UpdateDatabase` take it as.
pub(super) const GLUE_DATABASE_INPUT_PROPERTY: &str = "DatabaseInput";

/// The property an `AWS::Glue::Table` states the table's definition in, which is the
/// `TableInput` member `CreateTable` and `UpdateTable` take it as.
pub(super) const GLUE_TABLE_INPUT_PROPERTY: &str = "TableInput";

/// The member of either input structure that carries the entity's NAME. It is the name the
/// entity is created under, which is this resource's physical id — `Ref` of either type
/// returns it, and it is what every AWS Glue operation on the entity addresses it by — so
/// CloudFormation sets it on the way out rather than passing the declaration's own through
/// ([`super::template`] reads it to compute that id).
pub(super) const GLUE_INPUT_NAME_MEMBER: &str = "Name";

/// The property both catalog types state the Data Catalog the entity belongs to in, which
/// every AWS Glue catalog call takes ("If none is provided, the Amazon Web Services account
/// ID is used by default") and which the delete and the read have to carry for the entity to
/// be addressed in the same catalog the create made it in.
pub(super) const GLUE_CATALOG_ID_PROPERTY: &str = "CatalogId";

/// The property an `AWS::Glue::Table` states the database that holds the table in — the
/// other half of the pair AWS Glue keys a table by.
pub(super) const GLUE_DATABASE_NAME_PROPERTY: &str = "DatabaseName";

/// The `AWS::Glue::Crawler` property that is the crawler's NAME, which is its identity:
/// `CreateCrawler`, `UpdateCrawler`, `GetCrawler` and `DeleteCrawler` all take it as the
/// request's own `Name`.
pub(super) const GLUE_CRAWLER_NAME_PROPERTY: &str = "Name";

/// The `AWS::Glue::Crawler` property that is no member of `CreateCrawler`'s configuration:
/// a crawler's tags are the tag MAP that call's own `Tags` member takes, and AWS Glue's
/// `TagResource`/`UntagResource` afterwards, rather than part of what an update overwrites.
pub(super) const GLUE_CRAWLER_TAGS_PROPERTY: &str = "Tags";

/// The one `AWS::Glue::Crawler` property that is not spelled as the matching AWS Glue member
/// is: the type publishes `Schedule` as the `Schedule` STRUCTURE a crawler is REPORTED with
/// (its `ScheduleExpression` and `State`), while `CreateCrawler` and `UpdateCrawler` take
/// the cron EXPRESSION itself as their `Schedule` member.
pub(super) const GLUE_CRAWLER_SCHEDULE_PROPERTY: &str = "Schedule";
pub(super) const GLUE_SCHEDULE_EXPRESSION_MEMBER: &str = "ScheduleExpression";

/// Create the Data Catalog database an `AWS::Glue::Database` declares, so a client that
/// deployed the stack and then calls `glue GetDatabase` (or `CreateTable`, or `StartJobRun`
/// over a job reading it) with the name the stack publishes reads the database back rather
/// than an `EntityNotFoundException`.
///
/// The type's two properties ARE `CreateDatabase`'s own members — `CatalogId` and the whole
/// `DatabaseInput` structure, named identically on both sides — so the resolved property
/// document IS the request, with the one member CloudFormation decides set on the way out:
/// the database's NAME, which is this resource's physical id ([`named_glue_input`]).
///
/// The type publishes no `Tags` property, so nothing is propagated to a database: a Data
/// Catalog database is not a resource CloudFormation tags.
pub(super) async fn create_glue_database(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    named_glue_input(
        &mut request,
        GLUE_DATABASE_INPUT_PROPERTY,
        &resource.physical_resource_id,
    );
    call_glue(ctx, region, "CreateDatabase", &Value::Object(request)).await?;
    // `CreateDatabase` answers nothing, and the type publishes no `Fn::GetAtt` attribute of
    // its own: the identity in force is the name the stack already computed.
    Ok(Created::default())
}

/// Apply a changed `AWS::Glue::Database` declaration to the database the stack created.
///
/// `UpdateDatabase` "updates an existing database definition in a Data Catalog" and takes
/// the same `DatabaseInput` the create did, against the `Name` the database is held under —
/// so the declaration this deployment states IS that input, and the database keeps its
/// identity.
///
/// That name is the one the resource is HELD under, not one this call could change: a
/// declaration stating a different name computes a different physical id, which is a
/// different resource ([`super::provisioning::is_same_resource`]), so the deployment creates
/// it and deletes the one it replaced instead of reaching this arm at all. This is therefore
/// only ever called where the declared name is unchanged, which is why setting it costs the
/// declaration nothing.
pub(super) async fn update_glue_database(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    named_glue_input(
        &mut properties,
        GLUE_DATABASE_INPUT_PROPERTY,
        &resource.physical_resource_id,
    );
    let mut request = glue_catalog_scope(declared);
    request.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert(
        GLUE_DATABASE_INPUT_PROPERTY.to_string(),
        properties
            .remove(GLUE_DATABASE_INPUT_PROPERTY)
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    call_glue(ctx, region, "UpdateDatabase", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Delete the database a stack created, addressed by its NAME in the catalog its declaration
/// names — this resource's physical id.
///
/// A database AWS Glue no longer holds (`EntityNotFoundException`, judged on the CODE and
/// never on a message that carries the request's own name) is already in the state the
/// delete asks for.
pub(super) async fn delete_glue_database(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut request = glue_catalog_scope(&resource.properties);
    request.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    match call_glue(ctx, region, "DeleteDatabase", &Value::Object(request)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The database as AWS Glue reports it, for drift detection: its own read's `Database`
/// document, reported under the property the type declares that definition in, beside the
/// catalog it resides in. The `Database` shape's members are spelled exactly as
/// `DatabaseInput`'s are, so a declared member is compared against the one AWS Glue holds.
pub(super) async fn describe_glue_database(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let mut request = glue_catalog_scope(&resource.properties);
    request.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    match call_glue(ctx, region, "GetDatabase", &Value::Object(request)).await {
        Ok(answer) => {
            let database = reported_member(&answer, "Database");
            Ok(Some(serde_json::json!({
                GLUE_CATALOG_ID_PROPERTY: reported_member(&database, GLUE_CATALOG_ID_PROPERTY),
                GLUE_DATABASE_INPUT_PROPERTY: database,
            })))
        }
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the catalog table an `AWS::Glue::Table` declares, in the database its declaration
/// names, so a client reading the deployed catalog back through `glue GetTable` finds it
/// under the name the stack publishes.
///
/// The type's properties ARE `CreateTable`'s own members — `CatalogId`, `DatabaseName`, the
/// whole `TableInput` structure and `OpenTableFormatInput`, named identically on both sides
/// — so the resolved property document IS the request, with the table's NAME (this
/// resource's physical id) set inside that input on the way out.
///
/// The type publishes no `Tags` property, so nothing is propagated to a table.
pub(super) async fn create_glue_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    let mut request = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    named_glue_input(
        &mut request,
        GLUE_TABLE_INPUT_PROPERTY,
        &resource.physical_resource_id,
    );
    call_glue(ctx, region, "CreateTable", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Apply a changed `AWS::Glue::Table` declaration to the table the stack created.
///
/// `UpdateTable` takes the same `TableInput` the create did, against the database that holds
/// the table — so a declaration that states another schema, another location or other
/// parameters reaches AWS Glue as that input, and the table keeps its identity.
///
/// The table's own name is the one it is HELD under, for the reason
/// [`update_glue_database`] records: a renamed table is a different resource, which the
/// deployment replaces rather than updates.
pub(super) async fn update_glue_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    named_glue_input(
        &mut properties,
        GLUE_TABLE_INPUT_PROPERTY,
        &resource.physical_resource_id,
    );
    let mut request = glue_table_scope(declared);
    request.insert(
        GLUE_TABLE_INPUT_PROPERTY.to_string(),
        properties
            .remove(GLUE_TABLE_INPUT_PROPERTY)
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    call_glue(ctx, region, "UpdateTable", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Delete the table a stack created, addressed by the pair AWS Glue keys a table by: the
/// database its declaration names and the table's own name, which is this resource's
/// physical id.
///
/// A table that service no longer holds (`EntityNotFoundException`, judged on the CODE) is
/// already in the state the delete asks for — as is one whose database is already gone,
/// which answers the same code.
pub(super) async fn delete_glue_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut request = glue_table_scope(&resource.properties);
    request.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    match call_glue(ctx, region, "DeleteTable", &Value::Object(request)).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The table as AWS Glue reports it, for drift detection: its own read's `Table` document,
/// reported under the property the type declares that definition in, beside the catalog and
/// the database it lives in. The `Table` shape's members are spelled exactly as
/// `TableInput`'s are.
pub(super) async fn describe_glue_table(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let mut request = glue_table_scope(&resource.properties);
    request.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    match call_glue(ctx, region, "GetTable", &Value::Object(request)).await {
        Ok(answer) => {
            let table = reported_member(&answer, "Table");
            Ok(Some(serde_json::json!({
                GLUE_CATALOG_ID_PROPERTY: reported_member(&table, GLUE_CATALOG_ID_PROPERTY),
                GLUE_DATABASE_NAME_PROPERTY: reported_member(&table, GLUE_DATABASE_NAME_PROPERTY),
                GLUE_TABLE_INPUT_PROPERTY: table,
            })))
        }
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Create the crawler an `AWS::Glue::Crawler` declares, so a client that deployed the stack
/// and then calls `glue GetCrawler` (or `StartCrawler`) with the name the stack publishes
/// reads the crawler back rather than an `EntityNotFoundException`.
///
/// The type's properties ARE `CreateCrawler`'s own members — `Role`, `DatabaseName`,
/// `Description`, `Targets`, `Classifiers`, `TablePrefix`, `SchemaChangePolicy`,
/// `RecrawlPolicy`, `LineageConfiguration`, `LakeFormationConfiguration`, `Configuration`
/// and `CrawlerSecurityConfiguration` are named identically on both sides — so the resolved
/// property document is the request, with two members decided on the way out: the crawler's
/// NAME (this resource's physical id, which is what "Ref returns the crawler name") and the
/// `Schedule`, which the type publishes as the structure a crawler is REPORTED with while
/// the call takes the cron expression inside it ([`glue_crawler_schedule`]).
///
/// `Tags` is the one member not carried verbatim: the crawler carries the tags the
/// declaration asks for beside the ones CloudFormation gives the resource ([`tags_of`]), as
/// the tag MAP `CreateCrawler`'s own `Tags` member takes. That set reaches AWS Glue on the
/// CREATE alone, for the reason an `AWS::Glue::Job`'s does ([`create_glue_job`]): a
/// crawler's tags are that service's own tagging calls rather than part of the configuration
/// an update overwrites.
pub(super) async fn create_glue_crawler(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut request = glue_crawler_configuration(&resource.properties, &resource.physical_resource_id);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert(
            GLUE_CRAWLER_TAGS_PROPERTY.to_string(),
            Value::Object(tags),
        );
    }
    call_glue(ctx, region, "CreateCrawler", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Apply a changed `AWS::Glue::Crawler` declaration to the crawler the stack created.
///
/// Every property of the type but its name and its tags is a member of `UpdateCrawler` too,
/// so the declaration this deployment states IS that request, against the `Name` the crawler
/// is held under.
pub(super) async fn update_glue_crawler(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let request = glue_crawler_configuration(declared, &resource.physical_resource_id);
    call_glue(ctx, region, "UpdateCrawler", &Value::Object(request)).await?;
    Ok(Created::default())
}

/// Delete the crawler a stack created, addressed by its NAME — this resource's physical id.
///
/// A crawler AWS Glue no longer holds (`EntityNotFoundException`, judged on the CODE and
/// never on a message that carries the request's own name) is already in the state the
/// delete asks for.
pub(super) async fn delete_glue_crawler(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ GLUE_CRAWLER_NAME_PROPERTY: physical_resource_id });
    match call_glue(ctx, region, "DeleteCrawler", &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The crawler as AWS Glue reports it, for drift detection: the `Crawler` document its own
/// read answers with, whose members are spelled exactly as the type's properties are — the
/// `Schedule` structure included, which is why the type's own `Schedule` is compared against
/// what that service reports rather than against the expression the create sent.
pub(super) async fn describe_glue_crawler(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ GLUE_CRAWLER_NAME_PROPERTY: physical_resource_id });
    match call_glue(ctx, region, "GetCrawler", &request).await {
        Ok(answer) => Ok(Some(reported_member(&answer, "Crawler"))),
        Err(refusal) if refusal.code == GLUE_ENTITY_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// The crawler configuration a declaration states, as the members `CreateCrawler` and
/// `UpdateCrawler` both take: the declared document with the NAME CloudFormation decided,
/// the tags lifted out (they are the create's own map, not part of the configuration) and
/// the `Schedule` reduced to the cron expression those calls take.
pub(super) fn glue_crawler_configuration(declared: &Value, name: &str) -> Map<String, Value> {
    let mut request = declared.as_object().cloned().unwrap_or_else(Map::new);
    request.remove(GLUE_CRAWLER_TAGS_PROPERTY);
    request.insert(
        GLUE_CRAWLER_NAME_PROPERTY.to_string(),
        Value::String(name.to_string()),
    );
    match request
        .get(GLUE_CRAWLER_SCHEDULE_PROPERTY)
        .and_then(glue_crawler_schedule)
    {
        Some(expression) => {
            request.insert(GLUE_CRAWLER_SCHEDULE_PROPERTY.to_string(), expression);
        }
        // A `Schedule` structure carrying no expression states no schedule to run the
        // crawler on, which is what a crawler with no `Schedule` member at all is.
        None => {
            request.remove(GLUE_CRAWLER_SCHEDULE_PROPERTY);
        }
    }
    request
}

/// The cron EXPRESSION a declared `Schedule` states, which is what `CreateCrawler` and
/// `UpdateCrawler` take as their `Schedule`: the structure's `ScheduleExpression` member,
/// or the value itself where the declaration wrote the expression directly.
pub(super) fn glue_crawler_schedule(declared: &Value) -> Option<Value> {
    match declared {
        Value::Object(_) => declared.get(GLUE_SCHEDULE_EXPRESSION_MEMBER).cloned(),
        other => Some(other.clone()),
    }
}

/// Set the NAME CloudFormation decided inside the input structure the owning API takes the
/// entity's definition in, leaving every other member the declaration states where it is —
/// and making the structure where a declaration carries none, so the create still names the
/// resource the stack published.
pub(super) fn named_glue_input(request: &mut Map<String, Value>, property: &str, name: &str) {
    let mut input = request
        .get(property)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(Map::new);
    input.insert(
        GLUE_INPUT_NAME_MEMBER.to_string(),
        Value::String(name.to_string()),
    );
    request.insert(property.to_string(), Value::Object(input));
}

/// The catalog a request about one of the two catalog entities is scoped to: the
/// `CatalogId` the declaration states, or nothing — which is that member's own documented
/// default, the account's own catalog.
pub(super) fn glue_catalog_scope(declared: &Value) -> Map<String, Value> {
    let mut request = Map::new();
    if let Some(catalog) = declared_text(declared, GLUE_CATALOG_ID_PROPERTY) {
        request.insert(
            GLUE_CATALOG_ID_PROPERTY.to_string(),
            Value::String(catalog),
        );
    }
    request
}

/// The catalog AND the database a request about a TABLE is scoped to: the pair every AWS
/// Glue call on a table takes beside the table's own name.
pub(super) fn glue_table_scope(declared: &Value) -> Map<String, Value> {
    let mut request = glue_catalog_scope(declared);
    if let Some(database) = declared_text(declared, GLUE_DATABASE_NAME_PROPERTY) {
        request.insert(
            GLUE_DATABASE_NAME_PROPERTY.to_string(),
            Value::String(database),
        );
    }
    request
}

/// Call one AWS Glue operation on the awsJson1_1 wire its model declares.
pub(super) async fn call_glue(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        GLUE_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{GLUE_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// One scalar member of an answered document as its text, or `None` where the answer does
/// not carry it (or carries it empty, which identifies nothing).
pub(super) fn reported_text(answered: &Value, member: &str) -> Option<String> {
    answered
        .get(member)
        .and_then(scalar_text)
        .filter(|text| !text.is_empty())
}

// ---------------------------------------------------------------------------
// AWS X-Ray: AWS::XRay::SamplingRule
// ---------------------------------------------------------------------------

/// AWS X-Ray's signing name, from its model (`sigv4Name` and `endpointPrefix` `xray`),
/// which speaks `restJson1`.
pub(super) const XRAY_SIGNING_NAME: &str = "xray";

/// The URI bindings of the three X-Ray calls a sampling-rule deployment makes —
/// `POST /CreateSamplingRule`, `POST /DeleteSamplingRule` and `POST /GetSamplingRules`
/// (`smithy_query(operation="CreateSamplingRule,DeleteSamplingRule,GetSamplingRules")`).
pub(super) const XRAY_CREATE_SAMPLING_RULE_URI: &str = "/CreateSamplingRule";
pub(super) const XRAY_DELETE_SAMPLING_RULE_URI: &str = "/DeleteSamplingRule";
pub(super) const XRAY_GET_SAMPLING_RULES_URI: &str = "/GetSamplingRules";

/// The one error class X-Ray answers a request of these calls it rejects under: "The
/// request is missing required parameters or has invalid parameters"
/// (`InvalidRequestException`, the 400 `CreateSamplingRule`, `DeleteSamplingRule` and
/// `GetSamplingRules` each model).
pub(super) const XRAY_INVALID_REQUEST: &str = "InvalidRequestException";

/// The property an `AWS::XRay::SamplingRule` states the rule under, which is also the
/// member `CreateSamplingRule` takes it as and the member a record of
/// `GetSamplingRules` carries it in.
pub(super) const XRAY_RULE_PROPERTY: &str = "SamplingRule";

/// The deprecated property an older declaration states the same rule under, as the
/// `SamplingRuleRecord` wrapper that type also publishes.
pub(super) const XRAY_RULE_RECORD_PROPERTY: &str = "SamplingRuleRecord";

/// The member of a rule that NAMES it: "the name of the sampling rule. Specify a rule by
/// either name or ARN, but not both" (`SamplingRule$RuleName`).
pub(super) const XRAY_RULE_NAME: &str = "RuleName";

/// The member of a rule that carries the ARN X-Ray minted for it
/// (`SamplingRule$RuleARN`), which is this type's identity.
pub(super) const XRAY_RULE_ARN: &str = "RuleARN";

/// How many pages of `GetSamplingRules` one walk reads before it stops.
///
/// The listing is paginated (`NextToken`), and a walk over a service's own answers needs a
/// bound rather than a loop that ends when that service says so: the cap is far above the
/// number of rules an account can hold, so a rule that exists is always found, and a
/// service answering an endless chain of pages cannot hold a stack operation open.
pub(super) const XRAY_SAMPLING_RULE_PAGES: usize = 64;

/// Create the sampling rule an `AWS::XRay::SamplingRule` declares, and report the
/// `RuleARN` X-Ray MINTED for it.
///
/// The rule itself is the declaration's own `SamplingRule` document, sent to
/// `CreateSamplingRule` as it was declared: its member names are that call's own
/// (`RuleName`, `ResourceARN`, `Priority`, `FixedRate`, `ReservoirSize`, `ServiceName`,
/// `ServiceType`, `Host`, `HTTPMethod`, `URLPath`, `Version`, `Attributes`), so nothing is
/// re-cased or re-mapped here and X-Ray's own validation is what judges the values — its
/// refusal leaves the stack `CREATE_FAILED` rather than a rule configured differently from
/// the template. An older declaration states the same document under the deprecated
/// `SamplingRuleRecord` wrapper, which is read when the live property is absent; a
/// declaration carrying neither describes no rule that could be created, so it is refused
/// here rather than sent as a request X-Ray would have to reject.
///
/// A rule is keyed by its NAME, and a declaration that states none is created under the
/// name CloudFormation decided for the resource ([`declared_name`] — the declared
/// `RuleName` the type still publishes, or the `<stack>-<logical id>-<unique id>` name it
/// generated, fitted to that member's own 32-character bound by the naming rules).
///
/// What comes back is the ARN that service minted, which no naming rule could compute:
/// it is this resource's physical id ([`super::template::identity_is_service_assigned`]
/// names the type) and its one published `Fn::GetAtt` attribute, so both are reported off
/// the create's answer.
pub(super) async fn create_sampling_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let declared = resource
        .properties
        .get(XRAY_RULE_PROPERTY)
        .or_else(|| {
            resource
                .properties
                .get(XRAY_RULE_RECORD_PROPERTY)
                .and_then(|record| record.get(XRAY_RULE_PROPERTY))
        })
        .and_then(Value::as_object);
    let Some(declared) = declared else {
        return Err(Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "{} declares no {XRAY_RULE_PROPERTY}",
                resource.resource_type
            ),
        });
    };
    let mut rule = declared.clone();
    // A rule is keyed by its NAME, so a declaration that identifies the rule nowhere is
    // created under the name CloudFormation decided for the resource. A declaration that
    // states the read-only `RuleARN` instead is left exactly as it stands: "specify a rule
    // by either name or ARN, but not both", so adding a name to it would make the request a
    // different one from the declaration, and X-Ray's own refusal is the honest answer.
    let states = |member: &str| {
        declared
            .get(member)
            .and_then(scalar_text)
            .is_some_and(|text| !text.is_empty())
    };
    if !states(XRAY_RULE_NAME) && !states(XRAY_RULE_ARN) {
        rule.insert(
            XRAY_RULE_NAME.to_string(),
            Value::String(declared_name(resource)),
        );
    }
    let mut request = Map::new();
    request.insert(XRAY_RULE_PROPERTY.to_string(), Value::Object(rule));
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("Tags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }
    let reported = call_rest_json(
        ctx,
        XRAY_SIGNING_NAME,
        region,
        "POST",
        XRAY_CREATE_SAMPLING_RULE_URI,
        Some(&Value::Object(request)),
    )
    .await?;
    let created = reported
        .get(XRAY_RULE_RECORD_PROPERTY)
        .and_then(|record| record.get(XRAY_RULE_PROPERTY))
        .cloned()
        .unwrap_or(Value::Null);
    let Some(rule_arn) = reported_text(&created, XRAY_RULE_ARN) else {
        // The create answered 2xx without the ARN its output models. Nothing addresses a
        // rule by an identity nothing minted, so this is reported as a failure rather
        // than recorded as a resource with a physical id this module invented.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("CreateSamplingRule answered no {XRAY_RULE_ARN}"),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert(XRAY_RULE_ARN.to_string(), rule_arn.clone());
    Ok(Created {
        physical_resource_id: Some(rule_arn),
        attributes,
    })
}

/// Delete the sampling rule a stack created, addressed the way X-Ray addresses one:
/// "specify a rule by either name or ARN, but not both" ([`sampling_rule_handle`]).
///
/// A rule X-Ray no longer holds is already in the state the delete asks for. That service
/// answers every client rejection of this call under ONE class, so the code alone does not
/// say WHICH rejection it was: a refusal is confirmed by reading the account's rules back
/// ([`held_sampling_rule`]), exactly as [`delete_guardduty_ip_set`] confirms one, and only
/// a rule that listing no longer holds is treated as already deleted.
pub(super) async fn delete_sampling_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (member, selector) = sampling_rule_handle(resource);
    let mut request = Map::new();
    request.insert(member.to_string(), Value::String(selector.to_string()));
    let refusal = match call_rest_json(
        ctx,
        XRAY_SIGNING_NAME,
        region,
        "POST",
        XRAY_DELETE_SAMPLING_RULE_URI,
        Some(&Value::Object(request)),
    )
    .await
    {
        Ok(_) => return Ok(()),
        Err(refusal) => refusal,
    };
    if refusal.code != XRAY_INVALID_REQUEST {
        return Err(refusal);
    }
    match held_sampling_rule(ctx, region, resource).await? {
        // The rule is still there, so the delete really was refused.
        Some(_) => Err(refusal),
        None => Ok(()),
    }
}

/// The rule's configuration as X-Ray reports it, for drift detection, in the shape the
/// type declares it in — the rule document under its own `SamplingRule` property — so a
/// declared member is compared against the member of the same name that service answers
/// with.
///
/// A rule the listing no longer holds was deleted, which for a READ is what
/// [`Provisioner::actual_configuration`] reports as `DELETED`.
///
/// `Tags` is not reported: the type publishes them as a `[{Key, Value}]` list, the listing
/// carries none, and a property the actual document does not carry is not evidence of
/// anything (`super::drift::differences`) — so a rule's tags are never read as drift
/// rather than compared against a set this read cannot see.
pub(super) async fn describe_sampling_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Some(rule) = held_sampling_rule(ctx, region, resource).await? else {
        return Ok(None);
    };
    let mut actual = Map::new();
    actual.insert(XRAY_RULE_PROPERTY.to_string(), rule);
    Ok(Some(Value::Object(actual)))
}

/// The rule this resource names, as X-Ray's own `GetSamplingRules` reports it, or `None`
/// for a rule that listing does not hold.
///
/// X-Ray publishes no read of ONE sampling rule, so the account's listing is what answers
/// whether a rule is there — walked page by page ([`XRAY_SAMPLING_RULE_PAGES`]) and matched
/// on the member that carries this resource's identity ([`sampling_rule_handle`]).
pub(super) async fn held_sampling_rule(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let (member, selector) = sampling_rule_handle(resource);
    let mut next_token: Option<String> = None;
    for _ in 0..XRAY_SAMPLING_RULE_PAGES {
        let mut request = Map::new();
        if let Some(token) = &next_token {
            request.insert("NextToken".to_string(), Value::String(token.clone()));
        }
        let answered = call_rest_json(
            ctx,
            XRAY_SIGNING_NAME,
            region,
            "POST",
            XRAY_GET_SAMPLING_RULES_URI,
            Some(&Value::Object(request)),
        )
        .await?;
        let records = answered
            .get("SamplingRuleRecords")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for record in &records {
            let Some(rule) = record.get(XRAY_RULE_PROPERTY) else {
                continue;
            };
            if reported_text(rule, member).as_deref() == Some(selector) {
                return Ok(Some(rule.clone()));
            }
        }
        match reported_text(&answered, "NextToken") {
            Some(token) => next_token = Some(token),
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The member a call on this resource's rule addresses it by, and the value to address it
/// with: the `RuleARN` X-Ray minted, which is this type's physical id once its create went
/// through, and otherwise the `RuleName` the resource carries — "specify a rule by either
/// name or ARN, but not both", so exactly one of the two travels.
pub(super) fn sampling_rule_handle(resource: &StoredResource) -> (&'static str, &str) {
    let identity = resource.physical_resource_id.as_str();
    if identity.starts_with("arn:") {
        return (XRAY_RULE_ARN, identity);
    }
    (XRAY_RULE_NAME, identity)
}

// ---------------------------------------------------------------------------
// Amazon Bedrock AgentCore: AWS::BedrockAgentCore::WorkloadIdentity
// ---------------------------------------------------------------------------

/// The AgentCore CONTROL plane's signing name, from its model (`sigv4Name:
/// bedrock-agentcore`, `endpointPrefix: bedrock-agentcore-control`), which speaks
/// `restJson1` — so its calls ride on [`call_rest_json`], each on the URI its own
/// operation's `@http` binding declares.
pub(super) const AGENTCORE_SIGNING_NAME: &str = "bedrock-agentcore";

/// The URIs the workload-identity operations a stack makes are bound to, as that model's
/// `@http` traits declare them (`POST /identities/CreateWorkloadIdentity` and so on: each
/// operation of this resource family is a POST to a URI named after it).
pub(super) const AGENTCORE_CREATE_WORKLOAD_IDENTITY_URI: &str = "/identities/CreateWorkloadIdentity";
pub(super) const AGENTCORE_GET_WORKLOAD_IDENTITY_URI: &str = "/identities/GetWorkloadIdentity";
pub(super) const AGENTCORE_UPDATE_WORKLOAD_IDENTITY_URI: &str = "/identities/UpdateWorkloadIdentity";
pub(super) const AGENTCORE_DELETE_WORKLOAD_IDENTITY_URI: &str = "/identities/DeleteWorkloadIdentity";

/// The error code the AgentCore control plane answers for a workload identity it does not
/// hold (`ResourceNotFoundException`, the modelled 404 of its reads, updates and deletes).
pub(super) const AGENTCORE_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The one `AWS::BedrockAgentCore::WorkloadIdentity` property beside the name and the tags
/// that is a member of the create and of the update, as (property, member).
///
/// Source: `CreateWorkloadIdentity`'s and `UpdateWorkloadIdentity`'s input members in that
/// service's model (`name`, `allowedResourceOauth2ReturnUrls`, and the create's `tags`)
/// against the upper-camel property names a CloudFormation template spells. The name is
/// not here because it is the one member CloudFormation decides
/// ([`workload_identity_request`]), and the tags because they are a map the declaration
/// and the stack both contribute to ([`create_workload_identity`]).
pub(super) const WORKLOAD_IDENTITY_MEMBERS: &[(&str, &str)] = &[(
    "AllowedResourceOauth2ReturnUrls",
    "allowedResourceOauth2ReturnUrls",
)];

/// Create the workload identity an `AWS::BedrockAgentCore::WorkloadIdentity` declares, so a
/// client that deployed the stack and then calls `GetWorkloadIdentity` with the name the
/// stack publishes reads the identity back rather than a `ResourceNotFoundException`.
///
/// "Ref returns the name", which is this resource's physical id and the handle every
/// AgentCore call on an identity takes, so the identity is created under exactly that name
/// and the ARN that service minted is reported back as the resource's
/// `WorkloadIdentityArn` attribute.
///
/// The identity carries the tags the declaration asks for beside the ones CloudFormation
/// gives the resource ([`tags_of`]), as the tag MAP `CreateWorkloadIdentity`'s own `tags`
/// member takes. That set reaches the control plane on the CREATE alone: this type has no
/// [`Provisioner::retag`] arm, so a later deployment that changes only the tags leaves the
/// identity carrying the set it was created with.
pub(super) async fn create_workload_identity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    // A property no arm below maps onto `CreateWorkloadIdentity`'s request would otherwise
    // be dropped silently, leaving an identity configured differently from the
    // declaration; refusing names which property it was. The type publishes exactly these
    // three (AWS's resource reference for it against `CreateWorkloadIdentityRequest`).
    refuse_unmapped_properties(
        resource,
        &["Name", "Tags", "AllowedResourceOauth2ReturnUrls"],
    )?;
    let mut request =
        workload_identity_request(&resource.physical_resource_id, &resource.properties);
    let tags = tags_as_map(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert("tags".to_string(), Value::Object(tags));
    }
    let answered = call_agentcore(
        ctx,
        region,
        AGENTCORE_CREATE_WORKLOAD_IDENTITY_URI,
        &Value::Object(request),
    )
    .await?;
    Ok(Created::attributes(workload_identity_attributes(
        &answered,
    )?))
}

/// Apply a changed declaration to the workload identity the stack created, through
/// `UpdateWorkloadIdentity` — the one call that changes an identity that exists.
///
/// The request carries the whole declared list, because that member "replaces the existing
/// list": a declaration that states none leaves the identity with none, rather than with
/// the list an earlier deployment gave it. The identity's NAME is create-only (the service
/// publishes no call that renames one), so a declaration that states another name replaces
/// the resource and never reaches here.
pub(super) async fn update_workload_identity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let request = workload_identity_request(&resource.physical_resource_id, declared);
    let answered = call_agentcore(
        ctx,
        region,
        AGENTCORE_UPDATE_WORKLOAD_IDENTITY_URI,
        &Value::Object(request),
    )
    .await?;
    Ok(Created::attributes(workload_identity_attributes(
        &answered,
    )?))
}

/// Delete the workload identity a stack created, by the name it was created under — this
/// resource's physical id. One the service no longer holds
/// (`ResourceNotFoundException`, judged on the CODE and never on a message that carries
/// the request's own name) is already in the state the delete asks for.
pub(super) async fn delete_workload_identity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({ "name": physical_resource_id });
    match call_agentcore(
        ctx,
        region,
        AGENTCORE_DELETE_WORKLOAD_IDENTITY_URI,
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == AGENTCORE_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The workload identity's configuration as the AgentCore control plane reports it, for
/// drift detection: `GetWorkloadIdentity`'s own answer, read by the name that is this
/// resource's physical id. One it no longer holds (`ResourceNotFoundException`) was
/// deleted.
///
/// The members its read reports under the names this type declares are the identity's name
/// and its allowed return URLs; the instants it reports are no property of the type, and a
/// property the read does not report is not evidence of anything ([`super::drift`]).
pub(super) async fn describe_workload_identity(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({ "name": physical_resource_id });
    match call_agentcore(ctx, region, AGENTCORE_GET_WORKLOAD_IDENTITY_URI, &request).await {
        Ok(answer) => {
            let mut members = vec![("Name", "name")];
            members.extend_from_slice(WORKLOAD_IDENTITY_MEMBERS);
            Ok(Some(reported_as_properties(&answer, &members)))
        }
        Err(refusal) if refusal.code == AGENTCORE_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// One workload-identity create or update request: the `name` CloudFormation decided, plus
/// the members `properties` declares ([`WORKLOAD_IDENTITY_MEMBERS`]). The two calls take
/// the same members, so one builder serves both; the create adds the tags on top of it.
pub(super) fn workload_identity_request(name: &str, properties: &Value) -> Map<String, Value> {
    let mut request = Map::new();
    request.insert("name".to_string(), Value::String(name.to_string()));
    if let Some(declared) = properties.as_object() {
        copy_members(declared, WORKLOAD_IDENTITY_MEMBERS, &mut request);
    }
    request
}

/// The attributes an answered workload identity reports: the two values the type publishes
/// as return values, read off the answer rather than recomputed here.
///
/// An answer carrying neither is reported as a refusal rather than recorded as a created
/// resource: there would be no handle a client could call the service with.
pub(super) fn workload_identity_attributes(answered: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let (Some(name), Some(arn)) = (
        reported_text(answered, "name"),
        reported_text(answered, "workloadIdentityArn"),
    ) else {
        return Err(Refusal {
            code: "InternalFailure".to_string(),
            message: "the Amazon Bedrock AgentCore control plane answered a workload identity \
                      without a name and an ARN"
                .to_string(),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert("Name".to_string(), name);
    attributes.insert("WorkloadIdentityArn".to_string(), arn);
    Ok(attributes)
}

/// Call one operation of the Amazon Bedrock AgentCore control plane on the `restJson1` wire
/// its model declares: a POST to the operation's own URI, carrying the request document.
pub(super) async fn call_agentcore(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    uri: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_rest_json(
        ctx,
        AGENTCORE_SIGNING_NAME,
        region,
        "POST",
        uri,
        Some(request),
    )
    .await
}
