// SPDX-License-Identifier: CC0-1.0

//! The identity store serves the surface its Smithy model declares, asserted end to end
//! through the production [`MultiServiceRouter`] with IAM Identity Center mounted beside
//! it, the way `core/router/src/lib.rs` assembles them.
//!
//! The rule under test: a service's wire surface is the surface its model declares — every
//! operation shape the model carries is routed to a handler of the crate rendered from
//! that model, every member it declares on a resource is stored and reported, and every
//! model fact the crate restates by hand (the `ResourceType` value each resource class
//! reports, the page-size bounds every listing applies) is the value the model states. So
//! a model refresh that adds an operation or a member, renames a resource class or moves a
//! bound fails here instead of leaving the emulator serving the previous model's surface.
//!
//! Each expectation is DERIVED from the model file the crate is built from — never a
//! second copy of it — and then driven over the wire, so the three ways the surface can
//! drift are one suite: the model, the handlers rendered from it, and the answers they
//! serve.
//!
//! Cross-service because the identity stores the service serves are the stores IAM
//! Identity Center's own instances have: the row where a store the account does not have
//! is reported as the missing resource needs that service to have answered which stores
//! the account holds. The single-service halves of these rules live in
//! `services/service-com-amazonaws-identitystore-awsidentitystore/tests/all/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_identitystore_awsidentitystore as identitystore;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix of the identity store's `awsJson1_1` wire (its crate's
/// OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "AWSIdentityStore";

/// The `X-Amz-Target` prefix of IAM Identity Center's own wire.
const IDENTITY_CENTER_TARGET_PREFIX: &str = "SWBExternalService";

/// The access key id these calls are signed with. Signatures are not what this suite is
/// about, but the credential scope is what routes a request to its service.
const AKID: &str = "AKIDMODELSURFACE";

/// The identity store IAM Identity Center provisioned for the instance `sso-admin` seeds.
/// Read from that crate's own constant rather than restated.
const SEED_STORE: &str = identitystore::state::SEED_IDENTITY_STORE_ID;

/// The identity store's crate directory, so a case can read the model the crate is built
/// from and the handler files rendered beside it.
const CRATE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../services/service-com-amazonaws-identitystore-awsidentitystore"
);

/// The identity store's Smithy model, so every expectation below is derived from the model
/// the crate is built from instead of a copy of it.
const MODEL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../services/service-com-amazonaws-identitystore-awsidentitystore",
    "/smithy-model/com-amazonaws-identitystore-awsidentitystore.json"
));

/// The marker line a handler carries while it is an unimplemented scaffold: the
/// machine-readable record of which operations are still stubs, which is why a case can
/// read it to know whether an operation's own answer is the scaffold's 501.
const SCAFFOLD_MARKER: &str = "//! SCAFFOLDED, NOT IMPLEMENTED:";

/// A well-formed `ResourceId` no resource of a fresh store carries, for the rows that
/// probe for something the store does not hold.
const UNMINTED_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A well-formed `IdentityStoreId` of the `d-` + 10 hexadecimal digits form that is not
/// the store of any instance this emulator holds.
const UNHELD_STORE: &str = "d-00000000e4";

/// The attribute extension a full profile carries: `Extensions`'s key shape admits
/// `^aws:identitystore:[a-z]{1,20}$`, and its own documentation says
/// `aws:identitystore:enterprise` is the only supported extension name
/// (`smithy_query(query="shape",shape="CreateUserRequest",member="Extensions")`).
const EXTENSION_NAME: &str = "aws:identitystore:enterprise";

/// The identity store AND IAM Identity Center on ONE engine, as `core/router/src/lib.rs`
/// mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what this suite is about; the request still carries a SigV4
    // credential scope, which is what routes it to its service.
    router.set_request_authentication(false);
    router.register_service(identitystore::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_ssoadmin_swbexternalservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call: the HTTP status, the wire error code and the parsed body.
async fn call_service(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("host", format!("{signing_name}.us-east-1.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/us-east-1\
                 /{signing_name}/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    let code = if header_code.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    (status, code, parsed)
}

/// One identity-store call.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, String, Value) {
    call_service(router, "identitystore", TARGET_PREFIX, operation, body).await
}

/// An identity-store call that must answer 200, as its body.
async fn served(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, code, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {code} with {parsed}"
    );
    parsed
}

/// An identity-store call that must be refused with `status` and an error code naming
/// `error`, as its body.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    let (actual_status, code, parsed) = call(router, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected {operation} to answer {status} {error}, \
         got {actual_status} {code} with {parsed}"
    );
    assert!(
        code.ends_with(error),
        "case {case:?}: expected {operation} to answer {error}, got {code:?} with {parsed}"
    );
    parsed
}

/// One IAM Identity Center call that must answer 200, as its body.
async fn identity_center(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, code, parsed) =
        call_service(router, "sso", IDENTITY_CENTER_TARGET_PREFIX, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected Identity Center's {operation} to answer 200, \
         got {status} {code} with {parsed}"
    );
    parsed
}

/// The `Message` of a refusal, whichever casing the envelope rendered it in.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The model, parsed.
fn model() -> Value {
    serde_json::from_str(MODEL).expect("the identity store's model parses")
}

/// A shape's short name: what follows the `#` of its absolute id.
fn short_name(shape_id: &str) -> &str {
    shape_id.rsplit('#').next().unwrap_or(shape_id)
}

/// The shape the model declares under the short name `name`.
fn shape<'a>(model: &'a Value, name: &str) -> &'a Value {
    model["shapes"]
        .as_object()
        .expect("the model carries a shape map")
        .iter()
        .find(|(id, _)| short_name(id) == name)
        .map(|(_, shape)| shape)
        .unwrap_or_else(|| panic!("expected the model to declare the shape {name:?}"))
}

/// Every operation shape the model declares, by short name, sorted.
fn modeled_operations(model: &Value) -> Vec<String> {
    let mut operations: Vec<String> = model["shapes"]
        .as_object()
        .expect("the model carries a shape map")
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .map(|(id, _)| short_name(id).to_string())
        .collect();
    operations.sort();
    operations
}

/// The input members `operation` marks `@required`, as the model names them.
fn required_input_members(model: &Value, operation: &str) -> Vec<String> {
    let input = shape(model, operation)["input"]["target"]
        .as_str()
        .map(|target| short_name(target).to_string())
        .unwrap_or_else(|| panic!("expected {operation} to declare an input shape"));
    let mut members: Vec<String> = shape(model, &input)["members"]
        .as_object()
        .map(|members| {
            members
                .iter()
                .filter(|(_, member)| member["traits"].get("smithy.api#required").is_some())
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    members.sort();
    members
}

/// A member name as the wire and the validation paths spell it: lower camel case.
fn lower_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// An operation name as its handler file is named: `ListGroupMemberships` ->
/// `list_group_memberships`.
fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (index, letter) in name.chars().enumerate() {
        if letter.is_ascii_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.extend(letter.to_lowercase());
    }
    snake
}

/// The source of the handler file that owns `operation`, which is also where the stub
/// census lives.
fn handler_source(operation: &str) -> String {
    let path = format!("{CRATE_DIR}/src/ops/{}.rs", snake_case(operation));
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "expected the operation {operation:?} the model declares to have a handler at \
             {path}: {error}"
        )
    })
}

/// The HTTP status the model's own `@httpError` declares for the error shape `name`.
fn modeled_http_error(model: &Value, name: &str) -> u16 {
    shape(model, name)["traits"]["smithy.api#httpError"]
        .as_u64()
        .unwrap_or_else(|| panic!("expected the model to declare an @httpError for {name:?}"))
        as u16
}

/// The values an enum shape the model declares admits.
fn enum_values(model: &Value, name: &str) -> BTreeSet<String> {
    let shape = shape(model, name);
    let values: BTreeSet<String> = shape["members"]
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(member, definition)| {
                    definition["traits"]["smithy.api#enumValue"]
                        .as_str()
                        .unwrap_or(member)
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !values.is_empty(),
        "expected the model's {name:?} to admit some value"
    );
    values
}

/// Every `@paginated` operation's `MaxResults` bounds as the model states them: the
/// inclusive range, and the page size its member declares by default where it declares
/// one.
fn page_size_bounds(model: &Value) -> BTreeMap<String, (i64, i64, Option<i64>)> {
    let mut bounds = BTreeMap::new();
    for operation in modeled_operations(model) {
        let shape_of_operation = shape(model, &operation);
        if shape_of_operation["traits"]
            .get("smithy.api#paginated")
            .is_none()
        {
            continue;
        }
        let input = short_name(
            shape_of_operation["input"]["target"]
                .as_str()
                .unwrap_or_default(),
        )
        .to_string();
        let Some(member) = shape(model, &input)["members"].get("MaxResults").cloned() else {
            continue;
        };
        let target = short_name(member["target"].as_str().unwrap_or_default()).to_string();
        let range = &shape(model, &target)["traits"]["smithy.api#range"];
        let min = range["min"].as_i64().unwrap_or_else(|| {
            panic!("expected {operation}'s MaxResults to declare a range minimum")
        });
        let max = range["max"].as_i64().unwrap_or_else(|| {
            panic!("expected {operation}'s MaxResults to declare a range maximum")
        });
        bounds.insert(
            operation,
            (min, max, member["traits"]["smithy.api#default"].as_i64()),
        );
    }
    assert!(
        !bounds.is_empty(),
        "expected the model to declare a paginated operation with a MaxResults member"
    );
    bounds
}

/// Every operation the model declares is routed to a handler of this crate, and answers
/// that operation's own modelled outcome rather than the protocol's routing failure.
///
/// The table is the model's whole operation surface, one row per operation shape, and each
/// row's expectation is derived from that operation's own input: an operation with a
/// `@required` member refuses a body that carries none with the modelled
/// `ValidationException`, naming a member it is missing, and one with no required member
/// reaches its handler — answering the scaffold's 501 while its file still carries the
/// stub marker, and 200 once it does not.
#[tokio::test]
async fn every_operation_the_model_declares_is_routed_to_a_handler_of_this_crate() {
    let model = model();
    let router = build_router();
    let operations = modeled_operations(&model);
    let validation_status = modeled_http_error(&model, "ValidationException");

    for operation in &operations {
        let source = handler_source(operation);
        assert!(
            source.contains(&format!("@op {operation}")),
            "case {operation:?}: expected the handler file to declare it with an @op header"
        );

        let required = required_input_members(&model, operation);
        let (status, code, body) = call(&router, operation, json!({})).await;

        assert!(
            status != 404 && !code.ends_with("UnknownOperationException"),
            "case {operation:?}: expected the operation the model declares to be routed, \
             got {status} {code} with {body}"
        );

        if required.is_empty() {
            let expected = if source.contains(SCAFFOLD_MARKER) { 501 } else { 200 };
            assert_eq!(
                status, expected,
                "case {operation:?}: expected the handler to answer {expected}, \
                 got {status} {code} with {body}"
            );
            continue;
        }

        assert_eq!(
            (status, code.ends_with("ValidationException")),
            (validation_status, true),
            "case {operation:?}: expected a request missing every required member to be \
             refused with {validation_status} ValidationException, got {status} {code} \
             with {body}"
        );
        // The validating walk reports the members it found missing; the clause it states
        // names one of this operation's own required members and nothing else.
        let message = message_of(&body);
        assert!(
            required
                .iter()
                .any(|member| message.contains(&lower_camel(member))),
            "case {operation:?}: expected the refusal to name one of the missing required \
             members {required:?}, got {message:?}"
        );
    }
}

/// The `ResourceType` the not-found class reports is one the model's own enum admits, for
/// every class of resource this service can fail to find.
///
/// The rows are the resource classes, not the operations: a user, a group, a membership
/// and the identity store itself, each probed through the operation that reads it. The
/// store row is why this suite is cross-service — a store is missing only once IAM
/// Identity Center has answered which stores the account holds.
#[tokio::test]
async fn the_resource_type_a_not_found_reports_is_one_the_model_admits() {
    let model = model();
    let admitted = enum_values(&model, "ResourceType");
    let not_found_status = identitystore::service::error_status(
        "ResourceNotFoundException",
        modeled_http_error(&model, "ResourceNotFoundException"),
    );

    // The classes the crate names by hand are the model's own values.
    for kind in [
        identitystore::state::ResourceKind::User,
        identitystore::state::ResourceKind::Group,
        identitystore::state::ResourceKind::Membership,
    ] {
        assert!(
            admitted.contains(kind.resource_type()),
            "case {kind:?}: expected the ResourceType it reports to be one the model \
             admits, got {:?} of {admitted:?}",
            kind.resource_type()
        );
    }

    let router = build_router();
    // Asking Identity Center for the account's instances is how a client learns which
    // identity store it has — and it settles the account's enablement, so from here a
    // store no instance has is a store the account does not hold.
    let instances = identity_center(&router, "ListInstances", json!({}), "a decided account").await;
    let account_stores: Vec<String> = instances["Instances"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|instance| instance["IdentityStoreId"].as_str().map(str::to_string))
        .collect();
    assert!(
        account_stores.contains(&SEED_STORE.to_string()),
        "setup: expected Identity Center to report the seeded store, got {account_stores:?}"
    );

    // (case, operation, request, the ResourceType the answer must name)
    let rows: [(&str, &str, Value, &str); 5] = [
        (
            "a user id no store minted",
            "DescribeUser",
            json!({ "IdentityStoreId": SEED_STORE, "UserId": UNMINTED_ID }),
            "USER",
        ),
        (
            "a group id no store minted",
            "DescribeGroup",
            json!({ "IdentityStoreId": SEED_STORE, "GroupId": UNMINTED_ID }),
            "GROUP",
        ),
        (
            "a membership id no store minted",
            "DescribeGroupMembership",
            json!({ "IdentityStoreId": SEED_STORE, "MembershipId": UNMINTED_ID }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a store the account does not have, read",
            "DescribeUser",
            json!({ "IdentityStoreId": UNHELD_STORE, "UserId": UNMINTED_ID }),
            "IDENTITY_STORE",
        ),
        (
            "a store the account does not have, listed",
            "ListGroups",
            json!({ "IdentityStoreId": UNHELD_STORE }),
            "IDENTITY_STORE",
        ),
    ];

    for (case, operation, request, expected) in rows {
        assert!(
            admitted.contains(expected),
            "case {case:?}: expected {expected:?} to be a ResourceType the model admits, \
             got {admitted:?}"
        );
        let body = refused(
            &router,
            operation,
            request,
            not_found_status,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str(),
            Some(expected),
            "case {case:?}: expected the ResourceType {expected:?}, got {body}"
        );
    }
}

/// The page-size bounds every listing applies are the model's own, and the crate's shared
/// constants are the values the model states.
///
/// The rows span the listings and, for each, both published bounds of the modelled range
/// and one past either: a size inside the range is served, one outside it is a
/// `ValidationException` — whichever listing was asked, because the bound belongs to the
/// member and not to the operation.
#[tokio::test]
async fn the_page_size_bounds_every_listing_applies_are_the_models_own() {
    let model = model();
    let bounds = page_size_bounds(&model);
    let validation_status = modeled_http_error(&model, "ValidationException");

    let (min, max) = {
        let ranges: BTreeSet<(i64, i64)> = bounds.values().map(|(min, max, _)| (*min, *max)).collect();
        assert_eq!(
            ranges.len(),
            1,
            "expected every listing's MaxResults to carry one range, got {ranges:?}"
        );
        *ranges.iter().next().expect("one range")
    };
    assert_eq!(
        i64::from(identitystore::state::MAX_MAX_RESULTS),
        max,
        "expected the largest page the service answers to be the modelled maximum, \
         got {}",
        identitystore::state::MAX_MAX_RESULTS
    );
    for (operation, (_, _, declared_default)) in &bounds {
        let Some(declared_default) = declared_default else {
            continue;
        };
        assert_eq!(
            i64::from(identitystore::state::DEFAULT_MAX_RESULTS),
            *declared_default,
            "case {operation:?}: expected the default page size to be the one its \
             MaxResults member declares, got {}",
            identitystore::state::DEFAULT_MAX_RESULTS
        );
    }

    let router = build_router();
    // One request per listing that is addressed by nothing but the store, so every row
    // reaches a handler: the bound is the member's, so the rows differ in the listing and
    // in the size, not in what else the request carries.
    let listings: [(&str, Value); 3] = [
        ("ListIdentityStores", json!({})),
        ("ListGroups", json!({ "IdentityStoreId": SEED_STORE })),
        ("ListUsers", json!({ "IdentityStoreId": SEED_STORE })),
    ];
    for (operation, request) in &listings {
        for size in [min, max] {
            let mut body = request.clone();
            body["MaxResults"] = json!(size);
            let case = format!("{operation} at MaxResults {size}");
            served(&router, operation, body, &case).await;
        }
        for size in [min - 1, max + 1] {
            let mut body = request.clone();
            body["MaxResults"] = json!(size);
            let case = format!("{operation} at MaxResults {size}");
            refused(
                &router,
                operation,
                body,
                validation_status,
                "ValidationException",
                &case,
            )
            .await;
        }
    }
}

/// The members an operation's input shape declares, as the model names them.
fn input_members(model: &Value, operation: &str) -> BTreeSet<String> {
    let input = short_name(
        shape(model, operation)["input"]["target"]
            .as_str()
            .unwrap_or_else(|| panic!("expected {operation} to declare an input shape")),
    )
    .to_string();
    shape(model, &input)["members"]
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether `got` carries everything `sent` does: equality for scalars, and for an object
/// every key of `sent` present in `got` with a value that in turn contains it.
///
/// Containment rather than equality because a response also carries what the service
/// MINTED (the ids, the revision, the audit members) and the defaults the model gives a
/// member the request left out.
fn contains(sent: &Value, got: &Value) -> bool {
    match (sent, got) {
        (Value::Object(sent), Value::Object(got)) => sent
            .iter()
            .all(|(key, value)| got.get(key).is_some_and(|actual| contains(value, actual))),
        (Value::Array(sent), Value::Array(got)) => {
            sent.len() == got.len()
                && sent
                    .iter()
                    .zip(got)
                    .all(|(value, actual)| contains(value, actual))
        }
        _ => sent == got,
    }
}

/// One value for every member `CreateUser` declares except the store it is addressed to,
/// each inside the constraints its own shape states
/// (`smithy_query(query="closure",operation="CreateUser")`): the strings admit letters,
/// marks, symbols, numbers, punctuation and blanks; `Emails`, `Addresses`, `PhoneNumbers`
/// and `Roles` hold exactly one element (length 1..1); `Photos` holds up to three, each
/// with the `Value` its shape requires; and `Extensions` maps an extension name to a
/// document.
///
/// The member set is asserted against the model's own, so a refreshed model that declares
/// one more profile member fails the case rather than leaving it untested.
fn full_profile_request() -> Map<String, Value> {
    let full = json!({
        "UserName": "model-surface-full-profile",
        "Name": {
            "Formatted": "Dr. Ada G. Lovelace M.D.",
            "FamilyName": "Lovelace",
            "GivenName": "Ada",
            "MiddleName": "Gordon",
            "HonorificPrefix": "Dr.",
            "HonorificSuffix": "M.D."
        },
        "DisplayName": "Ada Lovelace",
        "NickName": "Ada",
        "ProfileUrl": "https://example.invalid/ada",
        "Emails": [{ "Value": "ada@example.invalid", "Type": "Work", "Primary": true }],
        "Addresses": [{
            "StreetAddress": "12 Analytical Engine Way",
            "Locality": "London",
            "Region": "England",
            "PostalCode": "NW1",
            "Country": "GB",
            "Formatted": "12 Analytical Engine Way, London",
            "Type": "Home",
            "Primary": true
        }],
        "PhoneNumbers": [{ "Value": "+1 (800) 123-4567", "Type": "Mobile", "Primary": true }],
        "UserType": "Employee",
        "Title": "Analyst",
        "PreferredLanguage": "en-us",
        "Locale": "en-GB",
        "Timezone": "Europe/London",
        "Photos": [
            { "Value": "https://example.invalid/ada.png", "Type": "photo", "Display": "Ada", "Primary": true },
            { "Value": "https://example.invalid/ada-2.png" }
        ],
        "Website": "https://example.invalid/blog",
        "Birthdate": "1815-12-10",
        "Roles": [{ "Value": "Researcher", "Type": "Work", "Primary": true }]
    });
    let mut members = full.as_object().cloned().expect("an object");
    let mut extensions = Map::new();
    extensions.insert(
        EXTENSION_NAME.to_string(),
        json!({ "Department": "Research", "Headcount": 12 }),
    );
    members.insert("Extensions".to_string(), Value::Object(extensions));
    members
}

/// Every member the model declares on a user profile is stored by `CreateUser` and
/// reported by `DescribeUser`.
///
/// The request fills the model's whole `CreateUserRequest` member set — asserted equal to
/// it, so the case covers the refreshed model rather than the members it was written
/// against — and every value sent must come back from the read, beside every member
/// `DescribeUserResponse` marks `@required`. So a member a model refresh adds is served
/// end to end, not dropped on the way into or out of the store.
#[tokio::test]
async fn every_user_profile_member_the_model_declares_round_trips() {
    let model = model();
    let case = "a full user profile";

    let mut declared = input_members(&model, "CreateUser");
    assert!(
        declared.remove("IdentityStoreId"),
        "expected CreateUser to be addressed by an IdentityStoreId, got {declared:?}"
    );
    let sent = full_profile_request();
    let filled: BTreeSet<String> = sent.keys().cloned().collect();
    assert_eq!(
        filled, declared,
        "case {case:?}: expected the request to fill every profile member the model \
         declares: missing {:?}, extra {:?}",
        declared.difference(&filled).collect::<Vec<_>>(),
        filled.difference(&declared).collect::<Vec<_>>()
    );

    let router = build_router();
    let mut request = Map::new();
    request.insert("IdentityStoreId".to_string(), json!(SEED_STORE));
    for (member, value) in &sent {
        request.insert(member.clone(), value.clone());
    }
    let created = served(&router, "CreateUser", Value::Object(request), case).await;
    let user_id = created["UserId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a UserId, got {created}"))
        .to_string();

    // The extensions a read reports are the ones it ASKS for, which is what that member
    // is for, so the read names the extension the create stored.
    let described = served(
        &router,
        "DescribeUser",
        json!({
            "IdentityStoreId": SEED_STORE,
            "UserId": user_id,
            "Extensions": [EXTENSION_NAME],
        }),
        case,
    )
    .await;

    for (member, value) in &sent {
        let actual = described.get(member).unwrap_or(&Value::Null);
        assert!(
            contains(value, actual),
            "case {case:?}: expected {member} to come back as {value}, got {actual}"
        );
    }
    for member in shape(&model, "DescribeUserResponse")["members"]
        .as_object()
        .expect("the response shape carries members")
        .iter()
        .filter(|(_, member)| member["traits"].get("smithy.api#required").is_some())
        .map(|(name, _)| name.clone())
    {
        assert!(
            described.get(&member).is_some_and(|value| !value.is_null()),
            "case {case:?}: expected the read to report the required member {member:?}, \
             got {described}"
        );
    }
}
