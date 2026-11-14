// SPDX-License-Identifier: CC0-1.0

//! The Amazon Cognito resource types a template may declare, end to end over the awsQuery
//! wire.
//!
//! RULE under test: **an operation that submits a template accepts every resource type the
//! "AWS resource and property types reference" publishes** — the emulator's catalogue in
//! `state::resource_types` is a transcription of that published set, so a type the
//! reference publishes is recognised, the stack reaches `CREATE_COMPLETE` and the resource
//! is recorded under the type the template declared — **while a name the reference does
//! not publish is still refused** with HTTP 400, `ValidationError` and `Template format
//! error: Unrecognized resource types: [<types>]`.
//!
//! `unrecognized_resource_types.rs` is the suite of the REFUSING half of that rule, over
//! namespaces and spellings. This suite is the ADMITTING half over one whole published
//! namespace: Amazon Cognito, the namespace of the reported case
//! (`AWS::Cognito::IdentityPoolPrincipalTag`, which the catalogue did not carry and which
//! was therefore refused before any principal-tag scenario could deploy).
//!
//! The rows differ along what the rule generalises over — the resource NAME inside one
//! published namespace. Every type the reference publishes under `AWS::Cognito::` is a row,
//! which is what makes the reported type one row among its fourteen siblings rather than a
//! special case, and each row is crossed with the two things a published type owes a
//! caller: the submission is accepted, and the declared resource is readable back through
//! `DescribeStackResources` under its declared type. The refusing rows are the near misses
//! of the same namespace — a plural, a wrong case, an invented resource and the service
//! model's own namespace spelling — so admitting the published set is not admitting
//! `AWS::Cognito::<anything>`.
//!
//! Most of these types have no provisioner, so each is declared-only
//! (`unprovisioned_resources.rs` pins that: nothing is called in the owning service and the
//! stack completes). The ones that DO are created through the owning service's own API, which
//! this crate's harness mounts beside CloudFormation. Six of them cannot be created from a
//! declaration that states none of their properties, because the pool the resource belongs to
//! is one of them — `AWS::Cognito::UserPoolGroup`, `AWS::Cognito::UserPoolClient`,
//! `AWS::Cognito::UserPoolIdentityProvider` and `AWS::Cognito::UserPoolResourceServer`, whose
//! rows' stacks FAIL naming the property the type requires, and the two types that configure
//! an identity pool rather than being one — while `AWS::Cognito::IdentityPool` and `AWS::Cognito::UserPool` need no
//! property of the declaration and have their identity ASSIGNED by that service, so their rows
//! carry the `REGION:GUID` identity pool id and the `<region>_<id>` user pool id rather than a
//! name CloudFormation computed. What this suite asserts either way is
//! only what the rule is about — that a template may DECLARE the type at all, and that the
//! declared resource is recorded under the type it declared.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::identity_is_service_assigned;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::REGION;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_text, xml_texts,
};
use crate::stack_resources::{resources_of, wire_resources};

/// The namespace Amazon Cognito's resource types are published under. The service's own
/// models are named `cognito-identity` and `cognito-idp`, but a template spells the
/// published namespace, which is this one.
const COGNITO_NAMESPACE: &str = "AWS::Cognito::";

/// Every resource type the "AWS resource and property types reference" publishes under
/// `AWS::Cognito::`, with the operation of the owning service — present in this checkout's
/// `cognito-identity` and `cognito-idp` models — that deploying one calls.
///
/// Source: that reference's Amazon Cognito section. This is the expectation side of the
/// catalogue: the test below holds `AWS_RESOURCE_TYPES`' own `AWS::Cognito::` group to
/// exactly this set, so a missing entry (the reported defect) and a stray one both fail.
const PUBLISHED: &[&str] = &[
    // CreateIdentityPool (cognito-identity)
    "AWS::Cognito::IdentityPool",
    // SetPrincipalTagAttributeMap (cognito-identity) — the reported type.
    "AWS::Cognito::IdentityPoolPrincipalTag",
    // SetIdentityPoolRoles (cognito-identity)
    "AWS::Cognito::IdentityPoolRoleAttachment",
    // SetLogDeliveryConfiguration (cognito-idp)
    "AWS::Cognito::LogDeliveryConfiguration",
    // CreateManagedLoginBranding (cognito-idp)
    "AWS::Cognito::ManagedLoginBranding",
    // CreateUserPool (cognito-idp)
    "AWS::Cognito::UserPool",
    // CreateUserPoolClient (cognito-idp)
    "AWS::Cognito::UserPoolClient",
    // CreateUserPoolDomain (cognito-idp)
    "AWS::Cognito::UserPoolDomain",
    // CreateGroup (cognito-idp)
    "AWS::Cognito::UserPoolGroup",
    // CreateIdentityProvider (cognito-idp)
    "AWS::Cognito::UserPoolIdentityProvider",
    // CreateResourceServer (cognito-idp)
    "AWS::Cognito::UserPoolResourceServer",
    // SetRiskConfiguration (cognito-idp)
    "AWS::Cognito::UserPoolRiskConfigurationAttachment",
    // SetUICustomization (cognito-idp)
    "AWS::Cognito::UserPoolUICustomizationAttachment",
    // AdminCreateUser (cognito-idp)
    "AWS::Cognito::UserPoolUser",
    // AdminAddUserToGroup (cognito-idp)
    "AWS::Cognito::UserPoolUserToGroupAttachment",
];

/// The published types this emulator PROVISIONS in their owning service, each with the
/// property its create cannot be made without — the reason a property-free declaration of
/// one fails rather than completing.
///
/// A group is identified by the pair `(UserPoolId, GroupName)`, so a declaration naming no
/// pool describes no group `CreateGroup` could make: the provisioner refuses it, naming
/// that property, rather than sending a request the owning service would reject. An app
/// client, an identity provider and a resource server are in the same position — each belongs
/// to ONE user pool, which `CreateUserPoolClient`, `CreateIdentityProvider` and
/// `CreateResourceServer` all require — and a role attachment too: it configures ONE identity
/// pool, which its `IdentityPoolId` names. A principal-tag attribute map is the same case
/// twice over: it is the mapping of ONE provider of ONE pool, and
/// `SetPrincipalTagAttributeMap` requires both.
const REQUIRES_A_PROPERTY: &[(&str, &str)] = &[
    ("AWS::Cognito::UserPoolGroup", "UserPoolId"),
    ("AWS::Cognito::UserPoolClient", "UserPoolId"),
    ("AWS::Cognito::UserPoolIdentityProvider", "UserPoolId"),
    ("AWS::Cognito::UserPoolResourceServer", "UserPoolId"),
    (
        "AWS::Cognito::IdentityPoolRoleAttachment",
        "IdentityPoolId",
    ),
    (
        "AWS::Cognito::IdentityPoolPrincipalTag",
        "IdentityPoolId",
    ),
];

/// The character the owning service's own identifier form separates its parts with, per type
/// whose identity that service ASSIGNS: `IdentityPoolId` is `^[\w-]+:[0-9a-f-]+$` (the
/// `REGION:GUID` form) and `UserPoolIdType` is `^[\w-]+_[0-9a-zA-Z]+$` (the `<region>_<id>`
/// form), both from those services' own Smithy models. A row reads it to check the
/// identifier the stack reports is one of that form rather than a name CloudFormation could
/// have computed.
const ASSIGNED_IDENTIFIER_SEPARATOR: &[(&str, char)] = &[
    ("AWS::Cognito::IdentityPool", ':'),
    ("AWS::Cognito::UserPool", '_'),
];

/// The property a property-free declaration of `resource_type` cannot be created without,
/// or `None` for a type this emulator leaves declared-only.
fn required_property_of(resource_type: &str) -> Option<&'static str> {
    REQUIRES_A_PROPERTY
        .iter()
        .find(|(declared, _)| *declared == resource_type)
        .map(|(_, property)| *property)
}

/// The stack name a one-resource stack for `resource_type` is created under: the type's
/// last segment, lower-cased, which is inside a stack name's own alphabet.
fn stack_name_for(resource_type: &str) -> String {
    let resource = resource_type
        .rsplit("::")
        .next()
        .unwrap_or(resource_type)
        .to_ascii_lowercase();
    format!("cognito-{resource}")
}

/// A one-resource template declaring `resource_type` under the logical id `Subject`.
///
/// No properties: a published type may be declared with whatever properties its own
/// reference gives it, and what this suite is about is the TYPE. The reported template's
/// own properties are the separate case below.
fn one_resource_template(resource_type: &str) -> String {
    json!({ "Resources": { "Subject": { "Type": resource_type } } }).to_string()
}

/// The catalogue's own `AWS::Cognito::` group, in catalogue order.
fn catalogued_cognito_types() -> Vec<&'static str> {
    AWS_RESOURCE_TYPES
        .iter()
        .copied()
        .filter(|entry| entry.starts_with(COGNITO_NAMESPACE))
        .collect()
}

/// The catalogue carries exactly the published Amazon Cognito namespace — every type the
/// reference publishes, and nothing else under it.
#[test]
fn the_catalogue_carries_the_published_cognito_namespace() {
    let catalogued = catalogued_cognito_types();
    let mut expected: Vec<&str> = PUBLISHED.to_vec();
    expected.sort_unstable();
    let mut actual = catalogued.clone();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "case {COGNITO_NAMESPACE:?}: expected the catalogue to carry exactly the published \
         Amazon Cognito types {expected:?}, got {actual:?}"
    );
    for resource_type in PUBLISHED {
        assert!(
            is_recognized(resource_type),
            "case {resource_type:?}: expected a published type to be recognized, it is not"
        );
    }
}

/// `CreateStack` accepts a template declaring any published Amazon Cognito type, the stack
/// completes, and the resource is readable back under the type that was declared.
#[tokio::test]
async fn every_published_cognito_type_may_be_declared_and_is_recorded() {
    let router = router();
    // Submit every row first, then wait once for the transition they all owe: the delay is
    // per stack and runs from its own create.
    for resource_type in PUBLISHED {
        let stack_name = stack_name_for(resource_type);
        let template = one_resource_template(resource_type);
        let (status_code, body) = create_stack(&router, &stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected HTTP 200 from CreateStack for a published \
             resource type, got {status_code} with body {body}"
        );
    }
    wait_for_the_transition().await;

    for resource_type in PUBLISHED {
        let stack_name = stack_name_for(resource_type);
        let (status_code, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected DescribeStacks to report the created stack, \
             got {status_code} with body {body}"
        );
        match required_property_of(resource_type) {
            // A type this emulator really creates, declared with none of its properties:
            // the owning service's create cannot be made, so the stack fails naming the
            // property the type requires rather than reporting a resource nothing created.
            Some(property) => {
                assert_eq!(
                    xml_text(&body, "StackStatus").as_deref(),
                    Some(status::ROLLBACK_COMPLETE),
                    "case {resource_type:?}: expected the failed create to be rolled back \
                     to {:?}, got body {body}",
                    status::ROLLBACK_COMPLETE
                );
                // The property the type requires is the FAILING RESOURCE's diagnosis, so
                // it is reported on that resource's own event; the stack's reason is the
                // summary of which resources failed.
                let reason = resource_event_reason(
                    &router,
                    &stack_name,
                    "Subject",
                    resource_status::CREATE_FAILED,
                )
                .await;
                assert!(
                    reason.contains(property),
                    "case {resource_type:?}: expected the failure to name the required \
                     property {property:?}, got {reason:?}"
                );
                // Nothing was created for it, and a stack whose deployment did not
                // complete installs no resource records, so there is no resource to read
                // back here — the admitting half of the rule (the submission was accepted
                // and the deployment named this very type) is what the assertions above
                // carry for such a row.
                continue;
            }
            None => assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {resource_type:?}: expected the stack to reach {:?}, got body {body}",
                status::CREATE_COMPLETE
            ),
        }

        let (status_code, body) = resources_of(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {resource_type:?}: expected DescribeStackResources to answer, got \
             {status_code} with body {body}"
        );
        let reported = wire_resources(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {resource_type:?}: expected the one declared resource, got {reported:?}"
        );
        assert_eq!(
            reported[0].resource_type.as_deref(),
            Some(*resource_type),
            "case {resource_type:?}: expected the declared type to be reported, got {:?}",
            reported[0].resource_type
        );
        assert_eq!(
            reported[0].logical_id.as_deref(),
            Some("Subject"),
            "case {resource_type:?}: expected the declared logical id, got {:?}",
            reported[0].logical_id
        );
        let physical_id = reported[0].physical_id.clone().unwrap_or_default();
        // The physical id a declared-only resource carries is the one CloudFormation's
        // naming rules computed. A type this emulator really PROVISIONS and whose identity
        // its owning service ASSIGNS carries that service's own identifier instead — for an
        // identity pool the `REGION:GUID` id `CreateIdentityPool` minted and for a user pool
        // the `<region>_<id>` `CreateUserPool` minted, which is what `Ref` returns for each
        // type ([`ASSIGNED_IDENTIFIER_SEPARATOR`] carries the form).
        if identity_is_service_assigned(resource_type) {
            let separator = ASSIGNED_IDENTIFIER_SEPARATOR
                .iter()
                .find(|(declared, _)| declared == resource_type)
                .map(|(_, separator)| *separator)
                .unwrap_or_else(|| {
                    panic!(
                        "case {resource_type:?}: this suite must state the identifier form \
                         the owning service assigns"
                    )
                });
            assert!(
                !physical_id.contains(&stack_name)
                    && !physical_id.contains("Subject")
                    && physical_id.contains(separator),
                "case {resource_type:?}: expected the identifier the owning service assigned, \
                 got {physical_id:?}"
            );
            continue;
        }
        assert!(
            physical_id.contains(&stack_name) && physical_id.contains("Subject"),
            "case {resource_type:?}: expected a generated physical id built from the stack \
             name and the logical id, got {physical_id:?}"
        );
    }
}

/// One refusing row: a name of the same namespace the reference does not publish.
struct NearMiss {
    case: &'static str,
    stack_name: &'static str,
    declared: &'static [&'static str],
}

/// The near misses of the admitted namespace. Admitting the published set must not admit
/// `AWS::Cognito::<anything>`.
const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "the reported type, pluralised",
        stack_name: "cognito-plural",
        declared: &["AWS::Cognito::IdentityPoolPrincipalTags"],
    },
    NearMiss {
        case: "the reported type in the wrong case",
        stack_name: "cognito-wrong-case",
        declared: &["AWS::Cognito::identitypoolprincipaltag"],
    },
    NearMiss {
        case: "a resource the Amazon Cognito namespace does not publish",
        stack_name: "cognito-invented",
        declared: &["AWS::Cognito::UserPoolPrincipalTag"],
    },
    NearMiss {
        case: "the service model's own name as the namespace",
        stack_name: "cognito-model-namespace",
        declared: &["AWS::CognitoIdentity::IdentityPoolPrincipalTag"],
    },
    NearMiss {
        case: "two unpublished names beside a published one, each named once",
        stack_name: "cognito-mixed",
        declared: &[
            "AWS::Cognito::PrincipalTag",
            "AWS::Cognito::IdentityPoolPrincipalTagMap",
        ],
    },
];

/// A name the reference does not publish is refused, whatever namespace it wears, and
/// nothing is created for it.
#[tokio::test]
async fn an_unpublished_cognito_name_is_still_refused() {
    let router = router();
    for near_miss in NEAR_MISSES {
        let NearMiss {
            case,
            stack_name,
            declared,
        } = near_miss;
        // The published type beside the unpublished ones: the refusal names only what it
        // did not recognise.
        let mut resources = serde_json::Map::new();
        resources.insert(
            "Pool".to_string(),
            json!({ "Type": "AWS::Cognito::IdentityPool" }),
        );
        for (index, resource_type) in declared.iter().enumerate() {
            resources.insert(format!("Unknown{index}"), json!({ "Type": resource_type }));
        }
        let template = Value::Object(
            [("Resources".to_string(), Value::Object(resources))]
                .into_iter()
                .collect(),
        )
        .to_string();

        let (status_code, body) = create_stack(&router, stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected HTTP 400 for an unpublished resource type, got \
             {status_code} with body {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the wire code ValidationError, got body {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        for resource_type in *declared {
            assert!(
                message.contains(resource_type),
                "case {case:?}: expected the refusal to name {resource_type:?}, got \
                 {message:?}"
            );
        }
        assert!(
            !message.contains("AWS::Cognito::IdentityPool,")
                && !message.contains("[AWS::Cognito::IdentityPool]"),
            "case {case:?}: expected the refusal not to name the published type beside \
             them, got {message:?}"
        );
        let (status_code, body) = describe_stack(&router, stack_name).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the refused stack not to exist, DescribeStacks \
             answered {status_code} with body {body}"
        );
    }
}

/// The reported template itself: an identity pool and the principal-tag mapping of one of
/// its providers, declared together as a production template does, with the mapping's
/// identifiers resolved through the pool it references.
#[tokio::test]
async fn the_reported_principal_tag_template_deploys() {
    let router: MultiServiceRouter = router();
    let case = "an identity pool and its provider's principal-tag mapping";
    let stack_name = "cognito-principal-tags";
    let template = json!({
        "Resources": {
            "Pool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": "principal-tag-pool",
                    "AllowUnauthenticatedIdentities": false,
                    "CognitoIdentityProviders": [{
                        "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_example",
                        "ClientId": "examplecl1ent1d"
                    }],
                    "SupportedLoginProviders": {
                        "accounts.google.com": "123.apps.googleusercontent.com"
                    }
                }
            },
            "PrincipalTags": {
                "Type": "AWS::Cognito::IdentityPoolPrincipalTag",
                "Properties": {
                    "IdentityPoolId": { "Ref": "Pool" },
                    "IdentityProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_example",
                    "UseDefaults": false,
                    "PrincipalTags": { "department": "custom:department" }
                }
            }
        },
        "Outputs": {
            "PoolId": { "Value": { "Ref": "Pool" } },
            "Mapping": { "Value": { "Ref": "PrincipalTags" } }
        }
    })
    .to_string();

    let (status_code, body) = create_stack(&router, stack_name, &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status_code} with body \
         {body}"
    );
    wait_for_the_transition().await;

    let (status_code, body) = describe_stack(&router, stack_name).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} with body {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the stack to reach {:?}, got body {body}",
        status::CREATE_COMPLETE
    );
    let keys = xml_texts(&body, "OutputKey");
    let values = xml_texts(&body, "OutputValue");
    assert_eq!(
        keys.len(),
        2,
        "case {case:?}: expected both declared outputs, got {keys:?} in {body}"
    );
    // Every reported output carries a VALUE: an `OutputKey` with no `OutputValue` beside it
    // is an output that resolved to nothing, which the counts are what catch — pairing by
    // position would silently skip it.
    assert_eq!(
        values.len(),
        keys.len(),
        "case {case:?}: expected one OutputValue per OutputKey {keys:?}, got {values:?} in \
         {body}"
    );
    // Each one resolves to the referenced resource's physical id: the pool's is the
    // `REGION:GUID` id the owning service MINTED for it ("Ref returns the Amazon Cognito
    // identity pool ID", which is the handle every cognito-identity operation takes), and
    // the mapping's is the PAIR it is addressed by, the pool id and the provider name joined
    // — which is what `cognito_principal_tag_identity.rs` is the suite of.
    let resolved: Vec<(String, String)> = keys.iter().cloned().zip(values.iter().cloned()).collect();
    let pool_id = resolved
        .iter()
        .find(|(key, _)| key == "PoolId")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    for (key, value) in &resolved {
        match key.as_str() {
            "PoolId" => assert!(
                value
                    .split_once(':')
                    .is_some_and(|(region, guid)| region == REGION
                        && !guid.is_empty()
                        && guid
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')),
                "case {case:?}: expected output {key:?} to resolve to the REGION:GUID pool id \
                 the owning service minted, got {value:?}"
            ),
            "Mapping" => assert_eq!(
                *value,
                format!(
                    "{pool_id}|cognito-idp.us-east-1.amazonaws.com/us-east-1_example"
                ),
                "case {case:?}: expected output {key:?} to resolve to the pair the mapping is \
                 addressed by, got {value:?}"
            ),
            other => panic!("case {case:?}: unexpected output {other:?} in {resolved:?}"),
        }
    }

    let (status_code, body) = resources_of(&router, stack_name).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} with \
         body {body}"
    );
    let mut reported: Vec<String> = wire_resources(&body)
        .into_iter()
        .map(|resource| resource.resource_type.unwrap_or_default())
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        vec![
            "AWS::Cognito::IdentityPool".to_string(),
            "AWS::Cognito::IdentityPoolPrincipalTag".to_string(),
        ],
        "case {case:?}: expected both declared resources to be recorded, got {reported:?}"
    );

    // The same template is accepted by the other operations that submit one for
    // deployment, which are held to the same catalogue.
    let (status_code, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "cognito-principal-tags-change-set"),
            member("ChangeSetName", "a-change-set"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateChangeSet to accept the same template, got \
         {status_code} with body {body}"
    );
    // UpdateStack refuses a template identical to the stack's own ("No updates are to be
    // performed."), so the submitted body declares one more mapping — of the same published
    // type, which is what this asserts the operation admits.
    let updated = json!({
        "Resources": {
            "Pool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": "principal-tag-pool",
                    "AllowUnauthenticatedIdentities": false,
                    "CognitoIdentityProviders": [{
                        "ProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_example",
                        "ClientId": "examplecl1ent1d"
                    }],
                    "SupportedLoginProviders": {
                        "accounts.google.com": "123.apps.googleusercontent.com"
                    }
                }
            },
            "PrincipalTags": {
                "Type": "AWS::Cognito::IdentityPoolPrincipalTag",
                "Properties": {
                    "IdentityPoolId": { "Ref": "Pool" },
                    "IdentityProviderName": "cognito-idp.us-east-1.amazonaws.com/us-east-1_example",
                    "UseDefaults": true
                }
            },
            "GooglePrincipalTags": {
                "Type": "AWS::Cognito::IdentityPoolPrincipalTag",
                "Properties": {
                    "IdentityPoolId": { "Ref": "Pool" },
                    "IdentityProviderName": "accounts.google.com",
                    "PrincipalTags": { "team": "custom:team" }
                }
            }
        }
    })
    .to_string();
    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", &updated),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected UpdateStack to accept a template declaring the same \
         published types, got {status_code} with body {body}"
    );
}
