// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the domain name access association surface:
//! `CreateDomainNameAccessAssociation` and `GetDomainNameAccessAssociations`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULES under test, each over a SET of inputs rather than one recorded call:
//!
//! * a created access association is PERSISTED and every read reports that record — the
//!   members the request set, the ARN the service assigned it, and its tags — over a table
//!   spanning the tag collections and source identifiers a create may carry and the
//!   private domain names it may name (including two private records that share one name,
//!   which are two domain names);
//! * an access association is the PAIR `(private custom domain name, access association
//!   source)`, so an account holds at most one for a pair: a second create of a pair is
//!   the modelled `BadRequestException` carrying AWS's fixed sentence for an association
//!   that already exists, while
//!   another source on the same domain name, or
//!   the same source on another domain name, is a new association — and concurrent creates
//!   of ONE pair store exactly one record, over a table of racer counts;
//! * a create names a STORED, PRIVATE custom domain name: a `domainNameArn` that is not an
//!   ARN, names another kind of resource, names no stored domain name, or names a domain
//!   name whose endpoint type is not `PRIVATE` is the modelled `BadRequestException`, and
//!   so is a source type outside the modelled enum;
//! * `GetDomainNameAccessAssociations` reports the stored records in creation order, one
//!   `position` / `limit` page at a time, over page sizes across the documented range
//!   (default 25, maximum 500), each published bound and one past each, crossed with every
//!   cursor the sequence holds;
//! * the `resourceOwner` filter over every value the modelled `ResourceOwner` enum
//!   declares (`SELF`, `OTHER_ACCOUNTS`) and the request that sets none;
//! * a read changes nothing a later caller sees.
//!
//! [`Gateway`], [`Answer`], [`assert_failure`], [`string_member`] and [`page_items`] are
//! this crate's one wire-test harness, reused rather than standing a second emulator up
//! another way; `created_domain` is the domain-name suite's one create of a domain name,
//! and `apigateway_arn` / `get_tags` the tagging suite's one spelling of an ARN and of a
//! tag read.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    PRIVATE_ENDPOINT_TYPE, REGIONAL_ENDPOINT_TYPE,
};

use crate::domain_name_collection::created_domain;
use crate::rest_api_lifecycle::{assert_failure, page_items, string_member, Answer, Gateway};
use crate::tag_reads_by_arn::{apigateway_arn, get_tags};

const BAD_REQUEST: &str = "BadRequestException";

/// What AWS refuses a second create of one `(domain name, access association source)` pair
/// with, verbatim: one sentence of entirely fixed text naming the two things the pair is
/// made of by their kind, with no part of it taken from the request or from the store.
const ALREADY_EXISTS: &str =
    "The domain name access association between this VPC endpoint and domain name already \
     exists.";

/// The one value the modelled `AccessAssociationSourceType` enum declares
/// (`smithy_query(query="shape", shape="AccessAssociationSourceType")`: `VPCE`).
const VPCE: &str = "VPCE";

/// `CreateDomainNameAccessAssociation` with `body`.
///
/// Shared with the `delete_domain_name` suite, which needs a STORED association to show
/// that a domain name one depends on cannot be deleted while it stands.
pub(crate) async fn create_association(gateway: &Gateway, body: Value) -> Answer {
    gateway
        .call("POST", "/domainnameaccessassociations", Some(body))
        .await
}

/// The request body for an association of `source` with the domain name `domain_name_arn`,
/// as the three required members the model declares.
pub(crate) fn association_body(domain_name_arn: &str, source: &str) -> Value {
    json!({
        "domainNameArn": domain_name_arn,
        "accessAssociationSourceType": VPCE,
        "accessAssociationSource": source,
    })
}

/// `GetDomainNameAccessAssociations` with `query` appended when it is not empty.
///
/// Shared with the `delete_domain_name_access_association` suite, which reads the
/// collection back to show which records a delete left.
pub(crate) async fn list_associations(gateway: &Gateway, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/domainnameaccessassociations".to_string()
    } else {
        format!("/domainnameaccessassociations?{query}")
    };
    gateway.call("GET", &uri, None).await
}

/// `CreateDomainName` for a PRIVATE custom domain name, answering the ARN of the record it
/// stored — the value an access association names its domain name by.
///
/// Shared with the `delete_domain_name_access_association` suite, which needs the same
/// kind of domain name to create the associations it removes.
pub(crate) async fn private_domain(gateway: &Gateway, domain_name: &str) -> String {
    let created = created_domain(
        gateway,
        domain_name,
        json!({ "endpointConfiguration": { "types": [PRIVATE_ENDPOINT_TYPE] } }),
    )
    .await;
    string_member(
        &created,
        "domainNameArn",
        &format!("CreateDomainName({domain_name:?}, PRIVATE)"),
    )
}

/// The `domainNameAccessAssociationArn` of every record on a page, in page order.
pub(crate) fn listed_arns(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| string_member(item, "domainNameAccessAssociationArn", case))
        .collect()
}

#[tokio::test]
async fn a_created_access_association_is_persisted_and_read_back_member_for_member() {
    let gateway = Gateway::new();
    // Two private records carrying ONE name are two domain names — a `domainNameId` is
    // "supported only for private custom domain names" and is what identifies one — so an
    // association against each is an association against a different domain name.
    let first = private_domain(&gateway, "private.example.com").await;
    let second = private_domain(&gateway, "private.example.com").await;
    let third = private_domain(&gateway, "other.example.org").await;
    assert_ne!(
        first, second,
        "case CreateDomainName(two private records of one name): expected distinct ARNs, \
         got {first:?} twice"
    );

    // Rows differ along the domain name named, the form of the access association source
    // identifier, and the tag collection the create carries — none, one, several, and a
    // tag whose value is empty.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "no tags",
            first.as_str(),
            "vpce-0123456789abcdef0",
            json!({}),
        ),
        (
            "one tag",
            first.as_str(),
            "vpce-00000000000000001",
            json!({ "Name": "payments" }),
        ),
        (
            "several tags, one with an empty value",
            second.as_str(),
            "vpce-0123456789abcdef0",
            json!({ "cost:center": "team/platform", "empty-value": "" }),
        ),
        (
            "another domain name, the shorter of the two source id widths",
            third.as_str(),
            "vpce-0a1b2c3d",
            json!({ "Name": "orders" }),
        ),
    ];

    let mut created: Vec<(String, Value)> = Vec::new();
    for (case, domain_name_arn, source, tags) in &rows {
        let mut body = association_body(domain_name_arn, source);
        if let Some(entries) = tags.as_object() {
            if !entries.is_empty() {
                body["tags"] = tags.clone();
            }
        }
        let answer = create_association(&gateway, body).await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["domainNameArn"].as_str(),
            Some(*domain_name_arn),
            "case {case}: expected the domain name it was created against, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["accessAssociationSource"].as_str(),
            Some(*source),
            "case {case}: expected the source it was created for, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["accessAssociationSourceType"].as_str(),
            Some(VPCE),
            "case {case}: expected the source type it was created with, got {:?}",
            answer.body
        );
        assert_eq!(
            &answer.body["tags"], tags,
            "case {case}: expected the tags it was created with, got {:?}",
            answer.body["tags"]
        );
        let arn = string_member(&answer.body, "domainNameAccessAssociationArn", case);
        assert!(
            arn.starts_with("arn:aws:apigateway:")
                && arn.contains("::/domainnameaccessassociations/"),
            "case {case}: expected an API Gateway access association ARN, got {arn:?}"
        );
        created.push((arn, answer.body));
    }

    // Every record is reported by the collection, in creation order, exactly as the create
    // reported it: one record, read two ways.
    let listed = list_associations(&gateway, "limit=500").await;
    assert_eq!(
        listed.status, 200,
        "case GetDomainNameAccessAssociations(limit=500): expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    let expected_arns: Vec<String> = created.iter().map(|(arn, _)| arn.clone()).collect();
    assert_eq!(
        listed_arns(&listed.body, "GetDomainNameAccessAssociations(limit=500)"),
        expected_arns,
        "case GetDomainNameAccessAssociations(limit=500): expected creation order \
         {expected_arns:?}, got {:?}",
        listed.body
    );
    let items = page_items(&listed.body, "GetDomainNameAccessAssociations(limit=500)");
    for (index, (arn, body)) in created.iter().enumerate() {
        assert_eq!(
            &items[index], body,
            "case {arn}: expected the created record member for member, got {:?}",
            items[index]
        );
    }

    // The ARN the create assigned resolves the stored record for the tag operations too,
    // which is the ARN this service says an association is addressed by.
    for (arn, body) in &created {
        let tags = get_tags(&gateway, arn).await;
        assert_eq!(
            tags.status, 200,
            "case GetTags({arn}): expected 200, got {} with {:?}",
            tags.status, tags.body
        );
        assert_eq!(
            tags.body["tags"], body["tags"],
            "case GetTags({arn}): expected the association's own tags, got {:?}",
            tags.body["tags"]
        );
    }

    // An ARN of this kind that no stored record answers to is still a missing resource.
    assert_failure(
        &get_tags(
            &gateway,
            &apigateway_arn("/domainnameaccessassociations/zzzzz99999"),
        )
        .await,
        404,
        "NotFoundException",
        "GetTags(an access association no record answers to)",
    );

    // Reading changed nothing a later caller sees.
    let again = list_associations(&gateway, "limit=500").await;
    assert_eq!(
        again.body, listed.body,
        "case GetDomainNameAccessAssociations(read twice): expected the same page, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn the_pair_of_domain_name_and_source_identifies_at_most_one_association() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "pair.example.com").await;
    let other_domain = private_domain(&gateway, "pair-other.example.com").await;
    let source = "vpce-0123456789abcdef0";

    let first = create_association(&gateway, association_body(&domain, source)).await;
    assert_eq!(
        first.status, 201,
        "case the first create of a pair: expected 201, got {} with {:?}",
        first.status, first.body
    );
    let first_arn = string_member(
        &first.body,
        "domainNameAccessAssociationArn",
        "the first create of a pair",
    );

    // Rows differ along WHICH member repeats: the whole pair (already there), only the
    // source, only the domain name. Only the whole pair names an association that already
    // exists.
    let rows: Vec<(&str, String, &str, Option<u16>)> = vec![
        (
            "the same pair again",
            domain.clone(),
            source,
            Some(400),
        ),
        (
            "the same pair a third time",
            domain.clone(),
            source,
            Some(400),
        ),
        (
            "another source on the same domain name",
            domain.clone(),
            "vpce-00000000000000002",
            None,
        ),
        (
            "the same source on another domain name",
            other_domain.clone(),
            source,
            None,
        ),
        (
            "the pair that is now also taken on the other domain name",
            other_domain.clone(),
            source,
            Some(400),
        ),
    ];

    let mut stored = vec![first_arn];
    for (case, domain_name_arn, repeated_source, refused) in &rows {
        let answer = create_association(
            &gateway,
            association_body(domain_name_arn, repeated_source),
        )
        .await;
        match refused {
            Some(status) => {
                assert_failure(&answer, *status, BAD_REQUEST, case);
                // The refusal states the condition — the pair is already bound — in one
                // sentence of fixed text, carrying neither half of the pair.
                let message = answer.body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, ALREADY_EXISTS,
                    "case {case}: expected the already-exists sentence, got {message:?}"
                );
                assert!(
                    !message.contains(*repeated_source)
                        && !message.contains(domain_name_arn.as_str()),
                    "case {case}: the sentence must carry nothing from the request, got \
                     {message:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 201,
                    "case {case}: expected 201, got {} with {:?}",
                    answer.status, answer.body
                );
                stored.push(string_member(
                    &answer.body,
                    "domainNameAccessAssociationArn",
                    case,
                ));
            }
        }
    }

    // A refused create stored nothing: the collection holds exactly the accepted ones.
    let listed = list_associations(&gateway, "limit=500").await;
    assert_eq!(
        listed_arns(&listed.body, "GetDomainNameAccessAssociations after the refusals"),
        stored,
        "case GetDomainNameAccessAssociations after the refusals: expected {stored:?}, got {:?}",
        listed.body
    );
}

/// RULE: concurrent creates of ONE pair store exactly one association — the pair is
/// decided and written under one hold of the store, so one caller wins and every other is
/// refused as a request naming an association that already exists, whatever the racer
/// count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_pair_store_exactly_one_association() {
    for racers in [2_usize, 3, 6] {
        let case = format!("{racers} concurrent creates of one pair");
        let gateway = Arc::new(Gateway::new());
        let domain = private_domain(&gateway, "race.example.com").await;
        let source = "vpce-0123456789abcdef0";

        let mut tasks = Vec::with_capacity(racers);
        for _ in 0..racers {
            let gateway = Arc::clone(&gateway);
            let body = association_body(&domain, source);
            tasks.push(tokio::spawn(async move {
                let answer = create_association(&gateway, body).await;
                (answer.status, answer.error)
            }));
        }
        let mut answers = Vec::with_capacity(racers);
        for task in tasks {
            answers.push(task.await.expect("the racing create answered"));
        }

        let created = answers.iter().filter(|(status, _)| *status == 201).count();
        assert_eq!(
            created, 1,
            "case {case}: expected exactly one accepted create, got {answers:?}"
        );
        for (status, error) in &answers {
            if *status == 201 {
                continue;
            }
            assert_eq!(
                (*status, error.as_deref()),
                (400, Some(BAD_REQUEST)),
                "case {case}: every loser is the modelled bad request, got {answers:?}"
            );
        }

        let listed = list_associations(&gateway, "limit=500").await;
        assert_eq!(
            listed_arns(&listed.body, &case).len(),
            1,
            "case {case}: expected exactly one stored association, got {:?}",
            listed.body
        );
    }
}

#[tokio::test]
async fn an_access_association_names_a_stored_private_custom_domain_name() {
    let gateway = Gateway::new();
    let private = private_domain(&gateway, "stored.example.com").await;

    // A regional and an edge-optimized (the endpoint type a request that names none gets)
    // custom domain name: both are stored records, and neither is a domain name an access
    // association is defined for.
    let regional = string_member(
        &created_domain(
            &gateway,
            "regional.example.com",
            json!({ "endpointConfiguration": { "types": [REGIONAL_ENDPOINT_TYPE] } }),
        )
        .await,
        "domainNameArn",
        "CreateDomainName(REGIONAL)",
    );
    let edge = string_member(
        &created_domain(&gateway, "edge.example.com", json!({})).await,
        "domainNameArn",
        "CreateDomainName(no endpoint type)",
    );

    // Rows differ along HOW the request fails to name a private custom domain name of this
    // account: an unreadable value, an ARN of another vendor, an API Gateway ARN of another
    // kind of resource, a well-formed domain name ARN no record answers to, and a stored
    // domain name whose endpoint type is not PRIVATE.
    let rows: Vec<(&str, String)> = vec![
        ("a value that is not an arn", "not-a-valid-arn".to_string()),
        ("a bare domain name", "private.example.com".to_string()),
        ("an empty value", String::new()),
        ("an arn with too few fields", "arn:aws:apigateway".to_string()),
        ("an arn of another vendor", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an arn naming a rest api",
            apigateway_arn("/restapis/abcde12345"),
        ),
        (
            "an arn naming an access association",
            apigateway_arn("/domainnameaccessassociations/abcde12345"),
        ),
        (
            "a domain name no record answers to",
            apigateway_arn("/domainnames/absent.example.com"),
        ),
        ("a regional domain name", regional),
        ("an edge-optimized domain name", edge),
    ];
    for (case, domain_name_arn) in &rows {
        let answer = create_association(
            &gateway,
            association_body(domain_name_arn, "vpce-0123456789abcdef0"),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, case);
    }

    // A source type outside the modelled enum is refused by the CONSTRAINT layer in
    // front of the handler — whose error class is `ValidationException`, not the
    // operation's own `BadRequestException` — and not stored as if it had named a source
    // type.
    for source_type in ["VPC", "vpce", "", "VPCE "] {
        let answer = create_association(
            &gateway,
            json!({
                "domainNameArn": private,
                "accessAssociationSourceType": source_type,
                "accessAssociationSource": "vpce-0123456789abcdef0",
            }),
        )
        .await;
        assert_failure(
            &answer,
            400,
            "ValidationException",
            &format!("accessAssociationSourceType={source_type:?}"),
        );
    }

    // None of the refusals stored anything.
    let listed = list_associations(&gateway, "limit=500").await;
    assert!(
        page_items(&listed.body, "GetDomainNameAccessAssociations after the refusals").is_empty(),
        "case GetDomainNameAccessAssociations after the refusals: expected an empty page, \
         got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn the_collection_is_reported_in_creation_order_one_page_at_a_time() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "pages.example.com").await;
    // Five associations of one domain name, created in this order.
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let answer = create_association(
            &gateway,
            association_body(&domain, &format!("vpce-0000000000000000{index}")),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case CreateDomainNameAccessAssociation(source {index}): expected 201, got {} \
             with {:?}",
            answer.status, answer.body
        );
        created.push(string_member(
            &answer.body,
            "domainNameAccessAssociationArn",
            "the created association",
        ));
    }

    // The cursor a page hands back is the identifier inside the ARN, so a caller pages with
    // what the previous page reported rather than with a value it built itself.
    let all = list_associations(&gateway, "").await;
    assert_eq!(
        listed_arns(&all.body, "GetDomainNameAccessAssociations(no limit)"),
        created,
        "case GetDomainNameAccessAssociations(no limit): expected creation order {created:?}, \
         got {:?}",
        all.body
    );
    assert!(
        all.body["position"].is_null(),
        "case GetDomainNameAccessAssociations(no limit): a page that holds the whole \
         collection reports no cursor, got {:?}",
        all.body["position"]
    );

    // Rows span the documented page-size range (default 25, maximum 500), each published
    // bound and one past each — a size below 1 and a size above 500 are clamped into the
    // range rather than refused.
    for limit in [0, 1, 2, 3, 5, 6, 25, 500, 501, 10_000] {
        let effective = usize::try_from(limit.clamp(1, 500)).expect("the clamped limit is positive");
        let query = format!("limit={limit}");
        let case = format!("GetDomainNameAccessAssociations({query})");
        let answer = list_associations(&gateway, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let expected: Vec<String> = created.iter().take(effective).cloned().collect();
        assert_eq!(
            listed_arns(&answer.body, &case),
            expected,
            "case {case}: expected the page {expected:?}, got {:?}",
            answer.body
        );
    }

    // Paging with each size reassembles the whole collection, once each, in order.
    for limit in [1, 2, 3, 25] {
        let case = format!("paging with limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut position: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &position {
                None => format!("limit={limit}"),
                Some(cursor) => format!("limit={limit}&position={cursor}"),
            };
            let answer = list_associations(&gateway, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            seen.extend(listed_arns(&answer.body, &case));
            pages += 1;
            assert!(pages <= 20, "case {case}: pagination did not terminate");
            match answer.body["position"].as_str() {
                None => break,
                Some(cursor) => position = Some(cursor.to_owned()),
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must reassemble creation order {created:?}, got {seen:?}"
        );
    }

    // A cursor naming nothing in the sequence is a bad request, not an empty page.
    for cursor in ["nosuchcursor", "zzzzz99999", ""] {
        let answer = list_associations(&gateway, &format!("position={cursor}")).await;
        assert_failure(
            &answer,
            400,
            BAD_REQUEST,
            &format!("GetDomainNameAccessAssociations(position={cursor:?})"),
        );
    }
}

#[tokio::test]
async fn the_resource_owner_filter_selects_by_owner() {
    let gateway = Gateway::new();
    let domain = private_domain(&gateway, "owner.example.com").await;
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        let answer = create_association(
            &gateway,
            association_body(&domain, &format!("vpce-0000000000000001{index}")),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case CreateDomainNameAccessAssociation(owner row {index}): expected 201, got {} \
             with {:?}",
            answer.status, answer.body
        );
        created.push(string_member(
            &answer.body,
            "domainNameAccessAssociationArn",
            "the created association",
        ));
    }

    // Every value the modelled `ResourceOwner` enum declares, plus the request that sets
    // none. This emulator serves one account, and every stored association was created by
    // its own call, so a request for another account's associations gets an empty page.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("no resourceOwner", "", created.clone()),
        ("resourceOwner=SELF", "resourceOwner=SELF", created.clone()),
        (
            "resourceOwner=OTHER_ACCOUNTS",
            "resourceOwner=OTHER_ACCOUNTS",
            Vec::new(),
        ),
    ];
    for (case, query, expected) in &rows {
        let answer = list_associations(&gateway, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &listed_arns(&answer.body, case),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            answer.body["position"].is_null(),
            "case {case}: a page the collection holds nothing after reports no cursor, got \
             {:?}",
            answer.body["position"]
        );
    }

    // A value outside the enum is refused by the CONSTRAINT layer under its own error
    // class, not served as if it had named an owner.
    assert_failure(
        &list_associations(&gateway, "resourceOwner=NOBODY").await,
        400,
        "ValidationException",
        "GetDomainNameAccessAssociations(resourceOwner=NOBODY)",
    );
}
