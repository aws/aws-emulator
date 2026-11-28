// SPDX-License-Identifier: CC0-1.0

//! The managed PRODUCTS this service publishes, end to end over the wire.
//!
//! The rule asserted here: `DescribeAllManagedProducts` reports one descriptor per
//! managed product the service publishes, of EITHER vendor family — the Amazon Web
//! Services Managed Rules rule groups of the catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`), the one that decides whether
//! a `ManagedRuleGroupStatement` names a group that exists, and the Amazon Web
//! Services Marketplace sellers' listings
//! (`service_…::state::MARKETPLACE_MANAGED_PRODUCT_CATALOGUE`) — each carrying its
//! publishing vendor's identity: a `ProductId` that identifies the product rather
//! than repeating its `ManagedRuleSetName`, the ARN of the topic that VENDOR
//! announces changes on, and, for a Marketplace product only, the `ProductLink` to
//! its product page. The report is identical in either scope and however much this
//! account holds, because a managed product is a vendor's publication rather than a
//! resource of the account.
//!
//! Every expectation below is composed FROM those catalogues rather than written as
//! a literal, and the tables span every product of either family, in both scopes.
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    managed_product_id, managed_product_topic_arn, marketplace_managed_product,
    MarketplaceManagedProduct, AWS_MANAGED_RULE_GROUP_VENDOR, AWS_MANAGED_RULE_NOTIFICATION_TOPIC,
    MANAGED_RULE_GROUP_CATALOGUE, MARKETPLACE_MANAGED_PRODUCT_CATALOGUE,
    MARKETPLACE_PRODUCT_PAGE_ADDRESS, SUBSCRIPTION_NOT_FOUND_MESSAGE,
};

use crate::managed_rule_groups::managed_statement;
use crate::web_acl_lifecycle::{create_web_acl, is_model_uuid, visibility, Waf};

/// The products a response reports, in the order it reported them.
///
/// Shared with the sibling suite in `crate::managed_product_catalog_data`, so both
/// read a report the same way.
pub(crate) fn reported(response: &Value) -> Vec<Value> {
    response["ManagedProducts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The products of the Amazon Web Services Managed Rules vendor among `products`.
fn of_aws_managed_rules(products: &[Value]) -> Vec<Value> {
    products
        .iter()
        .filter(|product| product["VendorName"] == json!(AWS_MANAGED_RULE_GROUP_VENDOR))
        .cloned()
        .collect()
}

/// The reported product the `(VendorName, ManagedRuleSetName)` pair names.
///
/// Shared with the sibling suite in `crate::managed_product_catalog_data`.
pub(crate) fn product_of<'a>(
    products: &'a [Value],
    vendor: &str,
    name: &str,
) -> Option<&'a Value> {
    products.iter().find(|product| {
        product["VendorName"] == json!(vendor) && product["ManagedRuleSetName"] == json!(name)
    })
}

/// The notification topic every change to a published managed rule group is
/// announced on: ONE topic, in the region its publisher publishes it in, whichever
/// group is reported and whatever region the caller signed for.
fn expected_topic() -> String {
    format!(
        "arn:aws:sns:us-east-1:{}:aws-managed-waf-rule-notifications",
        ::engine::emulator_account_id()
    )
}

/// Every group the catalogue publishes is reported as a managed product, in
/// published order, with its vendor, name, identifier, title, description, the two
/// flags it is published with and the ARN of the topic its changes are announced
/// on — and the same products are reported in either scope, because the products
/// are the vendors' and a scope is only the namespace a request is made in.
///
/// The table is the WHOLE catalogue, one row per published group, crossed with both
/// declared scopes: it spans versioned and unversioned groups, advanced and
/// ordinary ones, and capacities from the smallest to the largest the catalogue
/// publishes.
#[tokio::test]
async fn every_published_group_is_reported_as_a_managed_product() {
    // The rows below are only worth crossing if the catalogue spans both values of
    // each published flag.
    assert!(
        MANAGED_RULE_GROUP_CATALOGUE
            .iter()
            .any(|group| group.is_versioning_supported)
            && MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .any(|group| !group.is_versioning_supported),
        "the catalogue must publish both versioned and unversioned groups for this \
         table to span the class"
    );

    let waf = Waf::new();
    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let response = waf
            .ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await;
        let products = of_aws_managed_rules(&reported(&response));
        assert_eq!(
            products.len(),
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            "case {scope:?}: expected one product per published group \
             ({}), got {}: {response}",
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            products.len()
        );

        for (group, product) in MANAGED_RULE_GROUP_CATALOGUE.iter().zip(products.iter()) {
            let case = format!("{scope}/{}", group.name);
            assert_eq!(
                product["VendorName"],
                json!(group.vendor_name),
                "case {case:?}: expected the publishing vendor {:?}, got {product}",
                group.vendor_name
            );
            assert_eq!(
                product["ManagedRuleSetName"],
                json!(group.name),
                "case {case:?}: expected the published group name, got {product}"
            );
            assert_eq!(
                product["ProductId"],
                json!(managed_product_id(group.vendor_name, group.name)),
                "case {case:?}: expected the identifier published for this product, \
                 got {product}"
            );
            assert_eq!(
                product["ProductTitle"],
                json!(group.product_title),
                "case {case:?}: expected the published title {:?}, got {product}",
                group.product_title
            );
            assert_eq!(
                product["ProductDescription"],
                json!(group.product_description),
                "case {case:?}: expected the published description, got {product}"
            );
            assert_eq!(
                product["IsVersioningSupported"],
                json!(group.is_versioning_supported),
                "case {case:?}: expected IsVersioningSupported {}, got {product}",
                group.is_versioning_supported
            );
            assert_eq!(
                product["IsAdvancedManagedRuleSet"],
                json!(group.is_advanced_managed_rule_set),
                "case {case:?}: expected IsAdvancedManagedRuleSet {}, got {product}",
                group.is_advanced_managed_rule_set
            );
            // One topic for every announced change, in the emulator's account, like
            // every other ARN this service answers.
            let expected_topic = expected_topic();
            assert_eq!(
                product["SnsTopicArn"],
                json!(expected_topic),
                "case {case:?}: expected the notification topic {expected_topic:?}, \
                 got {product}"
            );
            // "For Amazon Web Services Marketplace managed rule groups only"
            // (`ManagedProductDescriptor$ProductLink`), and these groups are the
            // Amazon Web Services Managed Rules family's.
            assert!(
                product["ProductLink"].is_null(),
                "case {case:?}: a group its own vendor publishes has no marketplace \
                 product link, got {product}"
            );
        }

        // "A unique identifier for the rule group" (`ProductId`): no two reported
        // products share one.
        let identifiers: BTreeSet<&str> = products
            .iter()
            .filter_map(|product| product["ProductId"].as_str())
            .collect();
        assert_eq!(
            identifiers.len(),
            products.len(),
            "case {scope:?}: every product identifier must be unique, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal reports no products: the two it declares are the only namespaces a
/// request can be made in. One row per shape of a wrong value — an unknown word, a
/// declared value in the wrong case, one with a trailing space, and the empty
/// string.
///
/// `Scope` is a modelled enum, so the value set is enforced at the request
/// boundary and the refusal names the member and the admitted values; the
/// handler's own `is_known_scope` guard covers anything that gets past it.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "DescribeAllManagedProducts",
                json!({ "Scope": scope }),
                "ValidationException",
                scope,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'scope'"),
            "case {scope:?}: the refusal must name the member at fault, got {response}"
        );
        assert!(
            response["ManagedProducts"].is_null(),
            "case {scope:?}: a refusal must report no products, got {response}"
        );
    }
}

/// Reporting the managed products changes nothing a later caller sees, and what is
/// reported does not depend on what this account holds: the products are the
/// vendors' publications, so saving a web ACL that RUNS one of them leaves the
/// report identical.
///
/// The web ACL in the middle also crosses the two: a pair the report names is a
/// pair a save accepts, so the describe reports the same catalogue the save
/// validates against.
#[tokio::test]
async fn reporting_the_managed_products_changes_nothing() {
    let waf = Waf::new();
    let body = json!({ "Scope": "REGIONAL" });

    let first = waf.ok("DescribeAllManagedProducts", body.clone()).await;
    let second = waf.ok("DescribeAllManagedProducts", body.clone()).await;
    assert_eq!(
        first, second,
        "two reads in a row must report the same products, got {first} then {second}"
    );

    // Run a group the report named, from a web ACL of this account. The Amazon Web
    // Services Managed Rules groups are the ones available to an account subscribed
    // to no Marketplace seller (see
    // `a_marketplace_group_is_published_but_not_available_to_run`).
    let products = of_aws_managed_rules(&reported(&first));
    for product in &products {
        let vendor = product["VendorName"].as_str().unwrap_or_default();
        let name = product["ManagedRuleSetName"].as_str().unwrap_or_default();
        let case = format!("{vendor}/{name}");
        let acl_name = format!("product-{}", name.to_lowercase());
        let (acl_id, _arn, _lock) = create_web_acl(
            &waf,
            "REGIONAL",
            &acl_name,
            json!([{
                "Name": "managed",
                "Priority": 0,
                "Statement": managed_statement(vendor, name),
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("managed"),
            }]),
        )
        .await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"],
            managed_statement(vendor, name),
            "case {case:?}: a reported product must be a pair a web ACL may run, \
             got {got}"
        );
    }

    let after = waf.ok("DescribeAllManagedProducts", body).await;
    assert_eq!(
        after, first,
        "what this account holds must not change the reported products, got {after} \
         after {first}"
    );
}

/// Both vendor families are reported, each product as its own family publishes it:
/// every Amazon Web Services Marketplace listing with its seller's vendor name, its
/// seller's notification topic and a link to its product page, and every Amazon Web
/// Services Managed Rules rule set with the Amazon Web Services Managed Rules topic
/// and no product link at all.
///
/// The rows are the WHOLE product catalogue — one per Marketplace listing and one
/// per published rule set — crossed with both declared scopes. They span what a
/// listing's seller publishes about it too: its own title and summary, the link to
/// its product page, and the topic it announces its changes on.
#[tokio::test]
async fn both_vendor_families_are_reported_as_managed_products() {
    // The rows below are only worth crossing if the catalogue really spans both
    // families.
    assert!(
        !MARKETPLACE_MANAGED_PRODUCT_CATALOGUE.is_empty()
            && !MANAGED_RULE_GROUP_CATALOGUE.is_empty(),
        "the catalogue must publish products of both vendor families for this table \
         to span the class"
    );

    let waf = Waf::new();
    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let response = waf
            .ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await;
        let products = reported(&response);
        let expected_count =
            MARKETPLACE_MANAGED_PRODUCT_CATALOGUE.len() + MANAGED_RULE_GROUP_CATALOGUE.len();
        assert_eq!(
            products.len(),
            expected_count,
            "case {scope:?}: expected one product per published product of either \
             family ({expected_count}), got {}: {response}",
            products.len()
        );

        for published in MARKETPLACE_MANAGED_PRODUCT_CATALOGUE {
            let MarketplaceManagedProduct {
                seller,
                managed_rule_set_name,
                product_id: _,
                product_title,
                product_description,
                product_link,
                is_versioning_supported,
            } = *published;
            let case = format!("{scope}/{}/{managed_rule_set_name}", seller.vendor_name);
            let Some(product) = product_of(&products, seller.vendor_name, managed_rule_set_name)
            else {
                panic!("case {case:?}: the seller's listing must be reported, got {response}");
            };
            // The listing's own text, in the seller's published words: its title, and
            // the summary where the catalogue cites one (a listing whose summary no
            // source states carries no `ProductDescription` rather than text nobody
            // published).
            assert_eq!(
                product["ProductTitle"],
                json!(product_title),
                "case {case:?}: expected the listing's title {product_title:?}, got \
                 {product}"
            );
            assert_eq!(
                product["ProductDescription"],
                match product_description {
                    Some(summary) => json!(summary),
                    None => Value::Null,
                },
                "case {case:?}: expected the listing's published summary \
                 {product_description:?}, got {product}"
            );
            assert_eq!(
                product["IsVersioningSupported"],
                json!(is_versioning_supported),
                "case {case:?}: expected IsVersioningSupported \
                 {is_versioning_supported}, got {product}"
            );
            // "Such as the Amazon Web Services Managed Rules rule groups that are
            // used for WAF intelligent threat mitigation"
            // (`IsAdvancedManagedRuleSet`): those are the advanced ones, so a
            // seller's listing is not one.
            assert_eq!(
                product["IsAdvancedManagedRuleSet"],
                json!(false),
                "case {case:?}: a Marketplace listing is not an advanced managed rule \
                 set, got {product}"
            );
            // "For Amazon Web Services Marketplace managed rule groups only, the
            // link to the rule group product page" (`ProductLink`): the address its
            // product page is published under, cited whole.
            assert_eq!(
                product["ProductLink"],
                json!(product_link),
                "case {case:?}: expected the link to this listing's product page, got \
                 {product}"
            );
            assert!(
                product_link.contains(MARKETPLACE_PRODUCT_PAGE_ADDRESS),
                "case {case:?}: a listing's page is published on the Marketplace at \
                 {MARKETPLACE_PRODUCT_PAGE_ADDRESS:?}, got {product_link:?}"
            );
            // The topic its SELLER announces changes on, not the Amazon Web Services
            // Managed Rules one.
            let expected_topic = seller.notification_topic_arn();
            assert_eq!(
                product["SnsTopicArn"],
                json!(expected_topic),
                "case {case:?}: expected the seller's notification topic \
                 {expected_topic:?}, got {product}"
            );
            assert_ne!(
                product["SnsTopicArn"],
                json!(managed_product_topic_arn(
                    AWS_MANAGED_RULE_NOTIFICATION_TOPIC
                )),
                "case {case:?}: a seller announces its own changes, not on the Amazon \
                 Web Services Managed Rules topic, got {product}"
            );
        }

        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope}/{}/{}", group.vendor_name, group.name);
            let Some(product) = product_of(&products, group.vendor_name, group.name) else {
                panic!("case {case:?}: the published rule set must be reported, got {response}");
            };
            assert_eq!(
                product["ProductTitle"],
                json!(group.product_title),
                "case {case:?}: expected the published title {:?}, got {product}",
                group.product_title
            );
            assert_eq!(
                product["IsAdvancedManagedRuleSet"],
                json!(group.is_advanced_managed_rule_set),
                "case {case:?}: expected IsAdvancedManagedRuleSet {}, got {product}",
                group.is_advanced_managed_rule_set
            );
            let expected_topic = managed_product_topic_arn(AWS_MANAGED_RULE_NOTIFICATION_TOPIC);
            assert_eq!(
                product["SnsTopicArn"],
                json!(expected_topic),
                "case {case:?}: expected the publishing vendor's notification topic \
                 {expected_topic:?}, got {product}"
            );
            assert!(
                product["ProductLink"].is_null(),
                "case {case:?}: a product link is for a Marketplace managed rule group \
                 only, got {product}"
            );
        }
    }
}

/// Every reported product's `ProductId` identifies the PRODUCT: it is the product's
/// own identifier rather than a copy of its `ManagedRuleSetName`, it is unique
/// across the whole catalogue, it is within the form the model admits (`ProductId`:
/// `length 1..128`, pattern `\S`) — taking the form this service's identifiers have
/// wherever the catalogue publishes no identifier of the vendor's own — and it is the
/// same on a later call and under the other scope.
///
/// One row per published product of either family, crossed with both scopes and
/// with a repeated call.
#[tokio::test]
async fn every_product_carries_its_own_stable_identifier() {
    let waf = Waf::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();

    for scope in ["REGIONAL", "CLOUDFRONT", "REGIONAL"] {
        let response = waf
            .ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await;
        let products = reported(&response);

        for product in &products {
            let vendor = product["VendorName"].as_str().unwrap_or_default();
            let name = product["ManagedRuleSetName"].as_str().unwrap_or_default();
            let pair = format!("{vendor}/{name}");
            let case = format!("{scope}/{pair}");
            let identifier = product["ProductId"].as_str().unwrap_or_default();
            assert_ne!(
                identifier, name,
                "case {case:?}: a product identifier is the product's own, not its \
                 rule-set name, got {product}"
            );
            assert!(
                (1..=128).contains(&identifier.len()) && identifier.trim() == identifier,
                "case {case:?}: a ProductId is 1..128 non-blank characters, got \
                 {identifier:?}"
            );
            // A publishing vendor assigns the identifier of its own product, so an
            // identifier the catalogue cites is reported in the publisher's own form;
            // the form THIS service's identifiers take is what a product whose own
            // nobody states is published under.
            let published = marketplace_managed_product(vendor, name)
                .and_then(|listing| listing.product_id);
            assert!(
                published.is_some() || is_model_uuid(identifier),
                "case {case:?}: expected an identifier of the form this service's ids \
                 take, got {identifier:?}"
            );
            // The identifier the catalogue publishes for the pair, which is what the
            // report must carry rather than anything minted per call.
            assert_eq!(
                identifier,
                managed_product_id(vendor, name),
                "case {case:?}: expected the identifier published for this product, \
                 got {product}"
            );
            // The same identifier on a later call and under the other scope, and
            // never one another product already carries.
            if let Some(previous) = seen.insert(identifier.to_string(), pair.clone()) {
                assert_eq!(
                    previous, pair,
                    "case {case:?}: one identifier names one product, {identifier:?} \
                     is already {previous:?}"
                );
            }
        }

        let identifiers: BTreeSet<&str> = products
            .iter()
            .filter_map(|product| product["ProductId"].as_str())
            .collect();
        assert_eq!(
            identifiers.len(),
            products.len(),
            "case {scope:?}: every product identifier must be unique, got {response}"
        );
    }
}

/// A notification topic is per VENDOR: every product one vendor publishes is
/// reported with the same topic ARN, and no two vendors share one — which is what
/// lets a subscriber follow one vendor's changes.
///
/// The rows are every vendor the catalogue publishes under, of both families: the
/// one Amazon Web Services Managed Rules vendor with many groups, sellers with
/// several listings, and sellers with one.
#[tokio::test]
async fn a_notification_topic_belongs_to_one_vendor() {
    let waf = Waf::new();
    let products = reported(
        &waf.ok("DescribeAllManagedProducts", json!({ "Scope": "REGIONAL" }))
            .await,
    );

    let mut topic_of_vendor: BTreeMap<String, String> = BTreeMap::new();
    for product in &products {
        let vendor = product["VendorName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let topic = product["SnsTopicArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let case = format!(
            "{vendor}/{}",
            product["ManagedRuleSetName"].as_str().unwrap_or_default()
        );
        assert!(
            !topic.is_empty(),
            "case {case:?}: every product reports the topic its changes are announced \
             on, got {product}"
        );
        match topic_of_vendor.get(&vendor) {
            Some(known) => assert_eq!(
                known, &topic,
                "case {case:?}: one vendor announces on ONE topic, expected \
                 {known:?}, got {topic:?}"
            ),
            None => {
                topic_of_vendor.insert(vendor, topic);
            }
        }
    }

    let topics: BTreeSet<&String> = topic_of_vendor.values().collect();
    assert_eq!(
        topics.len(),
        topic_of_vendor.len(),
        "no two vendors may share a notification topic, got {topic_of_vendor:?}"
    );
    assert!(
        topic_of_vendor.len() > 1,
        "the catalogue must publish more than one vendor for this assertion to span \
         the class, got {topic_of_vendor:?}"
    );
}

/// A Marketplace group is reported as a product but is NOT available to this
/// account: `ListAvailableManagedRuleGroups` lists "all Amazon Web Services Managed
/// Rules rule groups and all of the Amazon Web Services Marketplace managed rule
/// groups that you're subscribed to" (the operation's own doc), and nothing
/// subscribes this account to a seller — so no seller's group is listed, and every
/// call that would run one answers the modelled `WAFSubscriptionNotFoundException`
/// ("you tried to use a managed rule group that's available by subscription, but
/// you aren't subscribed to it yet") and stores nothing. A pair NO vendor publishes
/// is a different refusal: `WAFNonexistentItemException`.
///
/// The rows are every Marketplace listing, crossed with every call that can RUN a
/// managed rule group and that the model gives this error to — `CreateWebACL`,
/// `UpdateWebACL` and `CheckCapacity` — against the whole available list. A rule
/// GROUP cannot carry a `ManagedRuleGroupStatement` at all, whoever publishes the
/// group, so `CreateRuleGroup`/`UpdateRuleGroup` refuse such a rule as a misplaced
/// statement before any subscription is in question.
#[tokio::test]
async fn a_marketplace_group_is_published_but_not_available_to_run() {
    let waf = Waf::new();
    let listed = waf
        .ok(
            "ListAvailableManagedRuleGroups",
            json!({ "Scope": "REGIONAL" }),
        )
        .await;
    let available = listed["ManagedRuleGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        available.len(),
        MANAGED_RULE_GROUP_CATALOGUE.len(),
        "only the Amazon Web Services Managed Rules groups are available to an \
         account subscribed to no seller, got {listed}"
    );

    // One live web ACL, running a group this account MAY run, for the update rows to
    // be refused against.
    let catalogued = MANAGED_RULE_GROUP_CATALOGUE[0];
    let live_rules = json!([{
        "Name": "managed",
        "Priority": 0,
        "Statement": managed_statement(catalogued.vendor_name, catalogued.name),
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility("managed"),
    }]);
    let (live_id, _live_arn, live_lock) =
        create_web_acl(&waf, "REGIONAL", "subscription-live", live_rules.clone()).await;

    for published in MARKETPLACE_MANAGED_PRODUCT_CATALOGUE {
        let vendor = published.seller.vendor_name;
        let name = published.managed_rule_set_name;
        let case = format!("{vendor}/{name}");
        assert!(
            !available
                .iter()
                .any(|group| group["VendorName"] == json!(vendor)),
            "case {case:?}: an unsubscribed seller's group is not available, got {listed}"
        );

        let seller_rules = json!([{
            "Name": "managed",
            "Priority": 0,
            "Statement": managed_statement(vendor, name),
            "OverrideAction": { "None": {} },
            "VisibilityConfig": visibility("managed"),
        }]);
        let acl_name = format!("marketplace-{}", name.to_lowercase());

        for (operation, body) in [
            (
                "CreateWebACL",
                json!({
                    "Name": &acl_name,
                    "Scope": "REGIONAL",
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility("marketplace"),
                    "Rules": seller_rules.clone(),
                }),
            ),
            (
                "UpdateWebACL",
                json!({
                    "Name": "subscription-live",
                    "Scope": "REGIONAL",
                    "Id": &live_id,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility("subscription-live"),
                    "LockToken": &live_lock,
                    "Rules": seller_rules.clone(),
                }),
            ),
            (
                "CheckCapacity",
                json!({ "Scope": "REGIONAL", "Rules": seller_rules.clone() }),
            ),
        ] {
            let row = format!("{case} through {operation}");
            let response = waf
                .refused(operation, body, "WAFSubscriptionNotFoundException", &row)
                .await;
            assert_eq!(
                response["Message"],
                json!(SUBSCRIPTION_NOT_FOUND_MESSAGE),
                "case {row:?}: expected the sentence that exception carries, got \
                 {response}"
            );
        }

        let web_acls = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
        assert!(
            !web_acls.to_string().contains(acl_name.as_str()),
            "case {case:?}: a refused save must store nothing, got {web_acls}"
        );
    }

    // The refused updates left the live web ACL, its rules and its lock token alone.
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "subscription-live", "Scope": "REGIONAL", "Id": &live_id }),
        )
        .await;
    assert_eq!(
        got["LockToken"],
        json!(&live_lock),
        "a refused update must not rotate the lock token, got {got}"
    );
    assert_eq!(
        got["WebACL"]["Rules"],
        live_rules,
        "a refused update must leave the stored rules alone, got {got}"
    );

    // And a pair no vendor publishes at all is the OTHER refusal: nothing to
    // subscribe to, rather than a subscription this account lacks.
    waf.refused(
        "CheckCapacity",
        json!({
            "Scope": "REGIONAL",
            "Rules": [{
                "Name": "managed",
                "Priority": 0,
                "Statement": managed_statement("SomeOtherVendor", "NoSuchRuleSet"),
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("managed"),
            }],
        }),
        "WAFNonexistentItemException",
        "a pair no vendor publishes",
    )
    .await;
}

/// A Marketplace seller is discoverable by name: `DescribeManagedProductsByVendor`
/// reports exactly that seller's listings, with the same descriptors the
/// whole-catalogue report carries, so a client that enumerates the catalogue to find
/// a third-party vendor's groups finds them.
///
/// One row per seller the Marketplace family publishes, plus the Amazon Web Services
/// Managed Rules vendor, in both scopes.
#[tokio::test]
async fn each_vendor_of_either_family_is_reported_its_own_products() {
    let waf = Waf::new();
    let vendors: BTreeSet<&str> = MARKETPLACE_MANAGED_PRODUCT_CATALOGUE
        .iter()
        .map(|listing| listing.seller.vendor_name)
        .chain(std::iter::once(AWS_MANAGED_RULE_GROUP_VENDOR))
        .collect();

    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let all = reported(
            &waf.ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
                .await,
        );
        for vendor in &vendors {
            let case = format!("{scope}/{vendor}");
            let response = waf
                .ok(
                    "DescribeManagedProductsByVendor",
                    json!({ "Scope": scope, "VendorName": vendor }),
                )
                .await;
            let products = reported(&response);
            let expected: Vec<Value> = all
                .iter()
                .filter(|product| product["VendorName"] == json!(vendor))
                .cloned()
                .collect();
            assert!(
                !expected.is_empty(),
                "case {case:?}: this vendor publishes products, so the whole-catalogue \
                 report must carry some, got {all:?}"
            );
            assert_eq!(
                products, expected,
                "case {case:?}: expected this vendor's {} products as the whole \
                 catalogue reports them, got {response}",
                expected.len()
            );
        }
    }
}
