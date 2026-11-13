// SPDX-License-Identifier: CC0-1.0

//! The feature plan a `CreateUserPool` request CONFIGURES, and the CSV header a
//! pool hands out for a user import, over the wire — driven through the router
//! exactly as an SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`) and read back through
//! `DescribeUserPool`.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * a plan-gated pool configuration member is refused only when the request
//!   CONFIGURES a plan that excludes it; a create that names no plan configures
//!   none — "`UserPoolTier` ... determines the eligibility of the user pool for
//!   features like managed login, access-token customization, and threat
//!   protection" (`CreateUserPool.UserPoolTier`) — so it is served and the pool
//!   is created on the cheapest plan that includes every feature its
//!   configuration asks for;
//! * `GetCSVHeader` answers one column per attribute of the pool a user import
//!   can carry — every attribute of the STORED schema but the minted `sub` and
//!   the `identities` attribute the service maintains — then the columns the
//!   import format adds, the last of which carries the user's password as a
//!   hash; and the ORDER of those columns is the import template's own, so a
//!   pool is handed the same template however its schema was configured
//!   (`Schema` at the create, in any order, or `AddCustomAttributes`
//!   afterwards), while the pool keeps storing its schema in the order it was
//!   created in: "an array of attributes for the new user pool"
//!   (`CreateUserPool.Schema`) is stored in the order the request names them,
//!   with the standard attributes it does not name following.
//!
//! Each table row names its case and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The modelled feature plans in INCLUSION order, cheapest first
/// (`smithy_query(query="shape",shape="UserPoolTierType")`).
const PLANS_IN_ORDER: [&str; 3] = ["LITE", "ESSENTIALS", "PLUS"];

/// The plan a request that names none carries: "Defaults to `ESSENTIALS`"
/// (`CreateUserPool.UserPoolTier`).
const DEFAULT_PLAN: &str = "ESSENTIALS";

/// The attribute every pool mints for its users, which no import supplies and
/// so no CSV column carries.
const MINTED_ATTRIBUTE: &str = "sub";

/// The attribute the SERVICE maintains on every pool — it renders the external
/// identities a profile is linked to — so a read of the schema reports it with
/// the standard attributes, and no import supplies it either.
const SERVICE_MAINTAINED_ATTRIBUTE: &str = "identities";

/// The columns Amazon Cognito's user-import CSV adds BEYOND the pool's own
/// attributes, in the order the import format puts them: the imported user's MFA
/// state, the username to create them under, and their password as a hash —
/// "the password hashing algorithm used to generate the hashes in the CSV file
/// for this import job" (`CreateUserImportJob.PasswordHashingAlgorithm`).
const IMPORT_FORMAT_COLUMNS: [&str; 3] = ["cognito:mfa_enabled", "cognito:username", "password_hash"];

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Whether `plan` includes a feature whose cheapest plan is `floor`: the plans
/// are ordered by inclusion, so every plan from the floor upwards includes it.
fn plan_includes(plan: &str, floor: &str) -> bool {
    let rank = |named: &str| {
        PLANS_IN_ORDER
            .iter()
            .position(|known| *known == named)
            .unwrap_or_else(|| panic!("{named} is one of the modelled plans"))
    };
    rank(plan) >= rank(floor)
}

/// The cheapest plan that includes a feature whose floor is `floor`, which is
/// the documented default when the default already includes it.
fn cheapest_plan_including(floor: &str) -> &'static str {
    PLANS_IN_ORDER
        .iter()
        .find(|plan| plan_includes(plan, floor) && plan_includes(plan, DEFAULT_PLAN))
        .copied()
        .unwrap_or_else(|| panic!("a plan that includes {floor}"))
}

/// The pool `user_pool_id` names, as `DescribeUserPool` reports it.
async fn described_pool(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// The names of the pool's stored schema attributes, in the order the pool
/// reports them.
async fn stored_schema(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let described = described_pool(router, user_pool_id).await;
    described["SchemaAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("a pool reports its schema: {described}"))
        .iter()
        .map(|attribute| attribute["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The pool's CSV header, as `GetCSVHeader` answers it.
async fn csv_header(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<String> {
    let answered = ok(
        router,
        "GetCSVHeader",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        answered["UserPoolId"],
        json!(user_pool_id),
        "the answer names the pool it was asked about, got {answered}"
    );
    answered["CSVHeader"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer carries its header columns: {answered}"))
        .iter()
        .map(|column| column.as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The plan a create configures
// ---------------------------------------------------------------------------

/// One plan-gated pool configuration: the members a `CreateUserPool` request
/// carries for it, the cheapest plan that includes it, and the feature a refusal
/// names.
struct GatedConfiguration {
    case: &'static str,
    /// The cheapest `UserPoolTierType` value whose plan includes every feature
    /// these members ask for.
    floor: &'static str,
    /// The feature name in the tail of the refusal message.
    feature: &'static str,
    members: fn() -> Value,
}

/// The table: each pool configuration whose own documentation names a feature
/// plan, asked for in more than one way, beside configurations that ask for
/// nothing gated at all.
const GATED_CONFIGURATIONS: [GatedConfiguration; 6] = [
    GatedConfiguration {
        // `UserPoolAddOnsType`: "To activate this setting, your user pool must
        // be on the Plus tier."
        case: "threat protection enforced",
        floor: "PLUS",
        feature: "Threat Protection",
        members: || json!({ "UserPoolAddOns": { "AdvancedSecurityMode": "ENFORCED" } }),
    },
    GatedConfiguration {
        // The same feature asked for together with the additional-flows member,
        // so the rule is not read off one spelling of the request.
        case: "threat protection enforced in custom authentication too",
        floor: "PLUS",
        feature: "Threat Protection",
        members: || {
            json!({
                "UserPoolAddOns": {
                    "AdvancedSecurityMode": "ENFORCED",
                    "AdvancedSecurityAdditionalFlows": { "CustomAuthMode": "AUDIT" },
                },
            })
        },
    },
    GatedConfiguration {
        // `SignInPolicyType`: "To activate this setting, your user pool must be
        // in the Essentials tier or higher."
        case: "choice-based sign-in with a one-time password factor",
        floor: "ESSENTIALS",
        feature: "Choice Based Authentication",
        members: || {
            json!({
                "Policies": {
                    "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] },
                },
            })
        },
    },
    GatedConfiguration {
        // The same member asking for passkeys, a factor the issue did not cite.
        case: "choice-based sign-in with passkeys",
        floor: "ESSENTIALS",
        feature: "Choice Based Authentication",
        members: || {
            json!({
                "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["WEB_AUTHN"] } },
            })
        },
    },
    GatedConfiguration {
        // Threat protection in the mode that "take[s] no action"
        // (`UserPoolAddOnsType`) and a policy of username and password alone
        // activate nothing, so the cheapest plan serves them.
        case: "threat protection logging only",
        floor: "LITE",
        feature: "Threat Protection",
        members: || {
            json!({
                "UserPoolAddOns": { "AdvancedSecurityMode": "AUDIT" },
                "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD"] } },
            })
        },
    },
    GatedConfiguration {
        // A configuration with no plan-gated member at all.
        case: "no plan-gated member",
        floor: "LITE",
        feature: "Threat Protection",
        members: || json!({ "MfaConfiguration": "OFF" }),
    },
];

/// Both halves of the rule at once: a create that CONFIGURES a plan is judged
/// against it, and a create that configures none is served on the cheapest plan
/// that includes what it asks for.
#[tokio::test]
async fn the_plan_a_create_configures_is_what_judges_its_configuration() {
    let router = router();

    for (index, row) in GATED_CONFIGURATIONS.iter().enumerate() {
        // The rows differ along the plan the request names, the fourth naming
        // none at all.
        let plans: Vec<Option<&str>> = PLANS_IN_ORDER
            .iter()
            .map(|plan| Some(*plan))
            .chain(std::iter::once(None))
            .collect();

        for requested in plans {
            let case = format!(
                "{} with {}",
                row.case,
                requested.map_or("no plan named".to_string(), |plan| format!("the {plan} plan"))
            );
            let mut body: Map<String, Value> = (row.members)()
                .as_object()
                .cloned()
                .expect("the members are a JSON object");
            body.insert(
                "PoolName".to_string(),
                json!(format!(
                    "plan-{index}-{}",
                    requested.unwrap_or("unnamed").to_lowercase()
                )),
            );
            if let Some(requested) = requested {
                body.insert("UserPoolTier".to_string(), json!(requested));
            }

            let pools_before = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            let (status, payload) = call(&router, "CreateUserPool", &Value::Object(body)).await;

            // A request that names no plan configures none, so nothing it asks
            // for can be refused for the plan: the pool lands on the cheapest
            // plan that includes it.
            let expected_plan = match requested {
                Some(requested) if !plan_includes(requested, row.floor) => {
                    let expected = format!(
                        "The following features need to be disabled for the {requested} pricing \
                         tier configured: {}",
                        row.feature
                    );
                    assert_eq!(
                        (status, error_code(&payload), error_message(&payload)),
                        (400, "FeatureUnavailableInTierException", expected.as_str()),
                        "case {case:?}: expected the tier refusal naming the feature, got {status} \
                         with {payload}"
                    );
                    let pools_after =
                        ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
                    assert_eq!(
                        pools_after, pools_before,
                        "case {case:?}: a refused create stores no pool, got {pools_after}"
                    );
                    continue;
                }
                Some(requested) => requested,
                None => cheapest_plan_including(row.floor),
            };

            assert_eq!(
                status, 200,
                "case {case:?}: the configuration is served, got {status} with {payload}"
            );
            let user_pool_id = payload["UserPool"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {payload}"))
                .to_string();
            assert_eq!(
                payload["UserPool"]["UserPoolTier"],
                json!(expected_plan),
                "case {case:?}: the created pool must be on {expected_plan:?}, got {payload}"
            );

            // The STORED pool is what the answer reported, read back through
            // the reporting operation.
            let stored = described_pool(&router, &user_pool_id).await;
            assert_eq!(
                stored["UserPoolTier"],
                json!(expected_plan),
                "case {case:?}: the stored pool must be on {expected_plan:?}, got {stored}"
            );
            for (member, requested_value) in (row.members)()
                .as_object()
                .expect("the members are a JSON object")
            {
                // `Policies` reads back with the password policy's documented
                // defaults filled in, and `UserPoolAddOns` with the mode the
                // additional authentication types run in, so inside those two
                // the comparison is against the member the request carried.
                let (path, expected, answered) = match member.as_str() {
                    "Policies" => (
                        "Policies.SignInPolicy",
                        &requested_value["SignInPolicy"],
                        &stored["Policies"]["SignInPolicy"],
                    ),
                    "UserPoolAddOns" => (
                        "UserPoolAddOns.AdvancedSecurityMode",
                        &requested_value["AdvancedSecurityMode"],
                        &stored["UserPoolAddOns"]["AdvancedSecurityMode"],
                    ),
                    other => (other, requested_value, &stored[other]),
                };
                assert_eq!(
                    answered, expected,
                    "case {case:?}: the stored pool must carry the {path} it was created with, \
                     got {stored}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The CSV header a pool hands out
// ---------------------------------------------------------------------------

/// Create a pool, optionally with a `Schema`, and answer its minted id.
async fn pool(router: &MultiServiceRouter, name: &str, schema: Option<Value>) -> String {
    let mut body = json!({ "PoolName": name });
    if let Some(schema) = schema {
        body.as_object_mut()
            .expect("a request body is a JSON object")
            .insert("Schema".to_string(), schema);
    }
    let created = ok(router, "CreateUserPool", &body).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string()
}

/// Add `attributes` to the pool's schema through `AddCustomAttributes`.
async fn add_custom_attributes(router: &MultiServiceRouter, user_pool_id: &str, attributes: Value) {
    ok(
        router,
        "AddCustomAttributes",
        &json!({ "UserPoolId": user_pool_id, "CustomAttributes": attributes }),
    )
    .await;
}

/// The columns of `header` that the pool's own schema does not hold, i.e. the
/// ones the import format adds.
fn trailing_columns(header: &[String]) -> Vec<String> {
    header
        .iter()
        .rev()
        .take(IMPORT_FORMAT_COLUMNS.len())
        .rev()
        .cloned()
        .collect()
}

/// Amazon Cognito's standard claim order: the attributes every pool's schema
/// starts with, in the order the standard that names them lists them (the
/// OpenID Connect standard claims, as the Amazon Cognito Developer Guide's
/// "User pool attributes" lists them). A `Schema` the create carries moves only
/// the attributes it names out of this order.
const STANDARD_CLAIM_ORDER: [&str; 20] = [
    "sub",
    "name",
    "given_name",
    "family_name",
    "middle_name",
    "nickname",
    "preferred_username",
    "profile",
    "picture",
    "website",
    "email",
    "email_verified",
    "gender",
    "birthdate",
    "zoneinfo",
    "locale",
    "phone_number",
    "phone_number_verified",
    "address",
    "updated_at",
];

/// The name a pool stores the requested attribute `requested` under: a standard
/// attribute keeps its name, and every other one takes the prefix Amazon
/// Cognito gives a custom attribute — "when you add an attribute with a `Name`
/// value of `MyAttribute`, Amazon Cognito creates the custom attribute
/// `custom:MyAttribute`" (`SchemaAttributeType.Name`).
fn stored_name(requested: &str) -> String {
    if STANDARD_CLAIM_ORDER.contains(&requested) {
        requested.to_string()
    } else {
        format!("custom:{requested}")
    }
}

/// The order the RULE says a pool stores its schema in: the minted attribute,
/// then the attributes the create's `Schema` names in the order it names them,
/// then the standard attributes it does not name in the standard claim order,
/// then the attribute the service maintains for every pool, then the attributes
/// `AddCustomAttributes` appended afterwards.
///
/// Computed from the request alone, so it is an expectation the handler cannot
/// satisfy by templating a constant order of its own.
fn expected_schema_order(named: &[&str], added: &[&str]) -> Vec<String> {
    let mut order = vec![MINTED_ATTRIBUTE.to_string()];
    for requested in named.iter().map(|name| stored_name(name)) {
        if requested != MINTED_ATTRIBUTE && !order.contains(&requested) {
            order.push(requested);
        }
    }
    for standard in STANDARD_CLAIM_ORDER {
        if !order.iter().any(|held| held == standard) {
            order.push(standard.to_string());
        }
    }
    order.push(SERVICE_MAINTAINED_ATTRIBUTE.to_string());
    for appended in added.iter().map(|name| stored_name(name)) {
        if !order.contains(&appended) {
            order.push(appended);
        }
    }
    order
}

/// Amazon Cognito's import template, as it lists a pool's STANDARD columns once
/// that pool carries attributes of its own: the pool's custom columns sit in the
/// slot marked below rather than behind every standard one. The minted `sub` is
/// not a column of the template.
const IMPORT_TEMPLATE_STANDARD_COLUMNS: [&str; 19] = [
    "profile",
    "address",
    "birthdate",
    "gender",
    "preferred_username",
    "updated_at",
    "website",
    "picture",
    "email_verified",
    "phone_number_verified",
    "phone_number",
    "zoneinfo",
    // The pool's own (custom) columns go here.
    "locale",
    "email",
    "given_name",
    "family_name",
    "middle_name",
    "name",
    "nickname",
];

/// How many of [`IMPORT_TEMPLATE_STANDARD_COLUMNS`] the template lists ahead of
/// the pool's custom columns.
const IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM: usize = 12;

/// The header the RULE says the pool a request built hands out: the standard
/// columns in the import template's order with the pool's custom columns in the
/// template's slot for them, or — for a pool carrying no custom attributes — its
/// standard attributes in the claim order the default schema declares them in,
/// then the columns the import format adds.
///
/// Computed from the request alone and never from the pool's stored order, so a
/// row whose `Schema` names its attributes in one order and a row that appends
/// them afterwards expect the SAME columns in the same places.
fn expected_header(named: &[&str], added: &[&str]) -> Vec<String> {
    let mut custom: Vec<String> = Vec::new();
    for requested in named.iter().chain(added.iter()) {
        let stored = stored_name(requested);
        if !STANDARD_CLAIM_ORDER.contains(requested) && !custom.contains(&stored) {
            custom.push(stored);
        }
    }

    let mut header: Vec<String> = if custom.is_empty() {
        STANDARD_CLAIM_ORDER
            .iter()
            .filter(|standard| **standard != MINTED_ATTRIBUTE)
            .map(|standard| (*standard).to_string())
            .collect()
    } else {
        IMPORT_TEMPLATE_STANDARD_COLUMNS[..IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM]
            .iter()
            .map(|column| (*column).to_string())
            .chain(custom)
            .chain(
                IMPORT_TEMPLATE_STANDARD_COLUMNS[IMPORT_TEMPLATE_COLUMNS_BEFORE_CUSTOM..]
                    .iter()
                    .map(|column| (*column).to_string()),
            )
            .collect()
    };
    header.extend(IMPORT_FORMAT_COLUMNS.iter().map(|column| column.to_string()));
    header
}

/// Assert the rule for one pool: it stores its schema in the order it was
/// created in, its CSV header carries exactly the attributes of that schema an
/// import can supply in the import template's own order, then the import
/// format's own columns — and asking for the header changes nothing.
async fn assert_header_is_the_import_template(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    expected_schema: &[String],
    expected: &[String],
) {
    let schema = stored_schema(router, user_pool_id).await;
    assert_eq!(
        schema, expected_schema,
        "case {case:?}: the pool stores its schema in the order it was created in, expected \
         {expected_schema:?}, got {schema:?}"
    );

    let header = csv_header(router, user_pool_id).await;
    assert_eq!(
        header, expected,
        "case {case:?}: the header is the import template's columns in the template's order, \
         expected {expected:?}, got {header:?} for schema {schema:?}"
    );

    // The columns are the pool's own attributes: whatever the order, the header
    // carries a column for every attribute of the stored schema an import can
    // supply and for no other attribute.
    let importable: Vec<String> = schema
        .iter()
        .filter(|name| *name != MINTED_ATTRIBUTE && *name != SERVICE_MAINTAINED_ATTRIBUTE)
        .cloned()
        .collect();
    let mut carried: Vec<String> = header[..header.len() - IMPORT_FORMAT_COLUMNS.len()].to_vec();
    carried.sort();
    let mut expected_set = importable.clone();
    expected_set.sort();
    assert_eq!(
        carried, expected_set,
        "case {case:?}: the header's columns are the stored schema's importable attributes, got \
         {header:?} for schema {schema:?}"
    );
    assert_eq!(
        trailing_columns(&header),
        IMPORT_FORMAT_COLUMNS.map(str::to_string).to_vec(),
        "case {case:?}: the import format's own columns close the header, got {header:?}"
    );
    assert!(
        !header[..header.len() - IMPORT_FORMAT_COLUMNS.len()]
            .contains(&MINTED_ATTRIBUTE.to_string()),
        "case {case:?}: the minted attribute is not importable, got {header:?}"
    );

    // A report changes nothing: the same question answers the same list, and
    // the pool's schema is as it was.
    let again = csv_header(router, user_pool_id).await;
    assert_eq!(
        again, header,
        "case {case:?}: asking twice must answer the same header, got {again:?}"
    );
    assert_eq!(
        stored_schema(router, user_pool_id).await,
        schema,
        "case {case:?}: reporting the header must not touch the stored schema"
    );
}

/// One pool whose schema the test builds: what the create's `Schema` names, in
/// that order, and what is added to the pool afterwards.
struct SchemaCase {
    case: &'static str,
    /// The attribute names the create's `Schema` carries, in order; empty means
    /// the create carries no `Schema` member at all.
    named: &'static [&'static str],
    /// The attribute names `AddCustomAttributes` appends after the create.
    added: &'static [&'static str],
}

/// The table: the rows differ along whether the create names a `Schema` at all,
/// how many attributes it names and in what order, whether a custom attribute
/// sits among the standard ones, and whether attributes were added after the
/// pool existed.
const SCHEMA_CASES: [SchemaCase; 8] = [
    SchemaCase {
        case: "the default schema",
        named: &[],
        added: &[],
    },
    SchemaCase {
        case: "a Schema naming a few standard attributes out of the claim order, with a custom one among them",
        named: &["email", "name", "shelfCode", "zoneinfo", "address"],
        added: &[],
    },
    SchemaCase {
        case: "a Schema naming standard attributes only, out of the claim order",
        named: &["updated_at", "locale", "given_name"],
        added: &[],
    },
    SchemaCase {
        case: "a Schema naming one custom attribute only",
        named: &["loyaltyRank"],
        added: &[],
    },
    SchemaCase {
        case: "a Schema that only modifies one standard attribute",
        named: &["email"],
        added: &[],
    },
    SchemaCase {
        case: "a default schema extended by two custom attributes afterwards",
        named: &[],
        added: &["department", "seatCount"],
    },
    SchemaCase {
        case: "a Schema with a custom attribute and another added afterwards",
        named: &["shelfCode"],
        added: &["department"],
    },
    SchemaCase {
        case: "a Schema naming every standard attribute it may name in reverse claim order, with a custom one among them",
        named: &[
            "updated_at",
            "address",
            "phone_number",
            "locale",
            "zoneinfo",
            "birthdate",
            "shelfCode",
            "gender",
            "email_verified",
            "email",
            "website",
            "picture",
            "profile",
            "preferred_username",
            "nickname",
            "middle_name",
            "family_name",
            "given_name",
            "name",
        ],
        added: &[],
    },
];

/// The `Schema` member a row's `named` attributes make, or `None` for a row
/// that names none: a standard attribute is named alone, so the create keeps its
/// modelled data type, and a custom one carries the type it is created with.
fn schema_member(named: &[&str]) -> Option<Value> {
    if named.is_empty() {
        return None;
    }
    Some(Value::Array(
        named
            .iter()
            .map(|name| {
                if STANDARD_CLAIM_ORDER.contains(name) {
                    json!({ "Name": name })
                } else {
                    json!({ "Name": name, "AttributeDataType": "String" })
                }
            })
            .collect(),
    ))
}

#[tokio::test]
async fn a_schema_is_served_up_to_the_pools_custom_attribute_quota_and_refused_past_it() {
    let router = router();

    // Amazon Cognito's per-pool quota for custom attributes is 50 and cannot be
    // raised (Amazon Cognito Developer Guide, "Quotas in Amazon Cognito"), so
    // the published bound is served and one past it is refused with nothing
    // stored.
    const CUSTOM_ATTRIBUTE_QUOTA: usize = 50;

    for (case, count, served) in [
        ("one custom attribute", 1, true),
        ("the quota itself", CUSTOM_ATTRIBUTE_QUOTA, true),
        ("one past the quota", CUSTOM_ATTRIBUTE_QUOTA + 1, false),
    ] {
        let schema = Value::Array(
            (0..count)
                .map(|index| json!({ "Name": format!("attr{index}"), "AttributeDataType": "String" }))
                .collect(),
        );
        let pools_before = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
        let (status, payload) = call(
            &router,
            "CreateUserPool",
            &json!({ "PoolName": format!("quota-{count}"), "Schema": schema }),
        )
        .await;

        if !served {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: a schema past the quota is refused, got {status} with {payload}"
            );
            let pools_after = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            assert_eq!(
                pools_after, pools_before,
                "case {case:?}: a refused create stores no pool, got {pools_after}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a schema inside the quota is served, got {status} with {payload}"
        );
        let user_pool_id = payload["UserPool"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {payload}"))
            .to_string();
        let header = csv_header(&router, &user_pool_id).await;
        let expected = STANDARD_CLAIM_ORDER.len() - 1 + count + IMPORT_FORMAT_COLUMNS.len();
        assert_eq!(
            header.len(),
            expected,
            "case {case:?}: every requested attribute is a column of its own, got {header:?}"
        );
    }
}

#[tokio::test]
async fn the_csv_header_is_the_import_template_whatever_order_the_pool_stores_its_schema_in() {
    let router = router();

    for (index, row) in SCHEMA_CASES.iter().enumerate() {
        let user_pool_id = pool(
            &router,
            &format!("csv-schema-{index}"),
            schema_member(row.named),
        )
        .await;
        if !row.added.is_empty() {
            add_custom_attributes(
                &router,
                &user_pool_id,
                Value::Array(
                    row.added
                        .iter()
                        .map(|name| json!({ "Name": name, "AttributeDataType": "String" }))
                        .collect(),
                ),
            )
            .await;
        }
        assert_header_is_the_import_template(
            &router,
            row.case,
            &user_pool_id,
            &expected_schema_order(row.named, row.added),
            &expected_header(row.named, row.added),
        )
        .await;
    }

    // A pool nothing created has no header to report.
    let (status, payload) = call(
        &router,
        "GetCSVHeader",
        &json!({ "UserPoolId": "us-east-1_absentpool" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "case {:?}: an absent pool has no header, got {status} with {payload}",
        "a pool nothing created"
    );
}

/// Two pools that end up holding the SAME attributes, each configured a
/// different way: the create's `Schema`, that `Schema` in another order, and a
/// create that names nothing with `AddCustomAttributes` behind it. The header is
/// the service's template, so all of them answer the identical column list even
/// though each pool stores its schema in its own order.
#[tokio::test]
async fn pools_holding_the_same_attributes_hand_out_the_same_header() {
    let router = router();

    // Each row reaches the same pool contents — the standard attributes plus
    // `custom:shelfCode` — by a different route.
    let routes: [(&str, &[&str], &[&str]); 4] = [
        ("a Schema naming the custom attribute alone", &["shelfCode"], &[]),
        (
            "a Schema naming the custom attribute among standard ones",
            &["email", "shelfCode", "address"],
            &[],
        ),
        (
            "the same Schema in the opposite order",
            &["address", "shelfCode", "email"],
            &[],
        ),
        ("a default schema the attribute is added to", &[], &["shelfCode"]),
    ];

    let mut answered: Option<(&str, Vec<String>)> = None;
    for (index, (case, named, added)) in routes.iter().enumerate() {
        let user_pool_id = pool(&router, &format!("csv-route-{index}"), schema_member(named)).await;
        if !added.is_empty() {
            add_custom_attributes(
                &router,
                &user_pool_id,
                Value::Array(
                    added
                        .iter()
                        .map(|name| json!({ "Name": name, "AttributeDataType": "String" }))
                        .collect(),
                ),
            )
            .await;
        }
        let header = csv_header(&router, &user_pool_id).await;
        assert_eq!(
            header,
            expected_header(named, added),
            "case {case:?}: the header is the import template, got {header:?}"
        );
        match &answered {
            None => answered = Some((case, header)),
            Some((first, expected)) => assert_eq!(
                &header, expected,
                "case {case:?}: the same attributes hand out the same header as case {first:?}, \
                 expected {expected:?}, got {header:?}"
            ),
        }
    }
}
