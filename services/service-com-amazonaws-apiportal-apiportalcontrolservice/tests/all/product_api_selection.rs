// SPDX-License-Identifier: CC0-1.0

//! `apiSelectionConfiguration` and the product status it drives, plus the ownership
//! filters `ListProducts` and `GetProduct` apply — all over HTTP through the same
//! router the emulator serves with.
//!
//! The rules under test, every one of them stated by this service's own model:
//!
//! 1. `ruleFilters` are "required for RULE_BASED, rejected for CUSTOM", so a
//!    configuration that breaks either half is the modelled `BadRequestException`
//!    (400) naming the member at fault, and nothing is stored for it.
//! 2. "The selection mode can't be changed after the product is created", so an update
//!    that names a different mode is that same 400 and the stored mode stands.
//! 3. `ACTIVE` is "the only status a CUSTOM product reports"; `IN_PROGRESS` means
//!    "ruleFilters changed and membership is still being evaluated". `ruleFilters` are
//!    "replaced as a whole on update, not merged", so a replacement that selects
//!    different APIs owes another evaluation and a replacement that selects the same
//!    ones owes none.
//! 4. `resourceOwner` chooses "whose products to return", defaulting to `ALL` — or to
//!    `SELF` when `managedApiId` is supplied — and `managedApiId` returns "only
//!    Products this Managed API is associated with".
//! 5. `resourceOwnerAccountId` reads "a product shared with you by another account".

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_apiportal_apiportalcontrolservice::ops::types::{
    API_SELECTION_MODE_VALUES, PRODUCT_STATUS_VALUES, RESOURCE_OWNER_VALUES,
};
use service_com_amazonaws_apiportal_apiportalcontrolservice::state::{
    API_SELECTION_MODE_CUSTOM, API_SELECTION_MODE_RULE_BASED, PRODUCT_STATUS_ACTIVE,
    PRODUCT_STATUS_IN_PROGRESS, RESOURCE_OWNER_ALL, RESOURCE_OWNER_OTHER_ACCOUNTS,
    RESOURCE_OWNER_SELF, RULE_EVALUATION_MILLIS,
};

use crate::product_harness::{Answer, Emulator, ACCOUNT_ID};

/// How long a case waits for a change the clock owes before giving up: far above the
/// emulator's own evaluation span ([`RULE_EVALUATION_MILLIS`]), so a loaded machine is
/// not a failure.
fn settle_deadline() -> Instant {
    let span = u64::try_from(RULE_EVALUATION_MILLIS).unwrap_or(3_000);
    Instant::now() + Duration::from_millis(span * 20)
}

/// A `ruleFilters` list selecting one host.
fn host_filters(host: &str) -> Value {
    json!([{"key": "HOST", "values": [host]}])
}

/// Assert a create succeeded, and hand back its `productId`.
fn created_id(case: &str, created: &Answer) -> String {
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateProduct must answer 201, got {} with {:?}",
        created.status, created.body
    );
    created.string("productId").to_string()
}

/// Poll the product until it reports `PRODUCT_STATUS_ACTIVE`, asserting that until then
/// it reports the transient `PRODUCT_STATUS_IN_PROGRESS` and never a third status.
async fn wait_until_active(emulator: &Emulator, case: &str, product_id: &str) {
    let deadline = settle_deadline();
    loop {
        let read = emulator.get_product(product_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetProduct must answer 200, got {} with {:?}",
            read.status, read.body
        );
        let status = read.string("status");
        if status == PRODUCT_STATUS_ACTIVE {
            return;
        }
        assert_eq!(
            status, PRODUCT_STATUS_IN_PROGRESS,
            "case {case:?}: before its evaluation comes due the product must report \
             {PRODUCT_STATUS_IN_PROGRESS}, got {:?}",
            read.body
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the evaluation must come due on its own, and the product still \
             reports {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One create the selection-rule table drives.
struct SelectionCase {
    case: &'static str,
    configuration: Value,
    status: u16,
    /// The wire error code a refusal must carry, or `None` for a create that succeeds.
    /// A configuration the MODEL's own bounds reject is refused at the boundary, which
    /// words it as `ValidationException`; one only the service's documented rule
    /// rejects is the operation's own `BadRequestException`.
    error_code: Option<&'static str>,
    /// A member name the refusal must name, so the caller can fix the request.
    names_member: Option<&'static str>,
}

fn selection_cases() -> Vec<SelectionCase> {
    vec![
        SelectionCase {
            case: "no apiSelectionConfiguration at all",
            configuration: Value::Null,
            status: 201,
            error_code: None,
            names_member: None,
        },
        SelectionCase {
            case: "CUSTOM with no ruleFilters",
            configuration: json!({"apiSelectionMode": "CUSTOM"}),
            status: 201,
            error_code: None,
            names_member: None,
        },
        SelectionCase {
            case: "RULE_BASED with one ruleFilter",
            configuration: json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            status: 201,
            error_code: None,
            names_member: None,
        },
        SelectionCase {
            case: "RULE_BASED with no ruleFilters, which it requires",
            configuration: json!({"apiSelectionMode": "RULE_BASED"}),
            status: 400,
            error_code: Some("BadRequestException"),
            names_member: Some("ruleFilters"),
        },
        SelectionCase {
            case: "CUSTOM with ruleFilters, which it rejects",
            configuration: json!({
                "apiSelectionMode": "CUSTOM",
                "ruleFilters": host_filters("api.example.com"),
            }),
            status: 400,
            error_code: Some("BadRequestException"),
            names_member: Some("ruleFilters"),
        },
        SelectionCase {
            case: "RULE_BASED with an empty ruleFilters list, below the list's minimum",
            configuration: json!({"apiSelectionMode": "RULE_BASED", "ruleFilters": []}),
            status: 400,
            error_code: None,
            names_member: None,
        },
        SelectionCase {
            case: "a selection mode the model does not declare",
            configuration: json!({"apiSelectionMode": "EVERYTHING"}),
            status: 400,
            error_code: None,
            names_member: None,
        },
    ]
}

/// `ruleFilters` are required for `RULE_BASED` and rejected for `CUSTOM`: a create that
/// breaks either half is refused, names the member at fault, and stores nothing.
#[tokio::test]
async fn the_rule_filter_requirement_is_enforced_on_create() {
    let emulator = Emulator::new();
    let mut accepted = 0usize;

    for SelectionCase {
        case,
        configuration,
        status,
        error_code,
        names_member,
    } in selection_cases()
    {
        let mut request = json!({"displayName": case});
        if !configuration.is_null() {
            request["apiSelectionConfiguration"] = configuration;
        }
        let answer = emulator.create_product(&request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if let Some(error_code) = error_code {
            assert_eq!(
                answer.error_code(),
                error_code,
                "case {case:?}: expected {error_code}, got {:?}",
                answer.body
            );
        }
        if let Some(member) = names_member {
            assert!(
                answer.message().contains(member),
                "case {case:?}: the refusal must name {member}, got {:?}",
                answer.message()
            );
        }
        if status == 201 {
            accepted += 1;
        }
    }

    // Exactly the accepted creates are in the store: a refused one wrote nothing.
    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.items().len(),
        accepted,
        "only the accepted creates may be stored, got {:?}",
        listed.body
    );
}

/// The selection mode can't be changed after the product is created: an update naming a
/// different mode is refused and the stored mode stands; one naming the same mode is
/// accepted.
#[tokio::test]
async fn the_selection_mode_cannot_change_after_the_create() {
    let emulator = Emulator::new();

    // (case, the create's configuration, the update's configuration, expected status)
    let cases: Vec<(&str, Value, Value, u16)> = vec![
        (
            "CUSTOM asked to become RULE_BASED",
            json!({"apiSelectionMode": "CUSTOM"}),
            json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            400,
        ),
        (
            "RULE_BASED asked to become CUSTOM",
            json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            json!({"apiSelectionMode": "CUSTOM"}),
            400,
        ),
        (
            "a product created with no configuration, asked to become RULE_BASED",
            Value::Null,
            json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            400,
        ),
        (
            "CUSTOM restated as CUSTOM",
            json!({"apiSelectionMode": "CUSTOM"}),
            json!({"apiSelectionMode": "CUSTOM"}),
            200,
        ),
        (
            "RULE_BASED restated with the same ruleFilters",
            json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            json!({
                "apiSelectionMode": "RULE_BASED",
                "ruleFilters": host_filters("api.example.com"),
            }),
            200,
        ),
    ];

    for (case, create_configuration, update_configuration, status) in cases {
        let mut request = json!({"displayName": case});
        if !create_configuration.is_null() {
            request["apiSelectionConfiguration"] = create_configuration;
        }
        let created = emulator.create_product(&request).await;
        let product_id = created_id(case, &created);
        let stored_mode = created.body["apiSelectionConfiguration"]["apiSelectionMode"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let updated = emulator
            .update_product(
                &product_id,
                &json!({"apiSelectionConfiguration": update_configuration}),
            )
            .await;
        assert_eq!(
            updated.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            updated.status, updated.body
        );
        if status == 400 {
            assert_eq!(
                updated.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {:?}",
                updated.body
            );
            assert!(
                updated.message().contains("apiSelectionMode"),
                "case {case:?}: the refusal must name apiSelectionMode, got {:?}",
                updated.message()
            );
        }

        // Either way the stored mode is the one the create settled.
        let read = emulator.get_product(&product_id).await;
        assert_eq!(
            read.body["apiSelectionConfiguration"]["apiSelectionMode"]
                .as_str()
                .unwrap_or_default(),
            stored_mode,
            "case {case:?}: the stored selection mode must still be {stored_mode:?}, got {:?}",
            read.body
        );
    }
}

/// A product created with no configuration takes the mode that needs no `ruleFilters`,
/// which is the only mode that can report `ACTIVE` from the create onward.
#[tokio::test]
async fn a_product_created_with_no_configuration_is_custom_and_active() {
    let emulator = Emulator::new();
    let created = emulator
        .create_product(&json!({"displayName": "Payments API"}))
        .await;
    let product_id = created_id("no configuration", &created);

    for (case, answer) in [("the create", &created), ("the read", &{
        emulator.get_product(&product_id).await
    })] {
        assert_eq!(
            answer.body["apiSelectionConfiguration"]["apiSelectionMode"]
                .as_str()
                .unwrap_or_default(),
            API_SELECTION_MODE_CUSTOM,
            "case {case:?}: the effective mode must be {API_SELECTION_MODE_CUSTOM}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("status"),
            PRODUCT_STATUS_ACTIVE,
            "case {case:?}: a {API_SELECTION_MODE_CUSTOM} product must report \
             {PRODUCT_STATUS_ACTIVE}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["failureReason"],
            Value::Null,
            "case {case:?}: failureReason is present only when status is FAILED, got {:?}",
            answer.body
        );
    }
}

/// A `RULE_BASED` product owes a membership evaluation: it reports `IN_PROGRESS` until
/// that falls due on its own, and `ACTIVE` from then on. Replacing `ruleFilters` with a
/// different list owes another one; replacing them with the same list owes none.
#[tokio::test]
async fn replacing_rule_filters_owes_another_evaluation() {
    let emulator = Emulator::new();
    let created = emulator
        .create_product(&json!({
            "displayName": "Rule based",
            "apiSelectionConfiguration": {
                "apiSelectionMode": API_SELECTION_MODE_RULE_BASED,
                "ruleFilters": host_filters("api.example.com"),
            },
        }))
        .await;
    let product_id = created_id("the create", &created);
    assert_eq!(
        created.string("status"),
        PRODUCT_STATUS_IN_PROGRESS,
        "a RULE_BASED create must report {PRODUCT_STATUS_IN_PROGRESS}, got {:?}",
        created.body
    );

    wait_until_active(&emulator, "the create's evaluation", &product_id).await;

    // The same ruleFilters select the same APIs, so nothing is re-evaluated.
    let unchanged = emulator
        .update_product(
            &product_id,
            &json!({
                "apiSelectionConfiguration": {
                    "apiSelectionMode": API_SELECTION_MODE_RULE_BASED,
                    "ruleFilters": host_filters("api.example.com"),
                },
            }),
        )
        .await;
    assert_eq!(
        unchanged.status, 200,
        "restating the same ruleFilters must answer 200, got {} with {:?}",
        unchanged.status, unchanged.body
    );
    assert_eq!(
        unchanged.string("status"),
        PRODUCT_STATUS_ACTIVE,
        "restating the same ruleFilters owes no evaluation, so the product stays \
         {PRODUCT_STATUS_ACTIVE}, got {:?}",
        unchanged.body
    );

    // A different list is a change, so membership is evaluated again.
    let replaced = emulator
        .update_product(
            &product_id,
            &json!({
                "apiSelectionConfiguration": {
                    "apiSelectionMode": API_SELECTION_MODE_RULE_BASED,
                    "ruleFilters": host_filters("api.example.org"),
                },
            }),
        )
        .await;
    assert_eq!(
        replaced.status, 200,
        "replacing the ruleFilters must answer 200, got {} with {:?}",
        replaced.status, replaced.body
    );
    assert_eq!(
        replaced.string("status"),
        PRODUCT_STATUS_IN_PROGRESS,
        "replacing the ruleFilters must put the product back to \
         {PRODUCT_STATUS_IN_PROGRESS}, got {:?}",
        replaced.body
    );
    assert_eq!(
        replaced.body["apiSelectionConfiguration"]["ruleFilters"],
        host_filters("api.example.org"),
        "ruleFilters are replaced as a whole, not merged, got {:?}",
        replaced.body
    );

    wait_until_active(&emulator, "the update's evaluation", &product_id).await;
}

/// `resourceOwner` chooses whose products to report. This emulator presents exactly one
/// account and holds only its products, so `SELF` and `ALL` report all of them and
/// `OTHER_ACCOUNTS` — products another account shared — reports none.
#[tokio::test]
async fn resource_owner_chooses_whose_products_are_reported() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        let answer = emulator
            .create_product(&json!({"displayName": format!("Product {index}")}))
            .await;
        created.push(created_id("the fixtures", &answer));
    }

    // (case, query, expected HTTP status, expected ids)
    let cases: Vec<(&str, String, u16, Vec<String>)> = vec![
        ("no resourceOwner, which defaults to ALL", String::new(), 200, created.clone()),
        (
            "resourceOwner=ALL",
            format!("resourceOwner={RESOURCE_OWNER_ALL}"),
            200,
            created.clone(),
        ),
        (
            "resourceOwner=SELF",
            format!("resourceOwner={RESOURCE_OWNER_SELF}"),
            200,
            created.clone(),
        ),
        (
            "resourceOwner=OTHER_ACCOUNTS, of which this account has none",
            format!("resourceOwner={RESOURCE_OWNER_OTHER_ACCOUNTS}"),
            200,
            Vec::new(),
        ),
        (
            "a resourceOwner the model does not declare",
            "resourceOwner=SOMEONE_ELSE".to_string(),
            400,
            Vec::new(),
        ),
    ];

    for (case, query, status, expected) in cases {
        let answer = emulator.list_products(&query).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        if status == 200 {
            assert_eq!(
                answer.item_ids(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.next_token(),
                None,
                "case {case:?}: one page holds every match, so there is no nextToken, got {:?}",
                answer.body
            );
        }
    }
}

/// `managedApiId` reports only the products that Managed API is a member of. The
/// emulator holds no Managed API records yet — every operation that would create or
/// associate one is still scaffolded — so no product has a member API and the filter
/// selects none.
#[tokio::test]
async fn the_managed_api_filter_selects_products_that_api_belongs_to() {
    let emulator = Emulator::new();
    let created = emulator
        .create_product(&json!({"displayName": "Payments API"}))
        .await;
    let product_id = created_id("the fixture", &created);

    // Ids `ManagedApiId`'s pattern and length admit, none of them a Managed API the
    // store holds: both published length bounds and one in between.
    for managed_api_id in ["aaaaaaaaaa", "m000000001", &"z".repeat(30)] {
        let answer = emulator
            .list_products(&format!("managedApiId={managed_api_id}"))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {managed_api_id:?}: ListProducts must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.items().is_empty(),
            "case {managed_api_id:?}: no product has a member Managed API, so the filter must \
             select none, got {:?}",
            answer.body
        );
    }

    // The filter narrows; it does not break the unfiltered listing.
    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.item_ids(),
        vec![product_id],
        "the unfiltered listing must still report the product, got {:?}",
        listed.body
    );
}

/// `resourceOwnerAccountId` reads a product another account shared. This emulator holds
/// one account's products, so its own account id reads the product and any other
/// account id is the modelled 404.
#[tokio::test]
async fn a_product_of_another_account_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_product(&json!({"displayName": "Payments API"}))
        .await;
    let product_id = created_id("the fixture", &created);

    let own = emulator
        .call(
            "GET",
            &format!("/products/{product_id}?resourceOwnerAccountId={ACCOUNT_ID}"),
            &Value::Null,
        )
        .await;
    assert_eq!(
        own.status, 200,
        "this account's own id must read the product, got {} with {:?}",
        own.status, own.body
    );
    assert_eq!(
        own.string("productId"),
        product_id,
        "the read must report the product asked for, got {:?}",
        own.body
    );

    // Account ids `AccountId`'s `^[0-9]{12}$` admits, none of them this emulator's.
    for account_id in ["111111111111", "999999999999", "000000000001"] {
        let answer = emulator
            .call(
                "GET",
                &format!("/products/{product_id}?resourceOwnerAccountId={account_id}"),
                &Value::Null,
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {account_id:?}: a product of another account must answer 404, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {account_id:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

/// Every status and mode this service reports is one the model declares.
///
/// The published value lists come straight from the model
/// (`smithy_query(query="shape", shape="ProductStatus" | "ApiSelectionMode" |
/// "ResourceOwner")`); this pins the constants the handlers report against them, so a
/// typo cannot reach the wire.
#[test]
fn the_values_this_service_reports_are_the_modelled_ones() {
    for status in [PRODUCT_STATUS_ACTIVE, PRODUCT_STATUS_IN_PROGRESS] {
        assert!(
            PRODUCT_STATUS_VALUES.contains(&status),
            "case {status:?}: a reported status must be a ProductStatus the model declares, \
             which are {PRODUCT_STATUS_VALUES:?}"
        );
    }
    for mode in [API_SELECTION_MODE_CUSTOM, API_SELECTION_MODE_RULE_BASED] {
        assert!(
            API_SELECTION_MODE_VALUES.contains(&mode),
            "case {mode:?}: a selection mode must be an ApiSelectionMode the model declares, \
             which are {API_SELECTION_MODE_VALUES:?}"
        );
    }
    for owner in [
        RESOURCE_OWNER_ALL,
        RESOURCE_OWNER_SELF,
        RESOURCE_OWNER_OTHER_ACCOUNTS,
    ] {
        assert!(
            RESOURCE_OWNER_VALUES.contains(&owner),
            "case {owner:?}: a resource owner must be a ResourceOwner the model declares, \
             which are {RESOURCE_OWNER_VALUES:?}"
        );
    }
}
