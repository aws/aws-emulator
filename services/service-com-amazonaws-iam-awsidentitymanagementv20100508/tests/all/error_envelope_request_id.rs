// SPDX-License-Identifier: CC0-1.0

//! The `<RequestId>` every awsQuery error document of this service carries, end to end over
//! the wire.
//!
//! The rule under test: an awsQuery error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>…</RequestId></ErrorResponse>`
//! — the `<RequestId>` is a SIBLING of `<Error>`, naming the request the failure answers,
//! and it is a property of the RESPONSE rather than of any error shape, so EVERY layer that
//! can answer an error carries it: a handler-returned modelled error, a refusal the
//! centralized walk raised before any handler ran, a request naming no modelled action, and
//! the scaffold's 501. The id is freshly minted per call — its VALUE is per-run and is not
//! part of the requirement, so what the rows read back is its presence, its position and
//! its form, plus that two calls do not share one.
//!
//! A successful response already names the request in its own `<ResponseMetadata>`, so it
//! carries exactly one id and is not touched.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: which layer
//! renders the failure, which HTTP status it carries, and which error code — no single error
//! path is the specification.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use std::collections::BTreeSet;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form an AWS request id carries. The value
/// itself is per-run, so this form is the whole requirement on it.
///
/// Shared with `tests/all/error_request_id_across_operations.rs`, which holds the same rule
/// across the service's operations rather than across the layers that render a failure, so
/// the form is read exactly one way.
pub(crate) fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .chars()
                        .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
            })
}

/// One row: the call to make, and the status and error code the layer under test answers.
struct ErrorCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

/// RULE: every awsQuery error document this service answers carries a `<RequestId>` beside
/// its `<Error>`, whichever layer rendered it — and a distinct one per call.
#[tokio::test]
async fn every_error_document_names_the_request_it_answers() {
    let router = router();
    // A group the account holds, so the row that asks for it a second time is refused as a
    // conflict rather than as anything else.
    let (status, body) = call(&router, "CreateGroup", &[("GroupName", "held-group")]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateGroup must succeed, got {status}: {body}"
    );

    // Rows: the four layers that can answer an error, each with the status and code it
    // answers under.
    let rows = vec![
        ErrorCase {
            case: "a value outside a modelled bound, refused before any handler ran",
            action: "ListUsers",
            params: vec![param("MaxItems", "0")],
            status: 400,
            code: "ValidationError",
        },
        ErrorCase {
            case: "a value whose form IAM takes itself",
            action: "ListGroups",
            params: vec![param("PathPrefix", "badpath")],
            status: 400,
            code: "ValidationError",
        },
        ErrorCase {
            case: "a modelled error a handler returned for an absent entity",
            action: "GetGroup",
            params: vec![param("GroupName", "no-such-group")],
            status: 404,
            code: "NoSuchEntity",
        },
        ErrorCase {
            case: "a modelled conflict a handler returned",
            action: "CreateGroup",
            params: vec![param("GroupName", "held-group")],
            status: 409,
            code: "EntityAlreadyExists",
        },
        ErrorCase {
            case: "an operation the emulator has not implemented",
            action: "ListMFADevices",
            params: vec![param("UserName", "no-such-user")],
            status: 501,
            code: "NotImplemented",
        },
        ErrorCase {
            case: "a request naming no modelled action",
            action: "NotAnActionThisModelDeclares",
            params: vec![],
            status: 400,
            code: "InvalidAction",
        },
    ];

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for ErrorCase {
        case,
        action,
        params,
        status: expected_status,
        code,
    } in rows
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got {body}"
        );
        let request_id = element(&body, "RequestId");
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected the document to name the request, got {request_id:?} in \
             {body}"
        );
        // The element is the envelope's, not the error's: it sits AFTER `</Error>`.
        let error_end = body
            .find("</Error>")
            .unwrap_or_else(|| panic!("case {case:?}: no <Error> element in {body}"));
        let named = body
            .find("<RequestId>")
            .unwrap_or_else(|| panic!("case {case:?}: no <RequestId> element in {body}"));
        assert!(
            named > error_end,
            "case {case:?}: expected <RequestId> beside <Error> rather than inside it, got \
             {body}"
        );
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: expected a fresh id per call, got {request_id:?} again"
        );
    }
}

/// RULE: the same error path answered twice names two different requests — the id is minted
/// per call and is nothing the store holds.
#[tokio::test]
async fn two_answers_to_one_error_path_name_two_requests() {
    let router = router();
    let mut named = Vec::new();
    for _ in 0..3 {
        let (status, body) = call(&router, "GetGroup", &[("GroupName", "no-such-group")]).await;
        assert_eq!(status, 404, "expected HTTP 404, got {status}: {body}");
        named.push(element(&body, "RequestId"));
    }
    let distinct: BTreeSet<&String> = named.iter().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "expected one id per call, got {named:?}"
    );
}

/// RULE: a SUCCESSFUL response already names the request in its own `<ResponseMetadata>`,
/// so it carries exactly one id and the error element is not added to it.
#[tokio::test]
async fn a_successful_response_names_the_request_exactly_once() {
    let router = router();
    let (status, body) = call(&router, "ListUsers", &[]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {body}");
    let named = elements(&body, "RequestId");
    assert_eq!(
        named.len(),
        1,
        "expected exactly one <RequestId> in a success envelope, got {named:?}: {body}"
    );
    assert!(
        is_request_id(&named[0]),
        "expected the success envelope to name the request, got {:?}",
        named[0]
    );
}
