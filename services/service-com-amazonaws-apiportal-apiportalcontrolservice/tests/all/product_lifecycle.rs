// SPDX-License-Identifier: CC0-1.0

//! The product lifecycle end to end over HTTP: `CreateProduct` -> `GetProduct` ->
//! `ListProducts` -> `UpdateProduct` -> `DeleteProduct`, driven through the same router
//! the emulator serves with.
//!
//! The rules under test, all stated by this service's own model:
//!
//! 1. `CreateProduct` mints a `productId` inside `ProductId`'s `@length(10, 30)` and
//!    `^[a-z0-9]+$`, builds the `productArn` `ProductArn`'s pattern describes, and
//!    PERSISTS the product: a later `GetProduct` answers the same record, member for
//!    member, and `ListProducts` reports a `ProductSummary` for it.
//! 2. `UpdateProduct` "changes only the fields supplied in the request": an absent
//!    member keeps its stored value, and the merged record is what the next read sees.
//! 3. `DeleteProduct` removes the product: the following `GetProduct` is the modelled
//!    `ResourceNotFoundException` (404) and `ListProducts` no longer reports it.
//! 4. `ListProducts` is `@paginated`: it honours `maxResults` (1..100) and round-trips
//!    `nextToken`, so walking the pages reports every product exactly once.
//!
//! Each table spans the class rather than one request: creates differ in name length,
//! description length and presence, tag count and selection mode (both published
//! `@length` bounds included); updates differ in which members they supply; pagination
//! is walked at both published `maxResults` bounds and in between.

use serde_json::{json, Value};

use crate::product_harness::{
    is_well_formed_product_arn, is_well_formed_product_id, string_of_length, Answer, Emulator,
    ACCOUNT_ID, DESCRIPTION_MAX, DISPLAY_NAME_MAX, DISPLAY_NAME_MIN, MAX_RESULTS_MAX,
    MAX_RESULTS_MIN, REGION,
};

/// `ProductStatus`' terminal value, from
/// `smithy_query(query="shape", shape="ProductStatus")`.
const ACTIVE: &str = "ACTIVE";

/// The status a product reports while its `ruleFilters` are still being evaluated.
const IN_PROGRESS: &str = "IN_PROGRESS";

/// One create the table drives, with what every later read of that product must report.
struct CreateCase {
    case: &'static str,
    request: Value,
    display_name: String,
    description: String,
    status: &'static str,
    tags: Value,
}

fn create_cases() -> Vec<CreateCase> {
    let longest_name = string_of_length(DISPLAY_NAME_MAX);
    let longest_description = string_of_length(DESCRIPTION_MAX);
    vec![
        CreateCase {
            case: "the shortest displayName the bound admits, and no description",
            request: json!({"displayName": "a"}),
            display_name: string_of_length(DISPLAY_NAME_MIN),
            description: String::new(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "the longest displayName the bound admits",
            request: json!({"displayName": longest_name}),
            display_name: longest_name.clone(),
            description: String::new(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "an ordinary product with a description",
            request: json!({
                "displayName": "Payments API",
                "description": "public payments product",
            }),
            display_name: "Payments API".to_string(),
            description: "public payments product".to_string(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "the longest description the bound admits",
            request: json!({
                "displayName": "Longest copy",
                "description": longest_description,
            }),
            display_name: "Longest copy".to_string(),
            description: longest_description.clone(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "the shortest description the bound admits, the empty string",
            request: json!({"displayName": "Empty copy", "description": ""}),
            display_name: "Empty copy".to_string(),
            description: String::new(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "a product carrying several tags",
            request: json!({
                "displayName": "Tagged",
                "tags": {"team": "payments", "tier": "gold"},
            }),
            display_name: "Tagged".to_string(),
            description: String::new(),
            status: ACTIVE,
            tags: json!({"team": "payments", "tier": "gold"}),
        },
        CreateCase {
            case: "a CUSTOM product, whose APIs are associated by hand",
            request: json!({
                "displayName": "Hand picked",
                "apiSelectionConfiguration": {"apiSelectionMode": "CUSTOM"},
            }),
            display_name: "Hand picked".to_string(),
            description: String::new(),
            status: ACTIVE,
            tags: Value::Null,
        },
        CreateCase {
            case: "a RULE_BASED product with one ruleFilter",
            request: json!({
                "displayName": "Rule based",
                "apiSelectionConfiguration": {
                    "apiSelectionMode": "RULE_BASED",
                    "ruleFilters": [{"key": "HOST", "values": ["api.example.com"]}],
                },
            }),
            display_name: "Rule based".to_string(),
            description: String::new(),
            status: IN_PROGRESS,
            tags: Value::Null,
        },
        CreateCase {
            case: "a RULE_BASED product with the most ruleFilters the bound admits",
            request: json!({
                "displayName": "Every key filtered",
                "apiSelectionConfiguration": {
                    "apiSelectionMode": "RULE_BASED",
                    // One filter per `RuleFilterKey`, which is also the list's
                    // modelled maximum of six
                    // (`smithy_query(query="shape", shape="RuleFilterList")`).
                    "ruleFilters": [
                        {"key": "RESOURCE_ARN", "values": ["arn:aws:execute-api:::abc/*"]},
                        {"key": "HOST", "values": ["api.example.com", "api.example.org"]},
                        {"key": "METHOD", "values": ["GET", "POST"]},
                        {"key": "PATH", "values": ["/payments"]},
                        {"key": "PORT", "values": ["443"]},
                        {"key": "ACCOUNT", "values": ["000000000000"]},
                    ],
                },
            }),
            display_name: "Every key filtered".to_string(),
            description: String::new(),
            status: IN_PROGRESS,
            tags: Value::Null,
        },
    ]
}

/// Assert a create succeeded, and hand back the `productId` it minted.
fn minted_product_id(case: &str, created: &Answer) -> String {
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateProduct must answer 201, got {} with {:?}",
        created.status, created.body
    );
    let product_id = created.string("productId").to_string();
    assert!(
        is_well_formed_product_id(&product_id),
        "case {case:?}: the minted productId must satisfy ProductId's pattern and length, \
         got {product_id:?}"
    );
    let product_arn = created.string("productArn");
    assert!(
        is_well_formed_product_arn(product_arn),
        "case {case:?}: the minted productArn must satisfy ProductArn's pattern, got \
         {product_arn:?}"
    );
    assert_eq!(
        product_arn,
        format!("arn:aws:apiportal:{REGION}:{ACCOUNT_ID}:products/{product_id}"),
        "case {case:?}: the ARN must carry the signed region, the emulator's account and the \
         minted id, got {product_arn:?}"
    );
    product_id
}

/// Create a product with `displayName` and hand back its id.
async fn create_named(emulator: &Emulator, display_name: &str) -> String {
    let created = emulator
        .create_product(&json!({"displayName": display_name}))
        .await;
    minted_product_id(display_name, &created)
}

/// Every create persists a product a later read answers from the store, member for
/// member — including the id and ARN the service minted for it.
#[tokio::test]
async fn a_created_product_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let mut seen_ids: Vec<String> = Vec::new();

    for CreateCase {
        case,
        request,
        display_name,
        description,
        status,
        tags,
    } in create_cases()
    {
        let created = emulator.create_product(&request).await;
        let product_id = minted_product_id(case, &created);
        assert!(
            !seen_ids.contains(&product_id),
            "case {case:?}: every product must get an id of its own, but {product_id:?} was \
             already minted for another product"
        );
        seen_ids.push(product_id.clone());

        assert_eq!(
            created.string("displayName"),
            display_name,
            "case {case:?}: CreateProduct must report the displayName it was given, got {:?}",
            created.body
        );
        assert_eq!(
            created.string("status"),
            status,
            "case {case:?}: CreateProduct must report status {status}, got {:?}",
            created.body
        );
        assert!(
            created.updated_at() > 0.0,
            "case {case:?}: CreateProduct must report an updatedAt, got {:?}",
            created.body
        );

        // The read answers from the store, so it must agree with the create on every
        // member — a canned or echoed body cannot.
        let read = emulator.get_product(&product_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetProduct must answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("productId"),
            product_id,
            "case {case:?}: GetProduct must report the id it was asked for, got {:?}",
            read.body
        );
        assert_eq!(
            read.string("productArn"),
            created.string("productArn"),
            "case {case:?}: GetProduct must report the stored ARN, got {:?}",
            read.body
        );
        assert_eq!(
            read.string("displayName"),
            display_name,
            "case {case:?}: GetProduct must report the stored displayName, got {:?}",
            read.body
        );
        assert_eq!(
            read.string("description"),
            description,
            "case {case:?}: GetProduct must report the stored description, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["tags"], tags,
            "case {case:?}: GetProduct must report the stored tags, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["apiSelectionConfiguration"], created.body["apiSelectionConfiguration"],
            "case {case:?}: GetProduct must report the stored apiSelectionConfiguration, got \
             {:?}",
            read.body
        );
        assert!(
            read.body["displayOrder"].is_object(),
            "case {case:?}: GetProduct must report a displayOrder, which its shape requires, \
             got {:?}",
            read.body
        );
        if status == ACTIVE {
            // An ACTIVE product owes the clock nothing, so its updatedAt is the instant
            // the create stored, read back unchanged.
            assert_eq!(
                read.updated_at(),
                created.updated_at(),
                "case {case:?}: GetProduct must report the stored updatedAt, got {:?} after \
                 {:?}",
                read.body,
                created.body
            );
        }

        // ListProducts reports the same product, computed from the same record.
        let listed = emulator.list_products("").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListProducts must answer 200, got {} with {:?}",
            listed.status, listed.body
        );
        let summary = listed
            .items()
            .into_iter()
            .find(|item| item["productId"].as_str() == Some(product_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListProducts must report the created product, got {:?}", listed.body)
            })
            .clone();
        assert_eq!(
            summary["displayName"].as_str().unwrap_or_default(),
            display_name,
            "case {case:?}: the summary must carry the stored displayName, got {summary:?}"
        );
        assert_eq!(
            summary["description"].as_str().unwrap_or_default(),
            description,
            "case {case:?}: the summary must carry the stored description, got {summary:?}"
        );
        assert_eq!(
            summary["productArn"].as_str().unwrap_or_default(),
            created.string("productArn"),
            "case {case:?}: the summary must carry the stored ARN, got {summary:?}"
        );
    }

    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.item_ids().len(),
        seen_ids.len(),
        "ListProducts must report every product the table created, got {:?}",
        listed.body
    );
}

/// What one update supplies, and what every member must read as afterwards.
struct UpdateCase {
    case: &'static str,
    request: Value,
    display_name: &'static str,
    description: &'static str,
    /// Whether the update supplied a `displayOrder`, which the read must then carry.
    overview_page_arn: Option<&'static str>,
}

/// A product page ARN inside `ProductPageArn`'s modelled pattern,
/// `^arn:aws[a-z\-]*:apiportal:[a-z0-9\-]+:[0-9]{12}:products/[a-z0-9]{10,30}/productpages/[a-z0-9]{10,30}$`
/// (`smithy_query(query="shape", shape="DisplayOrder")`).
const OVERVIEW_PAGE_ARN: &str =
    "arn:aws:apiportal:us-west-2:000000000000:products/p000000001/productpages/pp00000001";

fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "only the description",
            request: json!({"description": "updated copy"}),
            display_name: "Payments API",
            description: "updated copy",
            overview_page_arn: None,
        },
        UpdateCase {
            case: "only the displayName",
            request: json!({"displayName": "Payments"}),
            display_name: "Payments",
            description: "public payments product",
            overview_page_arn: None,
        },
        UpdateCase {
            case: "only the displayOrder",
            request: json!({"displayOrder": {"overviewPageArn": OVERVIEW_PAGE_ARN}}),
            display_name: "Payments API",
            description: "public payments product",
            overview_page_arn: Some(OVERVIEW_PAGE_ARN),
        },
        UpdateCase {
            case: "the displayName and the description together",
            request: json!({"displayName": "Payments", "description": "updated copy"}),
            display_name: "Payments",
            description: "updated copy",
            overview_page_arn: None,
        },
        UpdateCase {
            case: "the empty description, which the bound admits",
            request: json!({"description": ""}),
            display_name: "Payments API",
            description: "",
            overview_page_arn: None,
        },
        UpdateCase {
            case: "no member at all, so nothing changes",
            request: json!({}),
            display_name: "Payments API",
            description: "public payments product",
            overview_page_arn: None,
        },
    ]
}

/// An update changes only the members it supplies, writes the merge back, and the next
/// read answers the merged record.
#[tokio::test]
async fn an_update_changes_only_the_members_it_supplies() {
    let emulator = Emulator::new();

    for UpdateCase {
        case,
        request,
        display_name,
        description,
        overview_page_arn,
    } in update_cases()
    {
        let created = emulator
            .create_product(&json!({
                "displayName": "Payments API",
                "description": "public payments product",
                "tags": {"team": "payments"},
            }))
            .await;
        let product_id = minted_product_id(case, &created);

        let updated = emulator.update_product(&product_id, &request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateProduct must answer 200, got {} with {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("displayName"),
            display_name,
            "case {case:?}: expected displayName {display_name:?}, got {:?}",
            updated.body
        );
        assert_eq!(
            updated.string("description"),
            description,
            "case {case:?}: expected description {description:?}, got {:?}",
            updated.body
        );
        assert_eq!(
            updated.string("productArn"),
            created.string("productArn"),
            "case {case:?}: an update must not change the product's ARN, got {:?}",
            updated.body
        );
        assert!(
            updated.updated_at() >= created.updated_at(),
            "case {case:?}: updatedAt must not move backwards, got {:?} after {:?}",
            updated.body,
            created.body
        );

        // The merge is in the STORE, not only in the answer: the next read sees it, and
        // the members the update never named are still the ones the create stored.
        let read = emulator.get_product(&product_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetProduct must answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("displayName"),
            display_name,
            "case {case:?}: the merged displayName must persist, got {:?}",
            read.body
        );
        assert_eq!(
            read.string("description"),
            description,
            "case {case:?}: the merged description must persist, got {:?}",
            read.body
        );
        assert_eq!(
            read.updated_at(),
            updated.updated_at(),
            "case {case:?}: the read must report the updatedAt the update stored, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["tags"],
            json!({"team": "payments"}),
            "case {case:?}: an update that names no tags must leave them alone, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["displayOrder"]["overviewPageArn"].as_str(),
            overview_page_arn,
            "case {case:?}: expected overviewPageArn {overview_page_arn:?}, got {:?}",
            read.body
        );
    }
}

/// A deleted product is gone from every read: the `GetProduct` that follows is the
/// modelled 404, `ListProducts` no longer reports it, and the products beside it are
/// untouched.
#[tokio::test]
async fn a_deleted_product_is_gone_from_every_read() {
    let emulator = Emulator::new();
    let first = create_named(&emulator, "First").await;
    let deleted = create_named(&emulator, "Deleted").await;
    let last = create_named(&emulator, "Last").await;

    let answer = emulator.delete_product(&deleted).await;
    assert_eq!(
        answer.status, 204,
        "DeleteProduct must answer 204, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "a 204 carries no body, got {:?}",
        answer.body
    );

    let read = emulator.get_product(&deleted).await;
    assert_eq!(
        read.status, 404,
        "GetProduct on a deleted product must answer 404, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.error_code(),
        "ResourceNotFoundException",
        "GetProduct on a deleted product must answer ResourceNotFoundException, got {:?}",
        read.body
    );
    assert!(
        read.message().contains(&deleted),
        "the refusal must name the product that was asked for, got {:?}",
        read.message()
    );

    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.item_ids(),
        vec![first.clone(), last.clone()],
        "ListProducts must report exactly the products that remain, got {:?}",
        listed.body
    );

    // `DeleteProduct` models `ResourceNotFoundException`, so deleting it again is that
    // 404 rather than a second 204.
    let again = emulator.delete_product(&deleted).await;
    assert_eq!(
        again.status, 404,
        "deleting a product the store does not hold must answer 404, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "ResourceNotFoundException",
        "deleting a product the store does not hold must answer ResourceNotFoundException, \
         got {:?}",
        again.body
    );

    // The products beside it are untouched.
    for (case, product_id) in [("the first", &first), ("the last", &last)] {
        let read = emulator.get_product(product_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a delete must not disturb the other products, got {} with {:?}",
            read.status, read.body
        );
    }
}

/// Walking the pages at `page_size` reports every product exactly once, in one order.
async fn walk_pages(emulator: &Emulator, case: &str, page_size: Option<i64>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A generous ceiling on the walk, so a token that never ends fails instead of
    // looping forever.
    for _ in 0..64 {
        let mut query: Vec<String> = Vec::new();
        if let Some(page_size) = page_size {
            query.push(format!("maxResults={page_size}"));
        }
        if let Some(token) = token.as_deref() {
            query.push(format!("nextToken={token}"));
        }
        let page = emulator.list_products(&query.join("&")).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListProducts must answer 200, got {} with {:?}",
            page.status, page.body
        );
        let ids = page.item_ids();
        if let Some(page_size) = page_size {
            assert!(
                i64::try_from(ids.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page must hold at most maxResults items, got {} in {:?}",
                ids.len(),
                page.body
            );
        }
        for id in ids {
            assert!(
                !seen.contains(&id),
                "case {case:?}: a walk must not repeat a product, but {id:?} came back twice"
            );
            seen.push(id);
        }
        token = page.next_token();
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case:?}: the walk never ran out of pages, having seen {seen:?}");
}

/// `ListProducts` honours `maxResults` and round-trips `nextToken`: every page size
/// reports every product exactly once, in the same order.
#[tokio::test]
async fn every_page_size_walks_the_whole_store_exactly_once() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        created.push(create_named(&emulator, &format!("Product {index}")).await);
    }

    // The published `@range` bounds of `maxResults`, values in between, a size larger
    // than the store, and the absent member that pages at the service's own default.
    let page_sizes = [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(7),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let walked = walk_pages(&emulator, &case, page_size).await;
        assert_eq!(
            walked, created,
            "case {case:?}: the walk must report every product exactly once, in one order"
        );
    }
}

/// A `maxResults` outside its published `@range`, and a `nextToken` this service never
/// issued, are refused — and refusing writes nothing.
#[tokio::test]
async fn a_listing_outside_the_published_bounds_is_refused() {
    let emulator = Emulator::new();
    let created = create_named(&emulator, "Payments API").await;

    // (case, query, expected HTTP status)
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "maxResults on the published minimum",
            format!("maxResults={MAX_RESULTS_MIN}"),
            200,
        ),
        (
            "maxResults on the published maximum",
            format!("maxResults={MAX_RESULTS_MAX}"),
            200,
        ),
        (
            "maxResults one below the published minimum",
            format!("maxResults={}", MAX_RESULTS_MIN - 1),
            400,
        ),
        (
            "maxResults one above the published maximum",
            format!("maxResults={}", MAX_RESULTS_MAX + 1),
            400,
        ),
        (
            "a nextToken this service never issued",
            "nextToken=not-a-token".to_string(),
            400,
        ),
    ];

    for (case, query, status) in cases {
        let answer = emulator.list_products(&query).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.message().contains("not implemented"),
            "case {case:?}: the answer must not be the scaffold's 501 signal, got {:?}",
            answer.message()
        );
    }

    // Every refusal left the store exactly as it was.
    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.item_ids(),
        vec![created],
        "a refused listing must write nothing, got {:?}",
        listed.body
    );
}

/// A product id the store does not hold is the modelled 404 on every operation that
/// names one, and the refusal writes nothing.
#[tokio::test]
async fn an_unknown_product_id_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    let created = create_named(&emulator, "Payments API").await;
    let deleted = create_named(&emulator, "Gone").await;
    assert_eq!(
        emulator.delete_product(&deleted).await.status,
        204,
        "the fixture product must delete"
    );

    // Ids `ProductId`'s pattern and length admit, none of them a product the store
    // holds: both published length bounds, digits only, letters only, and one shaped
    // exactly like an id this service mints.
    let unknown_ids = [
        ("the shortest id the bound admits", "aaaaaaaaaa".to_string()),
        ("the longest id the bound admits", "z".repeat(30)),
        ("an id of digits only", "0123456789".to_string()),
        ("an id shaped like a minted one", "p0000ffffe".to_string()),
        ("an id the store held and released", deleted.clone()),
    ];

    for (case, product_id) in unknown_ids {
        let read = emulator.get_product(&product_id).await;
        let updated = emulator
            .update_product(&product_id, &json!({"description": "no such product"}))
            .await;
        let removed = emulator.delete_product(&product_id).await;

        for (operation, answer) in [
            ("GetProduct", &read),
            ("UpdateProduct", &updated),
            ("DeleteProduct", &removed),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {operation} must answer 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: {operation} must answer ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert!(
                answer.message().contains(&product_id),
                "case {case:?}: {operation}'s refusal must name the product asked for, got {:?}",
                answer.message()
            );
        }
    }

    let listed = emulator.list_products("").await;
    assert_eq!(
        listed.item_ids(),
        vec![created],
        "a refused call must write nothing, got {:?}",
        listed.body
    );
}

/// A create outside the members' published bounds is refused before any handler runs,
/// and nothing is stored for it.
#[tokio::test]
async fn a_create_outside_the_published_bounds_is_refused() {
    let emulator = Emulator::new();

    // (case, request)
    let cases: Vec<(&str, Value)> = vec![
        ("no displayName, though it is required", json!({})),
        (
            "a displayName one below the published minimum",
            json!({"displayName": ""}),
        ),
        (
            "a displayName one above the published maximum",
            json!({"displayName": string_of_length(DISPLAY_NAME_MAX + 1)}),
        ),
        (
            "a description one above the published maximum",
            json!({
                "displayName": "Payments API",
                "description": string_of_length(DESCRIPTION_MAX + 1),
            }),
        ),
    ];

    for (case, request) in cases {
        let answer = emulator.create_product(&request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.message().contains("not implemented"),
            "case {case:?}: the answer must not be the scaffold's 501 signal, got {:?}",
            answer.message()
        );
    }

    let listed = emulator.list_products("").await;
    assert!(
        listed.items().is_empty(),
        "a refused create must store nothing, got {:?}",
        listed.body
    );
}
