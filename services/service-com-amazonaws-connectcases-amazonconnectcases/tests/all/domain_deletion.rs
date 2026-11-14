// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `DeleteDomain`, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: deleting the container a service scopes its resources to
//! deletes the container AND everything scoped to it, so once it is gone every
//! call that resolves that identifier — the read of the container itself, a read
//! or a write of any resource inside it, a create inside it, and the delete
//! repeated — answers the modelled `ResourceNotFoundException` (404) naming that
//! container, the listing of containers leaves it out, and the name it held is
//! free for a new container; every OTHER container and its contents are
//! untouched, and a delete of an identifier the store does not hold is that same
//! 404.
//!
//! The tables below are the class, not one recorded call: their rows differ
//! along what the deleted domain holds (nothing, a field, a template, a layout,
//! a case, a case with a related item associated with it, a case-event
//! configuration), along how many domains the store holds while one of them is
//! deleted, and along the FORM of the identifier a refused delete carries —
//! including each published bound of the modelled `domainId` length and one past
//! it.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::RESOURCE_NOT_FOUND_MESSAGE;

// The request plumbing lives in `tests/all/harness.rs`, shared with this crate's
// other suites.
use crate::harness::{is_minted_id, string_at, Emulator};

/// `DeleteDomain`: `DELETE /domains/{domainId}`
/// (`smithy_query(query="operation", operation="DeleteDomain")`).
fn delete_path(domain_id: &str) -> String {
    format!("/domains/{domain_id}")
}

/// `ListDomains`: `POST /domains-list`.
const LIST_PATH: &str = "/domains-list";

/// The bounds the model puts on `DomainId`: `@length(1..500)`
/// (`smithy_query(query="shape", shape="DomainId")`).
const DOMAIN_ID_LENGTH: (usize, usize) = (1, 500);

/// One row of the content table: what the domain under test holds when it is
/// deleted. Every kind of resource a Cases domain contains is a row, so the rule
/// is asserted over a domain with nothing in it and over one holding each kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    /// Nothing but the reserved `System` fields every domain is provisioned
    /// with.
    Empty,
    /// One custom field.
    Field,
    /// A field and a template requiring it.
    Template,
    /// A field and a layout placing it.
    Layout,
    /// A field, a template and a case written against them.
    Case,
    /// A case with a related item associated with it (so the case also carries
    /// an audit history of more than one event).
    RelatedItem,
    /// A case-event publishing configuration, which is a property of the domain
    /// rather than a resource of its own.
    EventConfiguration,
}

/// Every content row, in one place so both suites below span the same class.
const CONTENTS: [Content; 7] = [
    Content::Empty,
    Content::Field,
    Content::Template,
    Content::Layout,
    Content::Case,
    Content::RelatedItem,
    Content::EventConfiguration,
];

/// One built domain: its id and name, and the ids of whatever its row put in it.
#[derive(Debug, Clone)]
struct Built {
    content: Content,
    domain_id: String,
    name: String,
    field_id: Option<String>,
    template_id: Option<String>,
    layout_id: Option<String>,
    case_id: Option<String>,
}

/// One call: a case name, the HTTP method, the path and the body.
type Call = (String, &'static str, String, Value);

impl Built {
    /// The reads that must succeed while this domain exists: the domain itself
    /// and whatever its row put in it. They are the evidence the resources were
    /// really there before the delete, and the same calls must be refused after
    /// it.
    fn reads(&self) -> Vec<Call> {
        let domain_id = &self.domain_id;
        let mut calls: Vec<Call> = vec![
            (
                self.case("GetDomain"),
                "POST",
                format!("/domains/{domain_id}"),
                json!({}),
            ),
            (
                self.case("ListFields"),
                "POST",
                format!("/domains/{domain_id}/fields-list"),
                json!({}),
            ),
            (
                self.case("ListTemplates"),
                "POST",
                format!("/domains/{domain_id}/templates-list"),
                json!({}),
            ),
            (
                self.case("ListLayouts"),
                "POST",
                format!("/domains/{domain_id}/layouts-list"),
                json!({}),
            ),
            (
                self.case("SearchCases"),
                "POST",
                format!("/domains/{domain_id}/cases-search"),
                json!({}),
            ),
            (
                self.case("GetCaseEventConfiguration"),
                "POST",
                format!("/domains/{domain_id}/case-event-configuration"),
                json!({}),
            ),
        ];
        if let Some(field_id) = &self.field_id {
            calls.push((
                self.case("BatchGetField"),
                "POST",
                format!("/domains/{domain_id}/fields-batch"),
                json!({ "fields": [{ "id": field_id }] }),
            ));
        }
        if let Some(template_id) = &self.template_id {
            calls.push((
                self.case("GetTemplate"),
                "POST",
                format!("/domains/{domain_id}/templates/{template_id}"),
                json!({}),
            ));
        }
        if let Some(layout_id) = &self.layout_id {
            calls.push((
                self.case("GetLayout"),
                "POST",
                format!("/domains/{domain_id}/layouts/{layout_id}"),
                json!({}),
            ));
        }
        if let Some(case_id) = &self.case_id {
            calls.push((
                self.case("GetCase"),
                "POST",
                format!("/domains/{domain_id}/cases/{case_id}"),
                json!({ "fields": [{ "id": self.field_id.clone().unwrap_or_default() }] }),
            ));
            calls.push((
                self.case("GetCaseAuditEvents"),
                "POST",
                format!("/domains/{domain_id}/cases/{case_id}/audit-history"),
                json!({}),
            ));
        }
        calls
    }

    /// The writes scoped to this domain, plus the delete repeated. They are
    /// exercised only AFTER the delete, where each one must be refused: run
    /// before it they would change the very state under test.
    fn writes_and_repeat_delete(&self) -> Vec<Call> {
        let domain_id = &self.domain_id;
        let mut calls: Vec<Call> = vec![
            (
                self.case("CreateField"),
                "POST",
                format!("/domains/{domain_id}/fields"),
                json!({ "name": "a-field-created-after-the-delete", "type": "Text" }),
            ),
            (
                self.case("CreateTemplate"),
                "POST",
                format!("/domains/{domain_id}/templates"),
                json!({ "name": "a-template-created-after-the-delete" }),
            ),
            (
                self.case("CreateLayout"),
                "POST",
                format!("/domains/{domain_id}/layouts"),
                json!({ "name": "a-layout-created-after-the-delete", "content": { "basic": {} } }),
            ),
            (
                self.case("PutCaseEventConfiguration"),
                "PUT",
                format!("/domains/{domain_id}/case-event-configuration"),
                json!({ "eventBridge": { "enabled": false } }),
            ),
            (
                self.case("DeleteDomain repeated"),
                "DELETE",
                delete_path(domain_id),
                json!({}),
            ),
        ];
        if let Some(field_id) = &self.field_id {
            calls.push((
                self.case("DeleteField"),
                "DELETE",
                format!("/domains/{domain_id}/fields/{field_id}"),
                json!({}),
            ));
        }
        if let Some(template_id) = &self.template_id {
            calls.push((
                self.case("UpdateTemplate"),
                "PUT",
                format!("/domains/{domain_id}/templates/{template_id}"),
                json!({ "name": "renamed-after-the-delete" }),
            ));
            calls.push((
                self.case("CreateCase"),
                "POST",
                format!("/domains/{domain_id}/cases"),
                json!({ "templateId": template_id, "fields": [] }),
            ));
        }
        if let Some(layout_id) = &self.layout_id {
            calls.push((
                self.case("DeleteLayout"),
                "DELETE",
                format!("/domains/{domain_id}/layouts/{layout_id}"),
                json!({}),
            ));
        }
        if let Some(case_id) = &self.case_id {
            calls.push((
                self.case("DeleteCase"),
                "DELETE",
                format!("/domains/{domain_id}/cases/{case_id}"),
                json!({}),
            ));
            calls.push((
                self.case("CreateRelatedItem"),
                "POST",
                format!("/domains/{domain_id}/cases/{case_id}/related-items/"),
                json!({
                    "type": "Comment",
                    "content": { "comment": { "body": "after the delete", "contentType": "Text/Plain" } },
                }),
            ));
        }
        calls
    }

    /// A case name naming the operation and the row it is exercised on.
    fn case(&self, operation: &str) -> String {
        format!("{operation} on a {:?} domain", self.content)
    }
}

/// Create a domain named `name` holding `content`, and hand back the ids of
/// everything it holds.
async fn build(emulator: &Emulator, name: &str, content: Content) -> Built {
    let created = emulator
        .ok(name, "POST", "/domains", json!({ "name": name }))
        .await;
    let domain_id = string_at(&created, "domainId", name);
    let mut built = Built {
        content,
        domain_id: domain_id.clone(),
        name: name.to_string(),
        field_id: None,
        template_id: None,
        layout_id: None,
        case_id: None,
    };
    if content == Content::Empty {
        return built;
    }

    // Every non-empty row hangs its resource off a field of the domain's schema.
    let field = emulator
        .ok(
            name,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": "subject", "type": "Text" }),
        )
        .await;
    let field_id = string_at(&field, "fieldId", name);
    built.field_id = Some(field_id.clone());

    match content {
        Content::Empty | Content::Field => {}
        Content::Layout => {
            let layout = emulator
                .ok(
                    name,
                    "POST",
                    &format!("/domains/{domain_id}/layouts"),
                    json!({
                        "name": "a-layout",
                        "content": {
                            "basic": {
                                "topPanel": {
                                    "sections": [{ "fieldGroup": { "fields": [{ "id": field_id }] } }]
                                }
                            }
                        },
                    }),
                )
                .await;
            built.layout_id = Some(string_at(&layout, "layoutId", name));
        }
        Content::EventConfiguration => {
            emulator
                .ok(
                    name,
                    "PUT",
                    &format!("/domains/{domain_id}/case-event-configuration"),
                    json!({
                        "eventBridge": {
                            "enabled": true,
                            "includedData": { "caseData": { "fields": [{ "id": field_id }] } },
                        },
                    }),
                )
                .await;
        }
        Content::Template | Content::Case | Content::RelatedItem => {
            let template = emulator
                .ok(
                    name,
                    "POST",
                    &format!("/domains/{domain_id}/templates"),
                    json!({
                        "name": "a-template",
                        "requiredFields": [{ "fieldId": field_id }],
                    }),
                )
                .await;
            let template_id = string_at(&template, "templateId", name);
            built.template_id = Some(template_id.clone());
            if content == Content::Template {
                return built;
            }
            let case = emulator
                .ok(
                    name,
                    "POST",
                    &format!("/domains/{domain_id}/cases"),
                    json!({
                        "templateId": template_id,
                        "fields": [{ "id": field_id, "value": { "stringValue": "a customer note" } }],
                    }),
                )
                .await;
            let case_id = string_at(&case, "caseId", name);
            built.case_id = Some(case_id.clone());
            if content == Content::RelatedItem {
                emulator
                    .ok(
                        name,
                        "POST",
                        &format!("/domains/{domain_id}/cases/{case_id}/related-items/"),
                        json!({
                            "type": "Comment",
                            "content": {
                                "comment": { "body": "called the customer back", "contentType": "Text/Plain" }
                            },
                        }),
                    )
                    .await;
            }
        }
    }
    built
}

/// The sentence a call refused for a domain the store does not hold must
/// carry. The service words this refusal per operation: `CreateCase` sends the
/// sentence that quotes the domain identifier the request carried, and every
/// other call here sends the service's generic sentence.
fn expected_domain_sentence(case: &str, domain_id: &str) -> String {
    if case.starts_with("CreateCase") {
        format!("Domain '{domain_id}' does not exist.")
    } else {
        RESOURCE_NOT_FOUND_MESSAGE.to_string()
    }
}

/// Assert one call is the modelled 404 for a domain the store does not hold.
async fn refused_as_unknown_domain(
    emulator: &Emulator,
    case: &str,
    method: &str,
    path: &str,
    body: Value,
    domain_id: &str,
) {
    let (status, response) = emulator.send(method, path, Some(body)).await;
    assert_eq!(
        (status, response.get("__type").and_then(Value::as_str)),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: a call scoped to a deleted domain must be refused with 404 \
         ResourceNotFoundException, got {status} {response:?}"
    );
    let expected = expected_domain_sentence(case, domain_id);
    assert_eq!(
        response.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected the sentence {expected:?}, got {response:?}"
    );
    assert_eq!(
        string_at(&response, "resourceType", case),
        "domain",
        "case {case:?}: the refusal must name the kind of resource that could not be \
         resolved, got {response:?}"
    );
    assert_eq!(
        string_at(&response, "resourceId", case),
        domain_id,
        "case {case:?}: the refusal must name the id the request carried, got {response:?}"
    );
}

/// The ids `ListDomains` reports, in the order it reports them.
async fn listed_domain_ids(emulator: &Emulator, case: &str) -> Vec<String> {
    let response = emulator.ok(case, "POST", LIST_PATH, json!({})).await;
    response
        .get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: ListDomains reported no domains: {response:?}"))
        .iter()
        .map(|entry| string_at(entry, "domainId", case))
        .collect()
}

#[tokio::test]
async fn delete_domain_removes_the_domain_and_everything_scoped_to_it() {
    for content in CONTENTS {
        let emulator = Emulator::new();
        // A second domain holding the same contents, so every assertion about
        // what the delete removed is also an assertion about what it left alone.
        let target = build(&emulator, "deleted-domain", content).await;
        let bystander = build(&emulator, "surviving-domain", content).await;

        // The resources exist before the delete: each read of the target
        // succeeds, so the refusals below are the delete's doing.
        for (case, method, path, body) in target.reads() {
            emulator.ok(&case, method, &path, body).await;
        }

        let case = target.case("DeleteDomain");
        let deleted = emulator
            .ok(&case, "DELETE", &delete_path(&target.domain_id), json!({}))
            .await;
        assert!(
            deleted.is_null() || deleted == json!({}),
            "case {case:?}: the documented success is an empty body, got {deleted:?}"
        );

        // Every call that resolves the deleted id is now the modelled 404 —
        // the reads that worked a moment ago, every write scoped to the domain,
        // and the delete repeated.
        for (case, method, path, body) in target
            .reads()
            .into_iter()
            .chain(target.writes_and_repeat_delete())
        {
            refused_as_unknown_domain(
                &emulator,
                &case,
                method,
                &path,
                body,
                &target.domain_id,
            )
            .await;
        }

        // The other domain is untouched: every read of it, and of everything it
        // holds, still answers.
        for (case, method, path, body) in bystander.reads() {
            emulator.ok(&case, method, &path, body).await;
        }

        // The listing reports what the store holds: the survivor, and not the
        // domain that was deleted.
        let listed = listed_domain_ids(&emulator, &target.case("ListDomains")).await;
        assert_eq!(
            listed,
            vec![bystander.domain_id.clone()],
            "case {:?}: ListDomains must report exactly the domains the store still holds, \
             got {listed:?}",
            target.case("ListDomains")
        );

        // The deleted domain's name went with it, so a new domain may take it —
        // and gets an id of its own rather than the deleted one's.
        let case = target.case("CreateDomain reusing the name");
        let recreated = emulator
            .ok(&case, "POST", "/domains", json!({ "name": target.name }))
            .await;
        let recreated_id = string_at(&recreated, "domainId", &case);
        assert!(
            is_minted_id(&recreated_id) && recreated_id != target.domain_id,
            "case {case:?}: the name must be free for a NEW domain, got {recreated:?}"
        );
        let read_back = emulator
            .ok(&case, "POST", &format!("/domains/{recreated_id}"), json!({}))
            .await;
        assert_eq!(
            string_at(&read_back, "name", &case),
            target.name,
            "case {case:?}: the new domain must hold the reused name, got {read_back:?}"
        );
        // A fresh domain is empty: nothing of the deleted domain's contents
        // survived into the name's new holder.
        let templates = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{recreated_id}/templates-list"),
                json!({}),
            )
            .await;
        assert_eq!(
            templates.get("templates").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: the new domain holds none of the deleted domain's templates, \
             got {templates:?}"
        );
    }
}

#[tokio::test]
async fn deleting_domains_one_at_a_time_leaves_the_rest_in_order() {
    let emulator = Emulator::new();

    // A store holding more than one domain, each with contents of its own, so
    // the rows differ in how many domains remain around the one being deleted:
    // the first, the last and a middle one are each deleted in turn.
    let mut remaining: Vec<Built> = Vec::new();
    for (index, content) in CONTENTS.into_iter().enumerate() {
        remaining.push(build(&emulator, &format!("domain-{index}"), content).await);
    }

    let listed = listed_domain_ids(&emulator, "every domain").await;
    assert_eq!(
        listed,
        remaining
            .iter()
            .map(|built| built.domain_id.clone())
            .collect::<Vec<String>>(),
        "case \"every domain\": the listing must report every created domain in creation \
         order, got {listed:?}"
    );

    // Delete the middle one, then the first, then the last of what is left,
    // until the store is empty.
    let mut deleted: Vec<Built> = Vec::new();
    while !remaining.is_empty() {
        let index = match remaining.len() % 3 {
            0 => remaining.len() / 2,
            1 => 0,
            _ => remaining.len() - 1,
        };
        let target = remaining.remove(index);
        let case = format!("DeleteDomain {} of {:?}", target.name, target.content);
        emulator
            .ok(&case, "DELETE", &delete_path(&target.domain_id), json!({}))
            .await;
        deleted.push(target);

        let listed = listed_domain_ids(&emulator, &case).await;
        let expected: Vec<String> = remaining
            .iter()
            .map(|built| built.domain_id.clone())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must drop exactly the deleted domain and keep the \
             rest in creation order, got {listed:?}"
        );

        // Every domain deleted so far is still gone, and every one still there
        // still reads back everything it holds.
        for gone in &deleted {
            refused_as_unknown_domain(
                &emulator,
                &format!("GetDomain after deleting {}", gone.name),
                "POST",
                &format!("/domains/{}", gone.domain_id),
                json!({}),
                &gone.domain_id,
            )
            .await;
        }
        for live in &remaining {
            for (case, method, path, body) in live.reads() {
                emulator.ok(&case, method, &path, body).await;
            }
        }
    }

    let listed = listed_domain_ids(&emulator, "after deleting every domain").await;
    assert_eq!(
        listed,
        Vec::<String>::new(),
        "case \"after deleting every domain\": an emptied store lists nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_of_a_domain_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let live = build(&emulator, "a-live-domain", Content::Case).await;
    let deleted = build(&emulator, "an-already-deleted-domain", Content::Case).await;
    emulator
        .ok(
            "DeleteDomain of the already-deleted domain",
            "DELETE",
            &delete_path(&deleted.domain_id),
            json!({}),
        )
        .await;

    // The rows differ in the FORM of the id: a word nothing minted, an id of the
    // minted shape that was never issued, an id this store holds for a resource
    // of ANOTHER kind (a case, a template), the id of a domain that was deleted,
    // and the id at each published bound of the modelled length.
    let shortest = "x".repeat(DOMAIN_ID_LENGTH.0);
    let longest = "x".repeat(DOMAIN_ID_LENGTH.1);
    let rows: Vec<(&str, String)> = vec![
        ("a word nothing minted", "no-such-domain".to_string()),
        (
            "an id of the minted shape that was never issued",
            "00000000-0000-4000-8000-000000000000".to_string(),
        ),
        (
            "the id of a case, not a domain",
            live.case_id.clone().expect("the row built a case"),
        ),
        (
            "the id of a template, not a domain",
            live.template_id.clone().expect("the row built a template"),
        ),
        ("the id of a deleted domain", deleted.domain_id.clone()),
        ("an id at the modelled minimum length", shortest),
        ("an id at the modelled maximum length", longest),
    ];
    for (case, domain_id) in rows {
        refused_as_unknown_domain(
            &emulator,
            case,
            "DELETE",
            &delete_path(&domain_id),
            json!({}),
            &domain_id,
        )
        .await;
    }

    // One past the modelled maximum is the input constraint, refused before the
    // store is consulted at all.
    let too_long = "x".repeat(DOMAIN_ID_LENGTH.1 + 1);
    let (status, code) = emulator
        .failure(
            "an id one past the modelled maximum length",
            "DELETE",
            &delete_path(&too_long),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"an id one past the modelled maximum length\": a domainId longer than {} \
         must be refused with a 400 ValidationException, got {status} {code}",
        DOMAIN_ID_LENGTH.1
    );

    // None of the refusals wrote anything: the live domain is still whole.
    for (case, method, path, body) in live.reads() {
        emulator.ok(&case, method, &path, body).await;
    }
    let listed = listed_domain_ids(&emulator, "after the refusals").await;
    assert_eq!(
        listed,
        vec![live.domain_id.clone()],
        "case \"after the refusals\": the store must still hold exactly the live domain, \
         got {listed:?}"
    );
}
