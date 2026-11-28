// SPDX-License-Identifier: CC0-1.0

//! The BODY of every refusal this service serves, driven end-to-end over its own
//! `restJson1` wire through the parent suite's emulator.
//!
//! Two rules, each asserted over a table that spans the class rather than one
//! recorded shape:
//!
//! 1. a not-found refusal states this service's own bracketed error code followed by the
//!    kind the id was looked up as and the id the request named, and carries the two
//!    members that state those same two values on their own — `resourceType`, which the
//!    shape makes `@required`, and `resourceId`
//!    (`smithy_query(query="shape",shape="ResourceNotFoundException")`) — and nothing else;
//! 2. a `@pattern` requirement in a validation message quotes the expression
//!    WITHOUT the whole-value anchors the model writes around it, while the check
//!    itself still runs the model's anchored pattern.

use serde_json::{json, Value};

use super::{Answer, Emulator};

/// The MODELLED members a refusal body carries, sorted. `__type` is the
/// protocol's own error discriminator rather than a member of the shape, so it is
/// left out: what is left is exactly what the service put on the wire.
fn modelled_members(answer: &Answer) -> Vec<String> {
    let mut names: Vec<String> = answer
        .body
        .as_object()
        .map(|members| {
            members
                .keys()
                .filter(|name| name.as_str() != "__type")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The message a not-found refusal of a resource addressed by its id carries:
/// this service's code for that class, the kind the id was looked up as, and the
/// id the REQUEST named.
fn not_found_by_id(kind: &str, id: &str) -> String {
    format!("[GR-ERR00309] No {kind} found with the ID: {id}")
}

/// The members a not-found body carries: the sentence, and the kind and the id it states.
fn not_found_members() -> Vec<String> {
    vec![
        "message".to_string(),
        "resourceId".to_string(),
        "resourceType".to_string(),
    ]
}

/// One row of the not-found table: an operation, the request it makes against an
/// id, and the message this service words that refusal with.
struct NotFoundRoute {
    operation: &'static str,
    method: &'static str,
    request: fn(&str) -> (String, Option<Value>),
    message: fn(&str) -> String,
}

/// Every route of this service that resolves a caller-supplied id to a resource.
fn not_found_routes() -> Vec<NotFoundRoute> {
    vec![
        NotFoundRoute {
            operation: "GetGlobalResolver",
            method: "GET",
            request: |id| (format!("/global-resolver/{id}"), None),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        NotFoundRoute {
            operation: "DeleteGlobalResolver",
            method: "DELETE",
            request: |id| (format!("/global-resolver/{id}"), None),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        NotFoundRoute {
            operation: "GetDNSView",
            method: "GET",
            request: |id| (format!("/dns-views/{id}"), None),
            // The kind is the API's spelling of the resource, not the token this
            // emulator keys its own store or its ARNs by.
            message: |id| not_found_by_id("DnsView", id),
        },
        NotFoundRoute {
            operation: "CreateDNSView",
            method: "POST",
            request: |id| (format!("/dns-views/{id}"), Some(json!({ "name": "a-view" }))),
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        NotFoundRoute {
            operation: "ListDNSViews",
            method: "GET",
            request: |id| (format!("/dns-views/resolver/{id}"), None),
            // The LIST path words a missing parent with its OWN code, and states
            // the id with no colon before it: a client branching on the code can
            // tell this refusal from a read of a resolver that does not exist.
            message: |id| format!("[GR-ERR02300] No GlobalResolver found with ID {id}"),
        },
        NotFoundRoute {
            operation: "GetFirewallDomainList",
            method: "GET",
            request: |id| (format!("/firewall-domain-lists/{id}"), None),
            message: |id| not_found_by_id("FirewallDomainList", id),
        },
        NotFoundRoute {
            operation: "CreateFirewallDomainList",
            method: "POST",
            request: |id| {
                (
                    format!("/firewall-domain-lists/{id}"),
                    Some(json!({ "name": "a-list" })),
                )
            },
            message: |id| not_found_by_id("GlobalResolver", id),
        },
        NotFoundRoute {
            operation: "ListFirewallDomainLists",
            method: "GET",
            request: |id| {
                (
                    format!("/firewall-domain-lists?global_resolver_id={id}"),
                    None,
                )
            },
            message: |id| not_found_by_id("GlobalResolver", id),
        },
    ]
}

#[tokio::test]
async fn a_not_found_body_is_this_services_own_wording_and_states_the_kind_and_the_id() {
    let emulator = Emulator::new();

    // Rows: ids of several forms — one shaped like an id this service mints, one
    // shaped like another kind's, one of no kind at all, and a single character —
    // none of them in the store. Every one satisfies `ResourceId`'s own
    // constraints (`smithy_query` shape ResourceId: length 1..64, pattern
    // `^[-.a-zA-Z0-9]+$`), so each request reaches a handler.
    let missing = [
        "gr-ffffffffffff",
        "dnsv-0000000000ff",
        "Does.Not-Exist9",
        "x",
    ];

    for id in missing {
        for route in not_found_routes() {
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
                (route.message)(id),
                "case {case:?}: expected this service's own not-found wording, got {:?}",
                answer.text("message")
            );
            assert_eq!(
                modelled_members(&answer),
                not_found_members(),
                "case {case:?}: the body carries the sentence and the two members that \
                 state the kind and the id, got {}",
                answer.body
            );
            assert_eq!(
                answer.text("resourceId"),
                id,
                "case {case:?}: expected the id the request named, got {:?}",
                answer.text("resourceId")
            );
            assert!(
                (route.message)(id).contains(&format!("No {} found", answer.text("resourceType"))),
                "case {case:?}: expected the kind the message states, got {:?}",
                answer.text("resourceType")
            );
        }
    }

    // The same routes still SERVE the resources the store does hold, so the rule
    // above is about the refusal body and not about refusing everything.
    let resolver = emulator
        .create_resolver(json!({ "name": "served", "regions": [super::REGION] }))
        .await;
    assert_eq!(
        resolver.status, 200,
        "case {:?}: setup must mint a resolver, got {} with {}",
        "a stored resolver", resolver.status, resolver.body
    );
    let resolver_id = resolver.text("id");
    let view = emulator
        .create_view(&resolver_id, json!({ "name": "served-view" }))
        .await;
    assert_eq!(
        view.status, 200,
        "case {:?}: setup must mint a DNS view, got {} with {}",
        "a stored view", view.status, view.body
    );
    for (case, path) in [
        ("GetGlobalResolver", format!("/global-resolver/{resolver_id}")),
        ("GetDNSView", format!("/dns-views/{}", view.text("id"))),
        (
            "ListDNSViews",
            format!("/dns-views/resolver/{resolver_id}"),
        ),
    ] {
        let answer = emulator.call("GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an id the store DOES hold reads back, got {} with {}",
            answer.status, answer.body
        );
    }
}

/// `ResourceId`'s own `@pattern`, as the model writes it and as a refusal quotes
/// it (`smithy_query` shape ResourceId: pattern `^[-.a-zA-Z0-9]+$`).
const RESOURCE_ID_PATTERN_AS_MODELLED: &str = "^[-.a-zA-Z0-9]+$";
const RESOURCE_ID_PATTERN_AS_QUOTED: &str = "[-.a-zA-Z0-9]+";

/// One row of the constraint table: an operation, the request carrying the
/// refused value, the member the clause names and the value it echoes.
struct ConstraintCase {
    operation: &'static str,
    method: &'static str,
    path: String,
    /// The request body, for an operation that carries the refused value in one.
    body: Option<Value>,
    member: &'static str,
    value: String,
    requirement: String,
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_whole_value_anchors() {
    let emulator = Emulator::new();
    let pattern_requirement =
        format!("Member must satisfy regular expression pattern: {RESOURCE_ID_PATTERN_AS_QUOTED}");

    // Rows: every operation of this slice whose `@httpLabel` targets `ResourceId`,
    // plus the two creates that carry such a member in the URI and in the BODY, each
    // with a value of a different form that the model's pattern rejects — a space, an
    // underscore, punctuation, a comma. Each value CONTAINS runs the pattern matches,
    // so a refusal proves the CHECK is still the model's anchored one even though the
    // MESSAGE quotes the expression unanchored. The last row is a different constraint
    // on the same member, which the quoting leaves alone.
    let cases = vec![
        ConstraintCase {
            operation: "GetGlobalResolver",
            method: "GET",
            path: "/global-resolver/bad%20id!".to_string(),
            body: None,
            member: "globalResolverId",
            value: "bad id!".to_string(),
            requirement: pattern_requirement.clone(),
        },
        ConstraintCase {
            operation: "DeleteGlobalResolver",
            method: "DELETE",
            path: "/global-resolver/gr_underscore".to_string(),
            body: None,
            member: "globalResolverId",
            value: "gr_underscore".to_string(),
            requirement: pattern_requirement.clone(),
        },
        ConstraintCase {
            operation: "GetDNSView",
            method: "GET",
            path: "/dns-views/view(1)".to_string(),
            body: None,
            member: "dnsViewId",
            value: "view(1)".to_string(),
            requirement: pattern_requirement.clone(),
        },
        ConstraintCase {
            operation: "ListDNSViews",
            method: "GET",
            path: "/dns-views/resolver/one,two".to_string(),
            body: None,
            member: "globalResolverId",
            value: "one,two".to_string(),
            requirement: pattern_requirement.clone(),
        },
        ConstraintCase {
            operation: "GetFirewallDomainList",
            method: "GET",
            path: "/firewall-domain-lists/list%20one".to_string(),
            body: None,
            member: "firewallDomainListId",
            value: "list one".to_string(),
            requirement: pattern_requirement.clone(),
        },
        // A create whose refused member is the one in its URI, with the rest of the
        // request well formed: the quoting is the service's, so it does not depend on
        // which operation or which member carried the value.
        ConstraintCase {
            operation: "CreateDNSView",
            method: "POST",
            path: "/dns-views/not%20an%20id%20!!!".to_string(),
            body: Some(json!({ "name": "a-view" })),
            member: "globalResolverId",
            value: "not an id !!!".to_string(),
            requirement: pattern_requirement.clone(),
        },
        // And a create whose refused member arrives in the BODY rather than the URI,
        // which is the other half of that rule.
        ConstraintCase {
            operation: "CreateFirewallRule",
            method: "POST",
            path: "/firewall-rules".to_string(),
            body: Some(json!({
                "dnsViewId": "not an id !!!",
                "name": "a-rule",
                "action": "BLOCK"
            })),
            member: "dnsViewId",
            value: "not an id !!!".to_string(),
            requirement: pattern_requirement.clone(),
        },
        ConstraintCase {
            operation: "GetGlobalResolver",
            method: "GET",
            // 65 characters: one past `ResourceId`'s published maximum of 64.
            path: format!("/global-resolver/{}", "a".repeat(65)),
            body: None,
            member: "globalResolverId",
            value: "a".repeat(65),
            requirement: "Member must have length less than or equal to 64".to_string(),
        },
    ];

    for case in cases {
        let answer = emulator
            .call(case.method, &case.path, case.body.clone())
            .await;
        let label = format!("{} with {:?}", case.operation, case.value);
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {label:?}: expected a ValidationException, got {:?} with {}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.text("message"),
            format!(
                "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: {}",
                case.value, case.member, case.requirement
            ),
            "case {label:?}: expected the service's own validation wording, got {:?}",
            answer.text("message")
        );
        assert!(
            !answer.text("message").contains(RESOURCE_ID_PATTERN_AS_MODELLED),
            "case {label:?}: a quoted pattern keeps neither anchor, got {:?}",
            answer.text("message")
        );
    }

    // And a value the model's pattern DOES admit reaches the handler, so quoting
    // the expression unanchored changed no decision.
    let served = emulator.get_resolver("gr-not-in-this-store").await;
    assert_eq!(
        served.status, 404,
        "case {:?}: a well-formed id is answered by the handler, not refused, got {} with {}",
        "a well-formed id", served.status, served.body
    );
}
