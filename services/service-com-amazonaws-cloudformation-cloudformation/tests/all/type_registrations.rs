// SPDX-License-Identifier: CC0-1.0

//! `ListTypeRegistrations` over the awsQuery wire: the registration tokens of this account
//! and Region's extension registration requests.
//!
//! The rule every case here holds the service to: **`ListTypeRegistrations` reports the
//! registration TOKENS of the registration requests this account and Region holds whose
//! status is the one the request filters on (`RegistrationStatusFilter`, "the default is
//! IN_PROGRESS") and which belong to the extension the request identifies — `TypeArn` on its
//! own (an extension-version ARN names the extension it is a version of), `Type` and
//! `TypeName` together, and every extension of the account and Region when it identifies
//! none — in the order the requests were started, one bounded page at a time with the
//! `NextToken` that resumes just past it; the status each request is reported under is the one
//! the clock has already made due; half of the name form identifies no extension and is
//! refused with the modelled `CFNRegistryException` (400); and the read leaves every record
//! as it was beyond that conclusion.**
//!
//! The rows differ along what the rule generalises over: which status a request holds and
//! which status the listing filters on (including none), each way a request can identify an
//! extension and each way it can identify none, the extension kinds the model admits
//! (`RESOURCE`, `MODULE`, `HOOK`), the Region a request is signed for, extensions with
//! several requests and extensions with none, the page size asked for, and the clock.
//!
//! `RegisterType`, the registry writer that STARTS a registration, writes ONE request per
//! call, and one case below seeds a bound's worth of them, so the requests here are seeded
//! straight into the store through `state::record_type_registration` — the same device
//! `handler_progress_reports.rs` and `type_deregistration.rs` use for records a case needs
//! more of than it could drive over the wire. (`extension_registrations.rs` is the suite that
//! drives `RegisterType` itself.) Every READ goes
//! through the real request path (form-encoded body in, XML envelope out), so what is
//! asserted is what a client sees.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registration_status, registry_type, type_arn, type_version_arn, StoredTypeRegistration,
    MAX_TYPE_REGISTRATIONS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    already_due, call_in, member, router_with_engine, still_running, store, xml_blocks, xml_text,
    xml_texts, REGION,
};

/// Another Region's registry, for the rows that hold the listing to its own Region: the
/// registry is regional, so a request started elsewhere is not in this Region's listing.
const OTHER_REGION: &str = "us-west-2";

/// The private RESOURCE extension with several registration requests behind it, one per
/// status a request can hold.
const RESOURCE_TYPE_NAME: &str = "My::Sample::Resource";

/// The private MODULE extension. `TypeName`'s pattern admits the trailing `::MODULE` segment
/// a module's name carries.
const MODULE_TYPE_NAME: &str = "My::Sample::Thing::MODULE";

/// The private HOOK extension, whose one request FAILED: an extension with no request still
/// running is not in the default listing.
const HOOK_TYPE_NAME: &str = "My::Sample::Hook";

/// An extension this Region's registry holds no registration request for at all.
const UNREGISTERED_TYPE_NAME: &str = "None::Sample::Resource";

/// The extension whose registration request ANOTHER Region holds.
const OTHER_REGION_TYPE_NAME: &str = "Other::Region::Resource";

/// The refusal the registry answers a request that names half of the name form.
const NO_IDENTITY: &str =
    "Either TypeArn, or both Type and TypeName, must be specified to identify an extension";

/// The error code the registry's own modelled refusal carries on the wire
/// (`CFNRegistryException`'s `awsQueryError`).
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// This service's wire code for a request it refuses on input grounds.
const INPUT_REFUSAL_CODE: &str = "ValidationError";

/// The `TypeVersionId` a seeded request registered: the registry's version ids are
/// zero-padded and the first version of a record carries this one. No case here is about
/// WHICH version a request registered, so every seeded request names that first version.
const FIRST_VERSION: &str = "00000001";

/// One registration request of `region` for `type_name`, holding `status`.
fn registration(
    region: &str,
    type_kind: &str,
    type_name: &str,
    token: &str,
    status: &str,
    concluded_at: SystemTime,
) -> StoredTypeRegistration {
    let mut request = StoredTypeRegistration::registering(
        region,
        type_kind,
        type_name,
        FIRST_VERSION,
        token,
        concluded_at,
    );
    request.status = status.to_string();
    request
}

/// The records every listing case below reads, in the order the requests were started.
///
/// One extension with a request in each status the model admits and TWO still running (so
/// "the requests of this extension" is more than one row and the order is observable), a
/// module and a Hook beside it, and one request another Region holds.
fn seed(engine: &Arc<engine::InMemoryEngine>) {
    let running = still_running();
    let concluded = already_due();
    for request in [
        registration(
            REGION,
            registry_type::RESOURCE,
            RESOURCE_TYPE_NAME,
            "registration-0001",
            registration_status::IN_PROGRESS,
            running,
        ),
        registration(
            REGION,
            registry_type::RESOURCE,
            RESOURCE_TYPE_NAME,
            "registration-0002",
            registration_status::COMPLETE,
            concluded,
        ),
        registration(
            REGION,
            registry_type::RESOURCE,
            RESOURCE_TYPE_NAME,
            "registration-0003",
            registration_status::FAILED,
            concluded,
        ),
        registration(
            REGION,
            registry_type::RESOURCE,
            RESOURCE_TYPE_NAME,
            "registration-0004",
            registration_status::IN_PROGRESS,
            running,
        ),
        registration(
            REGION,
            registry_type::MODULE,
            MODULE_TYPE_NAME,
            "registration-0005",
            registration_status::IN_PROGRESS,
            running,
        ),
        registration(
            REGION,
            registry_type::MODULE,
            MODULE_TYPE_NAME,
            "registration-0006",
            registration_status::COMPLETE,
            concluded,
        ),
        registration(
            REGION,
            registry_type::HOOK,
            HOOK_TYPE_NAME,
            "registration-0007",
            registration_status::FAILED,
            concluded,
        ),
        registration(
            OTHER_REGION,
            registry_type::RESOURCE,
            OTHER_REGION_TYPE_NAME,
            "registration-0008",
            registration_status::IN_PROGRESS,
            running,
        ),
    ] {
        store(engine).lock().record_type_registration(request);
    }
}

/// One `ListTypeRegistrations` call, as an SDK sends it.
async fn list(
    router: &MultiServiceRouter,
    region: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, region, "ListTypeRegistrations", members).await
}

/// The tokens one page reports, read INSIDE the `RegistrationTokenList` block so no other
/// list the envelope carries can be swept up with it. An empty list is an empty element, so
/// a page with no tokens reads as no tokens.
fn tokens(body: &str) -> Vec<String> {
    match xml_blocks(body, "RegistrationTokenList").into_iter().next() {
        Some(block) => xml_texts(&block, "member"),
        None => Vec::new(),
    }
}

/// One row of the listing table: what the request filters and identifies, and the tokens the
/// listing must report for it.
struct Case {
    case: &'static str,
    /// `RegistrationStatusFilter`, where the request names one.
    status_filter: Option<&'static str>,
    /// The `Type` and `TypeName` the request names, where it names them.
    named: Option<(&'static str, &'static str)>,
    /// The `TypeArn` the request names, where it names one.
    type_arn: Option<String>,
    /// The tokens the page must carry, in order.
    expected: Vec<&'static str>,
}

impl Case {
    /// The request members this row sends.
    fn members(&self) -> Vec<(String, String)> {
        let mut members = Vec::new();
        if let Some(filter) = self.status_filter {
            members.push(member("RegistrationStatusFilter", filter));
        }
        if let Some((type_kind, type_name)) = self.named {
            members.push(member("Type", type_kind));
            members.push(member("TypeName", type_name));
        }
        if let Some(type_arn) = &self.type_arn {
            members.push(member("TypeArn", type_arn));
        }
        members
    }
}

#[tokio::test]
async fn the_listing_reports_the_tokens_of_the_requests_it_was_asked_for() {
    let (router, engine) = router_with_engine();
    seed(&engine);

    let resource_arn = type_arn(REGION, registry_type::RESOURCE, RESOURCE_TYPE_NAME);
    let cases = [
        Case {
            case: "no filter and no extension: every request still running",
            status_filter: None,
            named: None,
            type_arn: None,
            expected: vec!["registration-0001", "registration-0004", "registration-0005"],
        },
        Case {
            case: "the default status named explicitly",
            status_filter: Some(registration_status::IN_PROGRESS),
            named: None,
            type_arn: None,
            expected: vec!["registration-0001", "registration-0004", "registration-0005"],
        },
        Case {
            case: "every completed request",
            status_filter: Some(registration_status::COMPLETE),
            named: None,
            type_arn: None,
            expected: vec!["registration-0002", "registration-0006"],
        },
        Case {
            case: "every failed request",
            status_filter: Some(registration_status::FAILED),
            named: None,
            type_arn: None,
            expected: vec!["registration-0003", "registration-0007"],
        },
        Case {
            case: "one extension by kind and name, at the default status",
            status_filter: None,
            named: Some((registry_type::RESOURCE, RESOURCE_TYPE_NAME)),
            type_arn: None,
            expected: vec!["registration-0001", "registration-0004"],
        },
        Case {
            case: "one extension by kind and name, completed",
            status_filter: Some(registration_status::COMPLETE),
            named: Some((registry_type::RESOURCE, RESOURCE_TYPE_NAME)),
            type_arn: None,
            expected: vec!["registration-0002"],
        },
        Case {
            case: "one extension by its own ARN, failed",
            status_filter: Some(registration_status::FAILED),
            named: None,
            type_arn: Some(resource_arn.clone()),
            expected: vec!["registration-0003"],
        },
        Case {
            case: "one extension by an extension-VERSION ARN",
            status_filter: None,
            named: None,
            type_arn: Some(type_version_arn(&resource_arn, "00000001")),
            expected: vec!["registration-0001", "registration-0004"],
        },
        Case {
            case: "the ARN wins over a kind and name beside it",
            status_filter: Some(registration_status::COMPLETE),
            named: Some((registry_type::RESOURCE, RESOURCE_TYPE_NAME)),
            type_arn: Some(resource_arn.clone()),
            expected: vec!["registration-0002"],
        },
        Case {
            case: "a module",
            status_filter: None,
            named: Some((registry_type::MODULE, MODULE_TYPE_NAME)),
            type_arn: None,
            expected: vec!["registration-0005"],
        },
        Case {
            case: "a module's completed request",
            status_filter: Some(registration_status::COMPLETE),
            named: Some((registry_type::MODULE, MODULE_TYPE_NAME)),
            type_arn: None,
            expected: vec!["registration-0006"],
        },
        Case {
            case: "a Hook's failed request",
            status_filter: Some(registration_status::FAILED),
            named: Some((registry_type::HOOK, HOOK_TYPE_NAME)),
            type_arn: None,
            expected: vec!["registration-0007"],
        },
        Case {
            case: "a Hook whose only request has concluded",
            status_filter: None,
            named: Some((registry_type::HOOK, HOOK_TYPE_NAME)),
            type_arn: None,
            expected: vec![],
        },
        Case {
            case: "an extension this Region registered nothing for",
            status_filter: None,
            named: Some((registry_type::RESOURCE, UNREGISTERED_TYPE_NAME)),
            type_arn: None,
            expected: vec![],
        },
        Case {
            case: "the right name under the wrong kind",
            status_filter: None,
            named: Some((registry_type::MODULE, RESOURCE_TYPE_NAME)),
            type_arn: None,
            expected: vec![],
        },
        Case {
            case: "an extension another Region holds the request for, by name",
            status_filter: None,
            named: Some((registry_type::RESOURCE, OTHER_REGION_TYPE_NAME)),
            type_arn: None,
            expected: vec![],
        },
        Case {
            case: "an extension another Region holds the request for, by ARN",
            status_filter: None,
            named: None,
            type_arn: Some(type_arn(
                OTHER_REGION,
                registry_type::RESOURCE,
                OTHER_REGION_TYPE_NAME,
            )),
            expected: vec![],
        },
    ];

    for case in &cases {
        let (status, body) = list(&router, REGION, &case.members()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the listing to succeed, got {status} {body}",
            case.case
        );
        let reported = tokens(&body);
        assert_eq!(
            reported,
            case.expected
                .iter()
                .map(|token| (*token).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected those registration tokens, got {reported:?} in {body}",
            case.case
        );
        assert!(
            xml_text(&body, "NextToken").is_none(),
            "case {:?}: a page that carries the whole listing resumes nothing, got {body}",
            case.case
        );
    }

    // The same listing, signed for the OTHER Region: that Region's own request is the one it
    // reports, which is what makes the namespacing above a property of the records.
    let (status, body) = list(&router, OTHER_REGION, &[]).await;
    assert_eq!(
        status, 200,
        "case the other Region's listing: expected it to succeed, got {status} {body}"
    );
    let reported = tokens(&body);
    assert_eq!(
        reported,
        vec!["registration-0008".to_string()],
        "case the other Region's listing: expected its own request, got {reported:?} in {body}"
    );
}

/// One row of the refusal table: the members the request names, and the refusal it answers.
struct RefusalCase {
    case: &'static str,
    members: Vec<(String, String)>,
    code: &'static str,
    message: &'static str,
}

#[tokio::test]
async fn a_request_that_identifies_half_an_extension_is_refused() {
    let (router, engine) = router_with_engine();
    seed(&engine);

    for RefusalCase {
        case,
        members,
        code,
        message,
    } in [
        RefusalCase {
            case: "a kind without a name",
            members: vec![member("Type", registry_type::RESOURCE)],
            code: REGISTRY_REFUSAL_CODE,
            message: NO_IDENTITY,
        },
        RefusalCase {
            case: "a name without a kind",
            members: vec![member("TypeName", RESOURCE_TYPE_NAME)],
            code: REGISTRY_REFUSAL_CODE,
            message: NO_IDENTITY,
        },
        RefusalCase {
            case: "a kind without a name, beside a status filter",
            members: vec![
                member("Type", registry_type::MODULE),
                member("RegistrationStatusFilter", registration_status::COMPLETE),
            ],
            code: REGISTRY_REFUSAL_CODE,
            message: NO_IDENTITY,
        },
        RefusalCase {
            case: "a name without a kind, for an extension nothing registered",
            members: vec![member("TypeName", UNREGISTERED_TYPE_NAME)],
            code: REGISTRY_REFUSAL_CODE,
            message: NO_IDENTITY,
        },
    ] {
        let (status, body) = list(&router, REGION, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused with HTTP 400, got {status} \
             {body}"
        );
        let reported_code = xml_text(&body, "Code").unwrap_or_default();
        assert_eq!(
            reported_code, code,
            "case {case:?}: expected the refusal code {code}, got {reported_code:?} in {body}"
        );
        let reported_message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            reported_message, message,
            "case {case:?}: expected the registry's own wording, got {reported_message:?}"
        );
        assert!(
            tokens(&body).is_empty(),
            "case {case:?}: a refusal reports no tokens, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_is_reported_under_the_status_the_clock_has_made_due() {
    let (router, engine) = router_with_engine();
    // Two requests started as still running, one of which the clock has already concluded:
    // the conclusion is owed at an instant whether or not anybody calls, so the listing
    // reports the concluded one as COMPLETE and the other as still running.
    for (token, concluded_at) in [
        ("registration-due", already_due()),
        ("registration-running", still_running()),
    ] {
        store(&engine)
            .lock()
            .record_type_registration(StoredTypeRegistration::registering(
                REGION,
                registry_type::RESOURCE,
                RESOURCE_TYPE_NAME,
                FIRST_VERSION,
                token,
                concluded_at,
            ));
    }

    for (case, filter, expected) in [
        (
            "the request still running",
            registration_status::IN_PROGRESS,
            vec!["registration-running"],
        ),
        (
            "the request whose conclusion was due",
            registration_status::COMPLETE,
            vec!["registration-due"],
        ),
        (
            "nothing concluded as failed",
            registration_status::FAILED,
            vec![],
        ),
    ] {
        let (status, body) =
            list(&router, REGION, &[member("RegistrationStatusFilter", filter)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to succeed, got {status} {body}"
        );
        let reported = tokens(&body);
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|token| (*token).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected those tokens, got {reported:?} in {body}"
        );
    }

    // What the records hold after the reads: the conclusion the clock owed, and nothing else
    // moved — a listing reports, it does not advance a request that was not due.
    for (case, token, expected) in [
        (
            "the concluded request",
            "registration-due",
            registration_status::COMPLETE,
        ),
        (
            "the request still running",
            "registration-running",
            registration_status::IN_PROGRESS,
        ),
    ] {
        let held = store(&engine)
            .lock()
            .type_registration(REGION, token)
            .map(|request| request.status.clone());
        assert_eq!(
            held.as_deref(),
            Some(expected),
            "case {case:?}: expected the record to hold {expected}, got {held:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_is_paged_by_the_maximum_the_request_asks_for() {
    let (router, engine) = router_with_engine();
    // More requests than any page below carries, every one of them still running and of one
    // extension, so the pages of one collection partition it.
    let seeded = 7;
    for index in 0..seeded {
        store(&engine)
            .lock()
            .record_type_registration(StoredTypeRegistration::registering(
                REGION,
                registry_type::RESOURCE,
                RESOURCE_TYPE_NAME,
                FIRST_VERSION,
                format!("registration-{index:04}"),
                still_running(),
            ));
    }
    let every_token: Vec<String> = (0..seeded)
        .map(|index| format!("registration-{index:04}"))
        .collect();

    // Page sizes spanning the way a maximum divides the collection: one at a time, a size
    // that divides it unevenly, exactly the whole collection, and more than there is.
    for max_results in [1_usize, 3, 7, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut members = vec![member("MaxResults", &max_results.to_string())];
            if let Some(token) = &token {
                members.push(member("NextToken", token));
            }
            let (status, body) = list(&router, REGION, &members).await;
            assert_eq!(
                status, 200,
                "case a page of {max_results}: expected the listing to succeed, got {status} \
                 {body}"
            );
            let page = tokens(&body);
            assert!(
                page.len() <= max_results,
                "case a page of {max_results}: a page carries at most that many tokens, got \
                 {page:?}"
            );
            walked.extend(page);
            token = xml_text(&body, "NextToken");
            pages += 1;
            assert!(
                pages <= seeded + 1,
                "case a page of {max_results}: the walk must terminate, got {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every_token,
            "case a page of {max_results}: expected the whole listing exactly once, got \
             {walked:?}"
        );
    }

    // A cursor this service never issued for this collection is a bad request, not an empty
    // page: a token of another collection's listing is one of those.
    let (status, body) = list(
        &router,
        REGION,
        &[member("NextToken", "not-a-token-this-service-issued")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case a cursor nothing issued: expected HTTP 400, got {status} {body}"
    );
    let code = xml_text(&body, "Code").unwrap_or_default();
    assert_eq!(
        code, INPUT_REFUSAL_CODE,
        "case a cursor nothing issued: expected {INPUT_REFUSAL_CODE}, got {code:?} in {body}"
    );
}

#[tokio::test]
async fn the_store_keeps_a_bounded_number_of_registration_requests() {
    let (router, engine) = router_with_engine();
    // One past the bound: the oldest request is reclaimed, so the listing reports the
    // newest ones and no longer the first.
    let seeded = MAX_TYPE_REGISTRATIONS + 1;
    for index in 0..seeded {
        store(&engine)
            .lock()
            .record_type_registration(StoredTypeRegistration::registering(
                REGION,
                registry_type::RESOURCE,
                RESOURCE_TYPE_NAME,
                FIRST_VERSION,
                format!("registration-{index:05}"),
                still_running(),
            ));
    }
    assert_eq!(
        store(&engine).lock().type_registrations.len(),
        MAX_TYPE_REGISTRATIONS,
        "case seeding one past the bound: the store keeps at most {MAX_TYPE_REGISTRATIONS} \
         requests"
    );

    // The whole listing, walked one maximum-size page at a time.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut members = vec![member("MaxResults", "100")];
        if let Some(token) = &token {
            members.push(member("NextToken", token));
        }
        let (status, body) = list(&router, REGION, &members).await;
        assert_eq!(
            status, 200,
            "case the bounded listing: expected it to succeed, got {status} {body}"
        );
        walked.extend(tokens(&body));
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked.len(),
        MAX_TYPE_REGISTRATIONS,
        "case the bounded listing: expected {MAX_TYPE_REGISTRATIONS} tokens, got {}",
        walked.len()
    );
    assert!(
        !walked.contains(&"registration-00000".to_string()),
        "case the reclaimed oldest request: its token is no longer in the listing"
    );
    assert!(
        walked.contains(&format!("registration-{:05}", seeded - 1)),
        "case the newest request: its token is in the listing, got {:?}",
        walked.last()
    );
}
