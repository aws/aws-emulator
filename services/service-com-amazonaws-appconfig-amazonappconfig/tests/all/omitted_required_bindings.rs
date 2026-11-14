// SPDX-License-Identifier: CC0-1.0

//! A request that omits a `@required` member bound to a HEADER or a QUERY parameter.
//!
//! RULE: such a request is refused by that member's own modelled constraint — this
//! service's `BadRequestException` (HTTP 400) with the clause naming the member — and never
//! reported as an operation that does not exist. A required header or query binding tells
//! apart operations that SHARE one method and path; AppConfig's model overlaps no such
//! route, so a member a request leaves out says the request is incomplete, not that the
//! route is unknown.
//!
//! The rows span the two members this service's model binds that way
//! (`smithy_query(query="operation", operation="GetConfiguration,CreateHostedConfigurationVersion")`:
//! `GetConfiguration$ClientId` is `@required` and `@httpQuery("client_id")`,
//! `CreateHostedConfigurationVersion$ContentType` is `@required` and
//! `@httpHeader("Content-Type")`) and the forms an omission takes on the wire: the binding
//! absent, another binding present without it, and the binding present with an empty value,
//! which is the member's `@length` minimum rather than its absence.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, host_version, json_call, router, send,
    send_without_content_type, Answer,
};

/// One request with a required binding left out, and the clause its refusal must carry.
struct Omitted {
    /// What a failing assertion calls this row: the operation and what was left out.
    case: &'static str,
    /// The refusal's clause: the member path the framework names, and the constraint text.
    member: &'static str,
    constraint: &'static str,
    /// The answer that request got.
    answer: Answer,
}

/// Assert one row: the modelled 400 with the clause naming the member.
fn assert_refused(row: &Omitted) {
    let Omitted {
        case,
        member,
        constraint,
        answer,
    } = row;
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let code = answer.error_code();
    assert!(
        code.contains("BadRequestException") && !code.contains("NotImplemented"),
        "case {case:?}: expected the modelled BadRequestException, got {code:?}"
    );
    let message = answer.string("Message").unwrap_or_default();
    assert!(
        message.contains(&format!("at '{member}'")) && message.contains(*constraint),
        "case {case:?}: expected a clause naming {member:?} and {constraint:?}, got {message:?}"
    );
}

/// RULE: a `@required` query member left out is the modelled constraint refusal.
#[tokio::test]
async fn an_omitted_required_query_member_is_the_modelled_constraint_refusal() {
    let router = router();
    let application = create_application(&router, "required-query").await;
    let environment = create_environment(&router, &application, "production").await;
    let profile = create_profile(&router, &application, "config").await;
    let configurations =
        format!("/applications/{application}/environments/{environment}/configurations/{profile}");

    // (case, the query string the request carries, the clause the refusal must state)
    let rows: [(&str, &str, &str); 3] = [
        (
            "GetConfiguration with no query string at all",
            "",
            "Member must not be null",
        ),
        (
            "GetConfiguration with another parameter but no client_id",
            "?client_configuration_version=1",
            "Member must not be null",
        ),
        (
            "GetConfiguration with client_id present and empty",
            "?client_id=",
            "Member must have length greater than or equal to 1",
        ),
    ];

    for (case, query, constraint) in rows {
        let answer = send(
            &router,
            "GET",
            &format!("{configurations}{query}"),
            "application/json",
            Vec::new(),
            &[],
        )
        .await;
        assert_refused(&Omitted {
            case,
            member: "clientId",
            constraint,
            answer,
        });
    }

    // The same request WITH the member reaches the operation, which answers the lookup's own
    // refusal for a triple nothing is deployed into — a 400 carrying no validation clause,
    // so the rows above are about the omitted member rather than about this triple.
    let served = json_call(
        &router,
        "GET",
        &format!("{configurations}?client_id=test-client"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        served.status,
        400,
        "a triple with nothing deployed is refused 400, got {} / {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    let message = served.string("Message").unwrap_or_default();
    assert!(
        !message.contains("validation error"),
        "expected the lookup's own refusal, got {message:?}"
    );
}

/// RULE: a `@required` header member left out is the same modelled constraint refusal, and
/// the request that carries it is served — so the refusal is about the member, not about the
/// route.
#[tokio::test]
async fn an_omitted_required_header_member_is_the_modelled_constraint_refusal() {
    let router = router();
    let application = create_application(&router, "required-header").await;
    let profile = create_profile(&router, &application, "config").await;
    let versions = format!(
        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
    );

    assert_refused(&Omitted {
        case: "CreateHostedConfigurationVersion with no Content-Type header",
        member: "contentType",
        constraint: "Member must not be null",
        answer: send_without_content_type(
            &router,
            "POST",
            &versions,
            br#"{"enabled":true}"#.to_vec(),
        )
        .await,
    });
    assert_refused(&Omitted {
        case: "CreateHostedConfigurationVersion with an empty Content-Type header",
        member: "contentType",
        constraint: "Member must have length greater than or equal to 1",
        answer: send(
            &router,
            "POST",
            &versions,
            "",
            br#"{"enabled":true}"#.to_vec(),
            &[],
        )
        .await,
    });

    // The version the refused requests did not write: the profile holds nothing until a
    // request carrying the member is served, and that one is stored as version 1.
    let listed = json_call(&router, "GET", &versions, &Value::Null).await;
    assert_eq!(
        listed
            .json()
            .get("Items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused write must host no version, got {}",
        listed.json()
    );
    let number = host_version(
        &router,
        &application,
        &profile,
        "the request that carries the member",
        br#"{"enabled":true}"#,
        None,
    )
    .await;
    assert_eq!(
        number, 1,
        "expected the served request to host version 1, got {number}"
    );
}

/// RULE: the fallback resolves a request by the route its method and path ADDRESS, and
/// answers for nothing else — a path this service's model does not bind is still answered by
/// the layer that owns it, never as a refused member of some operation.
#[tokio::test]
async fn a_path_no_modelled_route_claims_is_not_refused_as_a_member() {
    let router: MultiServiceRouter = router();
    // (case, method, uri, the status and error code the request is answered with)
    for (case, method, uri, status, code) in [
        // Outside the path space this service claims at all: the router finds no service for
        // it, which is its own answer and not any operation's.
        (
            "a collection this service does not bind",
            "GET",
            "/nosuchcollection",
            404,
            "UnknownService",
        ),
        // Inside the claimed space, but no modelled route names it: the emulator's own
        // not-implemented signal.
        (
            "a sub-resource under a bound collection that no route names",
            "GET",
            "/applications/abc1234/nosuchchildren",
            501,
            "NotImplemented",
        ),
    ] {
        let answer = json_call(&router, method, uri, &Value::Null).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {}",
            answer.json()
        );
    }
}
