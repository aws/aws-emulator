// SPDX-License-Identifier: CC0-1.0

//! `ListProfileResourceAssociations` over the wire: what the listing reports, and
//! what a refused `AssociateResourceToProfile` leaves behind.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! * the listing reports exactly the resource associations the store holds for the
//!   Profile named — every one it accepted, no association of another Profile, and
//!   an empty list for a Profile that holds none;
//! * a request the service refuses persists nothing, so no refused `ResourceArn`
//!   is ever reported by the listing, whatever the refusal was about (a reserved
//!   DNS Firewall `priority`, an ARN naming a resource no Profile holds);
//! * `ResourceType` narrows the listing to the associations reporting that type;
//! * `ResourceArn` narrows the listing to the association with that resource alone,
//!   and an ARN the Profile holds no association with selects nothing;
//! * the listing pages in a stable order, `MaxResults` bounds each page, a token
//!   resumes strictly after the page that issued it, and a token this operation
//!   never issued is refused;
//! * a `ProfileId` that names no Profile is `ResourceNotFoundException`;
//! * the listing reports each association in the lifecycle phase the CLOCK has
//!   moved it to, with the sentence that phase is worded in for the kind of
//!   resource held, and reports the same record on every later read;
//! * `ResourceType` narrows the set BEFORE the page is cut, so a narrowed listing
//!   fills every page but the last whatever `MaxResults` is;
//! * the listing reports the store as it stands after an update, a refused update
//!   and a disassociate, and a `MaxResults` outside the modelled range is refused.

use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::harness::{association_not_found_message, path_label, Emulator, Reply, ACCOUNT_ID};

/// The region the shared harness signs for, used to build the ARNs below.
const REGION: &str = "us-east-1";

/// The resources a Profile can hold, as `(ResourceArn, ResourceType)`.
///
/// One row per kind named by `AssociateResourceToProfile`'s own documentation (a
/// private hosted zone, a DNS Firewall rule group, a Resolver rule) plus the other
/// Resolver configuration a Profile carries; the `ResourceType` is the ARN's own
/// resource-type segment in the upper-snake spelling the wire uses, which is what
/// `ProfileResourceAssociation$ResourceType` reports.
fn associable_resources() -> Vec<(String, &'static str)> {
    vec![
        (
            "arn:aws:route53:::hostedzone/Z11111111111AAAAAAAAA".to_string(),
            "HOSTEDZONE",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-rule/rslvr-rr-1111111111111111"
            ),
            "RESOLVER_RULE",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-2222222222222222"
            ),
            "FIREWALL_RULE_GROUP",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-query-log-config/rslvr-rqlc-3333333333333333"
            ),
            "RESOLVER_QUERY_LOG_CONFIG",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-dnssec-config/rslvr-ds-4444444444444444"
            ),
            "RESOLVER_DNSSEC_CONFIG",
        ),
        (
            format!("arn:aws:ec2:{REGION}:{ACCOUNT_ID}:vpc-endpoint/vpce-5555555555555555"),
            "VPC_ENDPOINT",
        ),
    ]
}

/// A Profile to associate resources with, reported by its `Id`.
async fn profile(em: &Emulator, name: &str) -> String {
    let created = em.create_named(name).await;
    assert_eq!(
        created.status, 200,
        "case profile={name}: CreateProfile expected 200, got {} / {:?}",
        created.status, created.body
    );
    created.nested_string("Profile", "Id")
}

/// The string `field` of every association the listing reported, in the order it
/// reported them.
fn listed(reply: &Reply, field: &str) -> Vec<String> {
    reply
        .list("ProfileResourceAssociations")
        .iter()
        .map(|association| {
            association[field]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("ProfileResourceAssociation.{field} is not a string in {association}")
                })
                .to_string()
        })
        .collect()
}

/// Every association of `profile_id`, walking the pages `MaxResults` cuts, with
/// each page asserted to be no larger than asked for. Answers the `Id`s in the
/// order the listing reported them.
async fn all_pages(em: &Emulator, profile_id: &str, max_results: usize) -> Vec<String> {
    let case = format!("profile={profile_id} maxResults={max_results}");
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per stored association is the worst case; the bound keeps a broken
    // token from looping forever instead of failing.
    for page in 0..64 {
        let query = match token.as_deref() {
            None => format!("?maxResults={max_results}"),
            Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
        };
        let reply = em.list_resource_associations(profile_id, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case} page {page}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
        let page_ids = listed(&reply, "Id");
        assert!(
            page_ids.len() <= max_results,
            "case {case} page {page}: a page must hold at most maxResults, got {:?}",
            page_ids
        );
        ids.extend(page_ids);
        token = reply.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return ids;
        }
        assert!(
            !ids.is_empty(),
            "case {case} page {page}: a continuation token was issued for an empty page"
        );
    }
    panic!("case {case}: the listing never stopped paging, got {ids:?}");
}

#[tokio::test]
async fn the_listing_reports_exactly_the_resource_associations_of_the_profile_named() {
    let em = Emulator::new();
    let held = profile(&em, "listing-holder").await;
    let neighbour = profile(&em, "listing-neighbour").await;
    let empty = profile(&em, "listing-empty").await;

    // The holder takes every associable kind; the neighbour takes one of them, so a
    // listing that leaked across Profiles would be visible in both directions.
    let mut created: Vec<(String, String, &'static str)> = Vec::new();
    for (index, (resource_arn, resource_type)) in associable_resources().into_iter().enumerate() {
        // Only a DNS Firewall rule group's properties carry a priority, and 100 is
        // the lowest the documented range admits.
        let properties = (resource_type == "FIREWALL_RULE_GROUP").then_some(r#"{"priority":100}"#);
        let reply = em
            .associate_resource(&held, &resource_arn, &format!("held-{index}"), properties)
            .await;
        assert_eq!(
            reply.status, 200,
            "case arn={resource_arn}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
        let id = reply.member("ProfileResourceAssociation")["Id"]
            .as_str()
            .expect("a created association reports its Id")
            .to_string();
        created.push((id, resource_arn, resource_type));
    }
    let (neighbour_arn, _) = associable_resources()
        .into_iter()
        .next()
        .expect("the table has rows");
    let neighbours = em
        .associate_resource(&neighbour, &neighbour_arn, "neighbour", None)
        .await;
    assert_eq!(
        neighbours.status, 200,
        "the neighbour's associate expected 200, got {} / {:?}",
        neighbours.status, neighbours.body
    );
    let neighbour_id = neighbours.member("ProfileResourceAssociation")["Id"]
        .as_str()
        .expect("a created association reports its Id")
        .to_string();

    let listing = em.list_resource_associations(&held, "").await;
    assert_eq!(
        listing.status, 200,
        "the holder's listing expected 200, got {} / {:?}",
        listing.status, listing.body
    );
    let mut listed_ids = listed(&listing, "Id");
    listed_ids.sort();
    let mut expected_ids: Vec<String> = created.iter().map(|(id, ..)| id.clone()).collect();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "case profile={held}: the listing must report every association of this Profile and \
         no other, got {:?}",
        listing.body
    );
    assert!(
        !listed_ids.contains(&neighbour_id),
        "case profile={held}: another Profile's association ({neighbour_id}) was reported in \
         {:?}",
        listing.body
    );
    assert!(
        listing.body["NextToken"].is_null(),
        "case profile={held}: a listing that returned everything must issue no token, got {:?}",
        listing.body
    );

    // Every reported association is the record the read-back reports, member for
    // member except the clock-derived status the two calls may straddle.
    for (id, resource_arn, resource_type) in &created {
        let reported = listing
            .list("ProfileResourceAssociations")
            .iter()
            .find(|association| association["Id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case id={id}: absent from {:?}", listing.body));
        let fetched = em.get_resource_association(id).await;
        let fetched = fetched.member("ProfileResourceAssociation");
        for field in [
            "Id",
            "Name",
            "OwnerId",
            "ProfileId",
            "ResourceArn",
            "ResourceType",
            "ResourceProperties",
            "CreationTime",
            "ModificationTime",
        ] {
            assert_eq!(
                reported[field], fetched[field],
                "case id={id} field={field}: the listed association differs from the read-back \
                 one ({reported:?} vs {fetched:?})"
            );
        }
        assert_eq!(
            reported["ResourceArn"].as_str(),
            Some(resource_arn.as_str()),
            "case id={id}: unexpected ResourceArn in {reported:?}"
        );
        assert_eq!(
            reported["ResourceType"].as_str(),
            Some(*resource_type),
            "case id={id}: unexpected ResourceType in {reported:?}"
        );
        assert_eq!(
            reported["ProfileId"].as_str(),
            Some(held.as_str()),
            "case id={id}: unexpected ProfileId in {reported:?}"
        );
    }

    // A Profile that holds nothing lists nothing — an empty list, not a refusal.
    let none = em.list_resource_associations(&empty, "").await;
    assert_eq!(
        none.status, 200,
        "case profile={empty}: expected 200, got {} / {:?}",
        none.status, none.body
    );
    assert_eq!(
        none.list("ProfileResourceAssociations"),
        &[] as &[Value],
        "case profile={empty}: a Profile holding nothing must list nothing"
    );
    assert!(
        none.body["NextToken"].is_null(),
        "case profile={empty}: an empty listing must issue no token, got {:?}",
        none.body
    );

    // The neighbour's own listing reports only its own association.
    let theirs = em.list_resource_associations(&neighbour, "").await;
    assert_eq!(
        listed(&theirs, "Id"),
        vec![neighbour_id],
        "case profile={neighbour}: the listing must report only this Profile's association, \
         got {:?}",
        theirs.body
    );
}

#[tokio::test]
async fn no_refused_associate_is_ever_reported_by_the_listing() {
    let em = Emulator::new();
    let profile_id = profile(&em, "refusals-listing").await;
    let rule_group = |tag: u64| {
        format!(
            "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-{tag:016x}"
        )
    };

    // Each row is a request the service refuses: a DNS Firewall `priority` outside
    // the documented 100..9900 (both just past each published bound, and values far
    // outside it), and well-formed ARNs naming resources no Profile holds.
    let refused: Vec<(String, Option<String>)> = vec![
        (rule_group(0x11), Some(r#"{"priority": 99}"#.to_string())),
        (rule_group(0x12), Some(r#"{"priority": 9901}"#.to_string())),
        (rule_group(0x13), Some(r#"{"priority": 1}"#.to_string())),
        (rule_group(0x14), Some(r#"{"priority": -5}"#.to_string())),
        (rule_group(0x15), Some(r#"{"priority": 100000}"#.to_string())),
        ("arn:aws:s3:::listing-example-bucket".to_string(), None),
        (
            format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:answer"),
            None,
        ),
        (
            format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:a-queue"),
            Some(r#"{"priority": 150}"#.to_string()),
        ),
        ("still-not-an-arn".to_string(), None),
    ];

    // One accepted association, so each refusal is checked against a listing that is
    // not trivially empty.
    let accepted_arn = rule_group(0xaa);
    let accepted = em
        .associate_resource(&profile_id, &accepted_arn, "accepted", Some(r#"{"priority": 150}"#))
        .await;
    assert_eq!(
        accepted.status, 200,
        "an in-range priority must be accepted, got {} / {:?}",
        accepted.status, accepted.body
    );

    for (resource_arn, properties) in refused {
        let case = format!("arn={resource_arn} properties={properties:?}");
        let refusal = em
            .associate_resource(&profile_id, &resource_arn, "refused", properties.as_deref())
            .await;
        assert_eq!(
            refusal.status, 400,
            "case {case}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert_eq!(
            refusal.error.as_deref(),
            Some("ValidationException"),
            "case {case}: unexpected error code, got {:?}",
            refusal.body
        );
        let listing = em.list_resource_associations(&profile_id, "").await;
        let arns = listed(&listing, "ResourceArn");
        assert!(
            !arns.contains(&resource_arn),
            "case {case}: a refused request must persist nothing, yet the listing reports \
             {arns:?}"
        );
        assert_eq!(
            arns,
            vec![accepted_arn.clone()],
            "case {case}: the listing must still report exactly what was accepted"
        );
    }

    // The accepted association is readable through the listing with the properties
    // it was created with, priority and all.
    let listing = em.list_resource_associations(&profile_id, "").await;
    let reported = &listing.list("ProfileResourceAssociations")[0];
    assert_eq!(
        reported["ResourceProperties"].as_str(),
        Some(r#"{"priority": 150}"#),
        "the listed ResourceProperties must be the document given, got {reported:?}"
    );
}

#[tokio::test]
async fn resource_type_narrows_the_listing_to_the_associations_reporting_that_type() {
    let em = Emulator::new();
    let profile_id = profile(&em, "type-filter").await;

    let resources = associable_resources();
    for (index, (resource_arn, resource_type)) in resources.iter().enumerate() {
        let properties = (*resource_type == "FIREWALL_RULE_GROUP").then_some(r#"{"priority":9900}"#);
        let reply = em
            .associate_resource(&profile_id, resource_arn, &format!("typed-{index}"), properties)
            .await;
        assert_eq!(
            reply.status, 200,
            "case arn={resource_arn}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
    }

    for (resource_arn, resource_type) in &resources {
        let listing = em
            .list_resource_associations(&profile_id, &format!("?resourceType={resource_type}"))
            .await;
        assert_eq!(
            listing.status, 200,
            "case resourceType={resource_type}: expected 200, got {} / {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed(&listing, "ResourceArn"),
            vec![resource_arn.clone()],
            "case resourceType={resource_type}: the listing must report exactly the \
             associations of that type, got {:?}",
            listing.body
        );
        assert_eq!(
            listed(&listing, "ResourceType"),
            vec![resource_type.to_string()],
            "case resourceType={resource_type}: unexpected types in {:?}",
            listing.body
        );
    }

    // A type nothing is associated as matches nothing, rather than everything.
    for unmatched in ["SOMETHING_ELSE", "hostedzone", ""] {
        let listing = em
            .list_resource_associations(&profile_id, &format!("?resourceType={unmatched}"))
            .await;
        assert_eq!(
            listing.status, 200,
            "case resourceType={unmatched:?}: expected 200, got {} / {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed(&listing, "ResourceArn"),
            Vec::<String>::new(),
            "case resourceType={unmatched:?}: no association reports that type, got {:?}",
            listing.body
        );
    }
}

#[tokio::test]
async fn resource_arn_narrows_the_listing_to_the_association_with_that_resource() {
    let em = Emulator::new();
    let profile_id = profile(&em, "arn-filter").await;
    let neighbour_id = profile(&em, "arn-filter-neighbour").await;

    let resources = associable_resources();
    for (index, (resource_arn, resource_type)) in resources.iter().enumerate() {
        let properties = (*resource_type == "FIREWALL_RULE_GROUP").then_some(r#"{"priority":200}"#);
        let reply = em
            .associate_resource(&profile_id, resource_arn, &format!("arn-{index}"), properties)
            .await;
        assert_eq!(
            reply.status, 200,
            "case arn={resource_arn}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
    }

    // One row per kind of resource a Profile can hold: the filter selects the
    // association with THAT resource and no other.
    for (resource_arn, resource_type) in &resources {
        let listing = em
            .list_resource_associations(
                &profile_id,
                &format!("?resourceArn={}", path_label(resource_arn)),
            )
            .await;
        assert_eq!(
            listing.status, 200,
            "case resourceArn={resource_arn}: expected 200, got {} / {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed(&listing, "ResourceArn"),
            vec![resource_arn.clone()],
            "case resourceArn={resource_arn}: the listing must report exactly that \
             association, got {:?}",
            listing.body
        );

        // The two filters narrow together: the type the association reports selects it,
        // and any other type selects nothing.
        let both = em
            .list_resource_associations(
                &profile_id,
                &format!(
                    "?resourceArn={}&resourceType={resource_type}",
                    path_label(resource_arn)
                ),
            )
            .await;
        assert_eq!(
            listed(&both, "ResourceArn"),
            vec![resource_arn.clone()],
            "case resourceArn={resource_arn} resourceType={resource_type}: the listing must \
             report that association, got {:?}",
            both.body
        );
        let mismatched = em
            .list_resource_associations(
                &profile_id,
                &format!("?resourceArn={}&resourceType=VPC_ENDPOINT", path_label(resource_arn)),
            )
            .await;
        if *resource_type != "VPC_ENDPOINT" {
            assert_eq!(
                listed(&mismatched, "ResourceArn"),
                Vec::<String>::new(),
                "case resourceArn={resource_arn}: a type the association does not report must \
                 select nothing, got {:?}",
                mismatched.body
            );
        }

        // The filter is about the Profile named too: the same ARN under a Profile that
        // holds no association with it selects nothing.
        let elsewhere = em
            .list_resource_associations(
                &neighbour_id,
                &format!("?resourceArn={}", path_label(resource_arn)),
            )
            .await;
        assert_eq!(
            listed(&elsewhere, "ResourceArn"),
            Vec::<String>::new(),
            "case resourceArn={resource_arn}: another Profile holds no association with that \
             resource, got {:?}",
            elsewhere.body
        );
    }

    // An ARN nothing is associated with matches nothing rather than everything, and a
    // prefix of a held ARN is not that ARN.
    let held = resources[0].0.clone();
    for unmatched in [
        "arn:aws:route53:::hostedzone/ZNOTASSOCIATED00000000".to_string(),
        held[..held.len() - 1].to_string(),
        format!("{held}x"),
        // Exactly the modelled maximum (`smithy_query(query="operation",
        // operation="ListProfileResourceAssociations")`: `ResourceArn` is `len 1..255`).
        format!("arn:aws:route53:::hostedzone/{}", "Z".repeat(255 - 29)),
    ] {
        let listing = em
            .list_resource_associations(
                &profile_id,
                &format!("?resourceArn={}", path_label(&unmatched)),
            )
            .await;
        assert_eq!(
            listing.status, 200,
            "case resourceArn={unmatched:?}: expected 200, got {} / {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed(&listing, "ResourceArn"),
            Vec::<String>::new(),
            "case resourceArn={unmatched:?}: no association holds that resource, got {:?}",
            listing.body
        );
    }

    // And a value outside that modelled length is refused rather than filtering.
    for outside in [
        String::new(),
        format!("arn:aws:route53:::hostedzone/{}", "Z".repeat(256 - 29)),
    ] {
        let refused = em
            .list_resource_associations(
                &profile_id,
                &format!("?resourceArn={}", path_label(&outside)),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case resourceArn (len {}): a value outside the modelled length must be refused, \
             got {} / {:?}",
            outside.len(),
            refused.status,
            refused.body
        );
    }
}

#[tokio::test]
async fn the_listing_pages_in_a_stable_order_and_refuses_a_token_it_never_issued() {
    let em = Emulator::new();
    let profile_id = profile(&em, "paging").await;

    for (index, (resource_arn, resource_type)) in associable_resources().into_iter().enumerate() {
        let properties = (resource_type == "FIREWALL_RULE_GROUP").then_some(r#"{"priority":5000}"#);
        let reply = em
            .associate_resource(&profile_id, &resource_arn, &format!("page-{index}"), properties)
            .await;
        assert_eq!(
            reply.status, 200,
            "case arn={resource_arn}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
    }

    let whole = em.list_resource_associations(&profile_id, "").await;
    let expected = listed(&whole, "Id");
    assert_eq!(
        expected.len(),
        associable_resources().len(),
        "the unpaged listing must report every association, got {:?}",
        whole.body
    );

    // `MaxResults` is modelled `range 1..100`: both published bounds and the sizes
    // between them have to walk the same associations in the same order.
    for max_results in [1, 2, 3, 5, 6, 7, 100] {
        let paged = all_pages(&em, &profile_id, max_results).await;
        assert_eq!(
            paged, expected,
            "case maxResults={max_results}: paging must walk every association once, in the \
             order the unpaged listing reports"
        );
    }

    // A token this operation never issued resumes nothing.
    for bogus in ["not-hex", "6e6f70", "4c697374"] {
        let refusal = em
            .list_resource_associations(&profile_id, &format!("?nextToken={bogus}"))
            .await;
        assert_eq!(
            refusal.status, 400,
            "case nextToken={bogus}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert_eq!(
            refusal.error.as_deref(),
            Some("InvalidNextTokenException"),
            "case nextToken={bogus}: unexpected error code, got {:?}",
            refusal.body
        );
    }

    // A Profile that was never created holds no listing to report.
    for unknown in ["rp-00000000000000000000", "rp-doesnotexist00000"] {
        let refusal = em.list_resource_associations(unknown, "").await;
        assert_eq!(
            refusal.status, 400,
            "case profile={unknown}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert_eq!(
            refusal.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case profile={unknown}: expected ResourceNotFoundException, got {:?}",
            refusal.body
        );
        assert_eq!(
            refusal.message(),
            format!("[RSLVR-05007] Can't find the resource with ID \"{unknown}\"."),
            "case profile={unknown}: unexpected refusal message"
        );
    }
}

/// The `Status` an association carries while it is still being applied to the
/// Profile, and the one it carries once it is in force — the two
/// `ProfileResourceAssociation$Status` values this service reports for a resource
/// association it holds.
const BEING_APPLIED_STATUS: &str = "UPDATING";
const SETTLED_STATUS: &str = "COMPLETE";

/// How long this suite is willing to wait for a move the clock owes an association
/// to fall due, read by POLLING rather than by sleeping for a span: generous enough
/// that a loaded host still gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How long to wait between two readings of the listing while polling for that move.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The sentence reported while the association of a `kind` of resource is still
/// being applied to the Profile, transcribed so this suite states it independently
/// of the code under test.
fn being_applied_message(kind: &str) -> String {
    format!("Updating the Profile to {kind} association")
}

/// The sentence reported once that association is in force.
fn settled_message(kind: &str) -> String {
    format!("Completed creation of Profile to {kind} association")
}

/// Resources of three associable kinds as `(ResourceArn, ResourceType, kind
/// phrase)`: the kinds `ProfileResourceAssociation$ResourceType`'s documentation
/// names ("a private hosted zone, or DNS Firewall rule group") plus the Resolver
/// rule beside them, with the words AWS names each kind by in prose — which is what
/// a lifecycle sentence names it by.
fn phased_resources() -> Vec<(String, &'static str, &'static str)> {
    vec![
        (
            "arn:aws:route53:::hostedzone/Z77777777777CCCCCCCCC".to_string(),
            "HOSTEDZONE",
            "private hosted zone",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-rule/rslvr-rr-7777777777777777"
            ),
            "RESOLVER_RULE",
            "Resolver rule",
        ),
        (
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-7777777777777777"
            ),
            "FIREWALL_RULE_GROUP",
            "DNS Firewall rule group",
        ),
    ]
}

/// SEVERAL resources of each of three kinds, as `(ResourceArn, ResourceType)`: a
/// Profile holding more than one resource of a kind is what lets a narrowed listing
/// be paged, which one resource per kind cannot show.
fn mixed_resources() -> Vec<(String, &'static str)> {
    let mut rows: Vec<(String, &'static str)> = Vec::new();
    for tag in 0..5u64 {
        rows.push((
            format!("arn:aws:route53:::hostedzone/Z{tag:021}"),
            "HOSTEDZONE",
        ));
    }
    for tag in 0..3u64 {
        rows.push((
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:resolver-rule/rslvr-rr-{tag:016x}"
            ),
            "RESOLVER_RULE",
        ));
    }
    for tag in 0..2u64 {
        rows.push((
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT_ID}:firewall-rule-group/rslvr-frg-{tag:016x}"
            ),
            "FIREWALL_RULE_GROUP",
        ));
    }
    rows
}

/// Associate `arn` with `profile_id` under `name`, with the `priority` a DNS
/// Firewall rule group's properties carry and nothing for every other kind, and
/// answer the minted association `Id`.
async fn associate(
    em: &Emulator,
    profile_id: &str,
    arn: &str,
    resource_type: &str,
    name: &str,
) -> String {
    // 200 is inside the documented 100..9900 a rule group's priority has to state.
    let properties = (resource_type == "FIREWALL_RULE_GROUP").then_some(r#"{"priority":200}"#);
    let reply = em
        .associate_resource(profile_id, arn, name, properties)
        .await;
    assert_eq!(
        reply.status, 200,
        "case arn={arn}: AssociateResourceToProfile expected 200, got {} / {:?}",
        reply.status, reply.body
    );
    reply.member("ProfileResourceAssociation")["Id"]
        .as_str()
        .expect("a created association reports its Id")
        .to_string()
}

/// `UpdateProfileResourceAssociation`, through the shared harness method so this
/// suite builds no second copy of that request.
async fn update_association(em: &Emulator, id: &str, body: Value) -> Reply {
    em.update_resource_association(id, body).await
}

/// `DisassociateResourceFromProfile`: the (Profile, resource ARN) pair are both
/// path labels, so the ARN travels percent-encoded as one segment.
async fn disassociate_resource(em: &Emulator, profile_id: &str, arn: &str) -> Reply {
    em.call(
        "DELETE",
        &format!(
            "/profileresourceassociation/profileid/{profile_id}/resourcearn/{}",
            path_label(arn)
        ),
        None,
    )
    .await
}

/// Walk every page of the listing narrowed to `resource_type`, answering the `Id`s
/// in the order they were reported and the size of each page walked.
async fn pages_of_type(
    em: &Emulator,
    profile_id: &str,
    resource_type: &str,
    max_results: usize,
) -> (Vec<String>, Vec<usize>) {
    let case = format!("resourceType={resource_type} maxResults={max_results}");
    let mut ids: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    // One page per stored association is the worst case; the bound keeps a broken
    // token from looping forever instead of failing.
    for page in 0..64 {
        let query = match token.as_deref() {
            None => format!("?resourceType={resource_type}&maxResults={max_results}"),
            Some(token) => format!(
                "?resourceType={resource_type}&maxResults={max_results}&nextToken={token}"
            ),
        };
        let reply = em.list_resource_associations(profile_id, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case} page {page}: expected 200, got {} / {:?}",
            reply.status, reply.body
        );
        let reported = listed(&reply, "Id");
        assert!(
            reported.len() <= max_results,
            "case {case} page {page}: a page must hold at most maxResults, got {reported:?}"
        );
        for association in reply.list("ProfileResourceAssociations") {
            assert_eq!(
                association["ResourceType"].as_str(),
                Some(resource_type),
                "case {case} page {page}: a narrowed page must report only that type, got \
                 {association}"
            );
        }
        sizes.push(reported.len());
        ids.extend(reported);
        token = reply.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return (ids, sizes);
        }
    }
    panic!("case {case}: the listing never stopped paging, got {ids:?}");
}

#[tokio::test]
async fn the_listing_reports_each_association_in_the_phase_the_clock_has_moved_it_to() {
    let em = Emulator::new();
    let profile_id = profile(&em, "listing-phases").await;
    let resources = phased_resources();
    for (index, (arn, resource_type, _)) in resources.iter().enumerate() {
        associate(
            &em,
            &profile_id,
            arn,
            resource_type,
            &format!("phase-{index}"),
        )
        .await;
    }

    // The move out of the phase the associate call starts an association in is the
    // CLOCK's, not a caller's, so the listing is READ AGAIN until it reports every
    // association settled. Every reading in between has to report each association
    // in one of the two phases, with the sentence that phase is worded in for the
    // kind of resource that association holds.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let listing = em.list_resource_associations(&profile_id, "").await;
        assert_eq!(
            listing.status, 200,
            "case profile={profile_id}: expected 200, got {} / {:?}",
            listing.status, listing.body
        );
        let reported = listing.list("ProfileResourceAssociations");
        assert_eq!(
            reported.len(),
            resources.len(),
            "case profile={profile_id}: every association must be reported in every phase, got \
             {:?}",
            listing.body
        );
        let mut settled = 0usize;
        for (arn, resource_type, kind) in &resources {
            let association = reported
                .iter()
                .find(|association| association["ResourceArn"].as_str() == Some(arn.as_str()))
                .unwrap_or_else(|| panic!("case arn={arn}: absent from {:?}", listing.body));
            assert_eq!(
                association["ResourceType"].as_str(),
                Some(*resource_type),
                "case arn={arn}: unexpected ResourceType in {association}"
            );
            let status = association["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("case arn={arn}: no Status in {association}"));
            let message = association["StatusMessage"]
                .as_str()
                .unwrap_or_else(|| panic!("case arn={arn}: no StatusMessage in {association}"));
            if status == SETTLED_STATUS {
                assert_eq!(
                    message,
                    settled_message(kind),
                    "case arn={arn}: unexpected settled StatusMessage in {association}"
                );
                settled += 1;
            } else {
                assert_eq!(
                    status, BEING_APPLIED_STATUS,
                    "case arn={arn}: an association is reported as being applied or as settled, \
                     got {association}"
                );
                assert_eq!(
                    message,
                    being_applied_message(kind),
                    "case arn={arn}: unexpected in-progress StatusMessage in {association}"
                );
            }
        }
        if settled == resources.len() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case profile={profile_id}: the listing never reported every association settled, \
             last reading {:?}",
            listing.body
        );
        sleep(POLL_INTERVAL);
    }

    // The phase is settled, so reading the listing changes nothing a later caller
    // sees: two readings report the same record, `ModificationTime` included.
    let first = em.list_resource_associations(&profile_id, "").await;
    let second = em.list_resource_associations(&profile_id, "").await;
    assert_eq!(
        first.list("ProfileResourceAssociations"),
        second.list("ProfileResourceAssociations"),
        "case profile={profile_id}: a settled listing must report the same record on every read, \
         got {:?} then {:?}",
        first.body,
        second.body
    );
}

#[tokio::test]
async fn resource_type_narrows_the_listing_before_the_page_is_cut() {
    let em = Emulator::new();
    let profile_id = profile(&em, "filtered-paging").await;
    let resources = mixed_resources();
    for (index, (arn, resource_type)) in resources.iter().enumerate() {
        associate(
            &em,
            &profile_id,
            arn,
            resource_type,
            &format!("mixed-{index}"),
        )
        .await;
    }

    for wanted in ["HOSTEDZONE", "RESOLVER_RULE", "FIREWALL_RULE_GROUP"] {
        let held: Vec<String> = resources
            .iter()
            .filter(|(_, resource_type)| *resource_type == wanted)
            .map(|(arn, _)| arn.clone())
            .collect();
        let whole = em
            .list_resource_associations(&profile_id, &format!("?resourceType={wanted}"))
            .await;
        assert_eq!(
            whole.status, 200,
            "case resourceType={wanted}: expected 200, got {} / {:?}",
            whole.status, whole.body
        );
        let mut reported_arns = listed(&whole, "ResourceArn");
        reported_arns.sort();
        let mut expected_arns = held.clone();
        expected_arns.sort();
        assert_eq!(
            reported_arns, expected_arns,
            "case resourceType={wanted}: the unpaged narrowed listing must report exactly that \
             type's associations, got {:?}",
            whole.body
        );
        let expected_ids = listed(&whole, "Id");

        // `MaxResults` cuts the NARROWED set, so every page but the last is full and
        // the number of pages follows from how many associations match — not from
        // how many of an unfiltered page happened to.
        for max_results in [1, 2, 3, 100] {
            let (ids, sizes) = pages_of_type(&em, &profile_id, wanted, max_results).await;
            let case = format!("resourceType={wanted} maxResults={max_results}");
            assert_eq!(
                ids, expected_ids,
                "case {case}: paging must walk every matching association once, in the order the \
                 unpaged narrowed listing reports"
            );
            let expected_pages = held.len().div_ceil(max_results);
            assert_eq!(
                sizes.len(),
                expected_pages,
                "case {case}: {} matching associations must be reported in {expected_pages} \
                 page(s), got page sizes {sizes:?}",
                held.len()
            );
            for (page, size) in sizes.iter().enumerate() {
                let expected = if page + 1 == sizes.len() {
                    held.len() - page * max_results
                } else {
                    max_results
                };
                assert_eq!(
                    *size, expected,
                    "case {case} page {page}: expected {expected} association(s) on this page, \
                     got page sizes {sizes:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_listing_follows_the_store_through_an_update_and_a_disassociate() {
    let em = Emulator::new();
    let profile_id = profile(&em, "listing-follows-store").await;
    let (zone_arn, _, _) = phased_resources()[0].clone();
    let (rule_group_arn, rule_group_type, _) = phased_resources()[2].clone();
    let zone = associate(&em, &profile_id, &zone_arn, "HOSTEDZONE", "kept").await;
    let rule_group = associate(
        &em,
        &profile_id,
        &rule_group_arn,
        rule_group_type,
        "changed",
    )
    .await;

    /// The `(Id, Name, ResourceProperties)` of every association the listing
    /// reports, which is what an update has to show up in.
    fn records(reply: &Reply) -> Vec<(String, Value, Value)> {
        reply
            .list("ProfileResourceAssociations")
            .iter()
            .map(|association| {
                (
                    association["Id"].as_str().unwrap_or_default().to_string(),
                    association["Name"].clone(),
                    association["ResourceProperties"].clone(),
                )
            })
            .collect()
    }

    // An update of the members a request names is reported by the listing, and the
    // association it did not name is reported exactly as it stood.
    // 9900 is the top of the documented 100..9900 range a rule group's priority may
    // state.
    let updated_properties = r#"{"priority":9900}"#;
    let update = update_association(
        &em,
        &rule_group,
        json!({ "Name": "renamed", "ResourceProperties": updated_properties }),
    )
    .await;
    assert_eq!(
        update.status, 200,
        "case id={rule_group}: UpdateProfileResourceAssociation expected 200, got {} / {:?}",
        update.status, update.body
    );
    let after_update = em.list_resource_associations(&profile_id, "").await;
    let mut expected = vec![
        (
            rule_group.clone(),
            json!("renamed"),
            json!(updated_properties),
        ),
        (zone.clone(), json!("kept"), Value::Null),
    ];
    expected.sort_by_key(|(id, ..)| id.clone());
    let mut reported = records(&after_update);
    reported.sort_by_key(|(id, ..)| id.clone());
    assert_eq!(
        reported, expected,
        "case id={rule_group}: the listing must report the updated members and leave the other \
         association as it stood, got {:?}",
        after_update.body
    );

    // A refused update changes nothing the listing reports: a rule group's priority
    // just below the documented minimum is outside the range.
    let refused = update_association(&em, &rule_group, json!({ "ResourceProperties": r#"{"priority":99}"# })).await;
    assert_eq!(
        refused.status, 400,
        "case id={rule_group}: a priority below the documented minimum expected 400, got {} / {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case id={rule_group}: unexpected error code, got {:?}",
        refused.body
    );
    let after_refusal = em.list_resource_associations(&profile_id, "").await;
    let mut still = records(&after_refusal);
    still.sort_by_key(|(id, ..)| id.clone());
    assert_eq!(
        still, expected,
        "case id={rule_group}: a refused update must leave the listing exactly as it was, got \
         {:?}",
        after_refusal.body
    );

    // A disassociated resource stops being reported, and its id stops naming
    // anything.
    let removed = disassociate_resource(&em, &profile_id, &rule_group_arn).await;
    assert_eq!(
        removed.status, 200,
        "case arn={rule_group_arn}: DisassociateResourceFromProfile expected 200, got {} / {:?}",
        removed.status, removed.body
    );
    let after_removal = em.list_resource_associations(&profile_id, "").await;
    assert_eq!(
        listed(&after_removal, "Id"),
        vec![zone.clone()],
        "case arn={rule_group_arn}: a disassociated resource must stop being reported, got {:?}",
        after_removal.body
    );
    let gone = em.get_resource_association(&rule_group).await;
    assert_eq!(
        gone.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case id={rule_group}: a disassociated association's id must name nothing, got {:?}",
        gone.body
    );
    assert_eq!(
        gone.message(),
        association_not_found_message(&rule_group),
        "case id={rule_group}: expected {:?}, got {:?}",
        association_not_found_message(&rule_group),
        gone.message()
    );

    // `MaxResults` is modelled `range 1..100`, so a page size outside it is refused
    // rather than clamped — and the refusal reports nothing.
    for max_results in ["0", "101", "-1"] {
        let refusal = em
            .list_resource_associations(&profile_id, &format!("?maxResults={max_results}"))
            .await;
        assert_eq!(
            refusal.status, 400,
            "case maxResults={max_results}: expected 400, got {} / {:?}",
            refusal.status, refusal.body
        );
        assert!(
            refusal.body["ProfileResourceAssociations"].is_null(),
            "case maxResults={max_results}: a refused listing must report nothing, got {:?}",
            refusal.body
        );
    }
    let unaffected = em.list_resource_associations(&profile_id, "").await;
    assert_eq!(
        listed(&unaffected, "Id"),
        vec![zone],
        "case profile={profile_id}: a refused page size must leave the listing as it was, got \
         {:?}",
        unaffected.body
    );
}
