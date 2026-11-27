// SPDX-License-Identifier: CC0-1.0

//! What the schema-creation plane REPORTS about a creation that succeeded, and how the
//! call that submits a document refuses an `apiId` it cannot resolve.
//!
//! Two rules, each asserted over a class of inputs rather than over the one request that
//! exposed the defect:
//!
//! 1. the detail of a schema creation that SUCCEEDED states how many types the created
//!    schema holds ("Successfully created schema with `<N>` types.") — the types the
//!    submitted document defines plus the one the service contributes to every schema it
//!    creates — so the number is derived from the document the caller passed rather than
//!    being a sentence that
//!    holds for every schema. Asserted over a table of documents spanning the count of
//!    types they define, the kinds they declare them with, definitions that declare no
//!    type at all (a `schema` definition, a directive definition), extensions of types
//!    the document declares, and a generated document far past the counts written out by
//!    hand — each row cross-checked against the types `ListTypes` reports for the API, so
//!    the detail is pinned to the stored type system and not to the test's own
//!    arithmetic; and over a second creation on the SAME API, whose detail follows the new
//!    document's count;
//! 2. `StartSchemaCreation` classifies the `apiId` it is addressed at by that id's FORM:
//!    a value no id the service issues could be is a 400 `BadRequestException` whose
//!    message echoes the rejected id ("Invalid API ID `<apiId>`"), while a value of the
//!    form the service issues is a reference it could have handed out, so an API this
//!    account does not hold is the 404 `NotFoundException` it is — asserted over a value
//!    no API id can be, a value shorter than one, a value of another form altogether, an
//!    id of the issued form this account does not hold and one of the issued length drawn
//!    from outside the alphabet those ids are written in. The reporting call of the same
//!    plane classifies the identical id by its own rule — the contrast is asserted on the
//!    same pair of calls (`schema_creation_api_resolution` asserts the submitting call's
//!    rule over the whole class, deleted APIs included).
//!
//! The harness is the one the schema suites already carry (the router the production
//! emulator mounts, `start_creation`, `creation_status` and the poll that waits for the
//! creation the clock owes), reused rather than copied.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::error_contract::{call, create_api, router};
use super::schema_creation::{await_creation, creation_status, start_creation};

/// The page size a type listing is read in: `ListTypes$maxResults`' published maximum
/// (`smithy_query(query="operation", operation="ListTypes")`: range 0-25), so a document
/// of any size is read in as few calls as the service allows.
const TYPE_PAGE: usize = 25;

/// One row of the detail table: a schema document, and how many types it defines.
#[derive(Debug)]
struct Row {
    label: &'static str,
    /// The document the creation carries, in GraphQL schema language.
    document: String,
    /// The number of types the document defines; the success detail states this count
    /// plus the one type the service contributes to every schema it creates.
    types: usize,
}

/// The documents the detail rule is asserted over.
///
/// Source for the kinds and for which definitions declare a type: GraphQL's SDL type
/// definitions (object, interface, union, enum, input object, scalar), its type
/// extensions and its `schema` and directive definitions, which
/// `StartSchemaCreationRequest$definition` ("The schema definition, in GraphQL schema
/// language format") is written in — an extension and a `schema` or directive definition
/// declare no type of their own, so neither adds to the count.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one object type",
            document: "type Query {\n  hello: String\n}\n".to_string(),
            types: 1,
        },
        Row {
            label: "a schema definition and the types it reaches",
            document: "schema {\n  query: Query\n}\n\
                       type Query {\n  book: Book\n}\n\
                       type Book {\n  id: ID!\n  author: Author\n}\n\
                       type Author {\n  name: String\n}\n"
                .to_string(),
            types: 3,
        },
        Row {
            label: "every kind the SDL grammar declares",
            document: "interface Entity {\n  id: ID!\n}\n\
                       type Book implements Entity {\n  id: ID!\n  status: Status\n  tag: Label\n}\n\
                       type Magazine implements Entity {\n  id: ID!\n  issue: Int\n}\n\
                       enum Status {\n  DRAFT\n  PUBLISHED\n}\n\
                       input Filter {\n  status: Status\n}\n\
                       union Publication = Book | Magazine\n\
                       scalar Label\n\
                       type Query {\n  book(filter: Filter): Book\n  any: Publication\n}\n"
                .to_string(),
            types: 8,
        },
        Row {
            label: "extensions of the types the document declares",
            document: "type Query {\n  hello: String\n}\n\
                       extend type Query {\n  goodbye: String\n}\n\
                       enum Mood {\n  GOOD\n}\n\
                       extend enum Mood {\n  BAD\n}\n"
                .to_string(),
            types: 2,
        },
        Row {
            label: "a directive definition and the type that uses it",
            document: "directive @auth(level: Int) on FIELD_DEFINITION\n\
                       type Query {\n  hello: String @auth(level: 1)\n}\n"
                .to_string(),
            types: 1,
        },
        Row {
            label: "a generated document of many types",
            document: (0..37)
                .map(|index| format!("type T{index} {{\n  id: ID\n}}\n"))
                .collect(),
            types: 37,
        },
    ]
}

/// Every type name `api_id` holds, read through `ListTypes` page by page.
async fn listed_types(router: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = format!("format={}&maxResults={TYPE_PAGE}", state::SDL_FORMAT);
        if let Some(token) = token.as_deref() {
            query.push_str(&format!("&nextToken={token}"));
        }
        let answer = call(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/types?{query}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case list the types of {api_id}: expected ListTypes 200, got {} {:?}",
            answer.status, answer.body
        );
        for listed in answer.body["types"].as_array().into_iter().flatten() {
            names.push(listed["name"].as_str().unwrap_or_default().to_string());
        }
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
}

#[tokio::test]
async fn the_success_detail_states_how_many_types_the_document_defines() {
    let router = router();
    let rows = rows();
    let mut apis: Vec<String> = Vec::new();

    // Every row's creation is scheduled first, each on its own API, so the ONE wait below
    // crosses the due instant of all of them.
    for row in &rows {
        let api_id = create_api(&router, &format!("detail-{}", row.types)).await;
        let started = start_creation(&router, &api_id, row.document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected StartSchemaCreation 200, got {} {:?}",
            row.label, started.status, started.body
        );
        apis.push(api_id);
    }

    for (row, api_id) in rows.iter().zip(&apis) {
        await_creation(&router, api_id, row.label).await;

        let settled = creation_status(&router, api_id).await;
        let expected = format!(
            "Successfully created schema with {} types.",
            row.types + 1
        );
        assert_eq!(
            (
                settled.body["status"].as_str(),
                settled.body["details"].as_str()
            ),
            (Some(state::SCHEMA_SUCCESS), Some(expected.as_str())),
            "case {:?}: expected {:?} detailed as {expected:?}, got {:?}",
            row.label,
            state::SCHEMA_SUCCESS,
            settled.body
        );

        // The count the detail states is the type system the creation installed plus the
        // service's own type, not the test's own reading of the document: the API reports
        // exactly the types the document declared.
        let listed = listed_types(&router, api_id).await;
        assert_eq!(
            listed.len(),
            row.types,
            "case {:?}: expected the API to hold the {} types the document declared, got \
             {listed:?}",
            row.label,
            row.types
        );
    }
}

#[tokio::test]
async fn a_second_creation_detail_follows_the_new_document() {
    let router = router();
    let api_id = create_api(&router, "detail-recomputed").await;

    // A schema document is the whole type system, so the detail of the creation that ran
    // last counts the types THAT document defines — a different number from the first.
    for (label, document, types) in [
        (
            "the first document",
            "type Query {\n  book: Book\n}\ntype Book {\n  id: ID!\n}\n",
            2,
        ),
        ("the second document", "type Query {\n  hello: String\n}\n", 1),
    ] {
        let started = start_creation(&router, &api_id, document.as_bytes()).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: expected StartSchemaCreation 200, got {} {:?}",
            started.status, started.body
        );
        await_creation(&router, &api_id, label).await;

        let settled = creation_status(&router, &api_id).await;
        let expected = format!("Successfully created schema with {} types.", types + 1);
        assert_eq!(
            settled.body["details"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: expected the detail {expected:?}, got {:?}",
            settled.body
        );
    }
}

/// One row of the refusal table: an `apiId` that resolves to no API of this account, and
/// why it does not.
#[derive(Debug)]
struct Rejected {
    label: &'static str,
    api_id: &'static str,
    /// Whether the value has the form of an id the service ISSUES, which is what the
    /// submitting call reads: a value of that form is a reference the service could have
    /// handed out, so the API it does not resolve to is a missing resource; any other
    /// value is a request no API id appears in at all.
    issued_form: bool,
    /// The status and error shape the REPORTING call owes the same id, which classifies it
    /// by its own rule.
    read: (u16, &'static str),
}

/// The ways an `apiId` can resolve to no API: a value longer than the 26 characters an
/// AppSync API id has (`state::API_ID_LEN`), a well-formed id this account does not hold
/// both inside the alphabet the service mints ids over and outside it, a shorter value,
/// and a value of another form altogether.
fn rejected() -> Vec<Rejected> {
    vec![
        Rejected {
            label: "an id too long to be one",
            api_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaa",
            issued_form: false,
            read: (400, "BadRequestException"),
        },
        Rejected {
            label: "a well-formed id this account does not hold",
            api_id: "abcdefghijklmnopqrstuvwxyz",
            issued_form: true,
            read: (404, "NotFoundException"),
        },
        Rejected {
            label: "an id outside the id alphabet",
            api_id: "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            issued_form: false,
            read: (404, "NotFoundException"),
        },
        Rejected {
            label: "a value far shorter than an id",
            api_id: "zzz",
            issued_form: false,
            read: (404, "NotFoundException"),
        },
        Rejected {
            label: "a value of another form altogether",
            api_id: "no.such.api",
            issued_form: false,
            read: (404, "NotFoundException"),
        },
    ]
}

#[tokio::test]
async fn submitting_a_schema_classifies_an_unresolvable_api_id_by_its_form() {
    let router = router();
    let document = b"type Query {\n  hello: String\n}\n";

    for row in rejected() {
        let refused = start_creation(&router, row.api_id, document).await;
        let expected: (u16, &str, String) = if row.issued_form {
            (
                404,
                "NotFoundException",
                state::API_NOT_FOUND_MESSAGE.to_string(),
            )
        } else {
            (
                400,
                "BadRequestException",
                format!("Invalid API ID {}", row.api_id),
            )
        };
        assert_eq!(
            (
                refused.status,
                refused.error_type.as_str(),
                refused.body["message"].as_str().unwrap_or_default()
            ),
            (expected.0, expected.1, expected.2.as_str()),
            "case {:?}: expected {expected:?}, got {} {} {:?}",
            row.label,
            refused.status,
            refused.error_type,
            refused.body
        );

        // The reporting call of the same plane resolves a RESOURCE through the id, so the
        // identical value is classified by whether an API could carry it at all.
        let read = creation_status(&router, row.api_id).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            row.read,
            "case {:?} GetSchemaCreationStatus: expected {:?}, got {} {} {:?}",
            row.label,
            row.read,
            read.status,
            read.error_type,
            read.body
        );
    }

    // The refusal is about the id resolving to no API, nothing else: the same document on
    // an API this account does hold is accepted.
    let api_id = create_api(&router, "resolvable-api").await;
    let accepted = start_creation(&router, &api_id, document).await;
    assert_eq!(
        (accepted.status, accepted.body["status"].as_str()),
        (200, Some(state::SCHEMA_PROCESSING)),
        "case an API this account holds: expected 200 {:?}, got {} {:?}",
        state::SCHEMA_PROCESSING,
        accepted.status,
        accepted.body
    );
    assert_eq!(
        accepted.body["message"],
        Value::Null,
        "case an API this account holds: expected no refusal, got {:?}",
        accepted.body
    );
}
