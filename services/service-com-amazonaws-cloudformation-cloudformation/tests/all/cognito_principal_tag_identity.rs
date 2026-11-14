// SPDX-License-Identifier: CC0-1.0

//! What a stack publishes for an `AWS::Cognito::IdentityPoolPrincipalTag`, end to end over
//! the awsQuery wire with the owning service mounted beside CloudFormation.
//!
//! RULE under test: **the physical resource id of a resource whose type has a COMPOSITE
//! primary identifier — which is what `Ref`, `Fn::GetAtt`-free outputs and
//! `DescribeStackResources` report — is the RESOLVED values of that type's identifier
//! properties joined by `|`**, so a client can split the published value back into the
//! arguments the owning service's own calls take. For this type the identifier is the pair
//! `(IdentityPoolId, IdentityProviderName)` that `SetPrincipalTagAttributeMap` and
//! `GetPrincipalTagAttributeMap` address one provider's attribute map by, and the pool-id
//! half is the `REGION:GUID` the pool's own create answered with — never a name
//! CloudFormation generated, which carries neither half.
//!
//! The other half of the rule is that the published pair ANSWERS: the mapping a declaration
//! states is really set in Amazon Cognito Identity, so `GetPrincipalTagAttributeMap` driven
//! by nothing but the split Ref reports exactly what the template declared.
//!
//! The rows differ along what the rule generalises over — which pool the mapping is on, the
//! FORM of the identity provider the pair's second half names (an Amazon Cognito user pool,
//! a public login provider, an OpenID Connect provider's ARN, a SAML provider's ARN; the four
//! kinds of federated provider an identity pool's own configuration can declare, from
//! `CreateIdentityPool`'s `CognitoIdentityProviders`, `SupportedLoginProviders`,
//! `OpenIdConnectProviderARNs` and `SamlProviderARNs`) and the mapping itself (a custom map
//! of one, two or three tags, and the default mappings asked for with `UseDefaults`). The
//! pool id is per-run, so what every row asserts is the FORM and the round trip, never a
//! recorded value.
//!
//! `cognito_resource_types.rs` is the suite of the catalogue — that a template may DECLARE
//! every published Amazon Cognito type at all; this one is about the identifier a declared
//! mapping publishes and the mapping it really sets.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::DEFAULT_PRINCIPAL_TAGS;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::REGION;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, output_value, resource_event_reason, router,
    xml_text,
};
use crate::stack_owned_secret_teardown::poll_to_terminal;
use crate::stack_resources::{resources_of, wire_resources};

/// The separator CloudFormation joins the parts of a composite physical resource id with.
const PAIR_SEPARATOR: char = '|';

/// The type under test, and the pool type its mappings are declared on.
const PRINCIPAL_TAG_TYPE: &str = "AWS::Cognito::IdentityPoolPrincipalTag";
const IDENTITY_POOL_TYPE: &str = "AWS::Cognito::IdentityPool";

/// The four federated providers the stack's pools declare, one per form a pool's own
/// configuration can carry: an Amazon Cognito user pool's provider name, a public login
/// provider, an OpenID Connect provider's IAM ARN and a SAML provider's.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_pooltags";
const PUBLIC_PROVIDER: &str = "accounts.google.com";
const OIDC_PROVIDER_ARN: &str = "arn:aws:iam::000000000000:oidc-provider/accounts.example.com";
const SAML_PROVIDER_ARN: &str = "arn:aws:iam::000000000000:saml-provider/ExampleIdP";

/// One declared mapping: the provider it is for, the pool it is on, what the template states
/// and what `GetPrincipalTagAttributeMap` must then report for the pair.
struct Row {
    case: &'static str,
    logical_id: &'static str,
    /// The logical id of the `AWS::Cognito::IdentityPool` the mapping is declared on.
    pool: &'static str,
    provider: &'static str,
    /// The mapping members the declaration states beside the pair.
    declared: fn() -> Value,
    /// `UseDefaults` as the owning service must then report it.
    use_defaults: bool,
    /// The mapping the owning service must then report, as pairs.
    reported: &'static [(&'static str, &'static str)],
}

/// The rows: every provider form, on two different pools, with mappings of different sizes
/// and the default mappings among them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Amazon Cognito user pool's provider, mapped to one tag",
            logical_id: "UserPoolMapping",
            pool: "FirstPool",
            provider: USER_POOL_PROVIDER,
            declared: || json!({"UseDefaults": false, "PrincipalTags": {"tenant": "custom:tenant"}}),
            use_defaults: false,
            reported: &[("tenant", "custom:tenant")],
        },
        Row {
            case: "a public login provider, mapped to two tags",
            logical_id: "PublicMapping",
            pool: "FirstPool",
            provider: PUBLIC_PROVIDER,
            declared: || {
                json!({"PrincipalTags": {"team": "custom:team", "seat": "custom:seat"}})
            },
            use_defaults: false,
            reported: &[("seat", "custom:seat"), ("team", "custom:team")],
        },
        Row {
            case: "an OpenID Connect provider's ARN, on the default mappings",
            logical_id: "OidcMapping",
            pool: "SecondPool",
            provider: OIDC_PROVIDER_ARN,
            // Asking for the defaults is a choice INSTEAD of a custom map, so the provider is
            // mapped by the pair the owning service publishes for them.
            declared: || json!({"UseDefaults": true}),
            use_defaults: true,
            reported: &DEFAULT_PRINCIPAL_TAGS,
        },
        Row {
            case: "a SAML provider's ARN, mapped to three tags",
            logical_id: "SamlMapping",
            pool: "SecondPool",
            provider: SAML_PROVIDER_ARN,
            declared: || {
                json!({"UseDefaults": "false", "PrincipalTags": {
                    "department": "custom:department",
                    "region": "custom:region",
                    "tier": "custom:tier"
                }})
            },
            use_defaults: false,
            reported: &[
                ("department", "custom:department"),
                ("region", "custom:region"),
                ("tier", "custom:tier"),
            ],
        },
    ]
}

/// The declaration of one identity pool that federates all four providers above, so every
/// row's mapping is for a provider the pool's own configuration names.
fn pool_declaration(name: &str) -> Value {
    json!({
        "Type": IDENTITY_POOL_TYPE,
        "Properties": {
            "IdentityPoolName": name,
            "AllowUnauthenticatedIdentities": false,
            "CognitoIdentityProviders": [
                {"ProviderName": USER_POOL_PROVIDER, "ClientId": "examplecl1ent1d"}
            ],
            "SupportedLoginProviders": {PUBLIC_PROVIDER: "123.apps.googleusercontent.com"},
            "OpenIdConnectProviderARNs": [OIDC_PROVIDER_ARN],
            "SamlProviderARNs": [SAML_PROVIDER_ARN]
        }
    })
}

/// The template the table deploys: two pools and every row's mapping, with an output per
/// resource so each `Ref` is read off the wire.
fn table_template(rows: &[Row]) -> String {
    let mut resources = Map::new();
    resources.insert("FirstPool".to_string(), pool_declaration("ptag_pool_first"));
    resources.insert("SecondPool".to_string(), pool_declaration("ptag_pool_second"));
    let mut outputs = Map::new();
    for pool in ["FirstPool", "SecondPool"] {
        outputs.insert(pool.to_string(), json!({"Value": {"Ref": pool}}));
    }
    for row in rows {
        let mut properties = (row.declared)();
        let members = properties
            .as_object_mut()
            .expect("the declared members are an object");
        members.insert(
            "IdentityPoolId".to_string(),
            json!({"Ref": row.pool}),
        );
        members.insert(
            "IdentityProviderName".to_string(),
            Value::String(row.provider.to_string()),
        );
        resources.insert(
            row.logical_id.to_string(),
            json!({"Type": PRINCIPAL_TAG_TYPE, "Properties": properties}),
        );
        outputs.insert(
            row.logical_id.to_string(),
            json!({"Value": {"Ref": row.logical_id}}),
        );
    }
    json!({"Resources": resources, "Outputs": outputs}).to_string()
}

/// Whether `value` is an identity pool id of the form the owning service mints:
/// `<region>:<guid>`, the modelled `^[\w-]+:[0-9a-f-]+$`.
fn is_minted_pool_id(value: &str) -> bool {
    value.split_once(':').is_some_and(|(region, guid)| {
        region == REGION
            && !guid.is_empty()
            && guid
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    })
}

/// One Amazon Cognito Identity request, over the awsJson1.1 wire that service speaks: the
/// HTTP status and the parsed body.
async fn cognito_identity(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
        .header("host", format!("cognito-identity.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cognito-identity/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetPrincipalTagAttributeMap` for one pair, read back as `(UseDefaults, PrincipalTags)`.
async fn mapping_of(
    router: &MultiServiceRouter,
    case: &str,
    pool_id: &str,
    provider: &str,
) -> (bool, BTreeMap<String, String>) {
    let (status_code, answer) = cognito_identity(
        router,
        "GetPrincipalTagAttributeMap",
        &json!({"IdentityPoolId": pool_id, "IdentityProviderName": provider}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected GetPrincipalTagAttributeMap({pool_id}, {provider}) — driven \
         by nothing but the published pair — to be served, got {status_code} with {answer}"
    );
    let reported: BTreeMap<String, String> = answer["PrincipalTags"]
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (key.clone(), value.as_str().unwrap_or_default().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    (
        answer["UseDefaults"].as_bool().unwrap_or(false),
        reported,
    )
}

/// The expected mapping of a row, as the comparison takes it.
fn expected(reported: &[(&str, &str)]) -> BTreeMap<String, String> {
    reported
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every declared mapping publishes the pair it is addressed by, and that pair answers.
#[tokio::test]
async fn a_declared_mapping_publishes_the_pair_it_is_addressed_by() {
    let router = router();
    let rows = rows();
    let stack_name = "cognito-principal-tag-pairs";
    let (status_code, body) = create_stack(&router, stack_name, &table_template(&rows), &[]).await;
    assert_eq!(
        status_code, 200,
        "expected HTTP 200 from CreateStack, got {status_code} with body {body}"
    );
    let described = poll_to_terminal(&router, "the deployment", stack_name).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "expected the stack to reach {:?}, got body {described}",
        status::CREATE_COMPLETE
    );

    // The pool-id half is the one the owning service minted, which is what makes the
    // published pair callable at all.
    let mut pool_ids: BTreeMap<&str, String> = BTreeMap::new();
    for pool in ["FirstPool", "SecondPool"] {
        let published = output_value(&described, pool).unwrap_or_default();
        assert!(
            is_minted_pool_id(&published),
            "case {pool:?}: expected the output to carry the REGION:GUID pool id the owning \
             service minted, got {published:?}"
        );
        pool_ids.insert(pool, published);
    }

    let (status_code, resources) = resources_of(&router, stack_name).await;
    assert_eq!(
        status_code, 200,
        "expected DescribeStackResources to answer, got {status_code} with body {resources}"
    );
    let reported_ids: BTreeMap<String, String> = wire_resources(&resources)
        .into_iter()
        .map(|resource| {
            (
                resource.logical_id.unwrap_or_default(),
                resource.physical_id.unwrap_or_default(),
            )
        })
        .collect();

    for row in &rows {
        let case = row.case;
        let pool_id = pool_ids
            .get(row.pool)
            .unwrap_or_else(|| panic!("case {case:?}: the pool's own output must be read"));
        let published = output_value(&described, row.logical_id).unwrap_or_default();
        let expected_pair = format!("{pool_id}{PAIR_SEPARATOR}{}", row.provider);
        assert_eq!(
            published, expected_pair,
            "case {case:?}: expected the Ref of the mapping to be the pair it is addressed \
             by {expected_pair:?}, got {published:?}"
        );
        // The same identifier is what the stack reports for the resource: a `Ref` IS the
        // physical resource id, so the two cannot differ.
        assert_eq!(
            reported_ids.get(row.logical_id).map(String::as_str),
            Some(published.as_str()),
            "case {case:?}: expected DescribeStackResources to report the same physical id \
             {published:?}, got {:?}",
            reported_ids.get(row.logical_id)
        );
        // A client that holds nothing but the published value recovers both arguments of the
        // owning service's own call from it.
        let (held_pool, held_provider) = published
            .split_once(PAIR_SEPARATOR)
            .unwrap_or_else(|| panic!("case {case:?}: {published:?} must split into the pair"));
        assert_eq!(
            (held_pool, held_provider),
            (pool_id.as_str(), row.provider),
            "case {case:?}: expected the pair to split into the pool id and the provider, got \
             {held_pool:?} and {held_provider:?}"
        );
        let (use_defaults, reported) = mapping_of(&router, case, held_pool, held_provider).await;
        assert_eq!(
            use_defaults, row.use_defaults,
            "case {case:?}: expected UseDefaults {} for the pair, got {use_defaults}",
            row.use_defaults
        );
        assert_eq!(
            reported,
            expected(row.reported),
            "case {case:?}: expected the declared mapping to be the one the owning service \
             reports, got {reported:?}"
        );
    }
}

/// One declaration that cannot be created, and the property its failure must name.
struct Incomplete {
    case: &'static str,
    stack_name: &'static str,
    properties: fn() -> Value,
    named: &'static str,
}

/// Both halves of the pair are required, so a declaration stating neither describes no
/// mapping that could be set.
fn incomplete() -> Vec<Incomplete> {
    vec![
        Incomplete {
            case: "no identity provider at all",
            stack_name: "cognito-ptag-no-provider",
            properties: || json!({"IdentityPoolId": {"Ref": "FirstPool"}}),
            named: "IdentityProviderName",
        },
        Incomplete {
            case: "no identity pool at all",
            stack_name: "cognito-ptag-no-pool",
            properties: || json!({"IdentityProviderName": USER_POOL_PROVIDER}),
            named: "IdentityPoolId",
        },
    ]
}

/// A declaration missing either half of the pair fails naming the property it states none of.
#[tokio::test]
async fn a_mapping_missing_half_the_pair_is_refused_naming_the_property() {
    let router = router();
    let rows = incomplete();
    for row in &rows {
        let template = json!({
            "Resources": {
                "FirstPool": pool_declaration("ptag_pool_incomplete"),
                "Mapping": {
                    "Type": PRINCIPAL_TAG_TYPE,
                    "Properties": (row.properties)(),
                }
            }
        })
        .to_string();
        let (status_code, body) = create_stack(&router, row.stack_name, &template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected HTTP 200 from CreateStack, got {status_code} with body {body}",
            row.case
        );
    }
    for row in &rows {
        let case = row.case;
        let described = poll_to_terminal(&router, case, row.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {case:?}: expected the failed create to be rolled back to {:?}, got body \
             {described}",
            status::ROLLBACK_COMPLETE
        );
        let reason = resource_event_reason(
            &router,
            row.stack_name,
            "Mapping",
            resource_status::CREATE_FAILED,
        )
        .await;
        assert!(
            reason.contains(row.named),
            "case {case:?}: expected the failure to name the required property {:?}, got \
             {reason:?}",
            row.named
        );
    }
}

/// Deleting the resource puts the provider back on the DEFAULT mappings it has when no
/// mapping was ever declared for it: the owning service publishes no call that removes one.
///
/// The pool is created OUTSIDE the stack — so the stack's own teardown takes the mapping away
/// and leaves the pool to read back — which also drives the pair's first half as a LITERAL
/// pool id rather than through a `Ref`.
#[tokio::test]
async fn deleting_the_mapping_puts_the_provider_back_on_the_default_mappings() {
    let case = "a mapping on a pool the stack does not own";
    let router = router();
    let (status_code, created) = cognito_identity(
        &router,
        "CreateIdentityPool",
        &json!({
            "IdentityPoolName": "ptag_pool_outside_the_stack",
            "AllowUnauthenticatedIdentities": false,
            "CognitoIdentityProviders": [
                {"ProviderName": USER_POOL_PROVIDER, "ClientId": "examplecl1ent1d"}
            ]
        }),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateIdentityPool to be served, got {status_code} with \
         {created}"
    );
    let pool_id = created["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers a pool id: {created}"))
        .to_string();

    let stack_name = "cognito-ptag-teardown";
    let template = json!({
        "Resources": {
            "Mapping": {
                "Type": PRINCIPAL_TAG_TYPE,
                "Properties": {
                    "IdentityPoolId": pool_id,
                    "IdentityProviderName": USER_POOL_PROVIDER,
                    "PrincipalTags": {"tenant": "custom:tenant"}
                }
            }
        },
        "Outputs": {"Mapping": {"Value": {"Ref": "Mapping"}}}
    })
    .to_string();
    let (status_code, body) = create_stack(&router, stack_name, &template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status_code} with body {body}"
    );
    let described = poll_to_terminal(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected the stack to reach {:?}, got body {described}",
        status::CREATE_COMPLETE
    );
    let published = output_value(&described, "Mapping").unwrap_or_default();
    assert_eq!(
        published,
        format!("{pool_id}{PAIR_SEPARATOR}{USER_POOL_PROVIDER}"),
        "case {case:?}: expected the Ref to be the pair the mapping is addressed by, got \
         {published:?}"
    );
    let (use_defaults, reported) = mapping_of(&router, case, &pool_id, USER_POOL_PROVIDER).await;
    assert_eq!(
        (use_defaults, reported),
        (false, expected(&[("tenant", "custom:tenant")])),
        "case {case:?}: expected the declared mapping to be in force before the teardown"
    );

    let stack_id = xml_text(&described, "StackId").unwrap_or_default();
    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", stack_name)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DeleteStack to answer 200, got {status_code} with body {body}"
    );
    let described = poll_to_terminal(&router, case, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case {case:?}: expected the teardown to reach {:?}, got body {described}",
        status::DELETE_COMPLETE
    );

    let (use_defaults, reported) = mapping_of(&router, case, &pool_id, USER_POOL_PROVIDER).await;
    assert_eq!(
        (use_defaults, reported),
        (true, expected(&DEFAULT_PRINCIPAL_TAGS)),
        "case {case:?}: expected the teardown to put the provider back on the default \
         mappings"
    );
    // The pool the stack did not own is still there, addressed by the same id.
    let (status_code, pool) = cognito_identity(
        &router,
        "DescribeIdentityPool",
        &json!({"IdentityPoolId": pool_id}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the pool outside the stack to survive the teardown, got \
         {status_code} with {pool}"
    );
    // The stack is gone, so nothing holds the mapping it declared any more.
    let (status_code, body) = describe_stack(&router, stack_name).await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the deleted stack not to be readable by name, got \
         {status_code} with body {body}"
    );
}
