// SPDX-License-Identifier: CC0-1.0

//! `AWS::IoT::Authorizer`: the custom authorizer a stack declares, created in AWS IoT
//! itself.
//!
//! This is the per-type half of provisioning for one resource type, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::logs_resources`] for the reason that module records —
//! the catalogue file is far past the length a change to this repository may touch — and is
//! reached through the same seam ([`super::Owner`]).
//!
//! ## Why the authorizer is really created
//!
//! An authorizer's NAME is its identity in the owning service: `DescribeAuthorizer`,
//! `UpdateAuthorizer`, `SetDefaultAuthorizer`, `TestInvokeAuthorizer` and `DeleteAuthorizer`
//! each address one by that name, and `CreateAuthorizer` refuses a name the account already
//! holds with `ResourceAlreadyExistsException`. So a stack that reported an authorizer
//! created while nothing created it publishes a name that resolves to nothing — no device
//! connection could be authorized through it — and a SECOND stack declaring the same name
//! would be accepted, where AWS IoT refuses it. Creating the authorizer where it belongs is
//! what makes both answers the owning service's own.
//!
//! ## The properties
//!
//! `AWS::IoT::Authorizer`'s properties are `CreateAuthorizer`'s own members with an
//! upper-case initial — `AuthorizerFunctionArn`, `TokenKeyName`, `TokenSigningPublicKeys`,
//! `Status`, `SigningDisabled`, `EnableCachingForHttp` and `Tags` — beside the
//! `AuthorizerName` the authorizer is created UNDER, which is this resource's physical id
//! and what its `Ref` resolves to. A declaration that names none is created under the
//! identifier CloudFormation computed for the resource.
//!
//! A declared property this arm does not apply is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]): an authorizer reported created with less than its
//! declaration asked for is the shallow success this arm exists to avoid.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{applicable_tags, call_rest_json, percent_encoded, Created, Readiness, Refusal, Removal};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource type this module provisions.
pub(crate) const IOT_AUTHORIZER: &str = "AWS::IoT::Authorizer";

/// The SigV4 signing name of the service that owns it, which is what routes the in-process
/// call to AWS IoT's own handlers (that model's `sigv4Name`/`endpointPrefix`,
/// `smithy_query(query="service_info")`). It speaks `restJson1`, so each call is a method and
/// a URI rather than an `X-Amz-Target`.
const IOT_SIGNING_NAME: &str = "iot";

/// The error code AWS IoT answers for an authorizer it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of its reads, updates and deletes), which
/// is what says a delete has nothing left to remove and a read has nothing to report.
const AUTHORIZER_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// The status an authorizer must be in before AWS IoT will delete it, which is one of the
/// two values its `AuthorizerStatus` enum admits
/// (`smithy_query(query="shape",shape="AuthorizerStatus")`): the owning service refuses the
/// delete of an authorizer that is still in service, so a teardown moves it here first.
const INACTIVE_STATUS: &str = "INACTIVE";

/// The member of `UpdateAuthorizer` that carries the status, which is how an authorizer is
/// taken out of service.
const STATUS_MEMBER: &str = "status";

/// The property naming the authorizer, which is the name it is created under rather than a
/// member of the create's body.
const AUTHORIZER_NAME: &str = "AuthorizerName";

/// The attribute `AWS::IoT::Authorizer` publishes through `Fn::GetAtt` ("Arn: the authorizer
/// ARN"), read off the member the create and the read answer it under.
const ARN_ATTRIBUTE: &str = "Arn";
const AUTHORIZER_ARN_MEMBER: &str = "authorizerArn";

/// Every property the type publishes, paired with the member of `CreateAuthorizer` that
/// carries it — which is the property with a lower-case initial.
///
/// Source: `CreateAuthorizer`'s member list
/// (`smithy_query(query="operation",operation="CreateAuthorizer")`), intersected with the
/// properties `AWS::IoT::Authorizer` publishes. `AuthorizerFunctionArn` is required by both.
const CREATE_MEMBERS: &[(&str, &str)] = &[
    ("AuthorizerFunctionArn", "authorizerFunctionArn"),
    ("EnableCachingForHttp", "enableCachingForHttp"),
    ("SigningDisabled", "signingDisabled"),
    ("Status", "status"),
    ("TokenKeyName", "tokenKeyName"),
    ("TokenSigningPublicKeys", "tokenSigningPublicKeys"),
];

/// The members `UpdateAuthorizer` takes, which are the properties a declaration can change on
/// the authorizer that already exists.
///
/// Source: `UpdateAuthorizer`'s member list. `signingDisabled` is NOT one of them — whether
/// the token signature is validated is fixed when the authorizer is created — so a
/// declaration that changes it has no in-place path ([`update`]).
const UPDATE_MEMBERS: &[(&str, &str)] = &[
    ("AuthorizerFunctionArn", "authorizerFunctionArn"),
    ("EnableCachingForHttp", "enableCachingForHttp"),
    ("Status", "status"),
    ("TokenKeyName", "tokenKeyName"),
    ("TokenSigningPublicKeys", "tokenSigningPublicKeys"),
];

/// The two members AWS IoT's own `Tag` structure spells a tag's key and value with
/// (`smithy_query(query="shape",shape="Tag")`).
const TAG_MEMBERS: (&str, &str) = ("Key", "Value");

/// The members the API models as BOOLEANS, which a template may write as a quoted scalar or
/// feed through a `Ref` to a `String` parameter — every value of one arrives as text.
const AUTHORIZER_FLAGS: &[&str] = &["EnableCachingForHttp", "SigningDisabled"];

/// Every property `AWS::IoT::Authorizer` publishes. A declared property this list does not
/// carry is one no call here applies, and is refused rather than dropped.
const AUTHORIZER_PROPERTIES: &[&str] = &[
    "AuthorizerFunctionArn",
    "AuthorizerName",
    "EnableCachingForHttp",
    "SigningDisabled",
    "Status",
    "Tags",
    "TokenKeyName",
    "TokenSigningPublicKeys",
];

/// Create the authorizer an `AWS::IoT::Authorizer` declares, in AWS IoT itself — so a client
/// that deployed the stack can pass `!Ref <authorizer>` straight to `DescribeAuthorizer` or
/// `TestInvokeAuthorizer`, and a second stack declaring the same name is refused by the
/// service that owns the name.
///
/// `Err` is the owning service's own refusal of the create, which is what the resource fails
/// with: a name the account already holds, a function ARN it will not take, an authorizer
/// count past the account's limit.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        &resource.resource_type,
        AUTHORIZER_PROPERTIES,
    )?;
    let name = resource.physical_resource_id.clone();
    let mut request = declared_members(&resource.properties, CREATE_MEMBERS);
    // `authorizerFunctionArn` is required by the call and by the type: a declaration that
    // names no function describes no authorizer that could be created, so it is refused here
    // rather than sent as a request the owning service would reject.
    if !request.contains_key(CREATE_MEMBERS[0].1) {
        return Err(Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "{} declares no {}",
                resource.resource_type, CREATE_MEMBERS[0].0
            ),
        });
    }
    // The authorizer carries the tags its declaration asks for plus the ones CloudFormation
    // gives the resource, as `CreateAuthorizer`'s own `tags` list — less the keys in AWS's
    // reserved namespace, which this service documents as its own ("Do not use the `aws:`
    // prefix ... it is reserved for AWS use") and which no public call of it writes.
    let tags = applicable_tags(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert(
            "tags".to_string(),
            Value::Array(
                tags.into_iter()
                    .map(|(key, value)| {
                        let mut tag = Map::new();
                        tag.insert(TAG_MEMBERS.0.to_string(), Value::String(key));
                        tag.insert(TAG_MEMBERS.1.to_string(), Value::String(value));
                        Value::Object(tag)
                    })
                    .collect(),
            ),
        );
    }
    let answered = call_iot(
        ctx,
        region,
        "POST",
        &authorizer_uri(&name),
        Some(&Value::Object(request)),
    )
    .await?;
    // The authorizer was created under the name CloudFormation decided, so the computed
    // physical id is the one it answers to; what the create adds is the ARN that service
    // minted, which is the type's one published attribute.
    Ok(Created::attributes(reported_attributes(&answered)))
}

/// Whether the owning service reports the created authorizer ready.
///
/// Nothing is asked, and nothing waits: an authorizer EXISTS from the moment
/// `CreateAuthorizer` answers — `DescribeAuthorizer` resolves it and a connection can be
/// authorized through it — and the status it is in is the one the declaration asked for
/// rather than a transient state it settles out of.
pub(super) fn ready() -> Readiness {
    Readiness::Ready
}

/// Whether the owning service has finished removing a deleted authorizer.
///
/// Nothing is asked: `DeleteAuthorizer` takes the authorizer away as it answers, and AWS IoT
/// reports no state a deleted one is still being removed in.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Apply a CHANGED declaration to the authorizer the stack already has, through
/// `UpdateAuthorizer` — the call that takes the authorizer's name, so it keeps its identity
/// and nothing is replaced.
///
/// `Ok(None)` is a change that call cannot make: `signingDisabled` is not one of its members,
/// so a deployment that changes whether the token signature is validated reports the change as
/// not applied rather than as a success that dropped the caller's change.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    // The DECLARED document, not the record's: by the time this is reached the resource
    // carries the properties it was CREATED with, so a property the new declaration
    // introduces is only in `declared`.
    refuse_unapplied_properties(declared, &resource.resource_type, AUTHORIZER_PROPERTIES)?;
    if declared.get("SigningDisabled") != resource.properties.get("SigningDisabled") {
        return Ok(None);
    }
    let request = declared_members(declared, UPDATE_MEMBERS);
    let answered = call_iot(
        ctx,
        region,
        "PUT",
        &authorizer_uri(&resource.physical_resource_id),
        Some(&Value::Object(request)),
    )
    .await?;
    Ok(Some(Created::attributes(reported_attributes(&answered))))
}

/// Delete the authorizer a stack created, addressed by the name it was created under.
///
/// The owning service refuses the delete of an authorizer that is still in service, so this
/// walks the path that service documents for a teardown: `UpdateAuthorizer` takes it out of
/// service, then `DeleteAuthorizer` removes it — which is why a stack whose authorizer was
/// declared (or left) `ACTIVE` still releases it.
///
/// An authorizer the owning service no longer holds is already in the state the delete asks
/// for, judged on the CODE and never on a message.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let mut deactivate = Map::new();
    deactivate.insert(
        STATUS_MEMBER.to_string(),
        Value::String(INACTIVE_STATUS.to_string()),
    );
    match call_iot(
        ctx,
        region,
        "PUT",
        &authorizer_uri(&resource.physical_resource_id),
        Some(&Value::Object(deactivate)),
    )
    .await
    {
        Ok(_) => {}
        Err(refusal) if refusal.code == AUTHORIZER_NOT_FOUND => return Ok(()),
        Err(refusal) => return Err(refusal),
    }
    match call_iot(
        ctx,
        region,
        "DELETE",
        &authorizer_uri(&resource.physical_resource_id),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == AUTHORIZER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The authorizer's configuration as the owning service reports it, for drift detection.
///
/// `DescribeAuthorizer` answers an `authorizerDescription` whose members are the very ones
/// the type declares its properties with, so the answer maps onto the property vocabulary one
/// for one. An authorizer the owning service no longer holds was deleted
/// ([`AUTHORIZER_NOT_FOUND`]).
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_iot(
        ctx,
        region,
        "GET",
        &authorizer_uri(&resource.physical_resource_id),
        None,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == AUTHORIZER_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(described) = answered.get("authorizerDescription") else {
        return Ok(None);
    };
    let mut configuration = Map::new();
    for (property, member) in CREATE_MEMBERS
        .iter()
        .chain([&(AUTHORIZER_NAME, "authorizerName")])
    {
        if let Some(value) = described.get(*member) {
            configuration.insert((*property).to_string(), value.clone());
        }
    }
    Ok(Some(Value::Object(configuration)))
}

/// The URI the three per-authorizer operations are bound to
/// (`/authorizer/{authorizerName}`, that model's own `http` binding), with the name written
/// as ONE path segment so a name outside the modelled pattern is refused by that operation's
/// validation instead of addressing a different one.
fn authorizer_uri(name: &str) -> String {
    format!("/authorizer/{}", percent_encoded(name))
}

/// The attributes an answer reports for the authorizer: the ARN that service minted, which is
/// the type's one published `Fn::GetAtt` attribute.
fn reported_attributes(answered: &Value) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    if let Some(arn) = answered
        .get(AUTHORIZER_ARN_MEMBER)
        .and_then(Value::as_str)
        .filter(|arn| !arn.is_empty())
    {
        attributes.insert(ARN_ATTRIBUTE.to_string(), arn.to_string());
    }
    attributes
}

/// The members of `members` a declaration states, as the request carries them: the property
/// and the member are the same name up to the initial's case, so every value goes through as
/// the template wrote it — except a member the API models as a BOOLEAN, which is read through
/// [`declared_flag`], this module's one spelling rule for a property carrying a flag.
fn declared_members(properties: &Value, members: &[(&str, &str)]) -> Map<String, Value> {
    let mut request = Map::new();
    for (property, member) in members {
        let Some(value) = properties.get(*property).filter(|value| !value.is_null()) else {
            continue;
        };
        let carried = if AUTHORIZER_FLAGS.contains(property) {
            declared_flag(value).map_or_else(|| value.clone(), Value::Bool)
        } else {
            value.clone()
        };
        request.insert((*member).to_string(), carried);
    }
    request
}

/// A declared boolean in either spelling a template gives one: JSON's boolean, or the text a
/// template spells one as, read case-insensitively. `None` is a value that is no boolean at
/// all, which the owning service's own validation is what judges, so it is left exactly as
/// written.
fn declared_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// The refusal for a declared property this module does not apply, naming which property it
/// was: refusing is what keeps an authorizer from being reported as created — or as updated —
/// with less than its declaration asked for.
fn refuse_unapplied_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

/// Call one AWS IoT operation on the REST-JSON wire its model declares.
async fn call_iot(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&Value>,
) -> Result<Value, Refusal> {
    call_rest_json(ctx, IOT_SIGNING_NAME, region, method, uri, body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The declared properties reach `CreateAuthorizer` under that call's own member names,
    /// with a flag written as text carried as the boolean the API models — and the name the
    /// resource is created under is not one of them, being the URI's own path segment.
    #[test]
    fn the_declared_properties_are_carried_under_the_api_member_names() {
        let declared = serde_json::json!({
            "AuthorizerName": "an-authorizer",
            "AuthorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:authorize",
            "TokenKeyName": "token",
            "TokenSigningPublicKeys": {"KEY1": "-----BEGIN PUBLIC KEY-----"},
            "Status": "ACTIVE",
            "SigningDisabled": "false",
            "EnableCachingForHttp": true
        });
        let carried = Value::Object(declared_members(&declared, CREATE_MEMBERS));
        let expected = serde_json::json!({
            "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:authorize",
            "tokenKeyName": "token",
            "tokenSigningPublicKeys": {"KEY1": "-----BEGIN PUBLIC KEY-----"},
            "status": "ACTIVE",
            "signingDisabled": false,
            "enableCachingForHttp": true
        });
        assert_eq!(
            carried, expected,
            "case \"a full declaration\": expected {expected}, got {carried}"
        );
    }

    /// `signingDisabled` is not a member of the update: an authorizer's signature validation
    /// is fixed at creation, so the update request carries only what that call takes.
    #[test]
    fn the_update_carries_only_what_update_authorizer_takes() {
        let declared = serde_json::json!({
            "AuthorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:other",
            "SigningDisabled": true,
            "Status": "INACTIVE"
        });
        let carried = Value::Object(declared_members(&declared, UPDATE_MEMBERS));
        let expected = serde_json::json!({
            "authorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:other",
            "status": "INACTIVE"
        });
        assert_eq!(
            carried, expected,
            "case \"a changed declaration\": expected {expected}, got {carried}"
        );
    }

    /// The URI is the model's own binding with the name as ONE path segment, so a name
    /// carrying a character that is structure in a URI cannot address another operation.
    #[test]
    fn the_uri_carries_the_name_as_one_segment() {
        let cases: &[(&str, &str, &str)] = &[
            ("an ordinary name", "an-authorizer", "/authorizer/an-authorizer"),
            ("a name with an at sign", "a@b", "/authorizer/a%40b"),
            ("a name with a slash", "a/b", "/authorizer/a%2Fb"),
        ];
        for (case, name, expected) in cases {
            let actual = authorizer_uri(name);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the URI {expected:?}, got {actual:?}"
            );
        }
    }

    /// The ARN the owning service minted is reported as the type's published attribute, and
    /// an answer carrying none reports no attribute rather than an empty one.
    #[test]
    fn the_minted_arn_is_the_published_attribute() {
        let answered = serde_json::json!({
            "authorizerName": "an-authorizer",
            "authorizerArn": "arn:aws:iot:us-east-1:000000000000:authorizer/an-authorizer"
        });
        let attributes = reported_attributes(&answered);
        assert_eq!(
            attributes.get(ARN_ATTRIBUTE).map(String::as_str),
            Some("arn:aws:iot:us-east-1:000000000000:authorizer/an-authorizer"),
            "case \"an answered ARN\": expected it to be the Arn attribute, got {attributes:?}"
        );
        let silent = serde_json::json!({"authorizerName": "an-authorizer"});
        let attributes = reported_attributes(&silent);
        assert!(
            attributes.is_empty(),
            "case \"an answer carrying no ARN\": expected no attribute, got {attributes:?}"
        );
    }

    /// A property no call here applies is refused, naming it; every property the type
    /// publishes is accepted.
    #[test]
    fn an_unapplied_property_is_refused() {
        let declared = serde_json::json!({
            "AuthorizerName": "an-authorizer",
            "AuthorizerFunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:authorize",
            "TokenKeyName": "token",
            "TokenSigningPublicKeys": {},
            "Status": "ACTIVE",
            "SigningDisabled": false,
            "EnableCachingForHttp": false,
            "Tags": []
        });
        assert!(
            refuse_unapplied_properties(&declared, IOT_AUTHORIZER, AUTHORIZER_PROPERTIES).is_ok(),
            "case \"every published property\": expected it to be applied, got a refusal"
        );
        let unknown = serde_json::json!({"AuthorizerName": "an-authorizer", "Unheard": 1});
        let refusal = refuse_unapplied_properties(&unknown, IOT_AUTHORIZER, AUTHORIZER_PROPERTIES)
            .expect_err("case \"a property no call carries\": expected a refusal");
        assert!(
            refusal.message.contains("Unheard"),
            "case \"a property no call carries\": expected the refusal to name it, got {:?}",
            refusal.message
        );
    }

    /// The tag list carries the declaration's own tags and the ones CloudFormation
    /// propagates, under the member names AWS IoT's `Tag` spells — and never a key in the
    /// reserved namespace, which no public call of that service writes.
    #[test]
    fn the_tag_list_leaves_the_reserved_namespace_alone() {
        let declared = serde_json::json!({"Tags": [{"Key": "Owner", "Value": "fleet"}]});
        let propagated = vec![
            ("Stack".to_string(), "iot-suite".to_string()),
            (
                "aws:cloudformation:logical-id".to_string(),
                "Authorizer".to_string(),
            ),
        ];
        let tags = applicable_tags(&tags_of(&declared, &propagated));
        let keys: Vec<&str> = tags.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["Owner", "Stack"],
            "case \"a declaration beside the propagated tags\": expected the declared and \
             stack keys alone, got {keys:?}"
        );
    }
}
