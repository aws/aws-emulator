// SPDX-License-Identifier: CC0-1.0

//! The portal-product resources of this service's store: a `PortalProduct`, the product
//! pages and product REST endpoint pages it OWNS, and the bounds, defaults and refusals
//! those three records share.
//!
//! A module of its own under [`crate::state`], because these records form one subtree:
//! a product owns its pages (`/v2/portalproducts/{PortalProductId}/productpages/{id}`
//! and `/v2/portalproducts/{PortalProductId}/productrestendpointpages/{id}`), so the
//! ARNs that name a page through its product, the per-product growth bounds and the
//! display content a page reports all belong beside the record that holds them.
//! [`crate::state`] re-exports every public item here, so a handler reaches them the way
//! it reaches the rest of this service's state helpers.

use super::*;

/// The most portal products one account may hold, after which CreatePortalProduct is
/// refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: it keeps a loop of
/// CreatePortalProduct calls from growing this store without end, and is set to the
/// per-parent bound this service already uses for the other collections it mints ids
/// into ([`MAX_MODELS_PER_API`]). `TooManyRequestsException` ("A limit has been
/// exceeded") is the modelled error the operation refuses with. Do not cite this as a
/// published portal-products quota.
pub const MAX_PORTAL_PRODUCTS: usize = 300;

/// The `PolicyDocument` a portal product's sharing policy reads as while the product
/// holds no policy of its own: the empty JSON policy document.
///
/// Every portal product HAS a sharing policy to report — a product nothing has shared is
/// one whose policy grants nothing, not one with no policy sub-resource — so a product
/// created without one, and one whose policy `DeletePortalProductSharingPolicy` reset,
/// reports a policy document carrying no statements rather than no document at all.
pub const EMPTY_SHARING_POLICY: &str = "{}";

/// The owners `ListPortalProducts$ResourceOwner` selects between, in the order a
/// refusal lists them.
///
/// The member targets `__string` in this service's model and documents only "The
/// resource owner of the portal product", so the catalog is not modelled here; API
/// Gateway spells the SAME `resourceOwner` query parameter as an enum of
/// `OTHER_ACCOUNTS | SELF` on its REST collections, and these are those two values. A
/// value outside the catalog names no owner, so the request is not valid
/// (`BadRequestException`) rather than served as one owner or the other.
pub const RESOURCE_OWNER_VALUES: &[&str] = &["OTHER_ACCOUNTS", "SELF"];

/// The `ResourceOwner` that selects the portal products of ANOTHER account, from
/// [`RESOURCE_OWNER_VALUES`].
///
/// A portal product becomes visible to another account through the product's SHARING
/// POLICY (`PutPortalProductSharingPolicy`), which is why the single-product readers take
/// an owning account id at all. This emulator serves exactly one account
/// ([`::engine::emulator_account_id`]) and every product it holds was created by that
/// account's own call, so nothing it holds has another owner: a request for this owner
/// gets an empty page rather than this account's own products reported as somebody
/// else's.
pub const OTHER_ACCOUNTS_RESOURCE_OWNER: &str = "OTHER_ACCOUNTS";

/// Which GROUPS of portal products a request's `ResourceOwner` selects.
///
/// The products an account can see fall into two groups — the ones it OWNS and the ones
/// other accounts have SHARED with it through a product's sharing policy
/// (`PutPortalProductSharingPolicy`) — and the collection reader enumerates the groups
/// the request selects one after the other, so which groups those are is what decides
/// both what a page reports and whether another group is left to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedProductOwners {
    /// Whether the walk reports the products THIS account owns.
    pub owned: bool,
    /// Whether the walk reports the products other accounts shared with this one.
    pub shared: bool,
}

/// Which groups of portal products a `ResourceOwner` selects, or the message for the
/// operation's `BadRequestException` when the value names no owner at all.
///
/// A request naming NO owner enumerates everything the account can see: its own products
/// and then those shared with it. Naming one owner restricts the walk to that owner's
/// group. TOTAL over its input, so every string a caller can send either selects a group
/// or is refused.
pub fn resource_owner_selects(requested: Option<&str>) -> Result<SelectedProductOwners, String> {
    match requested {
        None => Ok(SelectedProductOwners {
            owned: true,
            shared: true,
        }),
        Some(OTHER_ACCOUNTS_RESOURCE_OWNER) => Ok(SelectedProductOwners {
            owned: false,
            shared: true,
        }),
        Some(owner) if RESOURCE_OWNER_VALUES.contains(&owner) => Ok(SelectedProductOwners {
            owned: true,
            shared: false,
        }),
        Some(owner) => Err(format!(
            "Invalid value for ResourceOwner: '{owner}'. Valid values are: {}.",
            RESOURCE_OWNER_VALUES.join(", ")
        )),
    }
}

/// The pagination scope of the portal products this account OWNS: the first group
/// `ListPortalProducts` walks, and the collection whose `NextToken`s resume it.
pub fn owned_portal_products_scope() -> String {
    page_scope(&["portalproducts"])
}

/// The pagination scope of the portal products other accounts have SHARED with this one:
/// the group `ListPortalProducts` walks AFTER the account's own, so a token issued for it
/// is what starts that second group.
pub fn shared_portal_products_scope() -> String {
    page_scope(&["portalproducts", OTHER_ACCOUNTS_RESOURCE_OWNER])
}

/// The message a portal write naming an `IncludedPortalProductArns` entry of THIS account
/// that it holds no product under is refused with, or `None` when there is no such entry.
///
/// "The ARNs of the portal products included in the portal" name the products a published
/// portal RENDERS, so a portal cannot include one of the account's own products that does
/// not exist: the write is refused `BadRequestException` carrying the sentence this API
/// reports an unresolved product id with ([`portal_product_not_found_message`]), with the id
/// read out of the ARN the request sent. The FIRST unresolved entry is the one reported, in
/// the order the request lists them.
///
/// An ARN naming ANOTHER account's product is not resolved here and not refused: a portal
/// product is a shareable resource (`PutPortalProductSharingPolicy`, and
/// `GetPortalProduct$ResourceOwnerAccountId` reports the owner), so such an ARN is a
/// legitimate inclusion this store holds no record of — the same reading every other
/// cross-account question of this crate takes of a record it cannot see.
///
/// The ARNs have already been held to their FORM by the writer ([`validate_arns_of`]), so an
/// entry that is not a well-formed portal-product ARN never reaches this; one whose resource
/// path carries no id resolves to nothing like any other.
///
/// Shared by both writers of a portal's included products, so the create and the patch
/// cannot differ about which products a portal may include.
pub fn unresolved_included_portal_product<'a, I>(
    products: &BTreeMap<String, PortalProduct>,
    arns: I,
) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    for arn in arns {
        let Some(parsed) = ::service_common::arn::parse_of(arn, ARN_SERVICE) else {
            continue;
        };
        if parsed.account != ::engine::emulator_account_id() {
            continue;
        }
        let id = parsed
            .resource
            .strip_prefix(PORTAL_PRODUCT_RESOURCE_PREFIX)
            .unwrap_or_default();
        if !products.contains_key(id) {
            return Some(portal_product_not_found_message(id));
        }
    }
    None
}

/// A persisted `PortalProduct` resource.
///
/// "Creates a new portal product" (`CreatePortalProduct`): an account-level resource,
/// NOT a child of an api, so it lives beside
/// [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`] rather than inside one.
#[derive(Debug, Clone)]
pub struct PortalProduct {
    pub portal_product_id: String,
    /// This product's position in the order the account created its portal products: the
    /// seed
    /// [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_product_id`] minted its
    /// id from, which counts the products created before it. `ListPortalProducts` reports
    /// the collection in this order (see [`CreatedInOrder`]), because a minted
    /// `PortalProductId` carries none.
    pub creation_order: u64,
    /// The ARN every reader reports, built once at create time by
    /// [`portal_product_arn`] from the Region the creating request addressed: an ARN
    /// names the Region its resource was created in, so it cannot be rebuilt per read
    /// from whatever Region a later caller signs for.
    pub portal_product_arn: String,
    pub display_name: String,
    pub description: Option<String>,
    /// "The visual ordering of the product pages and product REST endpoint pages in a
    /// published portal" (`PortalProduct$DisplayOrder`). A freshly created product owns
    /// no pages, so it has no ordering until one is configured.
    pub display_order: Option<wire::DisplayOrder>,
    /// "The timestamp when the portal product was last modified"
    /// (`PortalProduct$LastModified`) — the creation instant until something rewrites
    /// the record.
    pub last_modified: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// "The product sharing policy" (`PutPortalProductSharingPolicy$PolicyDocument`),
    /// the policy document the product's own sharing-policy sub-resource
    /// (`/v2/portalproducts/{PortalProductId}/sharingpolicy`) holds. `None` while the
    /// product carries no sharing policy — the state a freshly created product is in and
    /// the one `DeletePortalProductSharingPolicy` puts it back into, so the policy's
    /// absence is a value of this member rather than the absence of the product.
    pub sharing_policy: Option<String>,
    /// This product's product REST endpoint pages, keyed by
    /// `ProductRestEndpointPageId`. A page is addressed UNDER its product
    /// (`/v2/portalproducts/{PortalProductId}/productrestendpointpages/{id}`), so the
    /// product OWNS its pages the way an [`Api`] owns its routes: removing the product
    /// takes its pages with it, and no second map can drift out of step with it.
    /// `BTreeMap` for the stable key order a paginated listing resumes through.
    pub product_rest_endpoint_pages: BTreeMap<String, ProductRestEndpointPage>,
    /// Drives [`PortalProduct::mint_product_rest_endpoint_page_id`]. Per-product, for
    /// the same reason [`Api::route_id_seed`] is per-api: the id only has to be unique
    /// within the product that owns the page.
    pub product_rest_endpoint_page_id_seed: u64,
    /// This product's product pages, keyed by `ProductPageId`. A product page is
    /// addressed UNDER its product
    /// (`/v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}`), so the
    /// product OWNS its product pages the way it owns its product REST endpoint pages:
    /// removing the product takes them with it, and no second map can drift out of step
    /// with it. `BTreeMap` for the stable key order a paginated listing resumes through.
    pub product_pages: BTreeMap<String, ProductPage>,
    /// Drives [`PortalProduct::mint_product_page_id`]. Per-product, and separate from
    /// [`Self::product_rest_endpoint_page_id_seed`], so minting one kind of page never
    /// shifts the ids the other kind gets.
    pub product_page_id_seed: u64,
}

/// The members `CreatePortalProduct`, `GetPortalProduct` and `UpdatePortalProduct`
/// each report for one portal product — the three modelled responses carry exactly the
/// same set — so [`PortalProduct::to_wire`] projects the record ONCE and every reader
/// serializes that projection rather than assembling its own.
#[derive(Debug, Clone)]
pub struct PortalProductView {
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub display_order: Option<wire::DisplayOrder>,
    pub last_modified: Option<::aws_smithy_types::DateTime>,
    pub portal_product_arn: Option<String>,
    pub portal_product_id: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

impl PortalProduct {
    /// The `PolicyDocument` this product's sharing policy reports: the document stored
    /// by its last `PutPortalProductSharingPolicy`, or [`EMPTY_SHARING_POLICY`] while the
    /// product holds none.
    pub fn sharing_policy_document(&self) -> String {
        self.sharing_policy
            .clone()
            .unwrap_or_else(|| EMPTY_SHARING_POLICY.to_string())
    }
    /// Mint a fresh `ProductRestEndpointPageId` for this product: [`ACCOUNT_ID_LEN`]
    /// lowercase alphanumerics, never one this product already minted, and never one
    /// another product minted either — the product's own id is the SCOPE the id is drawn
    /// within. The member targets `__stringMin10Max30PatternAZ09` (len 10..30, pattern
    /// `^[a-z0-9]+$`), which [`mint_id`]'s lowercase alphanumeric id of that length
    /// satisfies by construction.
    pub fn mint_product_rest_endpoint_page_id(&mut self) -> String {
        self.product_rest_endpoint_page_id_seed =
            self.product_rest_endpoint_page_id_seed.wrapping_add(1);
        mint_id(
            IdKind::ProductRestEndpointPage,
            &self.portal_product_id,
            self.product_rest_endpoint_page_id_seed,
        )
    }

    /// Mint a fresh `ProductPageId` for this product: [`ACCOUNT_ID_LEN`] lowercase
    /// alphanumerics, never one this product already minted, and never one another
    /// product minted either — the product's own id is the SCOPE the id is drawn within.
    /// The member targets `__stringMin10Max30PatternAZ09` (len 10..30, pattern
    /// `^[a-z0-9]+$`), which [`mint_id`]'s lowercase alphanumeric id of that length
    /// satisfies by construction.
    pub fn mint_product_page_id(&mut self) -> String {
        self.product_page_id_seed = self.product_page_id_seed.wrapping_add(1);
        mint_id(
            IdKind::ProductPage,
            &self.portal_product_id,
            self.product_page_id_seed,
        )
    }

    /// Project the record onto the modelled `PortalProductSummary` shape the collection
    /// reader serializes — the SAME values [`Self::to_wire`] projects, so a listed
    /// product and the one the single-product reader reports cannot drift.
    ///
    /// `PortalProductSummary$Description` is a REQUIRED member of the summary
    /// (`smithy_query(query="shape", shape="PortalProductSummary")`) while
    /// `GetPortalProduct$Description` is optional, and its shape admits the empty string
    /// (`len 0..1024`): a product created without a description is summarised with no
    /// description text rather than with text this service invented.
    pub fn to_summary(&self) -> wire::PortalProductSummary {
        let view = self.to_wire();
        wire::PortalProductSummary {
            // [`Self::to_wire`] is what decides how a missing description and the tag
            // collection are reported, so the summary takes both from it rather than
            // deciding a second time.
            description: view.description.unwrap_or_default(),
            tags: view.tags,
            display_name: self.display_name.clone(),
            last_modified: to_timestamp(self.last_modified),
            portal_product_arn: self.portal_product_arn.clone(),
            portal_product_id: self.portal_product_id.clone(),
        }
    }

    /// Project the record onto the members every portal-product reader reports.
    pub fn to_wire(&self) -> PortalProductView {
        PortalProductView {
            description: self.description.clone(),
            display_name: Some(self.display_name.clone()),
            display_order: Some(display_order_view(self.display_order.as_ref())),
            last_modified: Some(to_timestamp(self.last_modified)),
            portal_product_arn: Some(self.portal_product_arn.clone()),
            portal_product_id: Some(self.portal_product_id.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
}

impl CreatedInOrder for PortalProduct {
    fn creation_order(&self) -> u64 {
        self.creation_order
    }
}

/// The `DisplayOrder` a portal product reports, from the one it stores.
///
/// A product ALWAYS has a display order to report: "the visual ordering of the product
/// pages and product REST endpoint pages in a published portal"
/// (`PortalProduct$DisplayOrder`) of a product whose pages nothing has ordered is the
/// EMPTY ordering, not the absence of one — so its two collection members
/// (`DisplayOrder$Contents`, `DisplayOrder$ProductPageArns`) are reported as empty lists
/// rather than left out, and a stored order that named only one of them reports the other
/// the same way. `DisplayOrder$OverviewPageArn` is a single ARN rather than a collection,
/// so a product with no overview page reports none: an empty list is a collection holding
/// nothing, while there is no such thing as an ARN naming nothing.
fn display_order_view(stored: Option<&wire::DisplayOrder>) -> wire::DisplayOrder {
    let stored = stored.cloned().unwrap_or_default();
    wire::DisplayOrder {
        contents: Some(stored.contents.unwrap_or_default()),
        overview_page_arn: stored.overview_page_arn,
        product_page_arns: Some(stored.product_page_arns.unwrap_or_default()),
    }
}

/// The most product pages one portal product may hold, after which CreateProductPage is
/// refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: it keeps a loop of
/// CreateProductPage calls from growing one product without end, and is set to the
/// per-parent bound this service already uses for the other child collections it mints
/// ids into ([`MAX_PRODUCT_REST_ENDPOINT_PAGES_PER_PRODUCT`]). `TooManyRequestsException`
/// ("A limit has been exceeded") is the modelled error the operation refuses with. Do not
/// cite this as a published quota.
pub const MAX_PRODUCT_PAGES_PER_PRODUCT: usize = 300;

/// The resource portion a product page's ARN carries after its product's: API Gateway
/// names a resource by its own REST path, and a product page's path is
/// `/portalproducts/{portalProductId}/productpages/{productPageId}`.
pub const PRODUCT_PAGE_RESOURCE_SEGMENT: &str = "/productpages/";

/// The ARN of a stored product page.
///
/// Built like every other ARN this store names — through [`::engine::emulator_arn`] under
/// [`ARN_SERVICE`], with the resource portion the page's own REST path (see
/// [`PRODUCT_PAGE_RESOURCE_SEGMENT`]) — so it names the page's product as well as the
/// page, and satisfies the modelled `len 20..2048` of `ProductPageArn`.
pub fn product_page_arn(region: &str, portal_product_id: &str, product_page_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{PORTAL_PRODUCT_RESOURCE_PREFIX}{portal_product_id}\
             {PRODUCT_PAGE_RESOURCE_SEGMENT}{product_page_id}"
        ),
    )
}

/// A persisted `ProductPage` resource: one page of free-form content a published portal
/// shows for a portal product.
///
/// Owned by its [`PortalProduct`] (see [`PortalProduct::product_pages`]). The record
/// keeps what the write RESOLVED — the id and ARN it minted, the display content the
/// request carried, the instant the page was last modified — so every reader of the page
/// reports one value.
#[derive(Debug, Clone)]
pub struct ProductPage {
    pub product_page_id: String,
    /// Built once at write time by [`product_page_arn`] from the Region the creating
    /// request addressed: an ARN names the Region its resource was created in, so it
    /// cannot be rebuilt per read from whatever Region a later caller signs for.
    pub product_page_arn: String,
    /// "The content of the product page" (`CreateProductPage$DisplayContent`): a title
    /// and a body, both required on the modelled `DisplayContent`, so the record always
    /// carries both.
    pub display_content: wire::DisplayContent,
    /// "The timestamp when the product page was last modified" — the creation instant
    /// until something rewrites the record.
    pub last_modified: SystemTime,
}

/// The members `CreateProductPage`, `GetProductPage` and `UpdateProductPage` each report
/// for one product page — the three modelled responses carry exactly the same set — so
/// [`ProductPage::to_wire`] projects the record ONCE and every reader serializes that
/// projection rather than assembling its own.
#[derive(Debug, Clone)]
pub struct ProductPageView {
    pub display_content: Option<wire::DisplayContent>,
    pub last_modified: Option<::aws_smithy_types::DateTime>,
    pub product_page_arn: Option<String>,
    pub product_page_id: Option<String>,
}

impl ProductPage {
    /// Project the record onto the members every product-page reader reports.
    pub fn to_wire(&self) -> ProductPageView {
        ProductPageView {
            display_content: Some(self.display_content.clone()),
            last_modified: Some(to_timestamp(self.last_modified)),
            product_page_arn: Some(self.product_page_arn.clone()),
            product_page_id: Some(self.product_page_id.clone()),
        }
    }
}

/// The most product REST endpoint pages one portal product may hold, after which
/// CreateProductRestEndpointPage is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: it keeps a loop of
/// CreateProductRestEndpointPage calls from growing one product without end, and is set
/// to the per-parent bound this service already uses for the other child collections it
/// mints ids into ([`MAX_ROUTES_PER_API`]). `TooManyRequestsException` ("A limit has
/// been exceeded") is the modelled error the operation refuses with. Do not cite this as
/// a published quota.
pub const MAX_PRODUCT_REST_ENDPOINT_PAGES_PER_PRODUCT: usize = 300;

/// The `Status` a resource carries while the service is still assembling it, from
/// `STATUS_VALUES` (`AVAILABLE | FAILED | IN_PROGRESS`).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `Status` a resource carries once the service finished assembling it, from
/// `STATUS_VALUES`.
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `Status` a resource carries when the service could not assemble it, from
/// `STATUS_VALUES`. It is the status the modelled `StatusException` explains — and no
/// product REST endpoint page this store holds reaches it, for the reason
/// [`ProductRestEndpointPage::status`] states.
pub const STATUS_FAILED: &str = "FAILED";

/// How long a product REST endpoint page reports [`STATUS_IN_PROGRESS`] after a write that
/// generates its content, before it reports [`STATUS_AVAILABLE`]: the page's content is
/// generated from the REST endpoint it documents, which is not done when the call answers —
/// the reason the modelled response carries a `Status` at all.
///
/// AWS documents no duration for it, so this span is the emulator's own, chosen short
/// enough not to slow a caller down and long enough that a caller reading straight after
/// the write still observes the transient status. It is the span
/// [`VPC_LINK_CREATE_SPAN`] uses for the same kind of transition.
pub const PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN: Duration = Duration::from_millis(500);

/// The `TryItState` a product REST endpoint page gets when the request names none, from
/// `TRY_IT_STATE_VALUES` (`DISABLED | ENABLED`).
///
/// The member is REQUIRED on the modelled page summary, so a page always reports one,
/// and the request may leave it out; `DISABLED` is the CHOSEN default, since a caller
/// that did not ask for the portal's try-it console to invoke this endpoint has not asked
/// for it to be invocable from the portal.
pub const DEFAULT_TRY_IT_STATE: &str = "DISABLED";

/// The resource portion a product REST endpoint page's ARN carries after its product's:
/// API Gateway names a resource by its own REST path, and a page's path is
/// `/portalproducts/{portalProductId}/productrestendpointpages/{pageId}`.
pub const PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT: &str = "/productrestendpointpages/";

/// The sentence this service answers `NotFoundException` with for a request addressing a
/// product REST endpoint page no record holds, whatever operation addressed it.
///
/// The portal resources of this API answer an unresolved identifier by naming the resource
/// type alone, rather than with the `Invalid {resource} identifier specified
/// {account}:{identifier}` sentence the older api-level resources use (see
/// [`invalid_identifier_message`]) — so the sentence names neither the account nor the
/// identifiers the request carried. Held here once so the wording cannot drift between the
/// readers and the writers of one page.
pub const PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND_MESSAGE: &str = "ProductRestEndpointPage not found";

/// The ARN of a stored product REST endpoint page.
///
/// Built like every other ARN this store names — through [`::engine::emulator_arn`] under
/// [`ARN_SERVICE`], with the resource portion the page's own REST path (see
/// [`PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT`]) — so it names the page's product as
/// well as the page, and satisfies the modelled `len 20..2048` of
/// `ProductRestEndpointPageArn`.
pub fn product_rest_endpoint_page_arn(
    region: &str,
    portal_product_id: &str,
    product_rest_endpoint_page_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{PORTAL_PRODUCT_RESOURCE_PREFIX}{portal_product_id}\
             {PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT}{product_rest_endpoint_page_id}"
        ),
    )
}

/// The longest `Endpoint` a page's display content may report: the modelled
/// `EndpointDisplayContentResponse$Endpoint` is `len 1..1024`.
pub const DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH: usize = 1024;

/// The identifier parts of the REST endpoint a `RestEndpointIdentifier` names, or the
/// refusal of one that names no endpoint.
///
/// `RestEndpointIdentifier` is a structure with exactly one alternative,
/// `IdentifierParts` ("The identifier parts of the REST endpoint identifier"), so an
/// identifier carrying none identifies nothing to document — incomplete input, which is
/// what `BadRequestException` reports. The wording is
/// [`exclusive_choice_required`]'s, so this refusal reads like the service's other
/// one-of refusals.
pub fn rest_endpoint_identifier_parts(
    identifier: &wire::RestEndpointIdentifier,
) -> Result<&wire::IdentifierParts, String> {
    match identifier.identifier_parts.as_ref() {
        Some(parts) => Ok(parts),
        None => Err(exclusive_choice_required(
            "RestEndpointIdentifier",
            &["IdentifierParts"],
        )),
    }
}

/// The URL a consumer invokes the REST API a page documents on.
///
/// "The URL to invoke your REST API" (`EndpointDisplayContentResponse$Endpoint`): what a
/// page reports is the INVOKE URL OF THE STAGE — the endpoint the identified API is
/// ANNOUNCED on, followed by the stage the endpoint is deployed to. The remaining parts of
/// an `IdentifierParts` select WHICH endpoint of that API the page documents rather than
/// where the API is served: neither the method (which is not part of a URL at all) nor the
/// endpoint's own `Path` is appended, so what is reported is the base a client calls the
/// API on, as every portal page of one stage reports the same base.
///
/// The endpoint is the one a consumer DIALS — it is the address a published portal sends
/// them to — so which address it names follows the one reading this service applies to
/// every value it announces for a client to reach ([`announced_execute_api_endpoint`]): the
/// emulator's own endpoint with the API behind the [`LOCAL_ENDPOINT_PREFIX`] marker for a
/// client that resolves nothing else, and the `{api-id}.execute-api.{region}.amazonaws.com`
/// host for a client that resolves AWS's own hosts. A REST (API Gateway v1) stage of this
/// emulator is served at exactly that marker
/// (`/_aws/execute-api/{api-id}/{stage}/{resource-path}`), so the address published for the
/// page is the one its stage answers on.
pub fn rest_endpoint_invoke_url(
    parts: &wire::IdentifierParts,
    region: &str,
    request: &::http::request::Parts,
    served_base: &str,
) -> String {
    format!(
        "{}/{}",
        announced_execute_api_endpoint(
            &parts.rest_api_id,
            region,
            HTTP_PROTOCOL_TYPE,
            request,
            served_base,
        ),
        parts.stage
    )
}

/// Resolve the display content a product REST endpoint page reports from the content the
/// request asked for.
///
/// "Contains any values that override the default configuration generated from API
/// Gateway" (`DisplayContentOverrides`): each member of the page's display content is
/// the value the request overrode it with, or the default API Gateway derives from the
/// REST endpoint the page documents. `Endpoint` is required on the response, and its
/// default is that endpoint's own invoke URL ("By default, API Gateway uses the default
/// execute API endpoint. You can provide a custom domain to override this value",
/// `DisplayContentOverrides$Endpoint`). `Body` ("By default, this is the documentation of
/// your REST API from API Gateway") and `OperationName` default to documentation that
/// lives in the REST API itself — a resource of the REST (API Gateway v1) API this
/// emulator does not hold — so nothing is invented for them and they are reported only
/// where the request supplied them.
///
/// `requested` is the request's `DisplayContent`, absent for a request that named none —
/// which says the same as its `None` alternative: this endpoint carries no overrides.
/// `request` and `served_base` are the WRITING request's parts and this emulator's own
/// endpoint base, which is what a derived endpoint is announced on (see
/// [`rest_endpoint_invoke_url`]); an overriding endpoint is the caller's own value and
/// names whatever the caller named. `Err` carries the message for the operation's
/// `BadRequestException`.
pub fn resolve_endpoint_display_content(
    region: &str,
    parts: &wire::IdentifierParts,
    requested: Option<&wire::EndpointDisplayContent>,
    request: &::http::request::Parts,
    served_base: &str,
) -> Result<wire::EndpointDisplayContentResponse, String> {
    let overrides = match requested {
        None => None,
        Some(content) => {
            validate_exclusive_choice(
                "DisplayContent",
                &[
                    ("None", content.none.is_some()),
                    ("Overrides", content.overrides.is_some()),
                ],
            )?;
            content.overrides.as_ref()
        }
    };

    let endpoint = match overrides.and_then(|overrides| overrides.endpoint.clone()) {
        Some(endpoint) => endpoint,
        None => {
            let derived = rest_endpoint_invoke_url(parts, region, request, served_base);
            // An overriding `Endpoint` is bounded by the model (len 1..1024) and refused
            // at the boundary, but a DERIVED one is this service's own value: the parts
            // it is built from are bounded far wider (a `Path` alone is len 1..4096), so
            // a request naming an endpoint whose invoke URL cannot be reported is refused
            // rather than answered with a response the model does not allow.
            if derived.len() > DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH {
                return Err(format!(
                    "Invalid value for RestEndpointIdentifier: the invoke URL of the \
                     identified REST endpoint is {} characters, past the \
                     {DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH} an endpoint may carry.",
                    derived.len()
                ));
            }
            derived
        }
    };

    Ok(wire::EndpointDisplayContentResponse {
        body: overrides.and_then(|overrides| overrides.body.clone()),
        endpoint,
        operation_name: overrides.and_then(|overrides| overrides.operation_name.clone()),
    })
}

/// A persisted `ProductRestEndpointPage` resource: one REST endpoint of a portal
/// product, as a published portal documents it.
///
/// Owned by its [`PortalProduct`] (see
/// [`PortalProduct::product_rest_endpoint_pages`]). The record keeps what the write
/// RESOLVED — the id and ARN it minted, the display content it resolved from the
/// request's overrides and the identified endpoint, the instant the page was last
/// modified — so every reader of the page reports one value.
#[derive(Debug, Clone)]
pub struct ProductRestEndpointPage {
    pub product_rest_endpoint_page_id: String,
    /// Built once at write time by [`product_rest_endpoint_page_arn`] from the Region the
    /// creating request addressed: an ARN names the Region its resource was created in,
    /// so it cannot be rebuilt per read from whatever Region a later caller signs for.
    pub product_rest_endpoint_page_arn: String,
    /// The Region the creating request addressed, kept beside the ARN that names it: a
    /// later WRITE that re-resolves this page's display content derives the identified
    /// endpoint's invoke URL in the page's OWN Region, rather than in whatever Region the
    /// writing caller happened to sign for.
    pub region: String,
    /// The REST endpoint this page documents, as the request identified it.
    pub rest_endpoint_identifier: wire::RestEndpointIdentifier,
    /// The display content this page reports, resolved by
    /// [`resolve_endpoint_display_content`] at every WRITE rather than per read, so the
    /// endpoint it names does not follow the Region a later reader signs for.
    pub display_content: wire::EndpointDisplayContentResponse,
    /// "The timestamp when the product REST endpoint page was last modified" — the
    /// creation instant until something rewrites the record.
    pub last_modified: SystemTime,
    /// One of `TRY_IT_STATE_VALUES`; [`DEFAULT_TRY_IT_STATE`] for a page whose create
    /// named none.
    pub try_it_state: String,
    /// When the generation of this page's content — started by the write that stored this
    /// record — comes due, after which the page reports [`STATUS_AVAILABLE`]; before it,
    /// [`STATUS_IN_PROGRESS`]. See [`ProductRestEndpointPage::status`] and
    /// [`PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN`].
    pub settles_at: Instant,
}

/// The members `CreateProductRestEndpointPage`, `GetProductRestEndpointPage` and
/// `UpdateProductRestEndpointPage` each
/// report for one page — the Get response carries the create's members plus
/// `RawDisplayContent` — so [`ProductRestEndpointPage::to_wire`] projects the record
/// ONCE and every reader serializes that projection rather than assembling its own.
#[derive(Debug, Clone)]
pub struct ProductRestEndpointPageView {
    pub display_content: Option<wire::EndpointDisplayContentResponse>,
    pub last_modified: Option<::aws_smithy_types::DateTime>,
    pub product_rest_endpoint_page_arn: Option<String>,
    pub product_rest_endpoint_page_id: Option<String>,
    pub rest_endpoint_identifier: Option<wire::RestEndpointIdentifier>,
    pub status: Option<String>,
    pub status_exception: Option<wire::StatusException>,
    pub try_it_state: Option<String>,
}

impl ProductRestEndpointPage {
    /// The `Status` this page reports now.
    ///
    /// A page's content is still being generated when the write that started it answers, so
    /// it is reported [`STATUS_IN_PROGRESS`] until
    /// [`PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN`] has passed, and [`STATUS_AVAILABLE`]
    /// after that. The transition is one the CLOCK owes, materialised from the stored due
    /// instant rather than by sleeping or by being read, so every reader agrees on it and a
    /// read changes nothing.
    ///
    /// No page this store holds settles [`STATUS_FAILED`]: the generation is from the REST
    /// endpoint the page's identifier names, and a write whose identifier names no such
    /// endpoint is REFUSED ([`rest_endpoint::UNRESOLVED_REST_ENDPOINT_MESSAGE`]) rather than
    /// stored as a page that then fails — so every stored page was generated from an
    /// endpoint that resolved.
    pub fn status(&self) -> &'static str {
        if Instant::now() < self.settles_at {
            STATUS_IN_PROGRESS
        } else {
            STATUS_AVAILABLE
        }
    }

    /// Start the generation of this page's content, as of `now`: it comes due one
    /// [`PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN`] from here.
    ///
    /// This is how a write that REWRITES an existing record starts it over (the update);
    /// the create sets this same field as it constructs the record it stores, since there is
    /// no earlier generation there to replace.
    pub fn start_generation(&mut self, now: Instant) {
        self.settles_at = now + PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN;
    }

    /// The page's content as it was supplied, for the reader that asks for it
    /// (`GetProductRestEndpointPage$IncludeRawDisplayContent`): the body of the page,
    /// before a portal renders it. A page whose body is API Gateway's own documentation
    /// of the REST API carries none here, for the reason
    /// [`resolve_endpoint_display_content`] gives.
    pub fn raw_display_content(&self) -> Option<String> {
        self.display_content.body.clone()
    }

    /// Project the record onto the members every page reader reports.
    pub fn to_wire(&self) -> ProductRestEndpointPageView {
        ProductRestEndpointPageView {
            display_content: Some(self.display_content.clone()),
            last_modified: Some(to_timestamp(self.last_modified)),
            product_rest_endpoint_page_arn: Some(self.product_rest_endpoint_page_arn.clone()),
            product_rest_endpoint_page_id: Some(self.product_rest_endpoint_page_id.clone()),
            rest_endpoint_identifier: Some(self.rest_endpoint_identifier.clone()),
            status: Some(self.status().to_string()),
            // "The status exception information" reports what went WRONG with a page the
            // service could not assemble, and no page this store holds is one: a write whose
            // `RestEndpointIdentifier` names no REST endpoint is refused rather than stored
            // (see [`Self::status`]), so there is never anything to explain.
            status_exception: None,
            try_it_state: Some(self.try_it_state.clone()),
        }
    }
}
