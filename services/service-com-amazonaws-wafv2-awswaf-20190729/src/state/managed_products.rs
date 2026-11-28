// SPDX-License-Identifier: CC0-1.0

//! The managed PRODUCTS this service publishes: both vendor families of managed
//! rule group — the Amazon Web Services Managed Rules rule sets this service
//! catalogues ([`MANAGED_RULE_GROUP_CATALOGUE`]) and the Amazon Web Services
//! Marketplace sellers' listings — and what one of them is reported as.
//!
//! `ManagedProductDescriptor` is "the properties of a managed product, such as an
//! Amazon Web Services Managed Rules rule group OR an Amazon Web Services
//! Marketplace managed rule group" (the shape's own doc), and
//! `DescribeAllManagedProducts` "provides high-level information for the Amazon Web
//! Services Managed Rules rule groups AND Amazon Web Services Marketplace managed
//! rule groups" (the operation's), so the product catalogue is the UNION of the two
//! families rather than either one alone.
//!
//! The two families differ in what a descriptor carries, which is why a seller's
//! listing is held here rather than as another [`ManagedRuleGroupDescriptor`]:
//!
//! * `ProductLink` is "for Amazon Web Services Marketplace managed rule groups
//!   only, the link to the rule group product page", so only a seller's listing
//!   carries one.
//! * `SnsTopicArn` is the topic "used to provide notification of changes to the
//!   managed rule group", and the vendor that publishes the group owns that topic —
//!   one per VENDOR, in that vendor's own account, so each seller has its own and
//!   not the Amazon Web Services Managed Rules one.
//! * A seller's listing is not something this account may RUN:
//!   `ListAvailableManagedRuleGroups` lists "all Amazon Web Services Managed Rules
//!   rule groups and all of the Amazon Web Services Marketplace managed rule groups
//!   that you're SUBSCRIBED to" (its own doc), and nothing here subscribes this
//!   account to a seller — so a listing is reported as a product, is not listed as
//!   available, and a rule that runs one is refused as unsubscribed
//!   ([`unsubscribed_managed_rule_group`]).

use super::*;

/// The address an Amazon Web Services Marketplace listing's product page is
/// published under, without its scheme or its listing segment.
///
/// `ManagedProductDescriptor$ProductLink` is "the link to the rule group product
/// page" of a Marketplace managed rule group, and every such page is published on the
/// Marketplace at this address followed by the identifier the listing is published as.
///
/// What it does NOT state is the whole link: the published catalogue carries a page's
/// own address verbatim, and those addresses differ in their scheme and in the form of
/// the segment they end with (a listing code, a product view), so each listing cites
/// its link rather than this constant composing one
/// ([`MarketplaceManagedProduct::product_link`]). The addresses AS PUBLISHED: a
/// product page is the Marketplace's own page and the link is catalogue reference
/// data, so nothing here normalises one. The URLs this service rewrites are the ones a
/// CALLER fetches from this emulator; a vendor's product page is neither of those.
pub const MARKETPLACE_PRODUCT_PAGE_ADDRESS: &str = "aws.amazon.com/marketplace/pp";

/// An Amazon Web Services Marketplace SELLER of managed rule groups: the name its
/// groups are published under and the topic it announces changes to them on.
///
/// A vendor is identified by its name (`VendorName` is "the name of the managed
/// rule group vendor. You use this, along with the rule group name, to identify a
/// rule group") and announces changes to everything it publishes on ONE topic, the
/// way Amazon Web Services Managed Rules do
/// ([`AWS_MANAGED_RULE_NOTIFICATION_TOPIC`]) — so the pair sits beside the seller
/// rather than being repeated on each of its listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceSeller {
    /// The seller's own name, as its listings publish it (`VendorName`:
    /// `length 1..128`, pattern `\S`).
    pub vendor_name: &'static str,
    /// The name of the topic this seller announces changes to its managed rule
    /// groups on, which `SnsTopicArn` reports the ARN of.
    pub notification_topic: &'static str,
    /// The account [`Self::notification_topic`] lives in, where this catalogue can
    /// cite the one the published catalogue states, and `None` where it cannot.
    ///
    /// A vendor's notification topic is the VENDOR's resource, published by the
    /// service rather than created by this account, so its ARN names the publishing
    /// vendor's account and not the caller's — citing it is what lets a subscriber
    /// follow that vendor's changes. Cited, never composed, like the rest of a
    /// listing: a seller whose account no source available here states reports its
    /// topic in this emulator's own account instead (see
    /// [`Self::notification_topic_arn`]) rather than under an account number nobody
    /// published.
    pub notification_topic_account: Option<&'static str>,
}

impl MarketplaceSeller {
    /// The ARN of the topic this seller announces changes to its managed rule
    /// groups on: the seller's own topic, in the seller's own account where this
    /// catalogue cites it.
    pub fn notification_topic_arn(&self) -> String {
        vendor_notification_topic_arn(self.notification_topic_account, self.notification_topic)
    }
}

/// One Amazon Web Services Marketplace managed rule group, as its seller lists it.
///
/// Flat and `Copy` like [`ManagedRuleGroupDescriptor`], and for the same reason: a
/// listing is published data rather than state this account owns — nothing creates,
/// updates or deletes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketplaceManagedProduct {
    /// The seller that publishes this listing.
    pub seller: &'static MarketplaceSeller,
    /// The name of the managed rule group the listing offers
    /// (`ManagedProductDescriptor$ManagedRuleSetName`, an `EntityName`:
    /// `length 1..128`, pattern `^[\w\-]+$`), in the seller's own naming form.
    pub managed_rule_set_name: &'static str,
    /// The identifier the seller publishes this listing's product under
    /// (`ProductId`: `length 1..128`, pattern `\S`), where this catalogue can cite
    /// it; `None` where it cannot, and then the identifier this catalogue publishes
    /// for the identifying pair stands in ([`managed_product_id`]).
    ///
    /// A product's identifier is assigned by the vendor that publishes it, so it is
    /// cited beside the rest of the listing rather than computed from the listing's
    /// name.
    pub product_id: Option<&'static str>,
    /// The listing's display name (`ProductTitle` is "the display name for the
    /// managed rule group"), in the seller's published words.
    ///
    /// Cited, never composed: a seller's listing text is the seller's. A listing
    /// whose own title no source available here states is absent from this catalogue
    /// rather than reported with a text nobody published — the same reading
    /// [`PublishedManagedRule::published_label`] takes of a label.
    pub product_title: &'static str,
    /// The listing's one-line summary (`ProductDescription` is "a short description
    /// of the managed rule group"), in the seller's published words, where this
    /// catalogue cites it; `None` where it cannot, and then the member is left out of
    /// the descriptor.
    ///
    /// Cited, never composed: a seller's listing text is the seller's, so a summary no
    /// source available here states is omitted rather than worded by this emulator —
    /// the same reading [`PublishedManagedRule::published_label`] takes of a label.
    pub product_description: Option<&'static str>,
    /// The link to this listing's product page on Amazon Web Services Marketplace
    /// (`ProductLink`), exactly as the catalogue publishes it.
    ///
    /// Cited whole rather than composed from
    /// [`MARKETPLACE_PRODUCT_PAGE_ADDRESS`] and a page segment, because the published
    /// links differ in their scheme and in the form of the segment the page is
    /// addressed by (a listing code, a product view) — both of which are the
    /// Marketplace's own and read nothing like the group's name or its identifier.
    pub product_link: &'static str,
    /// Whether the seller offers this group in named versions a web ACL can choose
    /// between (`IsVersioningSupported`), reported as stored like the same property
    /// of an Amazon Web Services Managed Rules group
    /// ([`ManagedRuleGroupDescriptor::is_versioning_supported`]).
    pub is_versioning_supported: bool,
}

impl MarketplaceManagedProduct {
    /// This listing as the managed PRODUCT a describe reports it.
    ///
    /// `IsAdvancedManagedRuleSet` is "whether the rule group provides an advanced
    /// set of protections, such as the Amazon Web Services Managed Rules rule groups
    /// that are used for WAF intelligent threat mitigation" (the member's own doc):
    /// those groups are the advanced ones ([`advanced`]), so a seller's listing
    /// reports `false`.
    pub fn product(&self) -> ManagedProductDescriptor {
        ManagedProductDescriptor {
            vendor_name: Some(self.seller.vendor_name.to_string()),
            managed_rule_set_name: Some(self.managed_rule_set_name.to_string()),
            // The identifier its publisher states for it, and otherwise the one this
            // catalogue publishes for the pair — what [`managed_product_id`] answers
            // for this product, read off the entry rather than looked up again.
            product_id: Some(self.product_id.map(str::to_string).unwrap_or_else(|| {
                product_id_of_pair(self.seller.vendor_name, self.managed_rule_set_name)
            })),
            product_link: Some(self.product_link.to_string()),
            product_title: Some(self.product_title.to_string()),
            product_description: self.product_description.map(str::to_string),
            sns_topic_arn: Some(self.seller.notification_topic_arn()),
            is_versioning_supported: self.is_versioning_supported,
            is_advanced_managed_rule_set: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The Marketplace sellers, one constant each, and their listings below
// ([`MARKETPLACE_MANAGED_PRODUCT_CATALOGUE`]). A seller publishes the same
// listings whatever `Scope` a caller asks in, so the catalogue is one list rather
// than one per namespace.
// ---------------------------------------------------------------------------

/// The seller of the `CyberSecurityCloud-` managed rule groups, with the topic it
/// announces changes to them on and the account that topic lives in.
const CYBER_SECURITY_CLOUD: MarketplaceSeller = MarketplaceSeller {
    vendor_name: "Cyber Security Cloud Inc.",
    notification_topic: "CyberSecurityCloud-ManagedRule",
    notification_topic_account: Some("343255486711"),
};

/// The seller of the `Fortinet` managed rule groups, with the topic it announces
/// changes to them on and the account that topic lives in.
///
/// A second seller, so the catalogue's per-VENDOR facts — the vendor name a group is
/// identified by and the one topic that vendor announces its changes on — are carried
/// per seller rather than per listing.
const FORTINET: MarketplaceSeller = MarketplaceSeller {
    vendor_name: "Fortinet",
    notification_topic: "Fortinet_OWASP_Top_10_Notifications",
    notification_topic_account: Some("040422370703"),
};

/// The Amazon Web Services Marketplace managed rule groups this catalogue
/// publishes, grouped by seller.
///
/// The second family `DescribeAllManagedProducts` reports: third-party sellers'
/// listings in the Marketplace's web application firewall category, each offering a
/// managed rule group a subscriber may reference by the seller's
/// `(VendorName, Name)` pair.
///
/// A seller's inventory is the SELLER's publication, so every entry here is CITED
/// rather than composed: the vendor name, the group name, the identifier and the
/// listing text the seller publishes the product under, the address of its product
/// page and the seller's notification topic. A listing this
/// catalogue cannot cite is absent rather than invented, and so is any part of one it
/// cannot cite (a summary no source states is omitted) — adding a seller's listing
/// is a data change in this one list. Listed in the order the service's catalogue
/// publishes them.
pub const MARKETPLACE_MANAGED_PRODUCT_CATALOGUE: &[MarketplaceManagedProduct] = &[
    listing(
        &CYBER_SECURITY_CLOUD,
        "CyberSecurityCloud-APIGatewayServerless-",
        Some("8bbf295b-34b3-40c0-bee7-5ba8f602705f"),
        "Cyber Security Cloud Managed Rules for AWS WAF -API Gateway/Serverless-",
        Some(
            "The Cyber Security Cloud OWASP ruleset is designed to mitigate and \
             minimize vulnerabilities, including all those on OWASP API \
             Security/Serverless Top 10 Threats.",
        ),
        "http://aws.amazon.com/marketplace/pp/B081TKTBQ8",
    ),
    listing(
        &CYBER_SECURITY_CLOUD,
        "CyberSecurityCloud-HighSecurityOWASPSet-",
        Some("fa8c6ca7-12c3-4e3f-8524-782177f2f2bd"),
        "Cyber Security Cloud Managed Rules for AWS WAF -HighSecurity OWASP Set-",
        Some(
            "The Cyber Security Cloud OWASP ruleset is designed to mitigate and \
             minimize vulnerabilities, including all those on OWASP Top 10 Web \
             Application Threats list.",
        ),
        "http://aws.amazon.com/marketplace/pp/B081TD6XL1",
    ),
    listing(
        &CYBER_SECURITY_CLOUD,
        "CyberSecurityCloud-ProtocolEnforcement-",
        Some("prod-qqlwxtijbjsm2"),
        "Cyber Security Cloud Managed Rules for AWS WAF -Protocol Enforcement by \
         WafCharm-",
        None,
        "https://aws.amazon.com/marketplace/pp/prodview-g2dp6u5k5w5jo",
    ),
    listing(
        &FORTINET,
        "all_rules",
        Some("cdd10222-b7e0-466d-a710-6a35cbab6f9a"),
        "OWASP Top 10 - The Complete Ruleset",
        None,
        "http://aws.amazon.com/marketplace/pp/B081SK32C7",
    ),
];

/// One [`MARKETPLACE_MANAGED_PRODUCT_CATALOGUE`] entry: a seller's listing of a
/// managed rule group it offers in named versions.
///
/// A Marketplace seller publishes its rule group as a versioned managed rule set —
/// "vendors, you can use the managed rule set APIs to provide controlled rollout of
/// your VERSIONED managed rule group offerings for your customers"
/// (`ManagedRuleSetSummary`'s own doc, whose vendors "are Amazon Web Services and
/// Amazon Web Services Marketplace sellers") — so a listing built here reports its
/// versioning as supported, which is what tells a subscriber it can pin a version of
/// it.
const fn listing(
    seller: &'static MarketplaceSeller,
    managed_rule_set_name: &'static str,
    product_id: Option<&'static str>,
    product_title: &'static str,
    product_description: Option<&'static str>,
    product_link: &'static str,
) -> MarketplaceManagedProduct {
    MarketplaceManagedProduct {
        seller,
        managed_rule_set_name,
        product_id,
        product_title,
        product_description,
        product_link,
        is_versioning_supported: true,
    }
}

/// Every managed product this service publishes, in published order: the Amazon Web
/// Services Marketplace sellers' listings, then the Amazon Web Services Managed
/// Rules rule sets.
///
/// `DescribeAllManagedProducts` reports "the Amazon Web Services Managed Rules rule
/// groups and Amazon Web Services Marketplace managed rule groups" (its own doc), so
/// the report is the union of both families — one descriptor per product of either —
/// and a product of one family is reported exactly as that family publishes it (see
/// [`MarketplaceManagedProduct::product`] and
/// [`ManagedRuleGroupDescriptor::product`]).
///
/// The same products whatever `Scope` the request names: a product is a vendor's
/// publication, and a scope is only the namespace a request is made in.
pub fn managed_products() -> Vec<ManagedProductDescriptor> {
    MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
        .iter()
        .map(MarketplaceManagedProduct::product)
        .chain(
            MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .map(ManagedRuleGroupDescriptor::product),
        )
        .collect()
}

/// The managed products `vendor_name` publishes, in the same order and with the same
/// descriptors [`managed_products`] reports them in.
///
/// `DescribeManagedProductsByVendor` "provides high-level information for the managed
/// rule groups owned by a specific vendor" (its own doc), and `VendorName` is matched
/// exactly, like the vendor segment of the pair that identifies a group: a vendor
/// that publishes nothing here is reported an empty list rather than a refusal.
pub fn managed_products_of_vendor(vendor_name: &str) -> Vec<ManagedProductDescriptor> {
    managed_products()
        .into_iter()
        .filter(|product| product.vendor_name.as_deref() == Some(vendor_name))
        .collect()
}

/// The identifier the managed product of `vendor_name`'s group `managed_rule_set_name`
/// is published as: stable per product, distinct per product, and not the group's
/// name.
///
/// `ManagedProductDescriptor$ProductId` is "a unique identifier for the rule group.
/// This ID is returned in the responses to create and list commands. You provide it
/// to operations like update and delete" — the same thing `ManagedRuleSetSummary$Id`
/// is for the vendor that maintains the set, which is an `EntityId`: a UUID
/// (`^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`). So a product's identifier is
/// its own, separate from the `ManagedRuleSetName` a caller writes in a
/// `ManagedRuleGroupStatement`, and a caller that round-trips one into another call
/// keys on this.
///
/// A vendor assigns that identifier when it publishes the set, so the identifier a
/// product is reported under is the one its PUBLISHER states where this catalogue
/// cites it ([`MarketplaceManagedProduct::product_id`]). Where no source available
/// here states one, the catalogue publishes one instead: the UUID the identifying
/// pair spells, shaped like every other id this service mints ([`new_uuid`]) but
/// DERIVED from the pair rather than from fresh entropy — so the same product is
/// reported under the same identifier on every call and by every operation that
/// reports it, and no two products share one.
pub fn managed_product_id(vendor_name: &str, managed_rule_set_name: &str) -> String {
    if let Some(published) = published_product_id(vendor_name, managed_rule_set_name) {
        return published.to_string();
    }
    product_id_of_pair(vendor_name, managed_rule_set_name)
}

/// The identifier the vendor that publishes the product of this pair states for it,
/// where this catalogue cites it.
///
/// Only a Marketplace seller's listing carries one today: the Amazon Web Services
/// Managed Rules family's catalogue entries state no published identifier, so their
/// products are reported under [`product_id_of_pair`].
fn published_product_id(vendor_name: &str, managed_rule_set_name: &str) -> Option<&'static str> {
    marketplace_managed_product(vendor_name, managed_rule_set_name)
        .and_then(|listing| listing.product_id)
}

/// The identifier this catalogue publishes for a product whose own is not cited: the
/// UUID the identifying pair spells (see [`managed_product_id`]).
fn product_id_of_pair(vendor_name: &str, managed_rule_set_name: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    // The pair, each part length-prefixed, so no two different pairs hash the same
    // bytes.
    for part in [vendor_name, managed_rule_set_name] {
        // A fixed-width prefix, so the identifier a product is reported under is the
        // same whatever target the emulator was built for.
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid_text(bytes)
}

/// The ARN of the topic a vendor announces changes to its managed rule groups on,
/// from the topic's name.
///
/// `ManagedProductDescriptor$SnsTopicArn` is the ARN of the topic "that's used to
/// provide notification of changes to the managed rule group. You can subscribe to
/// the SNS topic to receive notifications when the managed rule group is modified",
/// and the vendor that publishes the group owns that topic — so the ARN is per
/// VENDOR, the same for every group one vendor publishes and different for every
/// vendor. Minted through the emulator's own ARN builder, like every other ARN this
/// service answers, in the region a notification topic is published in
/// ([`AWS_MANAGED_RULE_NOTIFICATION_REGION`]).
pub fn managed_product_topic_arn(topic: &str) -> String {
    ::engine::emulator_arn("sns", AWS_MANAGED_RULE_NOTIFICATION_REGION, topic)
}

/// The ARN of the topic a vendor announces changes on, in the vendor's OWN account
/// where this catalogue cites it (`account`), and in this emulator's account where
/// it cannot ([`managed_product_topic_arn`]).
///
/// A notification topic is not a resource of the calling account: the vendor that
/// publishes the managed rule group owns it, and a caller subscribes to it to learn
/// of that vendor's changes. So where the published catalogue states the account the
/// topic lives in, the ARN reported for it carries THAT account, as catalogue
/// reference data rather than as an ARN this emulator minted for a resource of its
/// own. Same region either way: one topic per vendor, in the region that vendor
/// publishes it in ([`AWS_MANAGED_RULE_NOTIFICATION_REGION`]).
fn vendor_notification_topic_arn(account: Option<&str>, topic: &str) -> String {
    match account {
        // The partition is written out here, rather than taken from the emulator's
        // own ARN builder, because this is a published catalogue constant rather
        // than an ARN this emulator mints for a resource of its own.
        Some(account) => {
            format!("arn:aws:sns:{AWS_MANAGED_RULE_NOTIFICATION_REGION}:{account}:{topic}")
        }
        None => managed_product_topic_arn(topic),
    }
}

/// The Marketplace listing the pair `(vendor_name, name)` names, if a seller in
/// this catalogue offers that managed rule group.
///
/// Matched exactly on both members, like [`managed_rule_group`]: a managed rule
/// group is identified by its `(VendorName, Name)` pair and by nothing else.
pub fn marketplace_managed_product(
    vendor_name: &str,
    name: &str,
) -> Option<&'static MarketplaceManagedProduct> {
    MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
        .iter()
        .find(|listing| {
            listing.seller.vendor_name == vendor_name && listing.managed_rule_set_name == name
        })
}

/// The sentence `WAFSubscriptionNotFoundException` carries: the shape's own
/// documented sentence
/// (`smithy_query(query="shape",shape="WAFSubscriptionNotFoundException")`, its
/// wrapping whitespace flattened), which names no particular group — what a caller
/// has to correct is its subscription, not an input.
pub const SUBSCRIPTION_NOT_FOUND_MESSAGE: &str =
    "You tried to use a managed rule group that's available by subscription, but \
     you aren't subscribed to it yet.";

/// `WAFSubscriptionNotFoundException` as WAF returns it: nothing but
/// [`SUBSCRIPTION_NOT_FOUND_MESSAGE`].
pub fn subscription_not_found_exception() -> WAFSubscriptionNotFoundException {
    WAFSubscriptionNotFoundException {
        message: Some(SUBSCRIPTION_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The refusal a rule list earns by RUNNING a managed rule group that is available
/// by SUBSCRIPTION and that this account is not subscribed to, if one of its rules
/// runs one.
///
/// A Marketplace seller's group is published — a managed-product describe reports it
/// ([`managed_products`]) — but using it takes a subscription to that seller's
/// listing: `ListAvailableManagedRuleGroups` lists "all Amazon Web Services Managed
/// Rules rule groups and all of the Amazon Web Services Marketplace managed rule
/// groups that you're SUBSCRIBED to" (its own doc), and the model gives the save
/// path its own `WAFSubscriptionNotFoundException` — "you tried to use a managed
/// rule group that's available by subscription, but you aren't subscribed to it
/// yet" — for exactly this case. Nothing in this API subscribes an account to a
/// seller, so every listing here is unsubscribed: a rule that runs one is refused
/// as unsubscribed rather than as a group nobody publishes, which is what tells a
/// caller to subscribe rather than to correct the pair. A pair NO vendor publishes
/// stays [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`]'s
/// `WAFNonexistentItemException`.
///
/// Walked with an explicit worklist, like
/// [`expired_managed_rule_group_version`]: the tree is supplied by the request.
pub fn unsubscribed_managed_rule_group(
    rules: &[Rule],
) -> Option<WAFSubscriptionNotFoundException> {
    let mut pending: Vec<&Statement> = rules.iter().map(|rule| &rule.statement).collect();
    while let Some(statement) = pending.pop() {
        if statement
            .managed_rule_group_statement
            .as_ref()
            .is_some_and(|managed| {
                marketplace_managed_product(&managed.vendor_name, &managed.name).is_some()
            })
        {
            return Some(subscription_not_found_exception());
        }
        pending.extend(nested_statements(statement));
    }
    None
}
