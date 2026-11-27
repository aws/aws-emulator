// SPDX-License-Identifier: CC0-1.0

//! The AWS CodeArtifact PACKAGE GROUP a stack declares: `AWS::CodeArtifact::PackageGroup`,
//! created in CodeArtifact itself through that service's own `CreatePackageGroup`, read back
//! through `DescribePackageGroup`, changed by `UpdatePackageGroup` (and
//! `UpdatePackageGroupOriginConfiguration`), re-tagged through `TagResource`/`UntagResource`
//! and removed by `DeletePackageGroup`.
//!
//! One file of [`super`]'s per-type arms, beside the domain and repository arms the same
//! owning service has ([`super::network_and_delivery`]): a package group is addressed by a
//! PATTERN rather than by a name — the pattern IS the group's identifier — and its ARN is the
//! one CodeArtifact minted for it, so its arms live together here.
//!
//! ## The rule this file carries
//!
//! A stack reports a resource created only where the service that OWNS the type really holds
//! it, and the identifiers it publishes for the resource are the ones that service answered.
//! So a stack that reached `CREATE_COMPLETE` with a package group declared is one whose client
//! finds that group through `DescribePackageGroup` under the domain and the pattern the
//! template stated, and a stack that removed it is one whose group CodeArtifact no longer
//! holds.
//!
//! The group's handle is its ARN: "Ref returns the Amazon Resource Name (ARN) of the package
//! group", a value whose resource path carries the pattern with every character an ARN does
//! not hold literally escaped — which no naming rule here computes — so it is this resource's
//! physical id and comes off the create's own answer
//! ([`crate::state::template::identity_is_service_assigned`] names the type). The type
//! declares no name of its own at all: what a declaration states is the `Pattern` the group is
//! created under, which every CodeArtifact call on the group takes beside its domain.

use super::*;

/// The resource type this file provisions. Visible to the rest of this service's state because
/// the identity catalogue and the replacement-property catalogue key their entries for the
/// type by it.
pub(in crate::state) const CODEARTIFACT_PACKAGE_GROUP: &str = "AWS::CodeArtifact::PackageGroup";

/// The URI the CodeArtifact package-group calls are bound to, as that model's `@http` traits
/// declare them: `CreatePackageGroup` POSTs, `DescribePackageGroup` GETs,
/// `UpdatePackageGroup` PUTs and `DeletePackageGroup` DELETEs this one path, each naming the
/// domain in the `domain`/`domain-owner` query parameters ([`codeartifact_uri`]).
const PACKAGE_GROUP_URI: &str = "/v1/package-group";

/// The URI `UpdatePackageGroupOriginConfiguration` is bound to — the call that moves a
/// restriction off the `INHERIT` every created group starts in, since `CreatePackageGroup`
/// takes no origin configuration at all.
const PACKAGE_GROUP_ORIGIN_CONFIGURATION_URI: &str = "/v1/package-group-origin-configuration";

/// The query parameter `DescribePackageGroup`, `DeletePackageGroup` and
/// `UpdatePackageGroupOriginConfiguration` bind the group's own identifier to. (The create and
/// `UpdatePackageGroup` carry it in the BODY instead, which is why the member name below is
/// used for those two.)
const PACKAGE_GROUP_PARAMETER: &str = "package-group";

/// The property whose value IS the group's identifier: "the pattern determines which packages
/// are associated with the package group", and that pattern is also what addresses the group.
const PATTERN_PROPERTY: &str = "Pattern";

/// The `AWS::CodeArtifact::PackageGroup` properties `CreatePackageGroup` takes under its own
/// member name, as `(property, member)` pairs.
///
/// Source: AWS's CloudFormation resource reference for the type against
/// `smithy_query(query="operation",operation="CreatePackageGroup",
/// model_path="services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/\
/// smithy-model/com-amazonaws-codeartifact-codeartifactcontrolplaneservice.json")` — that
/// call's members are `domain`, `domainOwner`, `packageGroup`, `contactInfo`, `description`
/// and `tags`.
///
/// `Pattern` is not here because it is the `packageGroup` member the request carries as the
/// group's identifier ([`PATTERN_PROPERTY`]); `DomainName` and `DomainOwner` are not because
/// that call binds each to a query parameter; `Tags` is not because the member spells a tag's
/// halves in lower camel case ([`LOWER_CASED_TAG`]) and the tags CloudFormation gives the
/// resource travel beside the declared ones; and `OriginConfiguration` is not because it is no
/// member of that create at all ([`PACKAGE_GROUP_ORIGIN_CONFIGURATION_URI`]).
const PACKAGE_GROUP_MEMBERS: &[(&str, &str)] = &[
    ("ContactInfo", "contactInfo"),
    ("Description", "description"),
];

/// The property carrying the origin controls a declaration asks for, and the member INSIDE it
/// that carries them: `OriginConfiguration: { Restrictions: { … } }`.
const ORIGIN_CONFIGURATION_PROPERTY: &str = "OriginConfiguration";
/// See [`ORIGIN_CONFIGURATION_PROPERTY`].
const ORIGIN_RESTRICTIONS_PROPERTY: &str = "Restrictions";

/// Each restriction the type publishes inside `Restrictions`, and the
/// `PackageGroupOriginRestrictionType` it is keyed by in the `OriginRestrictions` MAP
/// `UpdatePackageGroupOriginConfiguration` takes (`smithy_query(query="shape",
/// shape="PackageGroupOriginRestrictionType")`: `EXTERNAL_UPSTREAM`, `INTERNAL_UPSTREAM`,
/// `PUBLISH`).
///
/// The value of each is a `PackageGroupOriginRestrictionMode`, which the declaration states
/// and that service's own validation judges — a mode it refuses fails the resource rather
/// than leaving a group whose origin controls differ from the template.
const ORIGIN_RESTRICTIONS: &[(&str, &str)] = &[
    ("ExternalUpstream", "EXTERNAL_UPSTREAM"),
    ("InternalUpstream", "INTERNAL_UPSTREAM"),
    ("Publish", "PUBLISH"),
];

/// Every property the type publishes, which is what a declaration may state: a property no arm
/// here maps onto a CodeArtifact request is refused rather than dropped silently
/// ([`refuse_unmapped_properties`]), so a group the stack reports created is never one
/// configured with less than its declaration asked for.
const PACKAGE_GROUP_PROPERTIES: &[&str] = &[
    PATTERN_PROPERTY,
    CODEARTIFACT_DOMAIN_PROPERTY,
    CODEARTIFACT_DOMAIN_OWNER_PROPERTY,
    "ContactInfo",
    "Description",
    ORIGIN_CONFIGURATION_PROPERTY,
    "Tags",
];

/// The member every one of those calls carries the group in — in the request as the identifier
/// (`CreatePackageGroup$packageGroup`) and in the answer as the whole
/// `PackageGroupDescription` — and the member of that description carrying the ARN
/// CodeArtifact minted.
const PACKAGE_GROUP_MEMBER: &str = "packageGroup";
/// See [`PACKAGE_GROUP_MEMBER`].
const ARN_MEMBER: &str = "arn";

/// The attribute name this type publishes that ARN under, which is the one value its "Return
/// values" section names beside `Ref`.
const ARN_ATTRIBUTE: &str = "Arn";

/// The `PackageGroupDescription` members `DescribePackageGroup` reports under this type's own
/// property names, as `(property, member)` pairs, for drift detection.
///
/// `Tags` is left out for the reason it is everywhere else here — the tags a resource carries
/// change independently of its declaration — and `OriginConfiguration` because the read
/// reports it as a map of `PackageGroupOriginRestriction` STRUCTURES (each carrying the
/// effective mode and the group it was inherited from) while the type publishes a map of plain
/// modes, so the two documents could not be compared without inventing a reading of one of
/// them.
const PACKAGE_GROUP_REPORTED_PROPERTIES: &[(&str, &str)] = &[
    (PATTERN_PROPERTY, "pattern"),
    (CODEARTIFACT_DOMAIN_PROPERTY, "domainName"),
    (CODEARTIFACT_DOMAIN_OWNER_PROPERTY, "domainOwner"),
    ("ContactInfo", "contactInfo"),
    ("Description", "description"),
];

/// The URIs CodeArtifact's own tagging calls are bound to, as their `@http` traits declare
/// them: each names the resource in the `resourceArn` query parameter and carries the tags or
/// the keys in its body.
const PACKAGE_GROUP_TAGS_URI: &str = "/v1/tags";
/// See [`PACKAGE_GROUP_TAGS_URI`].
const PACKAGE_GROUP_TAG_URI: &str = "/v1/tag";
/// See [`PACKAGE_GROUP_TAGS_URI`].
const PACKAGE_GROUP_UNTAG_URI: &str = "/v1/untag";

/// The member every one of those calls carries the tag set in, in a request and in an answer
/// alike (`TagResource$tags`, `ListTagsForResourceResult$tags`), and the one `UntagResource`
/// takes the keys to drop in.
const TAGS_MEMBER: &str = "tags";
/// See [`TAGS_MEMBER`].
const TAG_KEYS_MEMBER: &str = "tagKeys";

/// Create the package group an `AWS::CodeArtifact::PackageGroup` declares, in the domain its
/// declaration names, and apply the origin restrictions it asks for — so a client that deployed
/// the stack and then calls `codeartifact describe-package-group` with the domain and the
/// pattern the stack published reads the group back rather than a `ResourceNotFoundException`.
///
/// The group is created under the `Pattern` the declaration states, which is the group's own
/// identifier and the value `CreatePackageGroup` carries as its `packageGroup` member —
/// CodeArtifact's own validation of that pattern is what judges it, so a pattern it refuses
/// leaves the stack `CREATE_FAILED` rather than a group matching packages the template never
/// described.
///
/// `OriginConfiguration` is no member of that create: every restriction of a created group
/// inherits, and `UpdatePackageGroupOriginConfiguration` is what moves one off `INHERIT`, so
/// the declared restrictions reach CodeArtifact through that call on the group just created —
/// the same second call a client makes. The resource the template declares is the group AND
/// those controls, so one CodeArtifact refuses fails the resource, and the group this arm
/// already created is removed before that refusal propagates (the stack records no physical id
/// for a resource whose create failed, so nothing else would ever delete it).
///
/// The group carries the tags the declaration asks for plus the stack's own ([`tags_of`]), as
/// the `[{key, value}]` list that model's own `Tag` shape spells.
///
/// What comes back is the ARN that service minted, whose resource path carries the pattern with
/// every character an ARN does not hold literally escaped: it is this resource's physical id
/// and its one published `Fn::GetAtt` attribute, so both are reported off the answer of the
/// last call made on the group.
pub(super) async fn create_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, PACKAGE_GROUP_PROPERTIES)?;
    let domain = codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY);
    let owner = codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY);
    let pattern = codeartifact_property(resource, PATTERN_PROPERTY);
    let addressed: &[(&str, &str)] = &[("domain", &domain), ("domain-owner", &owner)];

    let mut request = Map::new();
    request.insert(
        PACKAGE_GROUP_MEMBER.to_string(),
        Value::String(pattern.clone()),
    );
    if let Some(declared) = resource.properties.as_object() {
        copy_members(declared, PACKAGE_GROUP_MEMBERS, &mut request);
    }
    let tags = tags_of(&resource.properties, propagated);
    if !tags.is_empty() {
        request.insert("tags".to_string(), tags_as_list(&tags, LOWER_CASED_TAG));
    }
    let uri = codeartifact_uri(PACKAGE_GROUP_URI, addressed);
    let answered =
        call_codeartifact(ctx, region, "POST", &uri, Some(&Value::Object(request))).await?;
    // Each further call answers with the group as it stands, so what the resource reports is
    // what the LAST of them described.
    let mut described = reported_member(&answered, PACKAGE_GROUP_MEMBER);

    if let Some(restrictions) = declared_origin_restrictions(&resource.properties)? {
        let mut parameters = addressed.to_vec();
        parameters.push((PACKAGE_GROUP_PARAMETER, &pattern));
        let uri = codeartifact_uri(PACKAGE_GROUP_ORIGIN_CONFIGURATION_URI, &parameters);
        let configuration = serde_json::json!({ "restrictions": restrictions });
        match call_codeartifact(ctx, region, "PUT", &uri, Some(&configuration)).await {
            Ok(answer) => described = reported_member(&answer, PACKAGE_GROUP_MEMBER),
            Err(refusal) => {
                let removal =
                    delete_codeartifact_package_group(ctx, region, &domain, &owner, &pattern)
                        .await;
                return Err(rolled_back(refusal, removal, "package group"));
            }
        }
    }

    let Some(arn) = described.get(ARN_MEMBER).and_then(scalar_text) else {
        // The calls answered 2xx without the ARN their output models. Nothing addresses a
        // package group by an identity nothing minted, so this is reported as a failure rather
        // than recorded as a resource with a physical id this module invented.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("CreatePackageGroup answered no {ARN_MEMBER}"),
        });
    };
    let mut attributes = BTreeMap::new();
    attributes.insert(ARN_ATTRIBUTE.to_string(), arn.clone());
    Ok(Created {
        physical_resource_id: Some(arn),
        attributes,
    })
}

/// Delete the package group a stack created, addressed by the domain and the pattern it was
/// created under — the pair `DeletePackageGroup` takes, neither of which is this resource's
/// physical id (that is the group's ARN).
///
/// A group CodeArtifact no longer holds — or one whose DOMAIN it no longer holds, which answers
/// the same code — is already in the state the delete asks for
/// ([`CODEARTIFACT_RESOURCE_NOT_FOUND`], judged on the CODE and never on a message that carries
/// the request's own name).
pub(super) async fn delete_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    domain: &str,
    owner: &str,
    pattern: &str,
) -> Result<(), Refusal> {
    let uri = codeartifact_uri(
        PACKAGE_GROUP_URI,
        &[
            ("domain", domain),
            ("domain-owner", owner),
            (PACKAGE_GROUP_PARAMETER, pattern),
        ],
    );
    match call_codeartifact(ctx, region, "DELETE", &uri, None).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The group a stack's record names, deleted by the pair its own declaration carries: the arm
/// [`super::Provisioner::delete`] dispatches, which reads the domain and the pattern off the
/// record rather than off the physical id.
pub(super) async fn delete_declared_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_codeartifact_package_group(
        ctx,
        region,
        &codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY),
        &codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY),
        &codeartifact_property(resource, PATTERN_PROPERTY),
    )
    .await
}

/// Apply a CHANGED declaration to the group the stack already has, through
/// `UpdatePackageGroup` — "update a package group. This includes updating the group's contact
/// information and description" — and, where the declaration states them, the origin
/// restrictions `UpdatePackageGroupOriginConfiguration` writes.
///
/// Those are the whole of what a group that exists can be changed to: the domain it lives in
/// and the pattern that identifies it are no members of either call, which is why a declaration
/// stating either differently REPLACES the resource
/// ([`crate::state::template::replacement_properties`]) rather than reaching here.
///
/// What comes back is the group as that service now reports it, so the ARN the resource
/// publishes stays the one CodeArtifact keyed the group by.
pub(super) async fn update_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let asked = StoredResource {
        properties: declared.clone(),
        ..resource.clone()
    };
    refuse_unmapped_properties(&asked, PACKAGE_GROUP_PROPERTIES)?;
    let domain = codeartifact_property(&asked, CODEARTIFACT_DOMAIN_PROPERTY);
    let owner = codeartifact_property(&asked, CODEARTIFACT_DOMAIN_OWNER_PROPERTY);
    let pattern = codeartifact_property(&asked, PATTERN_PROPERTY);
    let addressed: &[(&str, &str)] = &[("domain", &domain), ("domain-owner", &owner)];

    let mut request = Map::new();
    request.insert(
        PACKAGE_GROUP_MEMBER.to_string(),
        Value::String(pattern.clone()),
    );
    if let Some(stated) = declared.as_object() {
        copy_members(stated, PACKAGE_GROUP_MEMBERS, &mut request);
    }
    let uri = codeartifact_uri(PACKAGE_GROUP_URI, addressed);
    let answered =
        call_codeartifact(ctx, region, "PUT", &uri, Some(&Value::Object(request))).await?;
    let mut described = reported_member(&answered, PACKAGE_GROUP_MEMBER);

    if let Some(restrictions) = declared_origin_restrictions(declared)? {
        let mut parameters = addressed.to_vec();
        parameters.push((PACKAGE_GROUP_PARAMETER, &pattern));
        let uri = codeartifact_uri(PACKAGE_GROUP_ORIGIN_CONFIGURATION_URI, &parameters);
        let configuration = serde_json::json!({ "restrictions": restrictions });
        let answer = call_codeartifact(ctx, region, "PUT", &uri, Some(&configuration)).await?;
        described = reported_member(&answer, PACKAGE_GROUP_MEMBER);
    }

    let mut attributes = BTreeMap::new();
    let Some(arn) = described.get(ARN_MEMBER).and_then(scalar_text) else {
        // The same refusal the create makes for an answer carrying no ARN: the group's one
        // published return value is that ARN, and reporting the resource updated without it
        // would leave `Ref` and `Fn::GetAtt Arn` resolving to nothing.
        return Err(Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("UpdatePackageGroup answered no {ARN_MEMBER}"),
        });
    };
    attributes.insert(ARN_ATTRIBUTE.to_string(), arn);
    Ok(Created::attributes(attributes))
}

/// The group's configuration as CodeArtifact reports it, for drift detection, under the
/// property names the type declares — so a declared member is compared against the member of
/// the same name that service answers with.
///
/// A group it no longer holds — or one whose DOMAIN it no longer holds — answers
/// [`CODEARTIFACT_RESOURCE_NOT_FOUND`], which is that service saying the resource was deleted.
pub(super) async fn describe_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let uri = codeartifact_uri(
        PACKAGE_GROUP_URI,
        &[
            (
                "domain",
                &codeartifact_property(resource, CODEARTIFACT_DOMAIN_PROPERTY),
            ),
            (
                "domain-owner",
                &codeartifact_property(resource, CODEARTIFACT_DOMAIN_OWNER_PROPERTY),
            ),
            (
                PACKAGE_GROUP_PARAMETER,
                &codeartifact_property(resource, PATTERN_PROPERTY),
            ),
        ],
    );
    match call_codeartifact(ctx, region, "GET", &uri, None).await {
        Ok(answer) => Ok(Some(reported_as_properties(
            &reported_member(&answer, PACKAGE_GROUP_MEMBER),
            PACKAGE_GROUP_REPORTED_PROPERTIES,
        ))),
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => Ok(None),
        Err(refusal) => Err(refusal),
    }
}

/// Bring the tags of the package group a resource record names to the ones this deployment
/// gives it, through CodeArtifact's own `TagResource` and `UntagResource`.
///
/// `CreatePackageGroup`'s `tags` list applies to the group that call creates and to no group
/// that already exists, and `UpdatePackageGroup` carries no tag member at all — so this is the
/// one place the set a group is to end up carrying, the stack's own tags included, is
/// reconciled. The set is the declaration's own `Tags` beside the ones CloudFormation
/// propagates ([`tags_of`]), the same set the create applied, so a key outside it is one this
/// deployment no longer gives the resource and is removed, and one in it with another value is
/// re-valued. Where the two sets already agree nothing is called at all, which is what makes
/// this safe to ask on every deployment.
///
/// Every one of those calls names the resource by its ARN, which is this group's physical id;
/// a record carrying no identity is a resource whose create never answered one, so there is
/// nothing to reconcile. A group CodeArtifact no longer holds has no tags to reconcile either.
pub(super) async fn retag_codeartifact_package_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    let arn = resource.physical_resource_id.clone();
    if arn.is_empty() {
        return Ok(());
    }
    let addressed = codeartifact_uri(PACKAGE_GROUP_TAGS_URI, &[("resourceArn", &arn)]);
    let empty = Value::Object(Map::new());
    let listed = match call_codeartifact(ctx, region, "POST", &addressed, Some(&empty)).await {
        Ok(answer) => answer,
        Err(refusal) if refusal.code == CODEARTIFACT_RESOURCE_NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    let held: Vec<(String, String)> = listed
        .get(TAGS_MEMBER)
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag.get(LOWER_CASED_TAG.0).and_then(scalar_text)?,
                        tag.get(LOWER_CASED_TAG.1).and_then(scalar_text)?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let (added, removed) = tag_delta(&held, &tags_of(declared, propagated));
    if !added.is_empty() {
        let uri = codeartifact_uri(PACKAGE_GROUP_TAG_URI, &[("resourceArn", &arn)]);
        let request = serde_json::json!({ TAGS_MEMBER: tags_as_list(&added, LOWER_CASED_TAG) });
        call_codeartifact(ctx, region, "POST", &uri, Some(&request)).await?;
    }
    if !removed.is_empty() {
        let uri = codeartifact_uri(PACKAGE_GROUP_UNTAG_URI, &[("resourceArn", &arn)]);
        let request = serde_json::json!({ TAG_KEYS_MEMBER: tag_keys_as_list(&removed) });
        call_codeartifact(ctx, region, "POST", &uri, Some(&request)).await?;
    }
    Ok(())
}

/// The `OriginRestrictions` map a declaration asks for, as
/// `UpdatePackageGroupOriginConfiguration` takes it: one entry per restriction the declaration
/// states inside `OriginConfiguration.Restrictions`, keyed by that restriction's own
/// `PackageGroupOriginRestrictionType` ([`ORIGIN_RESTRICTIONS`]).
///
/// `Ok(None)` for a declaration that states none, which is a group left with the `INHERIT`
/// every created group starts in — so no second call is made for it at all.
///
/// Anything this function cannot CARRY is REFUSED, exactly as an unmapped top-level property
/// is ([`refuse_unmapped_properties`]): a member of either document that no arm here maps, a
/// document that is not an object at all, and a restriction whose mode is no single value.
/// A declaration carried no further than this function would leave the stack reporting a group
/// whose origin controls are not the ones it asked for.
fn declared_origin_restrictions(properties: &Value) -> Result<Option<Map<String, Value>>, Refusal> {
    let Some(configuration) = properties.get(ORIGIN_CONFIGURATION_PROPERTY) else {
        return Ok(None);
    };
    let Value::Object(members) = configuration else {
        return Err(unmapped_origin_member(ORIGIN_CONFIGURATION_PROPERTY));
    };
    if let Some(member) = unmapped_property(members, &[ORIGIN_RESTRICTIONS_PROPERTY]) {
        return Err(unmapped_origin_member(&format!(
            "{ORIGIN_CONFIGURATION_PROPERTY}.{member}"
        )));
    }
    let Some(declared) = members.get(ORIGIN_RESTRICTIONS_PROPERTY) else {
        return Ok(None);
    };
    let Value::Object(declared) = declared else {
        return Err(unmapped_origin_member(&format!(
            "{ORIGIN_CONFIGURATION_PROPERTY}.{ORIGIN_RESTRICTIONS_PROPERTY}"
        )));
    };
    let published: Vec<&str> = ORIGIN_RESTRICTIONS
        .iter()
        .map(|(property, _)| *property)
        .collect();
    if let Some(member) = unmapped_property(declared, &published) {
        return Err(unmapped_origin_member(&format!(
            "{ORIGIN_CONFIGURATION_PROPERTY}.{ORIGIN_RESTRICTIONS_PROPERTY}.{member}"
        )));
    }
    let mut restrictions = Map::new();
    for (property, restriction) in ORIGIN_RESTRICTIONS {
        let Some(stated) = declared.get(*property) else {
            continue;
        };
        // The mode is one value of that restriction's own enum, so a declaration stating a
        // document or a list for it states something this call cannot carry — refused rather
        // than skipped, which would leave the restriction on the `INHERIT` it starts in.
        let Some(mode) = scalar_text(stated) else {
            return Err(unmapped_origin_member(&format!(
                "{ORIGIN_CONFIGURATION_PROPERTY}.{ORIGIN_RESTRICTIONS_PROPERTY}.{property}"
            )));
        };
        restrictions.insert((*restriction).to_string(), Value::String(mode));
    }
    Ok((!restrictions.is_empty()).then_some(restrictions))
}

/// The refusal for a member of the origin configuration this module cannot carry onto
/// `UpdatePackageGroupOriginConfiguration`'s request, naming it by its path from the property
/// — the same sentence [`refuse_unmapped_properties`] states for a top-level one.
fn unmapped_origin_member(path: &str) -> Refusal {
    Refusal {
        code: NOT_IMPLEMENTED.to_string(),
        message: format!(
            "this emulator does not apply the {path} property of {CODEARTIFACT_PACKAGE_GROUP}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The type is one a template may DECLARE, and one this emulator provisions: a published
    /// AWS resource type whose owning service's operations this emulator serves is in the
    /// registry of recognised types AND has a provisioner, so `CreateStack` neither refuses it
    /// as a template-format error nor records it as declared-only.
    #[test]
    fn the_type_is_recognised_and_provisioned() {
        assert!(
            crate::state::resource_types::is_recognized(CODEARTIFACT_PACKAGE_GROUP),
            "expected {CODEARTIFACT_PACKAGE_GROUP} to be a type a template may declare"
        );
        assert_eq!(
            super::super::provisioner_for(CODEARTIFACT_PACKAGE_GROUP),
            Some(Provisioner::CodeArtifactPackageGroup),
            "expected the type to be provisioned in CodeArtifact"
        );
    }

    /// The group is identified by the ARN that service minted for it, and the three values its
    /// create FIXES are the ones a changed declaration replaces the resource over.
    #[test]
    fn the_group_is_identified_by_its_arn_and_created_under_its_pattern() {
        assert_eq!(
            crate::state::primary_identifier::properties(CODEARTIFACT_PACKAGE_GROUP),
            vec![ARN_ATTRIBUTE.to_string()],
            "expected the group to be identified by the ARN its create answers"
        );
        assert!(
            crate::state::template::identity_is_service_assigned(CODEARTIFACT_PACKAGE_GROUP),
            "expected the identity in force to be the one CodeArtifact answered"
        );
        assert_eq!(
            crate::state::template::replacement_properties(CODEARTIFACT_PACKAGE_GROUP),
            &[
                PATTERN_PROPERTY,
                CODEARTIFACT_DOMAIN_PROPERTY,
                CODEARTIFACT_DOMAIN_OWNER_PROPERTY
            ],
            "expected the values the create fixes to replace the resource"
        );
    }

    /// Each restriction the type publishes inside `OriginConfiguration.Restrictions` reaches
    /// `UpdatePackageGroupOriginConfiguration` under its own
    /// `PackageGroupOriginRestrictionType`, a declaration stating none asks for no such call at
    /// all, and a member of either document that no arm maps is REFUSED rather than dropped.
    ///
    /// The rows span the class: every published restriction on its own, all of them together,
    /// an unmapped member of each of the two documents, and the two documents that state none.
    #[test]
    fn each_published_restriction_reaches_the_member_the_model_keys_it_by() {
        /// What the rule says one declaration's origin configuration becomes: the map the call
        /// takes, no call at all, or a refusal naming the member nothing maps.
        enum Expected {
            Map(Value),
            NoCall,
            Refused(&'static str),
        }
        let cases: &[(&str, Value, Expected)] = &[
            (
                "the publish restriction alone",
                serde_json::json!({
                    "OriginConfiguration": { "Restrictions": { "Publish": "BLOCK" } }
                }),
                Expected::Map(serde_json::json!({ "PUBLISH": "BLOCK" })),
            ),
            (
                "the external-upstream restriction alone",
                serde_json::json!({
                    "OriginConfiguration": { "Restrictions": { "ExternalUpstream": "ALLOW" } }
                }),
                Expected::Map(serde_json::json!({ "EXTERNAL_UPSTREAM": "ALLOW" })),
            ),
            (
                "the internal-upstream restriction alone",
                serde_json::json!({
                    "OriginConfiguration": {
                        "Restrictions": { "InternalUpstream": "ALLOW_SPECIFIC_REPOSITORIES" }
                    }
                }),
                Expected::Map(serde_json::json!({
                    "INTERNAL_UPSTREAM": "ALLOW_SPECIFIC_REPOSITORIES"
                })),
            ),
            (
                "all three at once",
                serde_json::json!({
                    "OriginConfiguration": {
                        "Restrictions": {
                            "Publish": "INHERIT",
                            "ExternalUpstream": "BLOCK",
                            "InternalUpstream": "ALLOW",
                        }
                    }
                }),
                Expected::Map(serde_json::json!({
                    "PUBLISH": "INHERIT",
                    "EXTERNAL_UPSTREAM": "BLOCK",
                    "INTERNAL_UPSTREAM": "ALLOW",
                })),
            ),
            (
                "a restriction the type does not publish",
                serde_json::json!({
                    "OriginConfiguration": { "Restrictions": { "Download": "BLOCK" } }
                }),
                Expected::Refused("OriginConfiguration.Restrictions.Download"),
            ),
            (
                "a member of the configuration the type does not publish",
                serde_json::json!({
                    "OriginConfiguration": { "Allowed": ["a-repository"] }
                }),
                Expected::Refused("OriginConfiguration.Allowed"),
            ),
            (
                "an origin configuration that is no document at all",
                serde_json::json!({ "OriginConfiguration": "BLOCK" }),
                Expected::Refused("OriginConfiguration"),
            ),
            (
                "restrictions that are no document at all",
                serde_json::json!({
                    "OriginConfiguration": { "Restrictions": ["Publish"] }
                }),
                Expected::Refused("OriginConfiguration.Restrictions"),
            ),
            (
                "a published restriction whose mode is no single value",
                serde_json::json!({
                    "OriginConfiguration": { "Restrictions": { "Publish": ["BLOCK"] } }
                }),
                Expected::Refused("OriginConfiguration.Restrictions.Publish"),
            ),
            (
                "a configuration that states no restrictions",
                serde_json::json!({ "OriginConfiguration": {} }),
                Expected::NoCall,
            ),
            (
                "a declaration that states no origin configuration",
                serde_json::json!({ "DomainName": "a-domain", "Pattern": "/npm/*" }),
                Expected::NoCall,
            ),
        ];
        for (case, properties, expected) in cases {
            let answered = declared_origin_restrictions(properties);
            match expected {
                Expected::Map(wanted) => assert_eq!(
                    answered.as_ref().ok().and_then(|held| held.clone()).map(Value::Object).as_ref(),
                    Some(wanted),
                    "case {case:?}: expected {wanted:?}, got {answered:?}"
                ),
                Expected::NoCall => assert!(
                    matches!(&answered, Ok(None)),
                    "case {case:?}: expected no origin call at all, got {answered:?}"
                ),
                Expected::Refused(member) => {
                    let refusal = answered.as_ref().err().unwrap_or_else(|| {
                        panic!("case {case:?}: expected the member to be refused, got {answered:?}")
                    });
                    // The sentence names the member by its path from the property, which is
                    // what tells a client which part of the declaration was not applied.
                    let named = format!(
                        "the {member} property of AWS::CodeArtifact::PackageGroup"
                    );
                    assert!(
                        refusal.message.ends_with(&named),
                        "case {case:?}: expected the refusal to name {named:?}, got {refusal}"
                    );
                }
            }
        }
    }
}
