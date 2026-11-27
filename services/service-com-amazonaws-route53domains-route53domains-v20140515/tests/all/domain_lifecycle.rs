// SPDX-License-Identifier: CC0-1.0

//! The registered-domain lifecycle over awsJson1_1: `CheckDomainAvailability`,
//! `RegisterDomain`, `GetOperationDetail`, `GetDomainDetail`, `ListDomains`,
//! `DeleteDomain`, plus the auto-renew pair and `ListOperations`.
//!
//! Every test drives the real wire path — `POST /` with
//! `X-Amz-Target: Route53Domains_v20140515.<Operation>` and a JSON body of modelled
//! member names (OPERATIONS.md) — through the same `MultiServiceRouter` the production
//! router dispatches on, over ONE shared engine. Nothing calls a handler directly.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use time::{Month, OffsetDateTime};

/// One emulator: this service mounted on a single shared `InMemoryEngine`, exactly as
/// `router/src/lib.rs` mounts it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router
            // The suite exercises behaviour, not SigV4: the signature the harness sends
            // is not a real one.
            .set_request_authentication(false)
            .register_service(
                service_com_amazonaws_route53domains_route53domains_v20140515::EmulatedService::new(
                    engine,
                ),
            );
        Self { router }
    }

    /// Call `operation`, signed the way an AWS SDK signs it, and return the HTTP status
    /// and the parsed JSON body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(operation, body, true).await
    }

    async fn send(&self, operation: &str, body: &Value, signed: bool) -> (u16, Value) {
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "route53domains.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("Route53Domains_v20140515.{operation}"),
            );
        if signed {
            request = request.header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/route53domains/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0",
            );
        }
        let request = request
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }
}

/// The awsJson1_1 error code of a failure body: its `__type`, with any namespace prefix
/// (`com.amazonaws.route53domains#InvalidInput`) stripped.
pub(crate) fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    raw.rsplit_once('#')
        .map_or(raw, |(_, name)| name)
        .to_string()
}

/// The `DomainName`s a `ListDomains` body reports, in the order it reported them.
fn domain_names(body: &Value) -> Vec<String> {
    body["Domains"]
        .as_array()
        .map(|domains| {
            domains
                .iter()
                .map(|domain| {
                    domain["DomainName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A contact good enough for a registration: the fields Route 53 asks for, with modelled
/// enum members (`ContactType` -> `PERSON`, `CountryCode` -> `US`).
pub(crate) fn person_contact() -> Value {
    json!({
        "FirstName": "Ada",
        "LastName": "Lovelace",
        "ContactType": "PERSON",
        "AddressLine1": "1 Analytical Engine Way",
        "City": "Seattle",
        "State": "WA",
        "CountryCode": "US",
        "ZipCode": "98109",
        "PhoneNumber": "+1.2065550100",
        "Email": "ada@example.com"
    })
}

/// A `RegisterDomain` body for `name`, holding `years` and the contact `contact`.
pub(crate) fn registration(name: &str, years: i64, contact: &Value) -> Value {
    json!({
        "DomainName": name,
        "DurationInYears": years,
        "AdminContact": contact,
        "RegistrantContact": contact,
        "TechContact": contact
    })
}

/// Register `name` and return the `OperationId`, failing the test if it did not succeed.
pub(crate) async fn register(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call("RegisterDomain", &registration(name, 1, &person_contact()))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected RegisterDomain to succeed, got {status} {body}"
    );
    body["OperationId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CheckDomainAvailability`'s `Availability` for `name`, asserting a 200.
async fn availability(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call("CheckDomainAvailability", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CheckDomainAvailability to succeed, got {status} {body}"
    );
    body["Availability"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The lifecycle the bootstrap issue asks for, end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_register_inspect_list_delete_lifecycle_runs_over_the_wire() {
    let emulator = Emulator::new();
    let name = "example-bootstrap.com";

    assert_eq!(
        availability(&emulator, name).await,
        "AVAILABLE",
        "case {name:?}: an unregistered name must be AVAILABLE before registration"
    );

    let contact = person_contact();
    let (status, registered) = emulator
        .call("RegisterDomain", &registration(name, 1, &contact))
        .await;
    assert_eq!(status, 200, "RegisterDomain: got {status} {registered}");
    let operation_id = registered["OperationId"].as_str().unwrap_or_default();
    assert!(
        !operation_id.is_empty(),
        "RegisterDomain must return a non-empty OperationId, got {registered}"
    );

    // The operation record the registration appended resolves, and is still in its
    // transient status: this call happens microseconds after the one that started it.
    let (status, detail) = emulator
        .call(
            "GetOperationDetail",
            &json!({ "OperationId": operation_id }),
        )
        .await;
    assert_eq!(status, 200, "GetOperationDetail: got {status} {detail}");
    assert_eq!(
        detail["DomainName"].as_str(),
        Some(name),
        "GetOperationDetail must report the stored domain name, got {detail}"
    );
    assert_eq!(
        detail["Type"].as_str(),
        Some("REGISTER_DOMAIN"),
        "GetOperationDetail must report the stored type, got {detail}"
    );
    assert_eq!(
        detail["Status"].as_str(),
        Some("SUBMITTED"),
        "a just-submitted operation must still be SUBMITTED, got {detail}"
    );
    assert!(
        detail["SubmittedDate"].as_f64().is_some(),
        "GetOperationDetail must report SubmittedDate, got {detail}"
    );

    let (status, domain) = emulator
        .call("GetDomainDetail", &json!({ "DomainName": name }))
        .await;
    assert_eq!(status, 200, "GetDomainDetail: got {status} {domain}");
    assert_eq!(
        domain["AdminContact"], contact,
        "GetDomainDetail must return the registered AdminContact unchanged, got {domain}"
    );
    assert_eq!(
        domain["AutoRenew"].as_bool(),
        Some(true),
        "AutoRenew defaults to true, got {domain}"
    );
    assert!(
        domain["Nameservers"]
            .as_array()
            .is_some_and(|servers| servers.len() == 4),
        "a registered domain is delegated to four name servers, got {domain}"
    );
    assert_eq!(
        domain["StatusList"],
        json!(["clientTransferProhibited"]),
        "a newly registered domain carries the transfer lock, got {domain}"
    );

    assert_eq!(
        availability(&emulator, name).await,
        "UNAVAILABLE",
        "case {name:?}: a registered name must be UNAVAILABLE"
    );

    // Registering the same name again is a DuplicateRequest, not a second registration.
    let (status, duplicate) = emulator
        .call("RegisterDomain", &registration(name, 1, &contact))
        .await;
    assert_eq!(
        (status, error_code(&duplicate).as_str()),
        (400, "DuplicateRequest"),
        "re-registering {name:?} must be refused, got {status} {duplicate}"
    );

    // One domain, paged one at a time: listed exactly once, and the pagination stops.
    let listed = list_every_domain(&emulator, Some(1)).await;
    assert_eq!(
        listed,
        vec![name.to_string()],
        "a paginated ListDomains must list the one registered domain exactly once"
    );

    let (status, deleted) = emulator
        .call("DeleteDomain", &json!({ "DomainName": name }))
        .await;
    assert_eq!(status, 200, "DeleteDomain: got {status} {deleted}");
    let delete_operation = deleted["OperationId"].as_str().unwrap_or_default();
    let (status, detail) = emulator
        .call(
            "GetOperationDetail",
            &json!({ "OperationId": delete_operation }),
        )
        .await;
    assert_eq!(status, 200, "GetOperationDetail: got {status} {detail}");
    assert_eq!(
        detail["Type"].as_str(),
        Some("DELETE_DOMAIN"),
        "the deletion's operation record must be a DELETE_DOMAIN, got {detail}"
    );

    let (status, gone) = emulator
        .call("GetDomainDetail", &json!({ "DomainName": name }))
        .await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "InvalidInput"),
        "GetDomainDetail for a deleted domain must be refused, got {status} {gone}"
    );
    assert!(
        list_every_domain(&emulator, None).await.is_empty(),
        "a deleted domain must no longer be listed"
    );
    assert_eq!(
        availability(&emulator, name).await,
        "AVAILABLE",
        "case {name:?}: a deleted name must be AVAILABLE again"
    );
}

/// Walk every page of `ListDomains`, following `NextPageMarker`, and return the names in
/// the order the service reported them. Fails the test if the pagination does not
/// terminate within a bound far above any page count this suite creates.
async fn list_every_domain(emulator: &Emulator, max_items: Option<i64>) -> Vec<String> {
    let mut names = Vec::new();
    let mut marker: Option<String> = None;
    for page in 0..64 {
        let mut request = serde_json::Map::new();
        if let Some(max) = max_items {
            request.insert("MaxItems".to_string(), json!(max));
        }
        if let Some(previous) = &marker {
            request.insert("Marker".to_string(), json!(previous));
        }
        let (status, body) = emulator.call("ListDomains", &Value::Object(request)).await;
        assert_eq!(status, 200, "ListDomains page {page}: got {status} {body}");
        names.extend(domain_names(&body));
        match body["NextPageMarker"].as_str() {
            None => return names,
            Some(next) => marker = Some(next.to_string()),
        }
    }
    panic!("ListDomains pagination did not terminate: collected {names:?}");
}

// ---------------------------------------------------------------------------
// The rule: availability is computed from the domain store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn availability_tracks_the_domain_store_for_every_name() {
    // Rows differ along the TLD, the label and the case of the request, so the assertion
    // spans the class rather than the one name the issue cites. Each row is registered,
    // re-checked and deleted, so the flip is observed in both directions.
    let cases = [
        "example-bootstrap.com",
        "a.org",
        "zero0nine9.io",
        "hyphen-in-middle.xyz",
        "AlsoUpperCase.com",
    ];
    let emulator = Emulator::new();

    for case in cases {
        assert_eq!(
            availability(&emulator, case).await,
            "AVAILABLE",
            "case {case:?}: unregistered, so expected AVAILABLE"
        );
        register(&emulator, case).await;
        assert_eq!(
            availability(&emulator, case).await,
            "UNAVAILABLE",
            "case {case:?}: registered, so expected UNAVAILABLE"
        );
        // Domain names are not case sensitive, so the opposite case must agree.
        let swapped = if case.chars().any(char::is_uppercase) {
            case.to_lowercase()
        } else {
            case.to_uppercase()
        };
        assert_eq!(
            availability(&emulator, &swapped).await,
            "UNAVAILABLE",
            "case {swapped:?}: the same name in another case must agree"
        );

        let (status, body) = emulator
            .call("DeleteDomain", &json!({ "DomainName": case }))
            .await;
        assert_eq!(status, 200, "case {case:?}: DeleteDomain got {status} {body}");
        assert_eq!(
            availability(&emulator, case).await,
            "AVAILABLE",
            "case {case:?}: deleted, so expected AVAILABLE again"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: every operation taking a DomainName resolves it the same way
// ---------------------------------------------------------------------------

/// A domain name whose single label is `label_len` characters, under `.com`. 63 is the
/// longest label DNS allows (RFC 1035 §2.3.4); 251 + `.com` is exactly the 255 characters
/// the `DomainName` member's `@length(max: 255)` allows.
fn name_with_label(label_len: usize) -> String {
    format!("{}.com", "a".repeat(label_len))
}

#[tokio::test]
async fn a_malformed_domain_name_is_invalid_input_everywhere_it_is_accepted() {
    // The documented rule (`DomainName` member doc): letters a-z, digits 0-9, hyphens
    // that are neither the first nor the last character of a label, and `.` separating
    // labels. Rows differ along WHICH part of that rule they break.
    let malformed = [
        "",                    // empty
        "nodot",               // no TLD at all
        "-leading.com",        // hyphen first in a label
        "trailing-.com",       // hyphen last in a label
        "double..dot.com",     // empty label
        "space in.com",        // a character the rule does not allow
        "under_score.com",     // ditto
        "exclaim!.com",        // ditto
        "café.com",            // non-ASCII
        &name_with_label(64),  // one past the 63-octet DNS label bound
        &name_with_label(252), // one past the modelled 255-character name bound
    ];
    // Well formed, but not names a registry delegates: the RFC 2606 reserved names, an
    // unassigned TLD, and a third-level name (a subdomain, which no registrar sells).
    let unsupported = [
        "example.invalid",
        "example.test",
        "example.localhost",
        "example.zzz",
        "a.b.example.com",
    ];
    // TLDs the issue never cites, spanning a legacy gTLD, a new gTLD, a ccTLD used as a
    // gTLD, a two-label suffix and the published DNS label bound.
    let supported = [
        "rule-row.org",
        "rule-row.io",
        "rule-row.co.uk",
        "rule-row.de",
        "rule-row.xyz",
        "rule-row.com.au",
        "rule-row.app",
        "rule-row.dev",
        "rule-row.ai",
        "rule-row.shop",
        &name_with_label(63),
    ];

    let emulator = Emulator::new();
    // The operations that take a `DomainName` and model both refusals.
    let operations = ["CheckDomainAvailability", "GetDomainDetail", "DeleteDomain"];

    for operation in operations {
        for case in malformed {
            let (status, body) = emulator
                .call(operation, &json!({ "DomainName": case }))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "InvalidInput".to_string()),
                "case {operation}/{case:?}: expected 400 InvalidInput, got {status} {body}"
            );
        }
        for case in unsupported {
            let (status, body) = emulator
                .call(operation, &json!({ "DomainName": case }))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "UnsupportedTLD".to_string()),
                "case {operation}/{case:?}: expected 400 UnsupportedTLD, got {status} {body}"
            );
        }
    }

    // A supported TLD resolves: the name is simply not registered yet.
    for case in supported {
        assert_eq!(
            availability(&emulator, case).await,
            "AVAILABLE",
            "case {case:?}: a supported TLD must resolve and report AVAILABLE"
        );
    }

    // RegisterDomain refuses the same two classes with the same two codes.
    let contact = person_contact();
    for case in malformed {
        let (status, body) = emulator
            .call("RegisterDomain", &registration(case, 1, &contact))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case RegisterDomain/{case:?}: expected 400 InvalidInput, got {status} {body}"
        );
    }
    for case in unsupported {
        let (status, body) = emulator
            .call("RegisterDomain", &registration(case, 1, &contact))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "UnsupportedTLD".to_string()),
            "case RegisterDomain/{case:?}: expected 400 UnsupportedTLD, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_label_longer_than_dns_allows_is_refused_as_that_label() {
    // A label over the 63 octets DNS gives one (RFC 1035 §2.3.4) is reported as the
    // offending LABEL, so the rows differ along which label of the name is too long, by
    // how much, and which operation was called — and a name whose longest label is
    // exactly 63 is not refused at all, which the row below the loop pins.
    let longest_allowed = "a".repeat(63);
    let one_too_long = "a".repeat(64);
    let far_too_long = "b".repeat(200);
    let cases: [(&str, String, &str); 4] = [
        (
            "the first label one character too long",
            format!("{one_too_long}.com"),
            &one_too_long,
        ),
        (
            "the first label far too long",
            format!("{far_too_long}.com"),
            &far_too_long,
        ),
        (
            "a later label too long",
            format!("host.{one_too_long}.com"),
            &one_too_long,
        ),
        (
            "the longest allowed label beside a too-long one",
            format!("{longest_allowed}.{one_too_long}.com"),
            &one_too_long,
        ),
    ];
    let emulator = Emulator::new();

    for operation in ["CheckDomainAvailability", "GetDomainDetail", "DeleteDomain"] {
        for (case, name, label) in &cases {
            let (status, body) = emulator
                .call(operation, &json!({ "DomainName": name }))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "InvalidInput".to_string()),
                "case {operation}/{case:?}: expected 400 InvalidInput, got {status} {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("Invalid request. Problems: Domain label too long: [{label}]").as_str()),
                "case {operation}/{case:?}: the refusal names the offending label, got {body}"
            );
        }
    }

    // ...and a name whose label is exactly the longest DNS allows resolves.
    assert_eq!(
        availability(&emulator, &format!("{longest_allowed}.com")).await,
        "AVAILABLE",
        "a label of 63 octets is a name a registry delegates"
    );
}

// ---------------------------------------------------------------------------
// The rule: a name carrying no top-level domain is refused for its label count
// ---------------------------------------------------------------------------

/// The sentence a name of fewer labels than a registrable one is refused with: the
/// service's request-problem envelope plus its own fixed sentence for the missing
/// top-level domain. No variable part — in particular the requested name is not echoed.
const MISSING_TLD_MESSAGE: &str =
    "Invalid request. Problems: Give domain name must contain more than 1 label";

/// A request body for `operation` carrying `name` as the domain name it asks about, with
/// every other member the model marks `@required` and nothing else (smithy_query
/// operation GetDomainSuggestions/RegisterDomain; `GetContactReachabilityStatus` spells
/// its member `domainName`).
fn domain_request(operation: &str, name: &str) -> Value {
    match operation {
        "GetDomainSuggestions" => {
            json!({ "DomainName": name, "SuggestionCount": 5, "OnlyAvailable": true })
        }
        "RegisterDomain" => registration(name, 1, &person_contact()),
        "GetContactReachabilityStatus" => json!({ "domainName": name }),
        _ => json!({ "DomainName": name }),
    }
}

/// Every operation that resolves a request's domain name and models `InvalidInput`.
const DOMAIN_NAME_OPERATIONS: &[&str] = &[
    "CheckDomainAvailability",
    "CheckDomainTransferability",
    "DeleteDomain",
    "DisableDomainAutoRenew",
    "DisableDomainTransferLock",
    "EnableDomainAutoRenew",
    "GetContactReachabilityStatus",
    "GetDomainDetail",
    "GetDomainSuggestions",
    "ListTagsForDomain",
    "RegisterDomain",
    "UpdateTagsForDomain",
];

#[tokio::test]
async fn a_name_with_no_top_level_domain_is_refused_for_its_label_count() {
    // A value of one label is not a name that broke a character rule: the top-level
    // domain it would sit under is MISSING, which the service states in its own words.
    // Rows differ along the shape of the lone label — a word, a single character, digits,
    // interior hyphens, the longest label DNS allows, and a label in mixed case — and
    // along the operation asked, because every domain-scoped operation resolves the name
    // by the one rule.
    let longest_allowed = "a".repeat(63);
    let single_label: [&str; 6] = [
        "example-no-tld",
        "r",
        "9",
        "a-0-b",
        &longest_allowed,
        "ExampleNoTld",
    ];
    let emulator = Emulator::new();

    for operation in DOMAIN_NAME_OPERATIONS {
        for case in single_label {
            let (status, body) = emulator
                .call(operation, &domain_request(operation, case))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "InvalidInput".to_string()),
                "case {operation}/{case:?}: expected 400 InvalidInput, got {status} {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(MISSING_TLD_MESSAGE),
                "case {operation}/{case:?}: the refusal states the missing top-level \
                 domain, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_of_two_labels_or_more_is_never_refused_for_its_label_count() {
    // The same builder keeps its other sentences, so the label-count refusal is reported
    // for that problem alone. Rows differ along what else is wrong with a name that DOES
    // carry a top-level domain: nothing (it resolves), a character the member disallows,
    // a hyphen at a label's edge, an empty label, and a label over the 63 octets DNS
    // gives one — plus a member with no name in it at all, which is refused as blank.
    let one_too_long = "a".repeat(64);
    let cases: [(&str, &str); 6] = [
        ("sim probe.com", "constraint"),
        ("-sim-probe.com", "constraint"),
        ("sim-probe-.com", "constraint"),
        ("sim..probe.com", "constraint"),
        (&one_too_long, "too long"),
        ("", "blank"),
    ];
    let emulator = Emulator::new();

    for operation in DOMAIN_NAME_OPERATIONS {
        for (case, kind) in cases {
            let (status, body) = emulator
                .call(operation, &domain_request(operation, case))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "InvalidInput".to_string()),
                "case {operation}/{case:?}: expected 400 InvalidInput, got {status} {body}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert_ne!(
                message, MISSING_TLD_MESSAGE,
                "case {operation}/{case:?}: a {kind} problem is not the label count, \
                 got {body}"
            );
            let expected = match kind {
                "blank" => "Invalid request. Problems: Domain name cannot be blank".to_string(),
                "too long" => {
                    format!("Invalid request. Problems: Domain label too long: [{case}]")
                }
                _ => format!(
                    "Value '{case}' at 'domainName' failed to satisfy constraint: Member must \
                     be a domain name of one label under a top-level domain, where every label \
                     is made up of letters, digits and interior hyphens"
                ),
            };
            assert_eq!(
                message, expected,
                "case {operation}/{case:?}: expected the {kind} wording, got {body}"
            );
        }
    }

    // ...and a well-formed name of two labels resolves, so the refusal is about the
    // label count and not about every name the rule sees.
    assert_eq!(
        availability(&emulator, "example-with-tld.com").await,
        "AVAILABLE",
        "a name of two labels under a supported TLD must resolve"
    );
}

// ---------------------------------------------------------------------------
// The rule: GetDomainDetail reports the stored record, not the request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_registered_contacts_come_back_unchanged() {
    // Rows differ along the SHAPE of the contact: how many members it carries, whether
    // it is a person or an organisation, whether it carries TLD extra parameters, and
    // whether its text is ASCII.
    let cases: [(&str, Value); 4] = [
        ("empty.com", json!({})),
        ("person.com", person_contact()),
        (
            "company.net",
            json!({
                "FirstName": "Grace",
                "LastName": "Hopper",
                "ContactType": "COMPANY",
                "OrganizationName": "Example Compilers LLC",
                "AddressLine1": "500 Main St",
                "AddressLine2": "Suite 1200",
                "City": "Arlington",
                "State": "VA",
                "CountryCode": "US",
                "ZipCode": "22201",
                "PhoneNumber": "+1.7035550123",
                "Email": "grace@example.com",
                "Fax": "+1.7035550124",
                "ExtraParams": [
                    { "Name": "DUNS_NUMBER", "Value": "123456789" },
                    { "Name": "VAT_NUMBER", "Value": "DE123456789" }
                ]
            }),
        ),
        (
            "unicode.org",
            json!({
                "FirstName": "Zoë",
                "LastName": "Müller",
                "ContactType": "PERSON",
                "City": "München",
                "CountryCode": "DE",
                "Email": "zoe@example.com"
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (name, contact) in &cases {
        let (status, body) = emulator
            .call("RegisterDomain", &registration(name, 1, contact))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: RegisterDomain got {status} {body}"
        );

        let (status, domain) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: GetDomainDetail got {status} {domain}"
        );
        for member in ["AdminContact", "RegistrantContact", "TechContact"] {
            assert_eq!(
                &domain[member], contact,
                "case {name:?}/{member}: expected the registered contact back unchanged, \
                 got {}",
                domain[member]
            );
        }
        // Reading one domain must not disturb another: ask for an unrelated name and
        // then re-read this one.
        let _ = emulator
            .call("GetDomainDetail", &json!({ "DomainName": "person.com" }))
            .await;
        let (_, again) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            again["AdminContact"], *contact,
            "case {name:?}: a second read must report the same contact"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule: ExpirationDate is the registration anniversary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_expiration_date_is_the_registration_anniversary() {
    // `DurationInYears` is modelled `@range(min: 1, max: 10)`: both published bounds are
    // rows, and the values one past each are refused.
    let accepted: [i64; 4] = [1, 2, 5, 10];
    let refused: [i64; 2] = [0, 11];

    let emulator = Emulator::new();
    for years in accepted {
        let name = format!("anniversary-{years}.com");
        let (status, body) = emulator
            .call("RegisterDomain", &registration(&name, years, &person_contact()))
            .await;
        assert_eq!(
            status, 200,
            "case {years} years: RegisterDomain got {status} {body}"
        );

        let (status, domain) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            status, 200,
            "case {years} years: GetDomainDetail got {status} {domain}"
        );
        let created = instant(&domain, "CreationDate");
        let expires = instant(&domain, "ExpirationDate");
        assert_eq!(
            expires.year(),
            created.year() + i32::try_from(years).expect("a year count fits in an i32"),
            "case {years} years: expected the expiry {years} years after {created}, got {expires}"
        );
        assert_eq!(
            expires.month(),
            created.month(),
            "case {years} years: expected the same month as {created}, got {expires}"
        );
        assert!(
            expires.day() == created.day()
                || (created.month() == Month::February
                    && created.day() == 29
                    && expires.day() == 28),
            "case {years} years: expected the same day of month as {created}, got {expires}"
        );
        assert_eq!(
            expires.time(),
            created.time(),
            "case {years} years: expected the same time of day as {created}, got {expires}"
        );
    }

    for years in refused {
        let name = format!("refused-{years}.com");
        let (status, body) = emulator
            .call("RegisterDomain", &registration(&name, years, &person_contact()))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {years} years: outside the modelled 1..10 range, so expected 400 \
             InvalidInput, got {status} {body}"
        );
    }

    // A required contact that is absent is the same refusal.
    let (status, body) = emulator
        .call(
            "RegisterDomain",
            &json!({
                "DomainName": "no-contacts.com",
                "DurationInYears": 1,
                "AdminContact": person_contact()
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "a registration missing RegistrantContact/TechContact must be refused, got \
         {status} {body}"
    );
}

/// A modelled timestamp member of `body`, read as the instant it denotes. awsJson1_1
/// renders a `Timestamp` as epoch seconds.
fn instant(body: &Value, member: &str) -> OffsetDateTime {
    let seconds = body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be an epoch-seconds number, got {body}"));
    #[allow(clippy::cast_possible_truncation)]
    let nanos = (seconds * 1_000_000_000.0) as i128;
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .unwrap_or_else(|err| panic!("{member} must be a valid instant: {err}"))
}

// ---------------------------------------------------------------------------
// The rule: a paginating client sees every domain exactly once and terminates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pagination_covers_the_store_exactly_once_and_terminates() {
    // Rows differ along the collection size and the page size, including a page size
    // that divides the collection exactly, one that does not, and one larger than the
    // whole collection.
    let cases: [(usize, Option<i64>); 7] = [
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (4, Some(4)),
        (4, Some(100)),
        (25, None),
    ];

    for (count, max_items) in cases {
        let emulator = Emulator::new();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..count {
            let name = format!("page-{index:02}.com");
            register(&emulator, &name).await;
            expected.push(name);
        }
        // The store is enumerated in domain-name order when no sort is asked for.
        expected.sort();

        let listed = list_every_domain(&emulator, max_items).await;
        assert_eq!(
            listed, expected,
            "case {count} domains at MaxItems {max_items:?}: expected every domain once, \
             in order, got {listed:?}"
        );
    }

    // The published bound on `MaxItems` is 100; one past it, and a page of zero (which
    // could never make progress), are both refused.
    let emulator = Emulator::new();
    register(&emulator, "bound.com").await;
    for max_items in [0, 101] {
        let (status, body) = emulator
            .call("ListDomains", &json!({ "MaxItems": max_items }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case MaxItems {max_items}: expected 400 InvalidInput, got {status} {body}"
        );
    }
    // A marker no previous response handed out is refused rather than silently ignored.
    let (status, body) = emulator
        .call("ListDomains", &json!({ "Marker": "never-handed-out.com" }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "an unknown Marker must be refused, got {status} {body}"
    );
}

#[tokio::test]
async fn list_domains_filters_and_sorts_on_the_modelled_attributes() {
    let emulator = Emulator::new();
    for name in ["alpha.com", "beta.com", "gamma.org"] {
        register(&emulator, name).await;
    }

    // `FilterCondition`: Name in {DomainName, Expiry}, Operator in {LE, GE, BEGINS_WITH},
    // exactly one value. Rows cover each string operator.
    let cases: [(Value, Vec<&str>); 4] = [
        (
            json!({ "Name": "DomainName", "Operator": "BEGINS_WITH", "Values": ["b"] }),
            vec!["beta.com"],
        ),
        (
            json!({ "Name": "DomainName", "Operator": "GE", "Values": ["beta.com"] }),
            vec!["beta.com", "gamma.org"],
        ),
        (
            json!({ "Name": "DomainName", "Operator": "LE", "Values": ["beta.com"] }),
            vec!["alpha.com", "beta.com"],
        ),
        (
            json!({ "Name": "DomainName", "Operator": "BEGINS_WITH", "Values": ["zz"] }),
            vec![],
        ),
    ];
    for (condition, expected) in &cases {
        let (status, body) = emulator
            .call(
                "ListDomains",
                &json!({ "FilterConditions": [condition], "MaxItems": 100 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {condition}: ListDomains got {status} {body}"
        );
        let names = domain_names(&body);
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            names, expected,
            "case {condition}: expected {expected:?}, got {names:?}"
        );
    }

    // `SortOrder` DESC reverses the order; ASC keeps it.
    for (order, expected) in [
        ("ASC", vec!["alpha.com", "beta.com", "gamma.org"]),
        ("DESC", vec!["gamma.org", "beta.com", "alpha.com"]),
    ] {
        let (status, body) = emulator
            .call(
                "ListDomains",
                &json!({
                    "SortCondition": { "Name": "DomainName", "SortOrder": order },
                    "MaxItems": 100
                }),
            )
            .await;
        assert_eq!(status, 200, "case {order}: ListDomains got {status} {body}");
        let names = domain_names(&body);
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            names, expected,
            "case {order}: expected {expected:?}, got {names:?}"
        );
    }

    // "the field used for sorting has to be the same as the field used for filtering"
    // (`SortCondition$Name`).
    let (status, body) = emulator
        .call(
            "ListDomains",
            &json!({
                "FilterConditions": [
                    { "Name": "DomainName", "Operator": "BEGINS_WITH", "Values": ["a"] }
                ],
                "SortCondition": { "Name": "Expiry", "SortOrder": "ASC" }
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "a sort field that disagrees with the filter field must be refused, got {status} \
         {body}"
    );
}

// ---------------------------------------------------------------------------
// The rule: a mutating call appends an operation record the clock later settles
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_mutating_call_appends_an_operation_record_that_settles() {
    let emulator = Emulator::new();
    let name = "operations.com";

    let register_id = register(&emulator, name).await;
    let (_, enabled) = emulator
        .call("EnableDomainAutoRenew", &json!({ "DomainName": name }))
        .await;
    assert_eq!(enabled, json!({}), "EnableDomainAutoRenew: got {enabled}");
    let (_, disabled) = emulator
        .call("DisableDomainAutoRenew", &json!({ "DomainName": name }))
        .await;
    assert_eq!(disabled, json!({}), "DisableDomainAutoRenew: got {disabled}");
    let (status, deleted) = emulator
        .call("DeleteDomain", &json!({ "DomainName": name }))
        .await;
    assert_eq!(status, 200, "DeleteDomain: got {status} {deleted}");

    // Each call appended exactly one record, in submission order, each with its own id.
    let (status, body) = emulator
        .call("ListOperations", &json!({ "MaxItems": 100 }))
        .await;
    assert_eq!(status, 200, "ListOperations: got {status} {body}");
    let records = body["Operations"].as_array().cloned().unwrap_or_default();
    let types: Vec<&str> = records
        .iter()
        .map(|record| record["Type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        types,
        vec![
            "REGISTER_DOMAIN",
            "ENABLE_AUTORENEW",
            "DISABLE_AUTORENEW",
            "DELETE_DOMAIN"
        ],
        "every mutating call must append one record in submission order, got {body}"
    );
    let mut ids: Vec<&str> = records
        .iter()
        .map(|record| record["OperationId"].as_str().unwrap_or_default())
        .collect();
    let minted = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "every minted OperationId must be unique, got {body}"
    );

    // The clock owes the move out of SUBMITTED a short, non-zero span after the call,
    // and a reporting call materialises it once it is due.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (status, settled) = emulator
        .call(
            "GetOperationDetail",
            &json!({ "OperationId": register_id }),
        )
        .await;
    assert_eq!(status, 200, "GetOperationDetail: got {status} {settled}");
    assert_eq!(
        settled["Status"].as_str(),
        Some("SUCCESSFUL"),
        "the clock owed this operation's completion, so expected SUCCESSFUL, got {settled}"
    );
    let submitted = settled["SubmittedDate"].as_f64().unwrap_or_default();
    let updated = settled["LastUpdatedDate"].as_f64().unwrap_or_default();
    assert!(
        updated >= submitted,
        "LastUpdatedDate must not precede SubmittedDate, got {settled}"
    );

    // An id no record carries is refused, never answered with a fabricated success.
    for unknown in [
        "00000000-0000-4000-8000-000000000000",
        "not-a-uuid",
        &register_id.to_uppercase(),
    ] {
        let (status, body) = emulator
            .call("GetOperationDetail", &json!({ "OperationId": unknown }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {unknown:?}: an unknown OperationId must be refused, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn auto_renew_round_trips_through_the_stored_domain() {
    // Rows: the two auto-renew operations, and the flag each one leaves behind.
    let cases = [("DisableDomainAutoRenew", false), ("EnableDomainAutoRenew", true)];
    let emulator = Emulator::new();
    let name = "auto-renew.com";
    register(&emulator, name).await;

    for (operation, expected) in cases {
        let (status, body) = emulator
            .call(operation, &json!({ "DomainName": name }))
            .await;
        assert_eq!(status, 200, "case {operation}: got {status} {body}");

        let (_, domain) = emulator
            .call("GetDomainDetail", &json!({ "DomainName": name }))
            .await;
        assert_eq!(
            domain["AutoRenew"].as_bool(),
            Some(expected),
            "case {operation}: expected AutoRenew {expected}, got {domain}"
        );
        let (_, listed) = emulator.call("ListDomains", &json!({})).await;
        assert_eq!(
            listed["Domains"][0]["AutoRenew"].as_bool(),
            Some(expected),
            "case {operation}: ListDomains must agree with GetDomainDetail, got {listed}"
        );
    }

    // Both refuse a name this account does not hold.
    for operation in ["EnableDomainAutoRenew", "DisableDomainAutoRenew"] {
        let (status, body) = emulator
            .call(operation, &json!({ "DomainName": "not-registered.com" }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {operation}: an unheld domain must be refused, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_unsigned_request_routes_by_its_x_amz_target() {
    // The framework resolves a routing key from the SigV4 credential scope first and
    // falls back to the `X-Amz-Target` prefix, which for this service is
    // `Route53Domains_v20140515`. Both spellings must reach this service.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .send(
            "CheckDomainAvailability",
            &json!({ "DomainName": "unsigned.com" }),
            false,
        )
        .await;
    assert_eq!(
        status, 200,
        "an unsigned awsJson request must still route to this service, got {status} {body}"
    );
    assert_eq!(
        body["Availability"].as_str(),
        Some("AVAILABLE"),
        "got {body}"
    );
}

// ---------------------------------------------------------------------------
// The rule: a caller-driven store never grows past its stated ceiling
// ---------------------------------------------------------------------------

/// This emulator's stated ceilings (`crate::state::MAX_DOMAINS` /
/// `MAX_OPERATION_RECORDS`), restated here so the test fails loudly if they move.
const MAX_DOMAINS: usize = 200;
const MAX_OPERATION_RECORDS: usize = 1000;

#[tokio::test]
async fn neither_caller_driven_store_grows_past_its_ceiling() {
    let emulator = Emulator::new();

    // Fill the domain store to its ceiling. Every registration up to it succeeds...
    let mut first_operation = String::new();
    for index in 0..MAX_DOMAINS {
        let id = register(&emulator, &format!("limit-{index:04}.com")).await;
        if index == 0 {
            first_operation = id;
        }
    }
    // ...and the one past it is refused with the modelled quota error, not accepted.
    let (status, body) = emulator
        .call(
            "RegisterDomain",
            &registration("one-too-many.com", 1, &person_contact()),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "DomainLimitExceeded".to_string()),
        "case {MAX_DOMAINS} domains held: expected 400 DomainLimitExceeded, got {status} {body}"
    );
    // The refused registration left no trace: the name is still available.
    assert_eq!(
        availability(&emulator, "one-too-many.com").await,
        "AVAILABLE",
        "a refused registration must not have been stored"
    );

    // The operation log is append-only from a caller's point of view, so it is capped by
    // dropping its oldest records. Drive it past the cap with a call that mutates
    // nothing, which is the cheapest way a client can append.
    let oldest_still_held = emulator
        .call(
            "GetOperationDetail",
            &json!({ "OperationId": first_operation }),
        )
        .await;
    assert_eq!(
        oldest_still_held.0, 200,
        "the first operation is still inside the cap here, got {oldest_still_held:?}"
    );
    for _ in 0..=MAX_OPERATION_RECORDS {
        let (status, body) = emulator
            .call(
                "DisableDomainAutoRenew",
                &json!({ "DomainName": "limit-0000.com" }),
            )
            .await;
        assert_eq!(status, 200, "DisableDomainAutoRenew: got {status} {body}");
    }

    // Walking every page can never yield more than the cap...
    let mut seen = 0usize;
    let mut marker: Option<String> = None;
    loop {
        let mut request = serde_json::Map::new();
        request.insert("MaxItems".to_string(), json!(100));
        if let Some(previous) = &marker {
            request.insert("Marker".to_string(), json!(previous));
        }
        let (status, body) = emulator
            .call("ListOperations", &Value::Object(request))
            .await;
        assert_eq!(status, 200, "ListOperations: got {status} {body}");
        seen += body["Operations"].as_array().map_or(0, Vec::len);
        match body["NextPageMarker"].as_str() {
            None => break,
            Some(next) => marker = Some(next.to_string()),
        }
    }
    assert_eq!(
        seen, MAX_OPERATION_RECORDS,
        "the operation log must hold at most {MAX_OPERATION_RECORDS} records, saw {seen}"
    );

    // ...and the evicted record is gone rather than silently answered.
    let (status, body) = emulator
        .call(
            "GetOperationDetail",
            &json!({ "OperationId": first_operation }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInput".to_string()),
        "an evicted OperationId must be refused, got {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// The rule: an operation id that names no record is a problem with the REQUEST
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_operation_id_that_names_no_record_is_refused_as_a_request_problem() {
    let emulator = Emulator::new();
    // One id this account really did submit, so the rows that must be refused are refused
    // for naming no record rather than because nothing was ever submitted.
    let issued = register(&emulator, "operation-id-probe.com").await;

    // Rows differ along WHY the id names no record: a well-formed id never issued, an id
    // in a form this service never mints, an issued id in another case (ids are matched
    // exactly), and an id past the one the store holds. The model admits the empty string
    // (`OperationId` is `@length(0, 255)`), which carries no id to look up at all and is
    // reported as an invalid id instead; a blank-looking id of spaces is the same
    // condition.
    let never_issued = "00000000-0000-4000-8000-000000000000";
    let lookup_misses = [
        ("a well-formed id never issued", never_issued.to_string()),
        ("an id in no form this service mints", "not-a-uuid".to_string()),
        ("an issued id in another case", issued.to_uppercase()),
        ("an id with its last character dropped", issued[..issued.len() - 1].to_string()),
    ];
    for (case, operation_id) in lookup_misses {
        let (status, body) = emulator
            .call("GetOperationDetail", &json!({ "OperationId": operation_id }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!("Invalid request. Problems: No operation found for operationId : {operation_id}")
                    .as_str()
            ),
            "case {case:?}: a lookup miss names the id it missed on, got {body}"
        );
    }

    for (case, operation_id) in [("an empty id", ""), ("an id of spaces", "   ")] {
        let (status, body) = emulator
            .call("GetOperationDetail", &json!({ "OperationId": operation_id }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInput".to_string()),
            "case {case:?}: expected 400 InvalidInput, got {status} {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid request. Problems: Invalid OperationId"),
            "case {case:?}: an id with nothing in it is refused as invalid, got {body}"
        );
    }

    // ...and the id this account did submit still reads back, so none of the above is a
    // refusal of every id.
    let (status, detail) = emulator
        .call("GetOperationDetail", &json!({ "OperationId": issued }))
        .await;
    assert_eq!(
        (status, detail["OperationId"].as_str()),
        (200, Some(issued.as_str())),
        "the issued OperationId must still be reported, got {status} {detail}"
    );
}
