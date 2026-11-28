// SPDX-License-Identifier: CC0-1.0

//! The domain MEMBERSHIP of a DNS Firewall domain list, and the delete of the
//! list itself, driven end-to-end over this service's own `restJson1` wire
//! through the parent suite's emulator.
//!
//! Every test states a RULE over a set of inputs rather than one recorded shape:
//!
//! 1. every route that resolves a `firewallDomainListId` the store does not hold
//!    answers the same refusal — 404 `ResourceNotFoundException` whose body is
//!    `message` ALONE, worded with this service's own error code, the API's
//!    spelling of the resource and the id the REQUEST named;
//! 2. an update of the membership is observable to the next read, per operation
//!    the member documents (ADD, REMOVE, REPLACE), and an update this service
//!    refuses changes nothing;
//! 3. a page of domains is ordered, bounded and resumable;
//! 4. a deleted list is gone from every read, and a second delete of it is the
//!    refusal of rule 1 rather than a silent success.

use std::time::Instant;

use serde_json::{json, Value};

use super::{not_found_by_id, Answer, Emulator, POLL_INTERVAL, SETTLE_DEADLINE};

/// `UpdateFirewallDomains` for one list: the operation and the domains it carries.
async fn update(emulator: &Emulator, list_id: &str, operation: &str, domains: &[&str]) -> Answer {
    emulator
        .call(
            "PATCH",
            &format!("/firewall-domain-lists/{list_id}/domains"),
            Some(json!({ "operation": operation, "domains": domains })),
        )
        .await
}

/// `ListFirewallDomains` for one list, with whatever query string the case needs.
async fn list_domains(emulator: &Emulator, list_id: &str, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("/firewall-domain-lists/{list_id}/domains{query}"),
            None,
        )
        .await
}

/// `DeleteFirewallDomainList` for one list.
async fn delete_list(emulator: &Emulator, list_id: &str) -> Answer {
    emulator
        .call("DELETE", &format!("/firewall-domain-lists/{list_id}"), None)
        .await
}

/// The `domains` member of one page, in the order it came.
fn page_domains(answer: &Answer) -> Vec<String> {
    answer
        .member("domains")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every domain the store holds for one list, walked across as many pages as it
/// takes, so an assertion about the whole membership is never a one-page one.
async fn walk_domains(emulator: &Emulator, list_id: &str, query: &str) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let separator = if query.is_empty() { '?' } else { '&' };
        let page = match token.as_deref() {
            Some(token) => list_domains(emulator, list_id, &format!("{query}{separator}next_token={token}")).await,
            None => list_domains(emulator, list_id, query).await,
        };
        assert_eq!(
            page.status, 200,
            "walking the domains of {list_id:?}: expected 200, got {} with {}",
            page.status, page.body
        );
        domains.extend(page_domains(&page));
        token = page.member("nextToken").as_str().map(str::to_string);
        if token.is_none() {
            return domains;
        }
    }
}

/// A resolver and one domain list under it, for a test that needs somewhere to
/// put domains.
async fn resolver_and_list(emulator: &Emulator, name: &str) -> (String, String) {
    let resolver_id = emulator.create_resolver(name).await;
    let created = emulator
        .create_list(&resolver_id, json!({ "name": name }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: setup must mint a domain list, got {} with {}",
        created.status, created.body
    );
    (resolver_id, created.text("id"))
}

/// One row of the unknown-id table: an operation, its method, and the request it
/// makes against a `firewallDomainListId`.
struct ListRoute {
    operation: &'static str,
    method: &'static str,
    request: fn(&str) -> (String, Option<Value>),
}

/// Every route of this service that resolves a caller-supplied domain list id.
fn list_routes() -> Vec<ListRoute> {
    vec![
        ListRoute {
            operation: "GetFirewallDomainList",
            method: "GET",
            request: |id| (format!("/firewall-domain-lists/{id}"), None),
        },
        ListRoute {
            operation: "ListFirewallDomains",
            method: "GET",
            request: |id| (format!("/firewall-domain-lists/{id}/domains"), None),
        },
        ListRoute {
            operation: "UpdateFirewallDomains",
            method: "PATCH",
            request: |id| {
                (
                    format!("/firewall-domain-lists/{id}/domains"),
                    Some(json!({ "operation": "ADD", "domains": ["example.com"] })),
                )
            },
        },
        ListRoute {
            operation: "DeleteFirewallDomainList",
            method: "DELETE",
            request: |id| (format!("/firewall-domain-lists/{id}"), None),
        },
    ]
}

#[tokio::test]
async fn every_route_that_resolves_a_domain_list_id_refuses_an_unknown_one_the_same_way() {
    let emulator = Emulator::new();

    // Rows: ids of several forms, none of them in the store — one shaped like an
    // id this service mints, one shaped like another kind's, one of no kind at
    // all, and the shortest and longest `ResourceId` admits (smithy_query shape
    // ResourceId: length 1..64, pattern `^[-.a-zA-Z0-9]+$`), so each request
    // satisfies the member's own constraints and reaches a handler.
    let longest = "a".repeat(64);
    let missing = [
        "fdl-ffffffffffff",
        "gr-000000000001",
        "No.Such-List9",
        "x",
        longest.as_str(),
    ];

    for id in missing {
        for route in list_routes() {
            let case = format!("{} against {id:?}", route.operation);
            let (path, body) = (route.request)(id);
            let answer = emulator.call(route.method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_type.contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {:?} with {}",
                answer.error_type,
                answer.body
            );
            assert_eq!(
                answer.text("message"),
                not_found_by_id("FirewallDomainList", id),
                "case {case:?}: expected this service's own not-found wording naming the id the \
                 request carried, got {:?}",
                answer.text("message")
            );
            assert_eq!(
                answer.modelled_members(),
                vec![
                    "message".to_string(),
                    "resourceId".to_string(),
                    "resourceType".to_string()
                ],
                "case {case:?}: the refusal body states the kind and the id beside the \
                 sentence, got {}",
                answer.body
            );
            assert_eq!(
                answer.text("resourceId"),
                id,
                "case {case:?}: expected the id the request named, got {:?}",
                answer.text("resourceId")
            );
        }
    }

    // The same four routes SERVE the list the store does hold, so the rule above
    // is about an id that names nothing and not about refusing everything. The
    // delete comes last, because it is what makes the id stop naming a list.
    let (_, list_id) = resolver_and_list(&emulator, "served").await;
    for route in list_routes() {
        let case = format!("{} against the stored id", route.operation);
        let (path, body) = (route.request)(&list_id);
        let answer = emulator.call(route.method, &path, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an id the store DOES hold is served, got {} with {}",
            answer.status, answer.body
        );
    }
}

/// One row of the update table: what the request asks for, and the membership the
/// list holds afterwards.
struct UpdateRow {
    case: &'static str,
    operation: &'static str,
    domains: Vec<&'static str>,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn an_update_is_observable_to_the_next_read_for_every_documented_operation() {
    let emulator = Emulator::new();
    let (_, list_id) = resolver_and_list(&emulator, "membership").await;

    // Rows applied in order to one list, differing along the operation the member
    // documents (`UpdateFirewallDomainsInput$operation`: "The allowed values are
    // ADD, REMOVE, and REPLACE"), the count of domains carried, and the forms the
    // `Domains` member admits (smithy_query shape Domains: member length 1..256,
    // pattern `^\*?[a-zA-Z0-9!"#$%&'()*+,./:;<=>?@\[\\\]^_`{|}~-]+$`) — a
    // wildcard, a trailing dot, a punycode label, a long label.
    let long_label = "a".repeat(63);
    let long_domain = format!("{long_label}.example.com");
    let rows = vec![
        UpdateRow {
            case: "ADD to an empty list",
            operation: "ADD",
            domains: vec!["b.example.com", "a.example.com"],
            expected: vec!["a.example.com", "b.example.com"],
        },
        UpdateRow {
            case: "ADD one already held and one new",
            operation: "ADD",
            domains: vec!["a.example.com", "*.example.net"],
            expected: vec!["*.example.net", "a.example.com", "b.example.com"],
        },
        UpdateRow {
            case: "ADD the forms the member's pattern admits",
            operation: "ADD",
            domains: vec!["example.com.", "xn--80ak6aa92e.com"],
            expected: vec![
                "*.example.net",
                "a.example.com",
                "b.example.com",
                "example.com.",
                "xn--80ak6aa92e.com",
            ],
        },
        UpdateRow {
            case: "REMOVE one held and one the list never held",
            operation: "REMOVE",
            domains: vec!["a.example.com", "never-held.example.org"],
            expected: vec![
                "*.example.net",
                "b.example.com",
                "example.com.",
                "xn--80ak6aa92e.com",
            ],
        },
        UpdateRow {
            case: "REPLACE with a different set",
            operation: "REPLACE",
            domains: vec!["only.example.com"],
            expected: vec!["only.example.com"],
        },
        UpdateRow {
            case: "REPLACE with the empty set",
            operation: "REPLACE",
            domains: vec![],
            expected: vec![],
        },
    ];

    for row in rows {
        let answer = update(&emulator, &list_id, row.operation, &row.domains).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected 200, got {} with {}",
            row.case, answer.status, answer.body
        );
        assert_eq!(
            answer.text("id"),
            list_id,
            "case {:?}: the answer names the list the request addressed, got {:?}",
            row.case,
            answer.text("id")
        );
        assert_eq!(
            answer.text("name"),
            "membership",
            "case {:?}: the answer names the list as the store holds it, got {:?}",
            row.case,
            answer.text("name")
        );
        assert_eq!(
            answer.text("status"),
            "UPDATING",
            "case {:?}: an update answers the non-terminal status of the resource it changed, \
             got {:?}",
            row.case,
            answer.text("status")
        );

        // The membership is what the next reader sees — deduplicated and ordered,
        // whatever order the request listed it in.
        let held = walk_domains(&emulator, &list_id, "").await;
        let mut expected: Vec<String> = row.expected.iter().map(|d| (*d).to_string()).collect();
        expected.sort();
        assert_eq!(
            held, expected,
            "case {:?}: expected the store to hold {expected:?}, got {held:?}",
            row.case
        );

        // And `domainCount` on the list itself follows from that set rather than
        // from what any one request carried.
        let read = emulator.get_list(&list_id).await;
        assert_eq!(
            read.member("domainCount").as_i64(),
            Some(expected.len() as i64),
            "case {:?}: expected domainCount {}, got {}",
            row.case,
            expected.len(),
            read.member("domainCount")
        );
    }

    // A domain of the longest form the member admits is held like any other, so
    // the rule is about the set and not about the length of what goes in it.
    let answer = update(&emulator, &list_id, "ADD", &[long_domain.as_str()]).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "a long domain", answer.status, answer.body
    );
    assert_eq!(
        walk_domains(&emulator, &list_id, "").await,
        vec![long_domain.clone()],
        "case {:?}: expected the long domain to be held",
        "a long domain"
    );
}

#[tokio::test]
async fn a_page_of_domains_is_ordered_bounded_and_resumable() {
    let emulator = Emulator::new();
    let (_, list_id) = resolver_and_list(&emulator, "paging").await;
    let seeded = [
        "a.example.com",
        "b.example.com",
        "c.example.com",
        "d.example.com",
        "e.example.com",
        "f.example.com",
        "g.example.com",
    ];
    let added = update(&emulator, &list_id, "ADD", &seeded).await;
    assert_eq!(
        added.status, 200,
        "case {:?}: setup must add the domains, got {} with {}",
        "paging setup", added.status, added.body
    );
    let mut all: Vec<String> = seeded.iter().map(|d| (*d).to_string()).collect();
    all.sort();

    // Rows: page sizes that divide the membership, that do not, that ask for one
    // item, and that ask for more than the store holds.
    for page_size in [1_usize, 2, 3, 7, 50] {
        let walked = walk_domains(&emulator, &list_id, &format!("?max_results={page_size}")).await;
        assert_eq!(
            walked, all,
            "case {page_size:?}: walking at this page size reports the whole membership in \
             order, got {walked:?}"
        );
        let first = list_domains(&emulator, &list_id, &format!("?max_results={page_size}")).await;
        let page = page_domains(&first);
        assert!(
            page.len() <= page_size,
            "case {page_size:?}: a page holds at most what the caller asked for, got {page:?}"
        );
        let expects_more = all.len() > page.len();
        assert_eq!(
            first.member("nextToken").as_str().is_some(),
            expects_more,
            "case {page_size:?}: a token is present exactly when domains remain, got {:?}",
            first.member("nextToken")
        );
    }

    // A page size below one asks for nothing, which this service refuses rather
    // than serving a page of its own choosing.
    for refused in [0, -1] {
        let answer = list_domains(&emulator, &list_id, &format!("?max_results={refused}")).await;
        assert_eq!(
            answer.status, 400,
            "case {refused:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {refused:?}: expected a ValidationException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.text("message"),
            format!("maxResults: must be at least 1, got {refused}"),
            "case {refused:?}: expected the service's own wording naming the member, got {:?}",
            answer.text("message")
        );
    }

    // A cursor this service could not have issued is refused too: the token it
    // issues is one of the domain specifications it holds.
    for refused in ["not a token", "", "\u{1f600}.example.com"] {
        let answer = list_domains(
            &emulator,
            &list_id,
            &format!("?next_token={}", refused.replace(' ', "%20")),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {refused:?}: expected 400 for a token this service never issued, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.text("message"),
            "nextToken: is not a token this service issued",
            "case {refused:?}: expected the service's own wording naming the member, got {:?}",
            answer.text("message")
        );
    }

    // And a token the service DID issue resumes at the domain it names.
    let first = list_domains(&emulator, &list_id, "?max_results=2").await;
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let resumed = list_domains(&emulator, &list_id, &format!("?next_token={token}")).await;
    assert_eq!(
        page_domains(&resumed),
        all[2..],
        "case {:?}: a page resumed at its token holds the rest of the membership",
        "resuming"
    );
}

/// One row of the refused-update table: the request, and the refusal this service
/// answers it with.
struct RefusedUpdate {
    case: &'static str,
    operation: &'static str,
    domains: Vec<String>,
    status: u16,
    error_type: &'static str,
    message: String,
}

#[tokio::test]
async fn an_update_this_service_refuses_leaves_the_membership_as_it_was() {
    let emulator = Emulator::new();
    let (_, list_id) = resolver_and_list(&emulator, "refusals").await;
    let held = ["kept.example.com"];
    let seeded = update(&emulator, &list_id, "ADD", &held).await;
    assert_eq!(
        seeded.status, 200,
        "case {:?}: setup must add the kept domain, got {} with {}",
        "refusal setup", seeded.status, seeded.body
    );

    // Rows: an operation of the wrong case, one the member does not name, one
    // that looks like a neighbouring API's, and a request past the per-request
    // maximum the `domains` member documents ("You can add up to 1000 domains per
    // request") — both one at the maximum, which is served, and one past it.
    let allowed = "must be one of ADD, REMOVE, REPLACE";
    let over_limit: Vec<String> = (0..1001).map(|n| format!("d{n}.example.com")).collect();
    let rows = vec![
        RefusedUpdate {
            case: "an operation in the wrong case",
            operation: "add",
            domains: vec!["new.example.com".to_string()],
            status: 400,
            error_type: "ValidationException",
            message: format!("operation: {allowed}"),
        },
        RefusedUpdate {
            case: "an operation the member does not name",
            operation: "DELETE",
            domains: vec!["new.example.com".to_string()],
            status: 400,
            error_type: "ValidationException",
            message: format!("operation: {allowed}"),
        },
        RefusedUpdate {
            case: "an operation of a neighbouring spelling",
            operation: "REPLACE_ALL",
            domains: vec!["new.example.com".to_string()],
            status: 400,
            error_type: "ValidationException",
            message: format!("operation: {allowed}"),
        },
        RefusedUpdate {
            case: "one domain past the documented per-request maximum",
            operation: "ADD",
            domains: over_limit,
            status: 400,
            error_type: "ValidationException",
            message: "domains: must carry at most 1000 domains per request, got 1001".to_string(),
        },
    ];

    for row in rows {
        let carried: Vec<&str> = row.domains.iter().map(String::as_str).collect();
        let answer = update(&emulator, &list_id, row.operation, &carried).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected {}, got {} with {}",
            row.case, row.status, answer.status, answer.body
        );
        assert!(
            answer.error_type.contains(row.error_type),
            "case {:?}: expected a {}, got {:?}",
            row.case,
            row.error_type,
            answer.error_type
        );
        assert_eq!(
            answer.text("message"),
            row.message,
            "case {:?}: expected the service's own wording, got {:?}",
            row.case,
            answer.text("message")
        );
        let after = walk_domains(&emulator, &list_id, "").await;
        assert_eq!(
            after,
            vec!["kept.example.com".to_string()],
            "case {:?}: a refused update changes nothing, got {after:?}",
            row.case
        );
    }

    // A request AT the documented maximum is served, so the bound is the limit
    // itself and not the shape of the request.
    let at_limit: Vec<String> = (0..1000).map(|n| format!("d{n:04}.example.com")).collect();
    let carried: Vec<&str> = at_limit.iter().map(String::as_str).collect();
    let answer = update(&emulator, &list_id, "REPLACE", &carried).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "the documented maximum", answer.status, answer.body
    );
    let read = emulator.get_list(&list_id).await;
    assert_eq!(
        read.member("domainCount").as_i64(),
        Some(1000),
        "case {:?}: expected the whole request to be held, got {}",
        "the documented maximum",
        read.member("domainCount")
    );

    // One domain past what this store holds for a list is the modelled quota
    // refusal, and it leaves the membership where it was.
    let answer = update(&emulator, &list_id, "ADD", &["one.too.many.example.com"]).await;
    assert_eq!(
        answer.status, 402,
        "case {:?}: expected 402, got {} with {}",
        "past the store's ceiling", answer.status, answer.body
    );
    assert!(
        answer.error_type.contains("ServiceQuotaExceededException"),
        "case {:?}: expected a ServiceQuotaExceededException, got {:?}",
        "past the store's ceiling",
        answer.error_type
    );
    let read = emulator.get_list(&list_id).await;
    assert_eq!(
        read.member("domainCount").as_i64(),
        Some(1000),
        "case {:?}: a refused update changes nothing, got {}",
        "past the store's ceiling",
        read.member("domainCount")
    );
}

#[tokio::test]
async fn a_deleted_domain_list_is_gone_from_every_read() {
    let emulator = Emulator::new();
    let resolver_id = emulator.create_resolver("delete-parent").await;

    // Rows: a list carrying domains and an empty one, so the delete is not a
    // single-shape special case, plus a sibling that must survive both.
    let sibling = emulator
        .create_list(&resolver_id, json!({ "name": "the-survivor" }))
        .await;
    let sibling_id = sibling.text("id");
    for (case, name, domains) in [
        ("a list holding domains", "with-domains", vec!["a.example.com", "b.example.com"]),
        ("an empty list", "without-domains", vec![]),
    ] {
        let created = emulator
            .create_list(&resolver_id, json!({ "name": name }))
            .await;
        let list_id = created.text("id");
        if !domains.is_empty() {
            let added = update(&emulator, &list_id, "ADD", &domains).await;
            assert_eq!(
                added.status, 200,
                "case {case:?}: setup must add the domains, got {} with {}",
                added.status, added.body
            );
        }

        let deleted = delete_list(&emulator, &list_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("id"),
            list_id,
            "case {case:?}: the delete names the list it removed, got {:?}",
            deleted.text("id")
        );
        assert_eq!(
            deleted.text("arn"),
            created.text("arn"),
            "case {case:?}: the delete answers the ARN the store held, got {:?}",
            deleted.text("arn")
        );
        assert_eq!(
            deleted.text("name"),
            name,
            "case {case:?}: the delete answers the name the store held, got {:?}",
            deleted.text("name")
        );
        assert_eq!(
            deleted.text("status"),
            "DELETING",
            "case {case:?}: the delete answers the status it left the list in, got {:?}",
            deleted.text("status")
        );

        // Gone from the read of the list, from the page of lists, and from its own
        // domains — and a second delete is the refusal, not a silent success.
        let read = emulator.get_list(&list_id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a deleted list reads back 404, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.text("message"),
            not_found_by_id("FirewallDomainList", &list_id),
            "case {case:?}: expected this service's own not-found wording, got {:?}",
            read.text("message")
        );
        let domains_read = list_domains(&emulator, &list_id, "").await;
        assert_eq!(
            domains_read.status, 404,
            "case {case:?}: the domains of a deleted list are 404, got {} with {}",
            domains_read.status, domains_read.body
        );
        let again = delete_list(&emulator, &list_id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete is refused, got {} with {}",
            again.status, again.body
        );
        let walked = emulator
            .walk(&format!("?global_resolver_id={resolver_id}"))
            .await;
        assert!(
            !walked.contains(&list_id),
            "case {case:?}: the page of lists no longer reports {list_id:?}, got {walked:?}"
        );
        assert!(
            walked.contains(&sibling_id),
            "case {case:?}: the sibling list survives the delete, got {walked:?}"
        );
    }

    // The idempotency record of a deleted list goes with it, so a create replayed
    // with that `clientToken` mints a NEW list rather than naming the dead one.
    let token = "fdl-delete-token";
    let first = emulator
        .create_list(
            &resolver_id,
            json!({ "name": "tokened", "clientToken": token }),
        )
        .await;
    let first_id = first.text("id");
    assert_eq!(
        delete_list(&emulator, &first_id).await.status,
        200,
        "case {:?}: setup must delete the tokened list",
        "a replayed token"
    );
    let replayed = emulator
        .create_list(
            &resolver_id,
            json!({ "name": "tokened", "clientToken": token }),
        )
        .await;
    assert_eq!(
        replayed.status, 200,
        "case {:?}: expected 200, got {} with {}",
        "a replayed token", replayed.status, replayed.body
    );
    assert_ne!(
        replayed.text("id"),
        first_id,
        "case {:?}: the replay mints a new list rather than naming the deleted one",
        "a replayed token"
    );
}

#[tokio::test]
async fn the_clock_alone_turns_an_updated_domain_list_operational_again() {
    let emulator = Emulator::new();
    let (_, list_id) = resolver_and_list(&emulator, "settling-update").await;
    let updated = update(&emulator, &list_id, "ADD", &["a.example.com"]).await;
    assert_eq!(
        updated.text("status"),
        "UPDATING",
        "case {:?}: the update answers the non-terminal status, got {:?}",
        "the update",
        updated.text("status")
    );

    // The membership itself is visible straight away: it is the STATUS that the
    // clock owes, not the write.
    assert_eq!(
        walk_domains(&emulator, &list_id, "").await,
        vec!["a.example.com".to_string()],
        "case {:?}: the domains are readable before the status settles",
        "the update"
    );

    // The transition is owed by the clock, not by the read: this looks again until
    // the condition holds, and nothing here asserts on how long it took.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = emulator.get_list(&list_id).await;
        let status = read.text("status");
        if status == "OPERATIONAL" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected OPERATIONAL once the span passed, got {status:?}",
            "the settled read"
        );
        assert_eq!(
            status, "UPDATING",
            "case {:?}: an updating list reports UPDATING until it settles, got {status:?}",
            "the settled read"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}
