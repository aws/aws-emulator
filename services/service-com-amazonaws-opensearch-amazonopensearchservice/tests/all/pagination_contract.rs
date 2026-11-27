// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's PAGINATION CONTRACT, across every listing that
//! has one: what a `NextToken` on the wire means, and how a token that names no place
//! in a set is refused.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the endpoint helpers from [`super::vpc_endpoints`]; this suite adds no second
//! copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a `NextToken` is on the wire EXACTLY when the answer left part of the set
//!   unreported: an answer that reports the whole remaining set carries no such member
//!   at all — never the empty string, which a client testing for the member's presence
//!   rather than its truthiness paginates on forever — over a table spanning six
//!   listings, sets of several sizes including the empty one, and requests that name a
//!   page size or a token;
//! * where a set IS larger than the answer, the token names where the next answer
//!   resumes, and walking it reaches an answer that carries none — over a table of page
//!   sizes;
//! * a token that names no place in the set being paged is refused with the status, the
//!   modelled error and the wording AWS uses, naming nothing of the request — over a
//!   table of tokens and of the three operations that model that error;
//! * a catalog the service PUBLISHES complete in one answer — the version catalog and the
//!   instance-type catalog — reports its whole set however the paging members are set:
//!   it hands out no token, so a `NextToken` a request carries neither resumes the set
//!   nor refuses the read, over a table of tokens and page sizes on both operations,
//!   while a page size outside the modelled range is still refused.

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX};
use super::vpc_endpoints::{create_endpoint, vpc_domain};

/// The engine version the instance-type catalog is read for: one the service publishes,
/// so the listing is a catalog rather than a refusal.
const PUBLISHED_VERSION: &str = "OpenSearch_2.11";

/// The fixed text this service refuses an unresolvable pagination token with. It carries
/// NO variable part: the token came from the caller, so quoting it back tells the caller
/// nothing it did not send.
const INVALID_TOKEN: &str = "Pagination token provided is invalid";

/// A listing to read: how to call it, and which member carries the set it reports.
struct Listing {
    /// What tells this row from the others, for the case label.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The response member the reported set lives in.
    member: &'static str,
}

/// Every listing that reports a complete set, on an account holding `endpoints`
/// endpoints and `principals` endpoint authorizations of `domain`.
fn complete_listings(domain: &str) -> Vec<Listing> {
    vec![
        Listing {
            case: "ListVersions",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/versions"),
            body: None,
            member: "Versions",
        },
        Listing {
            case: "ListVersions(maxResults=1)",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/versions?maxResults=1"),
            body: None,
            member: "Versions",
        },
        Listing {
            case: "ListInstanceTypeDetails",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/instanceTypeDetails/{PUBLISHED_VERSION}"),
            body: None,
            member: "InstanceTypeDetails",
        },
        Listing {
            case: "ListInstanceTypeDetails(maxResults=2)",
            method: "GET",
            uri: format!(
                "{PREFIX}/opensearch/instanceTypeDetails/{PUBLISHED_VERSION}?maxResults=2"
            ),
            body: None,
            member: "InstanceTypeDetails",
        },
        Listing {
            case: "ListVpcEndpoints",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/vpcEndpoints"),
            body: None,
            member: "VpcEndpointSummaryList",
        },
        Listing {
            case: "ListVpcEndpointsForDomain",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{domain}/vpcEndpoints"),
            body: None,
            member: "VpcEndpointSummaryList",
        },
        Listing {
            case: "ListVpcEndpointAccess",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{domain}/listVpcEndpointAccess"),
            body: None,
            member: "AuthorizedPrincipalList",
        },
        Listing {
            case: "DescribeInboundConnections",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/cc/inboundConnection/search"),
            body: Some(json!({})),
            member: "Connections",
        },
        Listing {
            case: "DescribeOutboundConnections",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/cc/outboundConnection/search"),
            body: Some(json!({})),
            member: "Connections",
        },
        Listing {
            case: "ListScheduledActions",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{domain}/scheduledActions"),
            body: None,
            member: "ScheduledActions",
        },
    ]
}

/// Read one listing, asserting it is served and reports its set.
async fn read(emulator: &Emulator, listing: &Listing, size: usize) -> Vec<Value> {
    let case = listing.case;
    let answer = emulator
        .call(listing.method, &listing.uri, listing.body.clone())
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}({size} held): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    // The whole remaining set is reported, so no member names a next answer: absent, and
    // not the empty string a `@required` member would have put on the wire.
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case}({size} held): the whole set is reported, so the answer carries no \
         NextToken, got {:?}",
        answer.body
    );
    answer.body[listing.member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}({size} held): expected a {} array, got {:?}",
                listing.member, answer.body
            )
        })
        .clone()
}

/// One grant of endpoint access to `domain`, so the authorization set grows by one.
async fn authorize(emulator: &Emulator, domain: &str, index: usize) {
    let account = format!("{:012}", index + 1);
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/authorizeVpcEndpointAccess"),
            Some(json!({ "Account": account })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case AuthorizeVpcEndpointAccess({account}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
}

#[tokio::test]
async fn an_answer_that_reports_the_whole_set_carries_no_next_token_member() {
    let emulator = Emulator::new();
    let domain = "pagination-contract";
    let arn = vpc_domain(&emulator, domain).await;

    // Rows differ in how large the set being reported is — the empty set, one entry, and
    // a set well past the ten entries the endpoint listings used to cap an answer at, so
    // a capped answer could not pass. Every listing is read at every size.
    for size in [0_usize, 1, 12] {
        while held(&emulator, domain).await < size {
            let index = held(&emulator, domain).await;
            let created = create_endpoint(
                &emulator,
                json!({
                    "DomainArn": arn,
                    "VpcOptions": { "SubnetIds": [format!("subnet-0aaa{index:013}")] },
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case an endpoint to report: expected 200, got {} with {:?}",
                created.status, created.body
            );
            authorize(&emulator, domain, index).await;
        }

        for listing in complete_listings(domain) {
            let reported = read(&emulator, &listing, size).await;
            // The set the listing reports is the stored one, so the endpoint and
            // authorization listings grow with the rows above while the published
            // catalogs stay as they are: either way the answer is complete and carries
            // no token.
            if listing.member == "VpcEndpointSummaryList"
                || listing.member == "AuthorizedPrincipalList"
            {
                assert_eq!(
                    reported.len(),
                    size,
                    "case {}({size} held): expected the whole stored set, got {} entries",
                    listing.case,
                    reported.len()
                );
            }
        }
    }
}

/// How many endpoints the account holds, read through the account-wide listing.
async fn held(emulator: &Emulator, domain: &str) -> usize {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/vpcEndpoints"),
            None,
        )
        .await;
    answer.body["VpcEndpointSummaryList"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| panic!("the listing reports a set, got {:?}", answer.body))
}

/// `DescribeReservedInstanceOfferings`, with `query` as its `@httpQuery` members.
async fn offerings(emulator: &Emulator, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/reservedInstanceOfferings")
    } else {
        format!("{PREFIX}/opensearch/reservedInstanceOfferings?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The offering ids one answer reports, in the order it reports them.
fn offering_ids(answer: &Answer, case: &str) -> Vec<String> {
    answer.body["ReservedInstanceOfferings"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a ReservedInstanceOfferings array, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|offering| string_member(offering, "ReservedInstanceOfferingId", case))
        .collect()
}

#[tokio::test]
async fn a_set_larger_than_the_answer_names_where_to_resume_and_the_last_answer_does_not() {
    let emulator = Emulator::new();

    // The offerings catalog is far larger than any page, so walking it crosses many
    // boundaries. The walk is at the largest page the model allows, which keeps the
    // number of requests down; the rows after it check the chaining at other sizes.
    let mut catalog: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match token.as_deref() {
            None => "maxResults=100".to_owned(),
            Some(token) => format!("maxResults=100&nextToken={token}"),
        };
        let answer = offerings(&emulator, &query).await;
        assert_eq!(
            answer.status, 200,
            "case walk at 100: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let page = offering_ids(&answer, "walk at 100");
        match answer.body["NextToken"].as_str() {
            // An answer that reports the rest of the set carries no token at all.
            None => {
                catalog.extend(page);
                break;
            }
            Some(next) => {
                // Every answer that names a next one is full: a short answer with a
                // token would mean the token skipped entries.
                assert_eq!(
                    page.len(),
                    100,
                    "case walk at 100: an answer naming a next one reports a full page, got {}",
                    page.len()
                );
                // The token names the FIRST entry of the next answer, so the walk resumes
                // exactly where this answer stopped.
                assert_eq!(
                    next.to_owned(),
                    {
                        let resumed = offerings(&emulator, &format!("nextToken={next}")).await;
                        offering_ids(&resumed, "resumed")
                            .first()
                            .cloned()
                            .unwrap_or_default()
                    },
                    "case walk at 100: the token names where the next answer starts"
                );
                catalog.extend(page);
                token = Some(next.to_owned());
            }
        }
        assert!(
            catalog.len() < 100_000,
            "case walk at 100: the walk terminates"
        );
    }
    assert!(
        catalog.len() > 100,
        "case catalog size: these rows need a catalog larger than one page, got {}",
        catalog.len()
    );

    // One row per page size the answer is bounded by: the smallest, two that do not
    // divide the catalog, and the largest the model allows. A request that names no page
    // size names no bound at all — the row below this loop.
    for (case, query, bound) in [
        ("maxResults=1", "maxResults=1".to_owned(), 1_usize),
        ("maxResults=7", "maxResults=7".to_owned(), 7),
        ("maxResults=100", "maxResults=100".to_owned(), 100),
    ] {
        let answer = offerings(&emulator, &query).await;
        let reported = offering_ids(&answer, case);
        assert_eq!(
            reported,
            catalog[..bound].to_vec(),
            "case {case}: expected the first {bound} of the catalog, got {} entries",
            reported.len()
        );
        assert_eq!(
            answer.body["NextToken"].as_str(),
            Some(catalog[bound].as_str()),
            "case {case}: entries remain, so the answer names where to resume, got {:?}",
            answer.body["NextToken"]
        );
    }

    // `MaxResults` is `@default(0)`: a request that names no page size, or names that
    // default, has named no bound of its own, so the whole set is reported in one answer
    // and there is nothing left to resume from.
    for (case, query) in [("no page size", ""), ("maxResults=0", "maxResults=0")] {
        let answer = offerings(&emulator, query).await;
        let reported = offering_ids(&answer, case);
        assert_eq!(
            reported,
            catalog.clone(),
            "case {case}: expected the whole set of {} entries, got {} entries",
            catalog.len(),
            reported.len()
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: the answer reports the whole set, so it names nothing to resume \
             from, got {:?}",
            answer.body["NextToken"]
        );
    }

    // An answer that reports the rest of the set carries no token, whatever page size
    // asked for it: resuming one entry before the end ends the walk.
    let last = catalog
        .last()
        .expect("the catalog is not empty")
        .clone();
    for query in [
        format!("nextToken={last}"),
        format!("maxResults=100&nextToken={last}"),
        format!("maxResults=1&nextToken={last}"),
    ] {
        let answer = offerings(&emulator, &query).await;
        let reported = offering_ids(&answer, &query);
        assert_eq!(
            reported,
            vec![last.clone()],
            "case {query}: expected the catalog's last entry, got {reported:?}"
        );
        assert_eq!(
            answer.body.get("NextToken"),
            None,
            "case {query}: nothing remains, so the answer carries no NextToken, got {:?}",
            answer.body
        );
    }
}

/// A token drawn from a reported catalog entry: the entry itself where the catalog is a
/// list of strings, and the entry's `InstanceType` where each entry is a structure.
fn entry_token(entry: &Value, case: &str) -> String {
    entry
        .as_str()
        .or_else(|| entry["InstanceType"].as_str())
        .unwrap_or_else(|| panic!("case {case}: expected a catalog entry, got {entry:?}"))
        .to_owned()
}

/// One published catalog read with `query` as its `@httpQuery` members: the set it
/// reports, asserted served and complete.
async fn published_catalog(
    emulator: &Emulator,
    path: &str,
    query: &str,
    member: &str,
    case: &str,
) -> Vec<Value> {
    let uri = if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    };
    let answer = emulator.call("GET", &uri, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}({query:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case}({query:?}): a catalog published complete carries no NextToken, got {:?}",
        answer.body
    );
    answer.body[member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}({query:?}): expected a {member} array, got {:?}",
                answer.body
            )
        })
        .clone()
}

#[tokio::test]
async fn a_catalog_published_complete_answers_the_whole_set_whatever_the_paging_members_ask() {
    let emulator = Emulator::new();

    // The reads of a catalog the SERVICE publishes rather than a caller creates, each
    // with the member carrying the set it reports (source:
    // `smithy_query(query="operation",operation="ListVersions,ListInstanceTypeDetails")`).
    // Such a read reports its set complete in one answer and hands out no token, so
    // neither paging member can bound it, resume it or refuse it — the rows span both
    // operations and, for the instance-type catalog, releases of both engine lines, whose
    // catalogs are of different lengths.
    for (case, path, member) in [
        (
            "ListVersions",
            format!("{PREFIX}/opensearch/versions"),
            "Versions",
        ),
        (
            "ListInstanceTypeDetails(OpenSearch_2.11)",
            format!("{PREFIX}/opensearch/instanceTypeDetails/{PUBLISHED_VERSION}"),
            "InstanceTypeDetails",
        ),
        (
            "ListInstanceTypeDetails(Elasticsearch_7.10)",
            format!("{PREFIX}/opensearch/instanceTypeDetails/Elasticsearch_7.10"),
            "InstanceTypeDetails",
        ),
        (
            "ListInstanceTypeDetails(Elasticsearch_1.5)",
            format!("{PREFIX}/opensearch/instanceTypeDetails/Elasticsearch_1.5"),
            "InstanceTypeDetails",
        ),
    ] {
        let whole = published_catalog(&emulator, &path, "", member, case).await;
        assert!(
            whole.len() > 2,
            "case {case}: expected a catalog larger than the page sizes under test, got {} \
             entries",
            whole.len()
        );

        // Tokens: entries of this very catalog (its second, its middle and its last), a
        // value of the member's shape this service never mints, an identifier-shaped one,
        // and one at the far end of what the member admits.
        let mut tokens = vec![
            entry_token(&whole[1], case),
            entry_token(&whole[whole.len() / 2], case),
            entry_token(&whole[whole.len() - 1], case),
            "not-a-token".to_owned(),
            "00000000-0000-0000-0000-000000000000".to_owned(),
            "t".repeat(128),
        ];
        tokens.dedup();

        let mut queries: Vec<String> = vec![
            "maxResults=1".to_owned(),
            "maxResults=2".to_owned(),
            "maxResults=100".to_owned(),
            "maxResults=0".to_owned(),
        ];
        for token in &tokens {
            queries.push(format!("nextToken={token}"));
            queries.push(format!("maxResults=1&nextToken={token}"));
            queries.push(format!("maxResults=100&nextToken={token}"));
        }

        for query in queries {
            let reported = published_catalog(&emulator, &path, &query, member, case).await;
            assert_eq!(
                reported, whole,
                "case {case}({query:?}): expected the whole catalog from its head, got {} \
                 entries headed by {:?}",
                reported.len(),
                reported.first()
            );
        }

        // A page size outside the modelled range is still refused at either end: the
        // paging members are inert, not unchecked.
        for out_of_range in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
            let answer = emulator
                .call("GET", &format!("{path}?{out_of_range}"), None)
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case}({out_of_range}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}({out_of_range}): expected the modelled validation refusal, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_token_naming_no_place_in_the_set_is_refused_in_the_words_aws_uses() {
    let emulator = Emulator::new();
    let domain = "pagination-refusals";
    let created = emulator.create(json!({ "DomainName": domain })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({domain}): expected 200, got {} with {:?}",
        created.status, created.body
    );

    // The three operations whose model declares `InvalidPaginationTokenException` — the
    // error AWS refuses an unresolvable token with — each read over a table of tokens:
    // one shaped like nothing this service mints, one shaped like a connection id, one
    // at the far end of what the member admits, and the empty-looking whitespace token.
    for (case, method, uri, body) in [
        (
            "DescribeInboundConnections",
            "POST",
            format!("{PREFIX}/opensearch/cc/inboundConnection/search"),
            true,
        ),
        (
            "DescribeOutboundConnections",
            "POST",
            format!("{PREFIX}/opensearch/cc/outboundConnection/search"),
            true,
        ),
        (
            "ListScheduledActions",
            "GET",
            format!("{PREFIX}/opensearch/domain/{domain}/scheduledActions"),
            false,
        ),
    ] {
        for token in [
            "not-a-token",
            "123456789012/alias-does-not-exist",
            "00000000-0000-0000-0000-000000000000",
            &"t".repeat(128),
        ] {
            let answer = if body {
                emulator
                    .call(method, &uri, Some(json!({ "NextToken": token })))
                    .await
            } else {
                emulator
                    .call(method, &format!("{uri}?nextToken={token}"), None)
                    .await
            };
            assert_eq!(
                answer.status, 400,
                "case {case}({token}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "InvalidPaginationTokenException",
                "case {case}({token}): expected the modelled refusal, got {:?}",
                answer.body
            );
            // The wording is fixed: it states the condition and quotes nothing of the
            // request back.
            assert_eq!(
                answer.body["message"].as_str(),
                Some(INVALID_TOKEN),
                "case {case}({token}): expected the refusal AWS words, got {:?}",
                answer.body
            );
            assert!(
                !answer.body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(token),
                "case {case}({token}): the refusal carries no part of the request, got {:?}",
                answer.body
            );
        }
    }
}
