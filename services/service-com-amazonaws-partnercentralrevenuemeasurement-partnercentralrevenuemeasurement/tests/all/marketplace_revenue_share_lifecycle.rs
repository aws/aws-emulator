// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the marketplace revenue share slice, driven over this
//! service's own `rpcv2Cbor` wire protocol through the same `MultiServiceRouter`
//! the production emulator dispatches with.
//!
//! Every test asserts a RULE over a SET of inputs: the shapes the model names
//! are one row each.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use service_com_amazonaws_partnercentralrevenuemeasurement_partnercentralrevenuemeasurement as prm;
use smithy_server_runtime::MultiServiceRouter;

use crate::cbor_request::{request_body, texts, Cbor};
use prm::ops::types::{
    CreateMarketplaceRevenueShareAllocationInput, CreateMarketplaceRevenueShareAllocationOutput,
    CreateMarketplaceRevenueShareInput, CreateMarketplaceRevenueShareOutput,
    GetMarketplaceRevenueShareAllocationInput, GetMarketplaceRevenueShareAllocationOutput,
    GetMarketplaceRevenueShareInput, GetMarketplaceRevenueShareOutput,
    ListMarketplaceRevenueShareAllocationsInput, ListMarketplaceRevenueShareAllocationsOutput,
    ListMarketplaceRevenueSharesInput, ListMarketplaceRevenueSharesOutput,
    MarketplaceRevenueShareProductEntry, RevenueShareProductAccountEntry, Tag,
    UpdateMarketplaceRevenueShareAllocationInput, UpdateMarketplaceRevenueShareAllocationOutput,
    UpdateMarketplaceRevenueShareInput, UpdateMarketplaceRevenueShareOutput,
    CREATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
    CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
    GET_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
    GET_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA, LIST_MARKETPLACE_REVENUE_SHARES_INPUT_SCHEMA,
    LIST_MARKETPLACE_REVENUE_SHARE_ALLOCATIONS_INPUT_SCHEMA,
    UPDATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
    UPDATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
};

/// The Region the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The Region the audit suite signs for, so the records it asserts on are
/// distinguishable from every other suite's in this test binary.
const AUDIT_REGION: &str = "us-west-2";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "partnercentral";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service
/// segment.
const SERVICE_SHAPE: &str = "PartnerCentralRevenueMeasurement";

/// A first-of-month date comfortably in the future, so an allocation created
/// with it is "future-dated" for as long as this emulator exists and the
/// model's time-based mutability rules allow it to be updated.
const FUTURE_FROM: &str = "2099-01-01";

/// A last-of-month date after [`FUTURE_FROM`].
const FUTURE_UNTIL: &str = "2099-12-31";

/// A first-of-month date in the past, so the allocation it starts is already in
/// force and the mutability rules refuse the members they protect.
const PAST_FROM: &str = "2020-01-01";

/// A last-of-month date after [`PAST_FROM`].
const PAST_UNTIL: &str = "2021-12-31";

/// One emulator holding exactly this service, dispatched over HTTP.
struct Emulator {
    router: MultiServiceRouter,
    /// The Region this emulator's requests are signed for, which is the Region
    /// segment of every ARN it mints. A suite that needs its own ARN namespace
    /// — the audit suite, which reads records emitted by every suite in this
    /// test binary — gets one of its own.
    region: String,
}

/// A dispatched response: the HTTP status and the raw CBOR body.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The decoded output of a call that must have succeeded.
    fn ok<T: DeserializableStruct>(&self, case: &str) -> T {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status,
            self.render()
        );
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&self.body);
        T::deserialize(&mut deserializer)
            .unwrap_or_else(|error| panic!("case {case:?}: undecodable response body: {error}"))
    }

    /// Assert a modelled failure: the status the model gives the error, and the
    /// error code the `rpcv2Cbor` envelope carries in `__type`.
    fn is_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status,
            status,
            "case {case:?}: expected HTTP {status} {code}, got {} with body {}",
            self.status,
            self.render()
        );
        assert!(
            self.mentions(code),
            "case {case:?}: expected the error code {code:?} in the body, got {}",
            self.render()
        );
    }

    /// Whether the CBOR body carries `needle` as a text string. CBOR writes a
    /// definite-length text string as its UTF-8 bytes after a header byte, so a
    /// byte-window search finds any string member's value.
    fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// The body as lossy text, for an assertion message.
    fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        Self::in_region(REGION)
    }

    fn in_region(region: &str) -> Self {
        // The SINGLE shared engine, exactly as `router/src/lib.rs` builds it.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(prm::EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        schema: &Schema,
        input: &I,
    ) -> Answer {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(schema, input)
            .expect("a serializable request body");
        self.call_raw(operation, serializer.finish()).await
    }

    async fn call_raw(&self, operation: &str, payload: Vec<u8>) -> Answer {
        let region = self.region.as_str();
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.body().to_vec(),
        }
    }

    async fn create_share(&self, input: &CreateMarketplaceRevenueShareInput) -> Answer {
        self.call(
            "CreateMarketplaceRevenueShare",
            CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get_share(&self, input: &GetMarketplaceRevenueShareInput) -> Answer {
        self.call(
            "GetMarketplaceRevenueShare",
            GET_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn update_share(&self, input: &UpdateMarketplaceRevenueShareInput) -> Answer {
        self.call(
            "UpdateMarketplaceRevenueShare",
            UPDATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn list_shares(&self, input: &ListMarketplaceRevenueSharesInput) -> Answer {
        self.call(
            "ListMarketplaceRevenueShares",
            LIST_MARKETPLACE_REVENUE_SHARES_INPUT_SCHEMA,
            input,
        )
        .await
    }

    /// `ListMarketplaceRevenueShares` with a body built directly, which is the
    /// only way to send one of its list-of-scalar filters. See
    /// `crate::cbor_request`.
    async fn list_shares_raw(&self, entries: Vec<(&str, Cbor)>) -> Answer {
        self.call_raw("ListMarketplaceRevenueShares", request_body(entries))
            .await
    }

    async fn create_allocation(
        &self,
        input: &CreateMarketplaceRevenueShareAllocationInput,
    ) -> Answer {
        self.call(
            "CreateMarketplaceRevenueShareAllocation",
            CREATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get_allocation(&self, input: &GetMarketplaceRevenueShareAllocationInput) -> Answer {
        self.call(
            "GetMarketplaceRevenueShareAllocation",
            GET_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn list_allocations(
        &self,
        input: &ListMarketplaceRevenueShareAllocationsInput,
    ) -> Answer {
        self.call(
            "ListMarketplaceRevenueShareAllocations",
            LIST_MARKETPLACE_REVENUE_SHARE_ALLOCATIONS_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn update_allocation(
        &self,
        input: &UpdateMarketplaceRevenueShareAllocationInput,
    ) -> Answer {
        self.call(
            "UpdateMarketplaceRevenueShareAllocation",
            UPDATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    /// Create a share holding exactly `products` and answer the stored record.
    async fn share_with(
        &self,
        case: &str,
        catalog: &str,
        products: Vec<MarketplaceRevenueShareProductEntry>,
    ) -> CreateMarketplaceRevenueShareOutput {
        self.create_share(&CreateMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            products,
            ..Default::default()
        })
        .await
        .ok(case)
    }
}

/// `MarketplaceRevenueShareId`'s pattern, `^mrs-[a-z0-9]{13}$`, as a predicate.
fn is_share_id(value: &str) -> bool {
    value.len() == 17
        && value.starts_with("mrs-")
        && value[4..]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// `MarketplaceRevenueShareAllocationId`'s pattern, `^mrsa-[a-z0-9]{13}$`, as a
/// predicate.
fn is_allocation_id(value: &str) -> bool {
    value.len() == 18
        && value.starts_with("mrsa-")
        && value[5..]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The ARN `MarketplaceRevenueShareArn`'s pattern spells for a revenue share.
fn expected_share_arn(catalog: &str, share_id: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:catalog/{catalog}/marketplace-revenue-share/{share_id}"
    )
}

/// One product as a table row compares it, with the number of account ids
/// answered beside each per-type count: `(ProductId, [(AccountType, Count,
/// AccountIds)])`.
type ProductDetail<'a> = (&'a str, Vec<(&'a str, i32, usize)>);

/// One row of the allocation-update table: `(case, RevenueSharePercent, Status,
/// the percentage that follows, the status that follows)`.
type AllocationUpdateRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    &'static str,
);

/// One row of the list-filter table: `(case, the request body's entries, the
/// share ids the filter keeps)`.
type FilterRow<'a> = (&'a str, Vec<(&'a str, Cbor)>, Vec<&'a str>);

/// A product entry naming `product_id` and no account list.
fn product(product_id: &str) -> MarketplaceRevenueShareProductEntry {
    MarketplaceRevenueShareProductEntry {
        product_id: product_id.to_string(),
        accounts: None,
    }
}

/// A product entry naming `product_id` and the typed account lists `accounts`,
/// each `(AccountType, [AccountIds])`.
fn product_with_accounts(
    product_id: &str,
    accounts: &[(&str, &[&str])],
) -> MarketplaceRevenueShareProductEntry {
    MarketplaceRevenueShareProductEntry {
        product_id: product_id.to_string(),
        accounts: Some(
            accounts
                .iter()
                .map(|(account_type, ids)| RevenueShareProductAccountEntry {
                    account_type: account_type.to_string(),
                    account_ids: ids.iter().map(|id| id.to_string()).collect(),
                })
                .collect(),
        ),
    }
}

/// A create-allocation request with every required member set.
fn create_allocation_input(
    catalog: &str,
    share_id: &str,
    revision: &str,
    effective_from: &str,
    effective_until: &str,
    percent: &str,
) -> CreateMarketplaceRevenueShareAllocationInput {
    CreateMarketplaceRevenueShareAllocationInput {
        catalog: catalog.to_string(),
        marketplace_revenue_share_identifier: share_id.to_string(),
        marketplace_revenue_share_revision: revision.to_string(),
        effective_from: effective_from.to_string(),
        effective_until: effective_until.to_string(),
        revenue_share_percent: percent.to_string(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// A share is stored, and read back member for member
// ---------------------------------------------------------------------------

/// One row of the create/read-back table: the products a create sends and the
/// per-type account counts the share must report for each of them.
struct ShareRow {
    case: &'static str,
    catalog: &'static str,
    products: Vec<MarketplaceRevenueShareProductEntry>,
    /// Per product: `(ProductId, [(AccountType, Count)])`, in the order the
    /// request listed them.
    expected: Vec<(&'static str, Vec<(&'static str, i32)>)>,
    tags: Option<Vec<Tag>>,
}

/// The rule: a created marketplace revenue share is stored, and the following
/// `GetMarketplaceRevenueShare` answers the same share — its minted id, its
/// ARN, its catalog, its products in the order the request listed them, each
/// with the account list the request carried, and revision 1.
///
/// The rows differ along what the rule generalises over: the catalog, the
/// number of products, whether a product carries account lists, which types it
/// carries (`RevenueShareProductAccountType`: DEPLOYMENT | ORGANIZATION) and
/// how many accounts each list holds, and whether the create carried tags.
fn share_rows() -> Vec<ShareRow> {
    vec![
        ShareRow {
            case: "one product, no account list",
            catalog: "AWS",
            products: vec![product("prod-abc1234567890")],
            expected: vec![("prod-abc1234567890", Vec::new())],
            tags: None,
        },
        ShareRow {
            case: "one product with one typed account list",
            catalog: "AWS",
            products: vec![product_with_accounts(
                "prod-def1234567890",
                &[("DEPLOYMENT", &["111122223333"])],
            )],
            expected: vec![("prod-def1234567890", vec![("DEPLOYMENT", 1)])],
            tags: None,
        },
        ShareRow {
            case: "one product with both typed account lists",
            catalog: "Sandbox",
            products: vec![product_with_accounts(
                "prod-ghi1234567890",
                &[
                    ("ORGANIZATION", &["111122223333", "444455556666"]),
                    ("DEPLOYMENT", &["777788889999"]),
                ],
            )],
            expected: vec![(
                "prod-ghi1234567890",
                vec![("ORGANIZATION", 2), ("DEPLOYMENT", 1)],
            )],
            tags: None,
        },
        ShareRow {
            case: "three products, mixed account lists, with tags",
            catalog: "AWS",
            products: vec![
                product("prod-jkl1234567890"),
                product_with_accounts(
                    "prod-mno1234567890",
                    &[("DEPLOYMENT", &["111122223333", "444455556666"])],
                ),
                product("my.product/v2"),
            ],
            expected: vec![
                ("prod-jkl1234567890", Vec::new()),
                ("prod-mno1234567890", vec![("DEPLOYMENT", 2)]),
                ("my.product/v2", Vec::new()),
            ],
            tags: Some(vec![Tag {
                key: "team".to_string(),
                value: "revenue".to_string(),
            }]),
        },
    ]
}

#[tokio::test]
async fn a_created_revenue_share_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    for row in share_rows() {
        let created: CreateMarketplaceRevenueShareOutput = emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                catalog: row.catalog.to_string(),
                products: row.products.clone(),
                tags: row.tags.clone(),
                ..Default::default()
            })
            .await
            .ok(row.case);

        assert!(
            is_share_id(&created.id),
            "case {:?}: expected an mrs- id, got {:?}",
            row.case,
            created.id
        );
        assert_eq!(
            created.arn,
            expected_share_arn(row.catalog, &created.id),
            "case {:?}: expected the ARN the model's pattern spells, got {:?}",
            row.case,
            created.arn
        );
        assert_eq!(
            (created.catalog.as_str(), created.revision.as_str()),
            (row.catalog, "1"),
            "case {:?}: expected the requested catalog at revision 1, got {created:?}",
            row.case
        );
        let summarised: Vec<(&str, Vec<(&str, i32)>)> = created
            .products
            .iter()
            .map(|product| {
                (
                    product.product_id.as_str(),
                    product
                        .accounts
                        .iter()
                        .flatten()
                        .map(|accounts| (accounts.account_type.as_str(), accounts.count))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            summarised, row.expected,
            "case {:?}: expected the products with their per-type counts, got {:?}",
            row.case, created.products
        );
        for product in &created.products {
            assert!(
                product
                    .product_code
                    .as_ref()
                    .is_some_and(|code| !code.is_empty()
                        && code.chars().all(|c| c.is_ascii_alphanumeric())),
                "case {:?}: expected an alphanumeric ProductCode, got {product:?}",
                row.case
            );
        }

        // The read back: the same share, with the FULL account lists.
        let fetched: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: row.catalog.to_string(),
                identifier: created.id.clone(),
                revision: None,
            })
            .await
            .ok(row.case);
        assert_eq!(
            (
                fetched.id.as_str(),
                fetched.arn.as_str(),
                fetched.catalog.as_str(),
                fetched.revision.as_str(),
                fetched.latest_revision.as_str()
            ),
            (
                created.id.as_str(),
                created.arn.as_str(),
                row.catalog,
                "1",
                "1"
            ),
            "case {:?}: expected the stored share back, got {fetched:?}",
            row.case
        );
        assert_eq!(
            (
                fetched.total_marketplace_revenue_share_allocation_count,
                fetched.total_active_marketplace_revenue_share_allocation_count
            ),
            (0, 0),
            "case {:?}: a share with no allocation counts zero of each, got {fetched:?}",
            row.case
        );
        assert_eq!(
            (
                fetched.effective_from.as_deref(),
                fetched.effective_until.as_deref()
            ),
            (None, None),
            "case {:?}: with no ACTIVE allocation both window members are absent, got {fetched:?}",
            row.case
        );
        let detailed: Vec<ProductDetail<'_>> = fetched
            .products
            .iter()
            .map(|product| {
                (
                    product.product_id.as_str(),
                    product
                        .accounts
                        .iter()
                        .flatten()
                        .map(|accounts| {
                            (
                                accounts.account_type.as_str(),
                                accounts.count,
                                accounts.account_ids.len(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        let expected_detail: Vec<ProductDetail<'_>> = row
            .expected
            .iter()
            .map(|(product_id, accounts)| {
                (
                    *product_id,
                    accounts
                        .iter()
                        .map(|(account_type, count)| {
                            (*account_type, *count, usize::try_from(*count).unwrap_or(0))
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            detailed, expected_detail,
            "case {:?}: expected every account id back under its type, got {:?}",
            row.case, fetched.products
        );
    }
}

/// The rule: the identifier of a marketplace revenue share is its `mrs-` id or
/// its ARN, and an identifier naming no share in the catalog is a 404.
#[tokio::test]
async fn a_share_is_named_by_its_id_or_by_its_arn() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the share every row below names",
            "AWS",
            vec![product("prod-idf1234567890")],
        )
        .await;

    // (case, Catalog, Identifier, expected status)
    let rows: Vec<(&str, &str, String, u16)> = vec![
        ("the short id form", "AWS", created.id.clone(), 200),
        ("the ARN form", "AWS", created.arn.clone(), 200),
        (
            "an id this catalog does not hold",
            "AWS",
            "mrs-0000000000000".to_string(),
            404,
        ),
        (
            "the right id in the other catalog",
            "Sandbox",
            created.id.clone(),
            404,
        ),
        (
            "an ARN whose id this catalog does not hold",
            "AWS",
            expected_share_arn("AWS", "mrs-0000000000000"),
            404,
        ),
    ];
    for (case, catalog, identifier, status) in rows {
        let answer = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: catalog.to_string(),
                identifier: identifier.clone(),
                revision: None,
            })
            .await;
        if status == 200 {
            let fetched: GetMarketplaceRevenueShareOutput = answer.ok(case);
            assert_eq!(
                fetched.id, created.id,
                "case {case:?}: expected the share {:?}, got {fetched:?}",
                created.id
            );
        } else {
            answer.is_error(case, status, "ResourceNotFoundException");
        }
    }
}

// ---------------------------------------------------------------------------
// The update replaces the whole product set
// ---------------------------------------------------------------------------

/// The rule: `UpdateMarketplaceRevenueShare` carries the COMPLETE desired set
/// of products — what it lists is what the share holds afterwards, a product it
/// does not list is removed with its account lists, and the write is a new
/// revision, so a read at the previous revision still answers the products that
/// were held then.
///
/// The rows differ along what the rule generalises over: replacing a product,
/// dropping one, adding one, and changing only a product's account lists.
#[tokio::test]
async fn an_update_replaces_the_whole_product_set_as_a_new_revision() {
    // (case, the products the update sends, the product ids the share then holds)
    let rows: Vec<(
        &str,
        Vec<MarketplaceRevenueShareProductEntry>,
        Vec<&str>,
    )> = vec![
        (
            "replacing one product with another",
            vec![product("prod-upd1111111111"), product("prod-upd3333333333")],
            vec!["prod-upd1111111111", "prod-upd3333333333"],
        ),
        (
            "dropping a product",
            vec![product("prod-upd1111111111")],
            vec!["prod-upd1111111111"],
        ),
        (
            "adding a product",
            vec![
                product("prod-upd1111111111"),
                product("prod-upd2222222222"),
                product("prod-upd4444444444"),
            ],
            vec![
                "prod-upd1111111111",
                "prod-upd2222222222",
                "prod-upd4444444444",
            ],
        ),
        (
            "changing only the account lists of a product it keeps",
            vec![
                product_with_accounts(
                    "prod-upd1111111111",
                    &[("ORGANIZATION", &["111122223333", "444455556666"])],
                ),
                product("prod-upd2222222222"),
            ],
            vec!["prod-upd1111111111", "prod-upd2222222222"],
        ),
    ];

    for (case, products, expected) in rows {
        // Each row starts from the same two-product share, in its own store, so
        // the revision numbers below are this row's alone.
        let emulator = Emulator::new();
        let created = emulator
            .share_with(
                case,
                "AWS",
                vec![
                    product_with_accounts(
                        "prod-upd1111111111",
                        &[("DEPLOYMENT", &["111122223333"])],
                    ),
                    product("prod-upd2222222222"),
                ],
            )
            .await;

        let updated: UpdateMarketplaceRevenueShareOutput = emulator
            .update_share(&UpdateMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: created.revision.clone(),
                products: products.clone(),
                ..Default::default()
            })
            .await
            .ok(case);
        assert_eq!(
            (
                updated.id.as_str(),
                updated.arn.as_str(),
                updated.latest_revision.as_str()
            ),
            (created.id.as_str(), created.arn.as_str(), "2"),
            "case {case:?}: expected the same share at revision 2, got {updated:?}"
        );
        let answered: Vec<&str> = updated
            .products
            .iter()
            .map(|product| product.product_id.as_str())
            .collect();
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the complete desired set back, got {:?}",
            updated.products
        );

        // The read back at the latest revision: the same set, with its account
        // lists.
        let fetched: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: None,
            })
            .await
            .ok(case);
        let held: Vec<&str> = fetched
            .products
            .iter()
            .map(|product| product.product_id.as_str())
            .collect();
        assert_eq!(
            held, expected,
            "case {case:?}: expected the stored share to hold the new set, got {:?}",
            fetched.products
        );
        let sent_accounts: Vec<Option<usize>> = products
            .iter()
            .map(|product| product.accounts.as_ref().map(Vec::len))
            .collect();
        let held_accounts: Vec<Option<usize>> = fetched
            .products
            .iter()
            .map(|product| product.accounts.as_ref().map(Vec::len))
            .collect();
        assert_eq!(
            held_accounts, sent_accounts,
            "case {case:?}: a product the update did not list carries no account list, got {:?}",
            fetched.products
        );

        // The historical read: revision 1 still holds what the create wrote.
        let before: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: Some("1".to_string()),
            })
            .await
            .ok(case);
        let then: Vec<&str> = before
            .products
            .iter()
            .map(|product| product.product_id.as_str())
            .collect();
        assert_eq!(
            then,
            vec!["prod-upd1111111111", "prod-upd2222222222"],
            "case {case:?}: a read at revision 1 answers the products held then, got {:?}",
            before.products
        );
        assert_eq!(
            (before.revision.as_str(), before.latest_revision.as_str()),
            ("1", "2"),
            "case {case:?}: a historical read names both revisions, got {before:?}"
        );
    }
}

/// The rule: an empty `Products` list closes the marketplace revenue share, and
/// closing is one-way — a later update that lists a product is refused and the
/// share stays closed.
#[tokio::test]
async fn an_empty_product_list_closes_the_share_one_way() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the share this test closes",
            "AWS",
            vec![product("prod-clo1234567890")],
        )
        .await;

    let closed: UpdateMarketplaceRevenueShareOutput = emulator
        .update_share(&UpdateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: created.id.clone(),
            revision: created.revision.clone(),
            products: Vec::new(),
            ..Default::default()
        })
        .await
        .ok("closing the share");
    assert!(
        closed.products.is_empty(),
        "case {:?}: a closed share holds no product, got {closed:?}",
        "closing the share"
    );

    // Every attempt to put a product back is refused, whatever it lists.
    for (case, products) in [
        ("the product it used to hold", vec![product("prod-clo1234567890")]),
        ("a different product", vec![product("prod-clo9999999999")]),
    ] {
        emulator
            .update_share(&UpdateMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: closed.latest_revision.clone(),
                products,
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
    }

    let fetched: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: created.id.clone(),
            revision: None,
        })
        .await
        .ok("the closed share");
    assert!(
        fetched.products.is_empty(),
        "a refused reopen leaves the share closed, got {fetched:?}"
    );
    assert_eq!(
        fetched.latest_revision, "2",
        "a refused reopen writes no revision, got {fetched:?}"
    );
}

/// The rule: a `Products` entry names a product by id or by product ARN, the
/// share stores and returns the product ID, and a value that names no product
/// id is refused with the modelled 400.
#[tokio::test]
async fn a_product_entry_is_resolved_to_the_product_id_it_names() {
    let emulator = Emulator::new();
    // (case, the entry's ProductId, the id the share stores)
    let accepted: Vec<(&str, &str, &str)> = vec![
        (
            "a plain product id",
            "prod-res1234567890",
            "prod-res1234567890",
        ),
        (
            "a product ARN",
            "arn:aws:aws-marketplace:us-east-1:123456789012:AWSMarketplace/ProductId/prod-res2222222222",
            "prod-res2222222222",
        ),
        ("a dotted product id", "my.product.v3", "my.product.v3"),
    ];
    for (case, sent, stored) in accepted {
        let created = emulator
            .share_with(case, "AWS", vec![product(sent)])
            .await;
        let ids: Vec<&str> = created
            .products
            .iter()
            .map(|product| product.product_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![stored],
            "case {case:?}: expected the product id stored and returned, got {:?}",
            created.products
        );
    }

    // Each value satisfies the entry's own `^[\w\-:/.]+$`, so the refusal is
    // this service's and not the boundary validator's: none of them is a
    // `MarketplaceProductId` (`^[a-zA-Z0-9][.a-zA-Z0-9/-]{1,254}[a-zA-Z0-9]$`).
    for (case, sent) in [
        ("an underscore, which a product id does not carry", "my_product"),
        ("a trailing separator", "prod-trailing-"),
        ("two characters, below the published minimum of three", "ab"),
        ("a colon-separated token that is no ARN", "not:an:arn"),
    ] {
        emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                products: vec![product(sent)],
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
    }
}

/// The rule: `Products` is a SET — one product may appear once, each product
/// carries at most one account list per type, and a list of accounts cannot be
/// empty.
#[tokio::test]
async fn a_malformed_product_set_is_refused() {
    let emulator = Emulator::new();
    // (case, the products the request sends)
    let rows: Vec<(&str, Vec<MarketplaceRevenueShareProductEntry>)> = vec![
        (
            "the same product twice",
            vec![product("prod-dup1234567890"), product("prod-dup1234567890")],
        ),
        (
            "the same product, once by id and once by ARN",
            vec![
                product("prod-dup1234567890"),
                product("arn:aws:aws-marketplace:us-east-1:123456789012:AWSMarketplace/ProductId/prod-dup1234567890"),
            ],
        ),
        (
            "two account lists of the same type",
            vec![product_with_accounts(
                "prod-dup2222222222",
                &[
                    ("DEPLOYMENT", &["111122223333"]),
                    ("DEPLOYMENT", &["444455556666"]),
                ],
            )],
        ),
        (
            "an account type the model does not declare",
            vec![product_with_accounts(
                "prod-dup3333333333",
                &[("BILLING", &["111122223333"])],
            )],
        ),
        (
            "no product at all, which only the update may send",
            Vec::new(),
        ),
    ];
    for (case, products) in rows {
        emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                products,
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
    }
}

/// The rule: `Catalog` is an enum the model declares two values for, and a
/// request carrying anything else — or nothing — is refused with the modelled
/// 400.
#[tokio::test]
async fn a_catalog_outside_the_modelled_enum_is_refused_everywhere_it_appears() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with("the share the rows below name", "AWS", vec![product("prod-cat1234567890")])
        .await;
    for (case, catalog) in [
        ("a catalog the model does not declare", "Staging"),
        ("the right value in the wrong case", "aws"),
        ("no catalog at all", ""),
    ] {
        emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                catalog: catalog.to_string(),
                products: vec![product("prod-cat1234567890")],
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
        emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: catalog.to_string(),
                identifier: created.id.clone(),
                revision: None,
            })
            .await
            .is_error(case, 400, "ValidationException");
        emulator
            .list_shares(&ListMarketplaceRevenueSharesInput {
                catalog: catalog.to_string(),
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
        emulator
            .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                catalog: catalog.to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
    }
}

// ---------------------------------------------------------------------------
// Optimistic concurrency, across every write that carries a revision
// ---------------------------------------------------------------------------

/// The rule: a write that carries a revision of the parent share must carry the
/// CURRENT one — a stale or future value is a `ConflictException` with reason
/// REVISION_MISMATCH, and the write does not happen.
///
/// The rows are the three writes that carry such a member, each with a revision
/// that is not the share's current one.
#[tokio::test]
async fn a_revision_that_is_not_the_current_one_is_refused_with_revision_mismatch() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the share these writes name",
            "AWS",
            vec![product("prod-rev1234567890")],
        )
        .await;
    let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &created.id,
            &created.revision,
            FUTURE_FROM,
            FUTURE_UNTIL,
            "10",
        ))
        .await
        .ok("the allocation the update below names");
    // The create above wrote revision 2, so 1 is stale and 9 was never reached.
    assert_eq!(
        allocation.marketplace_revenue_share_latest_revision, "2",
        "an allocation write moves the parent share to a new revision, got {allocation:?}"
    );

    for (case, stale) in [("a stale revision", "1"), ("a revision never reached", "9")] {
        emulator
            .update_share(&UpdateMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: stale.to_string(),
                products: vec![product("prod-rev9999999999")],
                ..Default::default()
            })
            .await
            .is_error(case, 409, "ConflictException");
        emulator
            .create_allocation(&create_allocation_input(
                "AWS",
                &created.id,
                stale,
                FUTURE_FROM,
                FUTURE_UNTIL,
                "20",
            ))
            .await
            .is_error(case, 409, "ConflictException");
        emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                marketplace_revenue_share_allocation_id: allocation
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: stale.to_string(),
                revenue_share_percent: Some("30".to_string()),
                ..Default::default()
            })
            .await
            .is_error(case, 409, "ConflictException");
    }

    // None of the refusals wrote anything: the share is still at revision 2
    // with the product and the allocation it had.
    let fetched: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: created.id.clone(),
            revision: None,
        })
        .await
        .ok("the share after six refusals");
    assert_eq!(
        (
            fetched.latest_revision.as_str(),
            fetched.total_marketplace_revenue_share_allocation_count,
            fetched
                .products
                .first()
                .map(|product| product.product_id.as_str())
        ),
        ("2", 1, Some("prod-rev1234567890")),
        "a refused write changes nothing, got {fetched:?}"
    );
}

// ---------------------------------------------------------------------------
// The allocation lifecycle
// ---------------------------------------------------------------------------

/// The rule: an allocation created under a share is stored and read back member
/// for member, the write moves the parent share to a new revision, and the
/// share's own counts and effective window are derived from the allocations it
/// holds.
///
/// The rows differ along the percentage form (whole, one decimal, two
/// decimals, the published bounds) and the effective interval.
#[tokio::test]
async fn an_allocation_round_trips_and_moves_the_parent_share_revision() {
    // (case, EffectiveFrom, EffectiveUntil, RevenueSharePercent)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("a whole percentage", "2099-01-01", "2099-12-31", "10"),
        ("one decimal place", "2099-02-01", "2099-02-28", "12.5"),
        ("two decimal places", "2099-03-01", "2099-03-31", "0.01"),
        ("the lower bound of the range", "2099-04-01", "2099-04-30", "0"),
        ("the upper bound of the range", "2099-05-01", "2099-05-31", "100"),
        (
            "a single-month interval in a leap February",
            "2096-02-01",
            "2096-02-29",
            "33.33",
        ),
    ];

    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the parent share",
            "AWS",
            vec![product("prod-all1234567890")],
        )
        .await;
    let mut revision = created.revision.clone();
    let mut expected_count = 0;
    for (case, from, until, percent) in rows {
        let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&create_allocation_input(
                "AWS", &created.id, &revision, from, until, percent,
            ))
            .await
            .ok(case);
        expected_count += 1;

        assert!(
            is_allocation_id(&allocation.marketplace_revenue_share_allocation_id),
            "case {case:?}: expected an mrsa- id, got {:?}",
            allocation.marketplace_revenue_share_allocation_id
        );
        assert_eq!(
            (
                allocation.marketplace_revenue_share_identifier.as_str(),
                allocation.effective_from.as_str(),
                allocation.effective_until.as_str(),
                allocation.revenue_share_percent.as_str(),
                allocation.status.as_str()
            ),
            (created.id.as_str(), from, until, percent, "ACTIVE"),
            "case {case:?}: expected the allocation the request asked for, got {allocation:?}"
        );
        // The parent share moved by exactly one revision, and the allocation
        // belongs to the revision the write produced.
        let expected_revision = (revision.parse::<i32>().expect("a revision token") + 1).to_string();
        assert_eq!(
            (
                allocation.marketplace_revenue_share_revision.as_str(),
                allocation.marketplace_revenue_share_latest_revision.as_str()
            ),
            (expected_revision.as_str(), expected_revision.as_str()),
            "case {case:?}: expected the write's own revision on both members, got {allocation:?}"
        );

        let fetched: GetMarketplaceRevenueShareAllocationOutput = emulator
            .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.arn.clone(),
                marketplace_revenue_share_allocation_id: allocation
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: None,
            })
            .await
            .ok(case);
        assert_eq!(
            (
                fetched.marketplace_revenue_share_allocation_id.as_str(),
                fetched.effective_from.as_str(),
                fetched.effective_until.as_str(),
                fetched.revenue_share_percent.as_str(),
                fetched.status.as_str(),
                fetched.marketplace_revenue_share_revision.as_str()
            ),
            (
                allocation.marketplace_revenue_share_allocation_id.as_str(),
                from,
                until,
                percent,
                "ACTIVE",
                expected_revision.as_str()
            ),
            "case {case:?}: expected the stored allocation back, got {fetched:?}"
        );

        revision = expected_revision;
        let share: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: "AWS".to_string(),
                identifier: created.id.clone(),
                revision: None,
            })
            .await
            .ok(case);
        assert_eq!(
            (
                share.latest_revision.as_str(),
                share.total_marketplace_revenue_share_allocation_count,
                share.total_active_marketplace_revenue_share_allocation_count
            ),
            (revision.as_str(), expected_count, expected_count),
            "case {case:?}: expected the counts derived from the stored allocations, got {share:?}"
        );
    }

    // The share's effective window is the earliest start and the latest end
    // over its ACTIVE allocations.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: created.id.clone(),
            revision: None,
        })
        .await
        .ok("the share after every allocation above");
    assert_eq!(
        (
            share.effective_from.as_deref(),
            share.effective_until.as_deref()
        ),
        (Some("2096-02-01"), Some("2099-12-31")),
        "expected the window derived from the ACTIVE allocations, got {share:?}"
    );
}

/// Month ends across the Gregorian leap rule, which the date type answers for
/// `state::is_last_day_of_month`: these rows pin that the service asks it the
/// right question on each side of a month boundary. Each row is `(case,
/// EffectiveFrom, EffectiveUntil)` where the end date IS the last day of its
/// month, so the create must be accepted: a leap February, a common February, a
/// century year divisible by 400 (a leap year), a century year that is not, a
/// 30- and a 31-day month, and the last month RFC 3339 represents — whose end
/// date is the one whose SUCCESSOR the date type cannot render, which the
/// service must answer rather than unwind on.
fn month_end_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("a leap February", "2024-02-01", "2024-02-29"),
        ("a common February", "2023-02-01", "2023-02-28"),
        ("a century year divisible by 400", "2000-02-01", "2000-02-29"),
        (
            "a century year not divisible by 400",
            "2100-02-01",
            "2100-02-28",
        ),
        ("a thirty-day month", "2024-04-01", "2024-04-30"),
        ("a thirty-one-day month", "2024-12-01", "2024-12-31"),
        (
            "the last month RFC 3339 represents",
            "9999-12-01",
            "9999-12-31",
        ),
    ]
}

/// The same rule's refusals: `(case, EffectiveFrom, EffectiveUntil)`, each value
/// satisfying `^\d{4}-\d{2}-\d{2}$` so the refusal is this service's and not the
/// boundary validator's.
fn month_end_refusals() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "the 29th of a February in a century year not divisible by 400",
            "2100-02-01",
            "2100-02-29",
        ),
        (
            "the 29th of February 1900, which was not a leap year",
            "1900-02-01",
            "1900-02-29",
        ),
        ("the 31st of a thirty-day month", "2024-04-01", "2024-04-31"),
        (
            "the 28th of a leap February, which is not its last day",
            "2024-02-01",
            "2024-02-28",
        ),
        ("month zero", "2024-01-01", "2024-00-31"),
        ("day zero", "2024-01-00", "2024-01-31"),
        // One past the published lower bound of the date range:
        // `^\d{4}-\d{2}-\d{2}$` admits year 0000, which is outside the years
        // RFC 3339 represents (0001..=9999), so it earns the modelled 400
        // rather than a panic in the formatter.
        (
            "a year below the first RFC 3339 represents",
            "0000-01-01",
            "0000-01-31",
        ),
        (
            "a start date that is not the first day of its month",
            "2024-01-15",
            "2024-01-31",
        ),
        (
            "an interval that ends before it starts",
            "2024-06-01",
            "2024-03-31",
        ),
    ]
}

/// The rule: `EffectiveFrom` must be the first day of a month and
/// `EffectiveUntil` the last day of one, both on the real calendar, and
/// `RevenueSharePercent` is a share of revenue, so it cannot exceed 100.
#[tokio::test]
async fn the_documented_calendar_and_percentage_rules_are_enforced() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the parent share",
            "AWS",
            vec![product("prod-cal1234567890")],
        )
        .await;
    let mut revision = created.revision.clone();

    for (case, from, until) in month_end_rows() {
        let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&create_allocation_input(
                "AWS", &created.id, &revision, from, until, "1",
            ))
            .await
            .ok(case);
        assert_eq!(
            (
                allocation.effective_from.as_str(),
                allocation.effective_until.as_str()
            ),
            (from, until),
            "case {case:?}: expected the interval the request asked for, got {allocation:?}"
        );
        revision = allocation.marketplace_revenue_share_latest_revision.clone();
    }

    for (case, from, until) in month_end_refusals() {
        emulator
            .create_allocation(&create_allocation_input(
                "AWS", &created.id, &revision, from, until, "1",
            ))
            .await
            .is_error(case, 400, "ValidationException");
    }

    // The percentage rule, over the published bound and one past it.
    for (case, percent, status) in [
        ("the upper bound of the range", "100", 200),
        ("one hundredth past the upper bound", "100.01", 400),
        ("more than three times the bound", "999", 400),
    ] {
        let answer = emulator
            .create_allocation(&create_allocation_input(
                "AWS",
                &created.id,
                &revision,
                "2098-01-01",
                "2098-12-31",
                percent,
            ))
            .await;
        if status == 200 {
            let allocation: CreateMarketplaceRevenueShareAllocationOutput = answer.ok(case);
            assert_eq!(
                allocation.revenue_share_percent, percent,
                "case {case:?}: expected the percentage back, got {allocation:?}"
            );
            revision = allocation.marketplace_revenue_share_latest_revision.clone();
        } else {
            answer.is_error(case, status, "ValidationException");
        }
    }
}

/// The rule: `UpdateMarketplaceRevenueShareAllocation` writes a new version of
/// the allocation and a new revision of the parent share; `RevenueSharePercent`
/// and `Status` are "only modifiable on future-dated allocations"; and a read at
/// the earlier revision still answers the version that was current then.
#[tokio::test]
async fn an_allocation_update_writes_through_and_honours_the_mutability_rules() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the parent share",
            "AWS",
            vec![product("prod-upa1234567890")],
        )
        .await;
    let future: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &created.id,
            &created.revision,
            FUTURE_FROM,
            FUTURE_UNTIL,
            "10",
        ))
        .await
        .ok("the future-dated allocation");
    let in_force: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &created.id,
            &future.marketplace_revenue_share_latest_revision,
            PAST_FROM,
            PAST_UNTIL,
            "10",
        ))
        .await
        .ok("the allocation already in force");

    // (case, what the update changes, the percentage and status that follow)
    let rows: Vec<AllocationUpdateRow> = vec![
        ("the percentage alone", Some("20"), None, "20", "ACTIVE"),
        ("the status alone", None, Some("INACTIVE"), "20", "INACTIVE"),
        (
            "both members at once",
            Some("42.75"),
            Some("ACTIVE"),
            "42.75",
            "ACTIVE",
        ),
    ];
    let mut revision = in_force.marketplace_revenue_share_latest_revision.clone();
    for (case, percent, status, expected_percent, expected_status) in rows {
        let updated: UpdateMarketplaceRevenueShareAllocationOutput = emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                marketplace_revenue_share_allocation_id: future
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: revision.clone(),
                revenue_share_percent: percent.map(str::to_string),
                status: status.map(str::to_string),
                ..Default::default()
            })
            .await
            .ok(case);
        let expected_revision = (revision.parse::<i32>().expect("a revision token") + 1).to_string();
        assert_eq!(
            (
                updated.revenue_share_percent.as_str(),
                updated.status.as_str(),
                updated.effective_from.as_str(),
                updated.effective_until.as_str(),
                updated.marketplace_revenue_share_revision.as_str()
            ),
            (
                expected_percent,
                expected_status,
                FUTURE_FROM,
                FUTURE_UNTIL,
                expected_revision.as_str()
            ),
            "case {case:?}: expected the updated version at the new revision, got {updated:?}"
        );
        // The earlier revision still answers the version that was current then.
        let before: GetMarketplaceRevenueShareAllocationOutput = emulator
            .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                marketplace_revenue_share_allocation_id: future
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: Some(
                    future.marketplace_revenue_share_revision.clone(),
                ),
            })
            .await
            .ok(case);
        assert_eq!(
            (
                before.revenue_share_percent.as_str(),
                before.status.as_str()
            ),
            ("10", "ACTIVE"),
            "case {case:?}: a historical read answers the version written then, got {before:?}"
        );
        revision = expected_revision;
    }

    // An allocation that has come into force keeps both protected members.
    for (case, percent, status) in [
        ("the percentage of an in-force allocation", Some("5"), None),
        (
            "the status of an in-force allocation",
            None,
            Some("INACTIVE"),
        ),
    ] {
        emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                marketplace_revenue_share_allocation_id: in_force
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: revision.clone(),
                revenue_share_percent: percent.map(str::to_string),
                status: status.map(str::to_string),
                ..Default::default()
            })
            .await
            .is_error(case, 400, "ValidationException");
    }

    // An update that names nothing to change, and one that names a value the
    // enum does not declare.
    emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            marketplace_revenue_share_allocation_id: future
                .marketplace_revenue_share_allocation_id
                .clone(),
            marketplace_revenue_share_revision: revision.clone(),
            ..Default::default()
        })
        .await
        .is_error("an update that changes nothing", 400, "ValidationException");
    emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            marketplace_revenue_share_allocation_id: future
                .marketplace_revenue_share_allocation_id
                .clone(),
            marketplace_revenue_share_revision: revision.clone(),
            status: Some("DELETED".to_string()),
            ..Default::default()
        })
        .await
        .is_error(
            "a status the model does not declare",
            400,
            "ValidationException",
        );

    // An allocation id under a share that does not hold it is a 404.
    emulator
        .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            marketplace_revenue_share_allocation_id: "mrsa-0000000000000".to_string(),
            marketplace_revenue_share_revision: None,
        })
        .await
        .is_error("an allocation id naming nothing", 404, "ResourceNotFoundException");
}

/// The rule: an INACTIVE allocation is counted in the total but not in the
/// active total, and it leaves the share's effective window to the ACTIVE ones.
#[tokio::test]
async fn a_soft_deleted_allocation_leaves_the_counts_and_the_window_to_the_active_ones() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the parent share",
            "AWS",
            vec![product("prod-ina1234567890")],
        )
        .await;
    let first: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &created.id,
            &created.revision,
            "2098-01-01",
            "2098-12-31",
            "10",
        ))
        .await
        .ok("the allocation that stays ACTIVE");
    let second: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &created.id,
            &first.marketplace_revenue_share_latest_revision,
            "2097-01-01",
            "2097-12-31",
            "10",
        ))
        .await
        .ok("the allocation this test deactivates");
    let deactivated: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            marketplace_revenue_share_allocation_id: second
                .marketplace_revenue_share_allocation_id
                .clone(),
            marketplace_revenue_share_revision: second
                .marketplace_revenue_share_latest_revision
                .clone(),
            status: Some("INACTIVE".to_string()),
            ..Default::default()
        })
        .await
        .ok("the soft delete");
    assert_eq!(
        deactivated.status, "INACTIVE",
        "expected the soft-deleted status back, got {deactivated:?}"
    );

    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: created.id.clone(),
            revision: None,
        })
        .await
        .ok("the share after the soft delete");
    assert_eq!(
        (
            share.total_marketplace_revenue_share_allocation_count,
            share.total_active_marketplace_revenue_share_allocation_count,
            share.effective_from.as_deref(),
            share.effective_until.as_deref()
        ),
        (2, 1, Some("2098-01-01"), Some("2098-12-31")),
        "expected the INACTIVE allocation out of the active count and the window, got {share:?}"
    );
}

// ---------------------------------------------------------------------------
// The two paginated lists
// ---------------------------------------------------------------------------

/// The rule: `ListMarketplaceRevenueShares` answers the shares of ONE catalog,
/// filtered by every filter the request carries, sorted as it asks, and paged by
/// `MaxResults` / `NextToken` over a total order.
#[tokio::test]
async fn the_share_list_filters_sorts_and_pages_from_state() {
    let emulator = Emulator::new();
    // Three shares in `AWS`, one in `Sandbox`, created in this order.
    let first = emulator
        .share_with("the first share", "AWS", vec![product("prod-lst1111111111")])
        .await;
    let second = emulator
        .share_with(
            "the second share",
            "AWS",
            vec![
                product("prod-lst2222222222"),
                product("prod-lst3333333333"),
            ],
        )
        .await;
    let third = emulator
        .share_with("the third share", "AWS", vec![product("prod-lst4444444444")])
        .await;
    let other_catalog = emulator
        .share_with(
            "the share in the other catalog",
            "Sandbox",
            vec![product("prod-lst1111111111")],
        )
        .await;

    let page: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: "AWS".to_string(),
            ..Default::default()
        })
        .await
        .ok("every share in the catalog");
    let ids: Vec<&str> = page
        .marketplace_revenue_share_summaries
        .iter()
        .map(|summary| summary.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![first.id.as_str(), second.id.as_str(), third.id.as_str()],
        "expected the catalog's own shares in id order, got {ids:?}"
    );
    assert!(
        !ids.contains(&other_catalog.id.as_str()),
        "a share of another catalog is never returned, got {ids:?}"
    );
    let summary = page
        .marketplace_revenue_share_summaries
        .iter()
        .find(|summary| summary.id == second.id)
        .expect("the second share's summary");
    let products: Vec<&str> = summary
        .products
        .iter()
        .map(|product| product.product_id.as_str())
        .collect();
    assert_eq!(
        (
            summary.arn.as_str(),
            summary.catalog.as_str(),
            summary.latest_revision.as_str(),
            products
        ),
        (
            second.arn.as_str(),
            "AWS",
            "1",
            vec!["prod-lst2222222222", "prod-lst3333333333"]
        ),
        "expected every summary member read off the stored share, got {summary:?}"
    );

    // The sort the model declares, in both directions.
    for (case, order, expected) in [
        (
            "ascending by LastModifiedDate",
            "ASCENDING",
            vec![first.id.as_str(), second.id.as_str(), third.id.as_str()],
        ),
        (
            "descending by LastModifiedDate",
            "DESCENDING",
            vec![third.id.as_str(), second.id.as_str(), first.id.as_str()],
        ),
    ] {
        let sorted: ListMarketplaceRevenueSharesOutput = emulator
            .list_shares(&ListMarketplaceRevenueSharesInput {
                catalog: "AWS".to_string(),
                sort_by: Some("LastModifiedDate".to_string()),
                sort_order: Some(order.to_string()),
                ..Default::default()
            })
            .await
            .ok(case);
        let ids: Vec<&str> = sorted
            .marketplace_revenue_share_summaries
            .iter()
            .map(|summary| summary.id.as_str())
            .collect();
        assert_eq!(ids, expected, "case {case:?}: expected {expected:?}, got {ids:?}");
    }

    // One page at a time, resumed by the token the previous page answered.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page: ListMarketplaceRevenueSharesOutput = emulator
            .list_shares(&ListMarketplaceRevenueSharesInput {
                catalog: "AWS".to_string(),
                max_results: Some(2),
                next_token: token.clone(),
                ..Default::default()
            })
            .await
            .ok("one page of two");
        assert!(
            page.marketplace_revenue_share_summaries.len() <= 2,
            "a page never holds more than MaxResults items, got {page:?}"
        );
        seen.extend(
            page.marketplace_revenue_share_summaries
                .iter()
                .map(|summary| summary.id.clone()),
        );
        token = page.next_token;
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![first.id.clone(), second.id.clone(), third.id.clone()],
        "paging through the list answers every share exactly once, got {seen:?}"
    );

    // A token this query never issued, and a MaxResults outside the published
    // range, are both refused.
    emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: "AWS".to_string(),
            next_token: Some("not-a-token".to_string()),
            ..Default::default()
        })
        .await
        .is_error("a token this query never issued", 400, "ValidationException");
    emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: "AWS".to_string(),
            max_results: Some(0),
            ..Default::default()
        })
        .await
        .is_error(
            "a page of no items, below the published minimum",
            400,
            "ValidationException",
        );

    // The three list-of-scalar filters, each sent as a body built directly
    // (see `crate::cbor_request`). A share matches a filter when any of its
    // products matches, and must match every filter sent.
    let rows: Vec<FilterRow<'_>> = vec![
        (
            "Identifiers naming two of the three",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("Identifiers", texts(&[&first.id, &third.id])),
            ],
            vec![first.id.as_str(), third.id.as_str()],
        ),
        (
            "Identifiers in ARN form",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("Identifiers", texts(&[&second.arn])),
            ],
            vec![second.id.as_str()],
        ),
        (
            "ProductIds naming a product of one share",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("ProductIds", texts(&["prod-lst3333333333"])),
            ],
            vec![second.id.as_str()],
        ),
        (
            "ProductIds naming products of two shares",
            vec![
                ("Catalog", Cbor::text("AWS")),
                (
                    "ProductIds",
                    texts(&["prod-lst1111111111", "prod-lst4444444444"]),
                ),
            ],
            vec![first.id.as_str(), third.id.as_str()],
        ),
        (
            "ProductCodes naming the code of one product",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("ProductCodes", texts(&["prodlst2222222222"])),
            ],
            vec![second.id.as_str()],
        ),
        (
            "two filters that no share matches together",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("Identifiers", texts(&[&first.id])),
                ("ProductIds", texts(&["prod-lst4444444444"])),
            ],
            Vec::new(),
        ),
        (
            "a ProductId no share holds",
            vec![
                ("Catalog", Cbor::text("AWS")),
                ("ProductIds", texts(&["prod-lst9999999999"])),
            ],
            Vec::new(),
        ),
    ];
    for (case, entries, expected) in rows {
        let filtered: ListMarketplaceRevenueSharesOutput =
            emulator.list_shares_raw(entries).await.ok(case);
        let ids: Vec<&str> = filtered
            .marketplace_revenue_share_summaries
            .iter()
            .map(|summary| summary.id.as_str())
            .collect();
        assert_eq!(ids, expected, "case {case:?}: expected {expected:?}, got {ids:?}");
    }

    // The two creation-time bounds, on each side of a stored `CreatedDate`:
    // `CreatedAfter` is inclusive and `CreatedBefore` exclusive.
    let created_at = page
        .marketplace_revenue_share_summaries
        .first()
        .map(|summary| summary.created_date)
        .unwrap_or_else(|| {
            panic!("the first page must hold a share to read a CreatedDate from")
        });
    let second_before: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: "AWS".to_string(),
            created_before: Some(created_at),
            ..Default::default()
        })
        .await
        .ok("CreatedBefore the share's own instant");
    assert!(
        second_before
            .marketplace_revenue_share_summaries
            .iter()
            .all(|summary| summary.created_date.secs() < created_at.secs()),
        "CreatedBefore is exclusive, got {second_before:?}"
    );
    let at_or_after: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: "AWS".to_string(),
            created_after: Some(created_at),
            ..Default::default()
        })
        .await
        .ok("CreatedAfter the share's own instant");
    assert!(
        at_or_after
            .marketplace_revenue_share_summaries
            .iter()
            .any(|summary| summary.created_date.secs() == created_at.secs()),
        "CreatedAfter is inclusive, got {at_or_after:?}"
    );
}

/// The rule: `ListMarketplaceRevenueShareAllocations` answers the allocations of
/// ONE share as they stood at the revision the request names, filtered by status
/// and by the four half-open date bounds, sorted as it asks, and paged over a
/// total order.
#[tokio::test]
async fn the_allocation_list_filters_sorts_and_pages_from_state() {
    let emulator = Emulator::new();
    let created = emulator
        .share_with(
            "the parent share",
            "AWS",
            vec![product("prod-lsa1234567890")],
        )
        .await;
    // (EffectiveFrom, EffectiveUntil), created in this order.
    let intervals = [
        ("2097-01-01", "2097-06-30"),
        ("2098-01-01", "2098-06-30"),
        ("2099-01-01", "2099-06-30"),
    ];
    let mut revision = created.revision.clone();
    let mut ids: Vec<String> = Vec::new();
    for (from, until) in intervals {
        let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&create_allocation_input(
                "AWS", &created.id, &revision, from, until, "10",
            ))
            .await
            .ok("one of the three allocations");
        revision = allocation.marketplace_revenue_share_latest_revision.clone();
        ids.push(allocation.marketplace_revenue_share_allocation_id.clone());
    }
    let revision_with_three = revision.clone();

    let page: ListMarketplaceRevenueShareAllocationsOutput = emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            ..Default::default()
        })
        .await
        .ok("every allocation under the share");
    let listed: Vec<&str> = page
        .marketplace_revenue_share_allocation_summaries
        .iter()
        .map(|summary| summary.marketplace_revenue_share_allocation_id.as_str())
        .collect();
    assert_eq!(
        listed,
        ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        "expected every allocation in creation order, got {listed:?}"
    );
    let summary = page
        .marketplace_revenue_share_allocation_summaries
        .first()
        .expect("the first summary");
    assert_eq!(
        (
            summary.marketplace_revenue_share_identifier.as_str(),
            summary.effective_from.as_str(),
            summary.effective_until.as_str(),
            summary.revenue_share_percent.as_str(),
            summary.status.as_str()
        ),
        (created.id.as_str(), "2097-01-01", "2097-06-30", "10", "ACTIVE"),
        "expected every summary member read off the stored allocation, got {summary:?}"
    );

    // (case, the filter, the allocations it keeps)
    let rows: Vec<(
        &str,
        ListMarketplaceRevenueShareAllocationsInput,
        Vec<usize>,
    )> = vec![
        (
            "AfterEffectiveFrom is an inclusive lower bound",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                after_effective_from: Some("2098-01-01".to_string()),
                ..Default::default()
            },
            vec![1, 2],
        ),
        (
            "BeforeEffectiveFrom is an exclusive upper bound",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                before_effective_from: Some("2098-01-01".to_string()),
                ..Default::default()
            },
            vec![0],
        ),
        (
            "AfterEffectiveUntil is an inclusive lower bound",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                after_effective_until: Some("2098-06-30".to_string()),
                ..Default::default()
            },
            vec![1, 2],
        ),
        (
            "BeforeEffectiveUntil is an exclusive upper bound",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                before_effective_until: Some("2098-06-30".to_string()),
                ..Default::default()
            },
            vec![0],
        ),
        (
            "a half-open window over one interval",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                after_effective_from: Some("2098-01-01".to_string()),
                before_effective_from: Some("2099-01-01".to_string()),
                ..Default::default()
            },
            vec![1],
        ),
        (
            "the ACTIVE status filter keeps every one of them",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                status_filter: Some("ACTIVE".to_string()),
                ..Default::default()
            },
            vec![0, 1, 2],
        ),
        (
            "the INACTIVE status filter keeps none",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                status_filter: Some("INACTIVE".to_string()),
                ..Default::default()
            },
            Vec::new(),
        ),
        (
            "descending by EffectiveFrom",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                sort_by: Some("EffectiveFrom".to_string()),
                sort_order: Some("DESCENDING".to_string()),
                ..Default::default()
            },
            vec![2, 1, 0],
        ),
        (
            "a historical read at the revision that held one allocation",
            ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                marketplace_revenue_share_revision: Some("2".to_string()),
                ..Default::default()
            },
            vec![0],
        ),
    ];
    for (case, input, expected) in rows {
        let filtered: ListMarketplaceRevenueShareAllocationsOutput =
            emulator.list_allocations(&input).await.ok(case);
        let listed: Vec<&str> = filtered
            .marketplace_revenue_share_allocation_summaries
            .iter()
            .map(|summary| summary.marketplace_revenue_share_allocation_id.as_str())
            .collect();
        let wanted: Vec<&str> = expected.iter().map(|index| ids[*index].as_str()).collect();
        assert_eq!(listed, wanted, "case {case:?}: expected {wanted:?}, got {listed:?}");
    }

    // One page at a time, over the same total order.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page: ListMarketplaceRevenueShareAllocationsOutput = emulator
            .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                catalog: "AWS".to_string(),
                marketplace_revenue_share_identifier: created.id.clone(),
                max_results: Some(1),
                next_token: token.clone(),
                ..Default::default()
            })
            .await
            .ok("one page of one");
        assert!(
            page.marketplace_revenue_share_allocation_summaries.len() <= 1,
            "a page never holds more than MaxResults items, got {page:?}"
        );
        seen.extend(
            page.marketplace_revenue_share_allocation_summaries
                .iter()
                .map(|summary| summary.marketplace_revenue_share_allocation_id.clone()),
        );
        token = page.next_token;
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, ids,
        "paging through the list answers every allocation exactly once, got {seen:?}"
    );

    // A revision the share never reached names nothing.
    emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: created.id.clone(),
            marketplace_revenue_share_revision: Some("99".to_string()),
            ..Default::default()
        })
        .await
        .is_error(
            "a revision the share never reached",
            404,
            "ResourceNotFoundException",
        );
    // And so does a share the catalog does not hold.
    emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: "mrs-0000000000000".to_string(),
            ..Default::default()
        })
        .await
        .is_error("a share the catalog does not hold", 404, "ResourceNotFoundException");
    assert_eq!(
        revision_with_three, "4",
        "three allocation writes move the share from revision 1 to 4, got {revision_with_three:?}"
    );
}

// ---------------------------------------------------------------------------
// Idempotency, on every `@idempotencyToken` member
// ---------------------------------------------------------------------------

/// The rule: a `ClientToken` replayed with the SAME parameters answers with the
/// resource the first call wrote and writes nothing more; replayed with
/// DIFFERENT parameters it is a `ConflictException`.
#[tokio::test]
async fn an_idempotency_token_replays_instead_of_writing_again() {
    let emulator = Emulator::new();
    let products = vec![product("prod-idm1234567890")];
    let first: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            client_token: Some("token-create".to_string()),
            products: products.clone(),
            ..Default::default()
        })
        .await
        .ok("the first create");
    let replay: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            client_token: Some("token-create".to_string()),
            products: products.clone(),
            ..Default::default()
        })
        .await
        .ok("the replayed create");
    assert_eq!(
        (replay.id.as_str(), replay.revision.as_str()),
        (first.id.as_str(), "1"),
        "expected the share the first call minted, got {replay:?}"
    );
    emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            client_token: Some("token-create".to_string()),
            products: vec![product("prod-idm9999999999")],
            ..Default::default()
        })
        .await
        .is_error(
            "the same token with different products",
            409,
            "ConflictException",
        );

    // The share update: the replay answers the revision it wrote, and does not
    // fail the concurrency check against the revision it has itself moved.
    let update = UpdateMarketplaceRevenueShareInput {
        catalog: "AWS".to_string(),
        identifier: first.id.clone(),
        client_token: Some("token-update".to_string()),
        revision: "1".to_string(),
        products: vec![product("prod-idm2222222222")],
    };
    let updated: UpdateMarketplaceRevenueShareOutput =
        emulator.update_share(&update).await.ok("the first update");
    let replayed: UpdateMarketplaceRevenueShareOutput = emulator
        .update_share(&update)
        .await
        .ok("the replayed update");
    assert_eq!(
        (
            replayed.latest_revision.as_str(),
            replayed
                .products
                .first()
                .map(|product| product.product_id.as_str())
        ),
        (
            updated.latest_revision.as_str(),
            Some("prod-idm2222222222")
        ),
        "expected the revision the first update wrote, got {replayed:?}"
    );
    emulator
        .update_share(&UpdateMarketplaceRevenueShareInput {
            products: vec![product("prod-idm3333333333")],
            ..update.clone()
        })
        .await
        .is_error(
            "the same update token with different products",
            409,
            "ConflictException",
        );

    // The allocation create and update, on the same share.
    let allocation_input = create_allocation_input(
        "AWS",
        &first.id,
        &updated.latest_revision,
        FUTURE_FROM,
        FUTURE_UNTIL,
        "10",
    );
    let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("token-allocation".to_string()),
            ..allocation_input.clone()
        })
        .await
        .ok("the first allocation create");
    let replayed_allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("token-allocation".to_string()),
            ..allocation_input.clone()
        })
        .await
        .ok("the replayed allocation create");
    assert_eq!(
        (
            replayed_allocation
                .marketplace_revenue_share_allocation_id
                .as_str(),
            replayed_allocation
                .marketplace_revenue_share_revision
                .as_str()
        ),
        (
            allocation.marketplace_revenue_share_allocation_id.as_str(),
            allocation.marketplace_revenue_share_revision.as_str()
        ),
        "expected the allocation the first call minted, got {replayed_allocation:?}"
    );
    emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("token-allocation".to_string()),
            revenue_share_percent: "20".to_string(),
            ..allocation_input.clone()
        })
        .await
        .is_error(
            "the same allocation token with a different percentage",
            409,
            "ConflictException",
        );

    let allocation_update = UpdateMarketplaceRevenueShareAllocationInput {
        catalog: "AWS".to_string(),
        marketplace_revenue_share_identifier: first.id.clone(),
        marketplace_revenue_share_allocation_id: allocation
            .marketplace_revenue_share_allocation_id
            .clone(),
        marketplace_revenue_share_revision: allocation
            .marketplace_revenue_share_latest_revision
            .clone(),
        client_token: Some("token-allocation-update".to_string()),
        revenue_share_percent: Some("25".to_string()),
        status: None,
    };
    let first_update: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&allocation_update)
        .await
        .ok("the first allocation update");
    let replayed_update: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&allocation_update)
        .await
        .ok("the replayed allocation update");
    assert_eq!(
        (
            replayed_update.revenue_share_percent.as_str(),
            replayed_update.marketplace_revenue_share_revision.as_str()
        ),
        (
            first_update.revenue_share_percent.as_str(),
            first_update.marketplace_revenue_share_revision.as_str()
        ),
        "expected the version the first update wrote, got {replayed_update:?}"
    );

    // Four writes and four replays: the share is at the revision those four
    // writes produced and holds exactly one allocation.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: first.id.clone(),
            revision: None,
        })
        .await
        .ok("the share after every call above");
    assert_eq!(
        (
            share.latest_revision.as_str(),
            share.total_marketplace_revenue_share_allocation_count
        ),
        ("4", 1),
        "expected a replay to have written nothing, got {share:?}"
    );
}

// ---------------------------------------------------------------------------
// The audit record every state-changing operation owes.
// ---------------------------------------------------------------------------

/// One captured `tracing` event: the target it was emitted on and its fields.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: std::collections::BTreeMap<String, String>,
}

/// Collects an event's fields as strings, whatever type each was recorded as.
#[derive(Default)]
struct FieldCollector(std::collections::BTreeMap<String, String>);

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A subscriber that keeps every event it is handed, so this suite can assert on
/// the audit records the service emits. Written over `tracing` itself, which this
/// crate already depends on.
struct CapturingSubscriber {
    events: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        self.events
            .lock()
            .expect("the captured events")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields: fields.0,
            });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The sink the capturing subscriber writes into: every record this test binary
/// emits, from any suite running in parallel, which is why the assertions match
/// each record by the resource it names.
static CAPTURED_EVENTS: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<CapturedEvent>>>> =
    std::sync::OnceLock::new();

/// Install the capturing subscriber as this process's default, once.
fn install_the_audit_capture() -> Arc<std::sync::Mutex<Vec<CapturedEvent>>> {
    let events =
        Arc::clone(CAPTURED_EVENTS.get_or_init(|| Arc::new(std::sync::Mutex::new(Vec::new()))));
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let subscriber = CapturingSubscriber {
            events: Arc::clone(&events),
        };
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
    events
}

/// The audit records captured so far.
fn captured_audit_records() -> Vec<CapturedEvent> {
    let events = install_the_audit_capture();
    let events = events.lock().expect("the captured events");
    events
        .iter()
        .filter(|event| event.target == prm::state::AUDIT_TARGET)
        .cloned()
        .collect()
}

/// Every operation that CHANGES stored state emits the structured audit record
/// it owes — the operation, the resource it changed, the outcome, the Region and
/// the access key id the caller presented — and no secret alongside it. A read
/// changes nothing, so it owes no record.
#[tokio::test]
async fn state_changing_operations_emit_an_audit_record() {
    install_the_audit_capture();
    // Its own Region, so every record this suite asserts on is its own: the
    // Region is the ARN segment that distinguishes it from the identical ids
    // another suite's store mints.
    let emulator = Emulator::in_region(AUDIT_REGION);
    let share: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            client_token: Some("audit-share".to_string()),
            products: vec![product("prod-aud1234567890")],
            ..Default::default()
        })
        .await
        .ok("the share create");
    // The idempotent re-send writes nothing, and says so.
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            client_token: Some("audit-share".to_string()),
            products: vec![product("prod-aud1234567890")],
            ..Default::default()
        })
        .await
        .ok("the replayed share create");
    let updated: UpdateMarketplaceRevenueShareOutput = emulator
        .update_share(&UpdateMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: share.id.clone(),
            revision: share.revision.clone(),
            products: vec![product("prod-aud2222222222")],
            ..Default::default()
        })
        .await
        .ok("the share update");
    let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            "AWS",
            &share.id,
            &updated.latest_revision,
            FUTURE_FROM,
            FUTURE_UNTIL,
            "12.5",
        ))
        .await
        .ok("the allocation create");
    let _: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: "AWS".to_string(),
            marketplace_revenue_share_identifier: share.id.clone(),
            marketplace_revenue_share_allocation_id: allocation
                .marketplace_revenue_share_allocation_id
                .clone(),
            marketplace_revenue_share_revision: allocation
                .marketplace_revenue_share_latest_revision
                .clone(),
            revenue_share_percent: Some("20".to_string()),
            ..Default::default()
        })
        .await
        .ok("the allocation update");
    // A read: it owes no record, which the count below pins.
    let _: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            identifier: share.id.clone(),
            revision: None,
        })
        .await
        .ok("the share read");

    let records = captured_audit_records();
    let mine: Vec<&CapturedEvent> = records
        .iter()
        .filter(|record| {
            record.fields.get("region").map(String::as_str) == Some(AUDIT_REGION)
        })
        .collect();
    let allocation_resource = format!(
        "{}/allocation/{}",
        share.arn, allocation.marketplace_revenue_share_allocation_id
    );

    // (case, the operation, the resource it names, the outcome)
    for (case, operation, resource, outcome) in [
        (
            "the share create",
            "CreateMarketplaceRevenueShare",
            share.arn.clone(),
            "created",
        ),
        (
            "the replayed share create",
            "CreateMarketplaceRevenueShare",
            share.arn.clone(),
            "replayed: no change",
        ),
        (
            "the share update",
            "UpdateMarketplaceRevenueShare",
            share.arn.clone(),
            "updated",
        ),
        (
            "the allocation create",
            "CreateMarketplaceRevenueShareAllocation",
            allocation_resource.clone(),
            "created",
        ),
        (
            "the allocation update",
            "UpdateMarketplaceRevenueShareAllocation",
            share.arn.clone(),
            "updated",
        ),
    ] {
        let matching: Vec<&&CapturedEvent> = mine
            .iter()
            .filter(|record| {
                record.fields.get("operation").map(String::as_str) == Some(operation)
                    && record.fields.get("resource") == Some(&resource)
                    && record.fields.get("outcome").map(String::as_str) == Some(outcome)
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "case {case:?}: no audit record for {operation} on {resource} with outcome \
             {outcome:?}, got {mine:?}"
        );
        for record in matching {
            assert_eq!(
                record.fields.get("access_key_id").map(String::as_str),
                Some("AKIDEMULATOR"),
                "case {case:?}: the record must name the presented credential, got {record:?}"
            );
            assert_eq!(
                record.fields.get("region").map(String::as_str),
                Some(AUDIT_REGION),
                "case {case:?}: the record must name the Region, got {record:?}"
            );
        }
    }
    assert_eq!(
        mine.len(),
        5,
        "expected one record per state-changing call and none for the read, got {mine:?}"
    );
    assert!(
        !mine.iter().any(|record| {
            record
                .fields
                .values()
                .any(|value| value.contains("Signature") || value.contains("AWS4-HMAC"))
        }),
        "an audit record must carry no signature, got {mine:?}"
    );
}
