// SPDX-License-Identifier: CC0-1.0

//! The SESSION TAGS a vend asks its web-identity assume for, and what they decide.
//!
//! RULE: where an identity pool carries a principal-tag attribute map for the identity
//! provider a presented login was made under, `GetCredentialsForIdentity` performs its
//! `sts:AssumeRoleWithWebIdentity` as a TAGGED assume — each mapped attribute's value, taken
//! from the presented token's own claim of that name, travels as a REQUESTED session tag. So
//! the assumed role's trust policy is decided with `aws:RequestTag/<key>` carrying those
//! values (and `aws:TagKeys` carrying the keys): an identity whose mapped value satisfies the
//! policy's condition is vended credentials, one whose value does not — and one whose token
//! carries no value for the mapped attribute at all — is refused with the modelled
//! `InvalidIdentityPoolConfigurationException`. The session that IS minted carries those
//! pairs as its session tags, which is what `aws:PrincipalTag/<key>` holds for every later
//! request the vended credential signs.
//!
//! The rows differ along what the rule generalises over: the VALUE the mapped claim carries
//! (the admitted one, another one, another case of it, none at all), how MANY attributes the
//! provider maps, and whether the pool maps the provider's attributes at all.
//!
//! The harness is `vended_role_session`'s: this service and STS on ONE shared engine behind
//! the same `MultiServiceRouter` the production router builds, with the IAM roles the
//! assume's decision reads seeded into that same engine, and the fixture pool is
//! `mapped_role_resolution`'s — a pool fronted by a user pool, whose `Logins` entry carries
//! the identity token the mapped claims are read out of.

use serde_json::{json, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use crate::mapped_role_resolution::{identity_token, pool, user_pool_claims, USER_POOL_PROVIDER};
use crate::vended_role_session::{vended, Emulator};

/// The Cognito Identity principal a role's trust policy names to admit an identity pool's
/// users, as `AssumeRoleWithWebIdentityRequest$RoleArn` documents it.
const COGNITO_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// The principal tag key the fixture role's trust policy is gated on, and the user attribute
/// the pool maps it from — the mapping `SetPrincipalTagAttributeMap` stores as
/// `PrincipalTags`: tag key -> the attribute (claim) its value is taken from.
const DEPARTMENT_TAG: &str = "Department";
/// See [`DEPARTMENT_TAG`].
const DEPARTMENT_ATTRIBUTE: &str = "custom:dept";

/// A SECOND tag key the pool maps, so a vend asking for more than one tag is read too.
const TENANT_TAG: &str = "Tenant";
/// See [`TENANT_TAG`].
const TENANT_ATTRIBUTE: &str = "custom:tenant";

/// The value of the mapped department attribute the fixture role's trust policy admits.
const ADMITTED_DEPARTMENT: &str = "eng";

/// How AWS words the refusal of a vend no role the pool names can be assumed for — among
/// them an assume the role's own trust policy does not admit. Written out here rather than
/// read back from the crate, so a message that drifted from it would fail.
const INVALID_POOL_CONFIGURATION: &str =
    "Invalid identity pool configuration. Check assigned IAM roles for this pool.";

/// The modelled error such a refusal carries.
const INVALID_POOL_CONFIGURATION_CODE: &str = "InvalidIdentityPoolConfigurationException";

/// A trust policy that admits this pool's users ONLY where the assume requests the session
/// tag `Department` with the value `ADMITTED_DEPARTMENT`: the `Federated` Cognito Identity
/// principal, the web-identity assume, the `:aud` condition restricting it to `audience`'s
/// own users, and the `aws:RequestTag/<key>` clause a tagged assume is gated on.
fn tag_gated_trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PRINCIPAL},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {
                format!("{COGNITO_PRINCIPAL}:aud"): audience,
                format!("aws:RequestTag/{DEPARTMENT_TAG}"): ADMITTED_DEPARTMENT,
            }},
        }],
    })
}

/// A trust policy that admits this pool's users whatever the assume requests: the same
/// statement without the tag clause, which is what makes the refusals below refusals of the
/// TAG rather than of the pool.
fn open_trust_policy(audience: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Federated": COGNITO_PRINCIPAL},
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {"StringEquals": {format!("{COGNITO_PRINCIPAL}:aud"): audience}},
        }],
    })
}

/// Configure `pool_id`'s authenticated role, which is the role every vend below assumes.
async fn set_authenticated_role(emulator: &Emulator, pool_id: &str, role_arn: &str) {
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": pool_id,
                "Roles": {"authenticated": role_arn},
            }),
        )
        .await;
}

/// Store the principal-tag attribute map `principal_tags` for the fixture pool's user-pool
/// provider.
async fn map_principal_tags(emulator: &Emulator, pool_id: &str, principal_tags: Value) {
    emulator
        .ok(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": pool_id,
                "IdentityProviderName": USER_POOL_PROVIDER,
                "UseDefaults": false,
                "PrincipalTags": principal_tags,
            }),
        )
        .await;
}

/// The `Logins` map of one exchange: the user pool's identity token for `subject`, carrying
/// the `claims` the mapped attributes are read out of.
fn login(subject: &str, claims: &Value) -> Value {
    json!({USER_POOL_PROVIDER: identity_token(&user_pool_claims(subject, claims))})
}

/// Vend for a fresh identity of `pool_id` presenting `logins`, and answer the status and the
/// body — the one exchange every row below drives.
async fn exchange(emulator: &Emulator, pool_id: &str, logins: &Value) -> (u16, Value) {
    let identity_id = emulator.identity(pool_id, Some(logins.clone())).await;
    emulator
        .call(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": identity_id, "Logins": logins.clone()}),
        )
        .await
}

/// The session tags the STS registry in the same engine holds for the session `access_key_id`
/// names — the pairs `aws:PrincipalTag/<key>` answers for a request that credential signs —
/// in tag-key order.
fn session_tags(emulator: &Emulator, access_key_id: &str) -> Vec<(String, String)> {
    let registry = emulator
        .engine
        .state::<sts::ComAmazonawsStsAwssecuritytokenservicev20110615State>();
    let held = registry.lock();
    let session = held
        .session(access_key_id)
        .unwrap_or_else(|| panic!("the vended session {access_key_id} is in the STS registry"));
    let mut tags = session.tags.clone();
    tags.sort();
    tags
}

/// One row of the table: what the presented token's mapped claims carry, and the session tags
/// the assume must therefore request — `None` for a row the gated role must refuse.
struct Row<'a> {
    case: &'a str,
    claims: Value,
    expected_tags: Option<Vec<(&'a str, &'a str)>>,
}

/// RULE: the mapped attribute values the presented token carries are what the trust policy's
/// `aws:RequestTag/<key>` clause is decided against, and what the minted session carries as
/// its session tags.
#[tokio::test]
async fn the_mapped_attribute_values_decide_the_tagged_assume() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "request-tag-gated").await;
    let gated = emulator.seed_role("tagged-vend", Some(tag_gated_trust_policy(&pool_id)));
    set_authenticated_role(&emulator, &pool_id, &gated).await;
    map_principal_tags(
        &emulator,
        &pool_id,
        json!({DEPARTMENT_TAG: DEPARTMENT_ATTRIBUTE, TENANT_TAG: TENANT_ATTRIBUTE}),
    )
    .await;

    // The rows differ along what the rule generalises over: the value the mapped attribute
    // carries, its case, how many mapped attributes the token answers for, and a token that
    // answers for none.
    let rows = [
        Row {
            case: "the mapped attribute carries the admitted value",
            claims: json!({DEPARTMENT_ATTRIBUTE: ADMITTED_DEPARTMENT}),
            expected_tags: Some(vec![(DEPARTMENT_TAG, ADMITTED_DEPARTMENT)]),
        },
        Row {
            case: "both mapped attributes carry a value",
            claims: json!({DEPARTMENT_ATTRIBUTE: ADMITTED_DEPARTMENT, TENANT_ATTRIBUTE: "acme"}),
            expected_tags: Some(vec![
                (DEPARTMENT_TAG, ADMITTED_DEPARTMENT),
                (TENANT_TAG, "acme"),
            ]),
        },
        Row {
            case: "only the second mapped attribute carries a value",
            claims: json!({TENANT_ATTRIBUTE: "acme"}),
            expected_tags: None,
        },
        Row {
            case: "the mapped attribute carries another value",
            claims: json!({DEPARTMENT_ATTRIBUTE: "sales"}),
            expected_tags: None,
        },
        Row {
            case: "the mapped attribute carries the value in another case",
            claims: json!({DEPARTMENT_ATTRIBUTE: "ENG"}),
            expected_tags: None,
        },
        Row {
            case: "the token carries neither mapped attribute",
            claims: json!({}),
            expected_tags: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let logins = login(&format!("tagged-subject-{index}"), &row.claims);
        let (status, body) = exchange(&emulator, &pool_id, &logins).await;
        match &row.expected_tags {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the vend must be served, got {status} with {body}"
                );
                let (access_key_id, _) = vended(&body);
                let actual = session_tags(&emulator, access_key_id);
                let expected: Vec<(String, String)> = expected
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect();
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected the session tags {expected:?}, got {actual:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the assume must be refused, got {status} with {body}"
                );
                assert_eq!(
                    body["__type"].as_str(),
                    Some(INVALID_POOL_CONFIGURATION_CODE),
                    "case {case:?}: expected {INVALID_POOL_CONFIGURATION_CODE}, got {body}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(INVALID_POOL_CONFIGURATION),
                    "case {case:?}: expected {INVALID_POOL_CONFIGURATION:?}, got {body}"
                );
                assert!(
                    body["Credentials"].is_null(),
                    "case {case:?}: a refused assume vends nothing, got {body}"
                );
            }
        }
    }
}

/// RULE, the other side: a pool that maps NO principal tags for the provider makes an
/// UNTAGGED assume — so a role gated on a requested tag admits none of its exchanges, and a
/// role that is not gated vends a session carrying no tags at all.
#[tokio::test]
async fn a_pool_that_maps_no_principal_tags_makes_an_untagged_assume() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "no-request-tags").await;
    let logins = login("untagged-subject", &json!({DEPARTMENT_ATTRIBUTE: ADMITTED_DEPARTMENT}));

    // The gated role first: the token carries the attribute the mapping WOULD map, so what
    // withholds the grant is that no mapping asks for it.
    let gated = emulator.seed_role("untagged-gated", Some(tag_gated_trust_policy(&pool_id)));
    set_authenticated_role(&emulator, &pool_id, &gated).await;
    let (status, body) = exchange(&emulator, &pool_id, &logins).await;
    assert_eq!(
        status, 400,
        "case \"an unmapped pool against a tag-gated role\": the assume must be refused, got \
         {status} with {body}"
    );
    assert_eq!(
        body["__type"].as_str(),
        Some(INVALID_POOL_CONFIGURATION_CODE),
        "case \"an unmapped pool against a tag-gated role\": expected \
         {INVALID_POOL_CONFIGURATION_CODE}, got {body}"
    );

    // The same pool against a role that is not gated: the vend is served, and the session it
    // mints carries no session tags.
    let open = emulator.seed_role("untagged-open", Some(open_trust_policy(&pool_id)));
    set_authenticated_role(&emulator, &pool_id, &open).await;
    let (status, body) = exchange(&emulator, &pool_id, &logins).await;
    assert_eq!(
        status, 200,
        "case \"an unmapped pool against an ungated role\": the vend must be served, got \
         {status} with {body}"
    );
    let (access_key_id, _) = vended(&body);
    let actual = session_tags(&emulator, access_key_id);
    assert!(
        actual.is_empty(),
        "case \"an unmapped pool against an ungated role\": an untagged assume attaches no \
         session tags, got {actual:?}"
    );
}
