// SPDX-License-Identifier: CC0-1.0

//! Eight of this service's refusal `Message` RULES, each asserted over a TABLE
//! whose rows differ along what the rule generalises over. The sibling suite
//! `error_message_rules.rs` pins the duplicate, not-found, state-conflict and
//! `ValidationException` families; these eight are the ones it does not cover.
//!
//! Every one of them is the same requirement seen from a different operation: a
//! refusal's `Message` is the text the SERVICE states for that error class — its fixed
//! part word for word, and its variable parts (a member name, a deciding member's value,
//! a resource identifier) filled from the request or from this service's state. A
//! sentence that only describes the rule in this emulator's own words is a defect even
//! when the status and the error code are right, because a client that matches or logs
//! the service's refusal no longer recognises it.
//!
//! 1. A documented either/or member pair ("specify exactly one of A and B") is violated
//!    by naming NEITHER member, or — where the pair's text is fixed for the condition —
//!    by naming BOTH, and the refusal states the half the request actually broke: one
//!    wording per condition, shared by every pair and every operation that resolves one.
//!    The rows differ along the member pair, the operation (create and update), which
//!    half of the rule was broken and which other members the request carried. (The
//!    agreement pair's both-half is NOT a row here: it reports the two configurations the
//!    request carried, which `agreement_directory_conflict_message.rs` pins.)
//! 2. An access operation addressed at a server whose `IdentityProviderType` is not the
//!    directory-backed one answers a text fixed for THAT OPERATION and that server's
//!    modelled type, carrying nothing the caller sent — so two requests refused for the
//!    same condition read identically, and the three operations are told apart by the
//!    action the text names. The rows span the whole non-directory half of the
//!    `IdentityProviderType` value set, the three implemented access operations, and two
//!    differently shaped `ExternalId`s.
//! 3. A `NextToken` this service did not issue answers ONE fixed text, whatever was
//!    being listed: the diagnostic belongs to the shared pagination-token codec, not to
//!    a listing. The rows span ten `@paginated` operations and a table of forged tokens
//!    differing in form and length, including both published bounds of `NextToken`'s
//!    modelled `len 1..6144`.
//! 4. A write of a web app the service could not carry out because another write of the
//!    same web app was in flight answers ONE fixed sentence with no slot at all, naming
//!    the write that could not be done. The rows differ along which members the writers
//!    carry, how many overlap and which of two web apps is written; a writer that
//!    arrives after the winner was answered overlaps nothing and is served.
//! 5. The both-half of the connector configuration either/or, over the operations and the
//!    stored configurations rule 1 does not reach: an update of a connector of each kind,
//!    and a create carrying each of the other members.
//! 6. A `DeleteWorkflow` of a workflow a server still attaches answers a fixed sentence
//!    plus ONE variable part read from this service's STATE — the referencing server's id
//!    — since the request names only the workflow. The rows differ along the trigger the
//!    workflow is attached through, how many servers attach it and which workflow is
//!    addressed.
//! 7. A server configuration whose `IdentityProviderType` makes `IdentityProviderDetails`
//!    required and that carries none answers a fixed sentence plus ONE variable part read
//!    from the REQUEST — the identity-provider type. The rows span every
//!    `IdentityProviderType` value that requires the details, crossed with the other
//!    members the request carries and with both operations that resolve a server
//!    configuration.
//! 8. Rule 7 one level up, over the WHOLE set of refusals one server configuration can
//!    earn: each condition states its OWN sentence, so the three defects a `CreateServer`
//!    is refused for (absent identity-provider details, absent VPC endpoint settings, a
//!    protocol family the requested provider cannot serve) are mutually distinguishable
//!    by their text alone — they all share one error code — and a refused configuration
//!    writes nothing, whichever of them it offends.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_transfer_transferservice::state as st;

use crate::harness::{drain_pages, member_of, Answer, Emulator, ROLE};
use crate::protocol_family_gate::family_gate_message;
use crate::web_app_customization_write_guard::{create_web_app, futures_of};

/// Satisfies `DirectoryId`'s modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`.
const DIRECTORY_ID: &str = "d-0123456789";

/// Satisfies `Function`'s modelled `len 1..170`, pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_IDP: &str = "arn:aws:lambda:us-east-1:000000000000:function:idp";

/// Group SIDs satisfying `ExternalId`'s modelled `len 1..256`, pattern `^S-1-[\d-]+$`:
/// the form `Get-ADGroup` reports, and the shortest form the pattern admits.
const EXTERNAL_IDS: [&str; 2] = ["S-1-5-21-1234567890-123456789-1234567890-1234", "S-1-0"];

/// The `Message` of an answer, or `""` when it carries none.
fn message_of(answer: &Answer) -> String {
    member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert `answer` is HTTP `status` with error `code` and exactly `expected` as its
/// `Message`, and that none of `absent` — the values the request carried — reached the
/// text. Returns the message so a caller can compare it across rows.
fn assert_message(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    expected: &str,
    absent: &[&str],
) -> String {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected {code}, got body {}",
        answer.body
    );
    let actual = message_of(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the fixed text {expected:?}, got {actual:?}"
    );
    for fragment in absent {
        assert!(
            !actual.contains(fragment),
            "case {case:?}: expected no request data in Message, got {actual:?} carrying \
             {fragment:?}"
        );
    }
    actual
}

// ---------------------------------------------------------------------------
// Rule 1: each half of an either/or requirement is reported as the condition it is
// ---------------------------------------------------------------------------

/// `As2ConnectorConfig.LocalProfileId` / `PartnerProfileId` satisfy `ProfileId`'s
/// modelled `len 19..19`, pattern `^p-([0-9a-f]{17})$`.
const LOCAL_PROFILE: &str = "p-11111111111111111";

/// RULE: a request that names NEITHER member of a documented either/or pair is told that
/// one of the two is needed; one that names BOTH the connector configurations is told that
/// only one of the two may be sent. The text follows the condition reported, not the whole
/// rule, and names only the two members.
///
/// The rows differ along the member pair (`As2Config`/`SftpConfig` on a connector,
/// `BaseDirectory`/`CustomDirectories` on an agreement), the operation that resolves it
/// (`CreateConnector`, `UpdateConnector`, `CreateAgreement`), which half of the rule was
/// broken, and the other members the request carried. The agreement pair's both-half
/// belongs to `agreement_directory_conflict_message.rs`, which pins the two
/// configurations it reports.
#[tokio::test]
async fn an_either_or_pair_reports_the_half_of_the_rule_the_request_broke() {
    let emulator = Emulator::new();

    // A stored SFTP connector, so an UPDATE can break the same pair a create does.
    let created = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the stored connector must be created, got {} with body {}",
        created.status, created.body
    );
    let connector_id = created
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnector answered no ConnectorId: {}", created.body))
        .to_string();

    // One server and two profiles, so the agreement rows reference resolvable resources
    // and are refused for their directory configuration alone.
    let server_id = emulator.create_server(json!({})).await;
    let local_profile = emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let partner_profile = emulator
        .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
        .await;
    let agreement_base = json!({
        "ServerId": &server_id,
        "LocalProfileId": &local_profile,
        "PartnerProfileId": &partner_profile,
        "AccessRole": ROLE,
    });

    /// The members a row adds to its operation's base request.
    fn with(base: &Value, extra: Value) -> Value {
        let mut body = base.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            object.insert(member.clone(), value.clone());
        }
        body
    }

    let connector_base = json!({"AccessRole": ROLE});
    let update_base = json!({"ConnectorId": &connector_id});

    // (case, operation, request, the expected Message)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "CreateConnector naming neither configuration, with a Url",
            "CreateConnector",
            with(
                &connector_base,
                json!({"Url": "sftp://partner.example.com"}),
            ),
            "One of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "CreateConnector naming neither configuration and no Url either",
            "CreateConnector",
            connector_base.clone(),
            "One of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "CreateConnector naming both configurations",
            "CreateConnector",
            with(
                &connector_base,
                json!({
                    "Url": "https://partner.example.com/as2",
                    "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                    "SftpConfig": {"UserSecretId": "sftp"},
                }),
            ),
            "Only one of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "UpdateConnector adding an As2Config to a stored SFTP connector",
            "UpdateConnector",
            with(
                &update_base,
                json!({"As2Config": {"LocalProfileId": LOCAL_PROFILE}}),
            ),
            "Only one of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "CreateAgreement naming neither directory configuration",
            "CreateAgreement",
            agreement_base.clone(),
            "One of BaseDirectory or CustomDirectories must be specified".to_string(),
        ),
        (
            "CreateAgreement whose BaseDirectory is the blank value its pattern admits, \
             which names no directory",
            "CreateAgreement",
            with(&agreement_base, json!({"BaseDirectory": ""})),
            "One of BaseDirectory or CustomDirectories must be specified".to_string(),
        ),
        // The OTHER half on the agreement pair — a request naming BOTH directory
        // configurations — reports the two configurations it carried rather than a text
        // fixed for the condition, so it is not a row of this rule:
        // `agreement_directory_conflict_message.rs` pins it over a table of conflicts.
    ];

    for (case, op, request, expected) in cases {
        let answer = emulator.call(op, request).await;
        // 400 is `InvalidRequestException`'s modelled `@httpError`.
        assert_message(
            case,
            &answer,
            400,
            "InvalidRequestException",
            &expected,
            &[
                ROLE,
                connector_id.as_str(),
                server_id.as_str(),
                local_profile.as_str(),
                partner_profile.as_str(),
                LOCAL_PROFILE,
            ],
        );
    }

    // The two halves are DIFFERENT texts: the condition reported is what the caller has
    // to act on.
    let neither = emulator
        .call("CreateConnector", json!({"AccessRole": ROLE}))
        .await;
    let both = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
        )
        .await;
    assert_ne!(
        message_of(&neither),
        message_of(&both),
        "case \"the two halves of one either/or rule\": expected the two conditions to be \
         worded differently, both are {:?}",
        message_of(&neither)
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the access directory-provider refusal is fixed per operation and type
// ---------------------------------------------------------------------------

/// Every `IdentityProviderType` an access cannot be addressed on, with the
/// `IdentityProviderDetails` that type requires. The value set is
/// `[SERVICE_MANAGED, API_GATEWAY, AWS_DIRECTORY_SERVICE, AWS_LAMBDA]` per
/// `smithy_query(query="shape",shape="IdentityProviderType")`; this is all of it except
/// the directory-backed one, plus the request that names no type at all and so resolves
/// to the documented default.
fn non_directory_servers() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "no IdentityProviderType at all",
            json!({}),
            "SERVICE_MANAGED",
        ),
        (
            "SERVICE_MANAGED",
            json!({"IdentityProviderType": "SERVICE_MANAGED"}),
            "SERVICE_MANAGED",
        ),
        (
            "API_GATEWAY",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
            "API_GATEWAY",
        ),
        (
            "AWS_LAMBDA",
            json!({
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
            }),
            "AWS_LAMBDA",
        ),
    ]
}

/// RULE: an access operation addressed at a server with no directory is refused 400
/// `InvalidRequestException` whose `Message` is fixed for that OPERATION and the server's
/// modelled `IdentityProviderType` — carrying neither the `ServerId` nor the `ExternalId`
/// the request sent, so the same condition always reads the same.
///
/// The rows differ along the operation (the three implemented access operations), the
/// server's identity-provider type (the whole non-directory half of the value set,
/// including the absent member that resolves to the default), the shape of the
/// `ExternalId`, and whether that `ExternalId` was ever granted anywhere.
#[tokio::test]
async fn the_access_directory_refusal_is_fixed_per_operation_and_server_type() {
    let emulator = Emulator::new();

    // (operation, the action its text names)
    let operations: [(&str, &str); 3] = [
        ("CreateAccess", "create"),
        ("DescribeAccess", "describe"),
        ("DeleteAccess", "delete"),
    ];

    for (configuration, server_request, idp_type) in non_directory_servers() {
        let server_id = emulator.create_server(server_request).await;
        for (op, action) in operations {
            let mut seen: Vec<String> = Vec::new();
            for external_id in EXTERNAL_IDS {
                let mut request = json!({"ServerId": &server_id, "ExternalId": external_id});
                if op == "CreateAccess" {
                    request["Role"] = json!(ROLE);
                }
                let case = format!("{op} on {configuration} for {external_id}");
                let answer = emulator.call(op, request).await;
                seen.push(assert_message(
                    &case,
                    &answer,
                    400,
                    "InvalidRequestException",
                    &format!("Cannot {action} access on server with IdP type: {idp_type}"),
                    &[server_id.as_str(), external_id, ROLE],
                ));
            }
            assert!(
                seen.windows(2).all(|pair| pair[0] == pair[1]),
                "case {op:?} on {configuration:?}: expected one text for the condition, got \
                 {seen:?}"
            );
        }

        // Nothing was written by any of the refusals.
        assert!(
            emulator.store().lock().accesses.is_empty(),
            "case {configuration:?}: a refused access operation must persist nothing"
        );
    }

    // The three operations are told APART by the action their text names.
    let server_id = emulator.create_server(json!({})).await;
    let mut texts: Vec<String> = Vec::new();
    for (op, _) in operations {
        let mut request = json!({"ServerId": &server_id, "ExternalId": EXTERNAL_IDS[0]});
        if op == "CreateAccess" {
            request["Role"] = json!(ROLE);
        }
        texts.push(message_of(&emulator.call(op, request).await));
    }
    for (index, text) in texts.iter().enumerate() {
        for other in texts.iter().skip(index + 1) {
            assert_ne!(
                text, other,
                "case \"one refusal per access operation\": expected each operation's text to \
                 differ, got {texts:?}"
            );
        }
    }

    // And the gate refuses only the servers that have no directory: on the
    // directory-backed one all three operations are served, so the grant round-trips.
    let directory_server = emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
        }))
        .await;
    for external_id in EXTERNAL_IDS {
        for (op, request) in [
            (
                "CreateAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id, "Role": ROLE}),
            ),
            (
                "DescribeAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id}),
            ),
            (
                "DeleteAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id}),
            ),
        ] {
            let answer = emulator.call(op, request).await;
            assert_eq!(
                answer.status, 200,
                "case \"{op} on the directory-backed server for {external_id}\": expected 200, \
                 got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one fixed text for a `NextToken` the service did not issue
// ---------------------------------------------------------------------------

/// The longest `NextToken` the model admits (`len 1..6144`), in a form the token codec
/// cannot have produced.
fn longest_forged_token() -> String {
    "z".repeat(6144)
}

/// RULE: a `NextToken` this service did not issue is refused 400
/// `InvalidNextTokenException` with ONE fixed text, the same for every listing and every
/// forged token, and the rejected token is not echoed back.
///
/// The rows differ along the operation (ten of this service's `@paginated` listings,
/// including the two scoped to a server and the one scoped to a resource ARN) and the
/// form and length of the forged token: not hex at all, odd-length hex, hex that decodes
/// to bytes that are not text, and both published bounds of the modelled `len 1..6144`.
#[tokio::test]
async fn a_forged_next_token_is_refused_with_one_fixed_text_for_every_listing() {
    let emulator = Emulator::new();

    // A server, a user and a connector, so the listings have something to page and the
    // server-scoped rows address a server that exists.
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "alice").await;
    emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    let server_arn = member_of(described.get("Server"), "Arn")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeServer answered no Arn: {}", described.body))
        .to_string();

    // (operation, the request members other than the paging ones)
    let listings: Vec<(&str, Value)> = vec![
        ("ListProfiles", json!({})),
        ("ListServers", json!({})),
        ("ListConnectors", json!({})),
        ("ListWorkflows", json!({})),
        ("ListWebApps", json!({})),
        ("ListCertificates", json!({})),
        ("ListSecurityPolicies", json!({})),
        ("ListUsers", json!({"ServerId": &server_id})),
        ("ListHostKeys", json!({"ServerId": &server_id})),
        ("ListTagsForResource", json!({"Arn": &server_arn})),
    ];

    let longest = longest_forged_token();
    // (what the token is, the token). None of them is a value the codec issues: it emits
    // the hex of a stored key, which is even-length hex decoding to text.
    let tokens: Vec<(&str, &str)> = vec![
        ("not hex at all", "not-a-token"),
        ("one character, the modelled minimum length", "z"),
        ("odd-length hex", "abc"),
        ("hex that decodes to bytes that are not text", "c0ffee00"),
        ("the shortest such hex", "ff"),
        ("the modelled maximum length", longest.as_str()),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (op, base) in &listings {
        for (shape, token) in &tokens {
            let mut request = (*base).clone();
            request["NextToken"] = json!(token);
            let case = format!("{op} with a NextToken that is {shape}");
            let answer = emulator.call(op, request).await;
            let message = assert_message(
                &case,
                &answer,
                400,
                "InvalidNextTokenException",
                "Invalid nextToken",
                &[token, server_id.as_str(), server_arn.as_str()],
            );
            seen.push((case, message));
        }
    }

    for (case, message) in &seen {
        assert_eq!(
            *message, seen[0].1,
            "case {case:?}: expected the one token diagnostic {:?}, got {message:?}",
            seen[0].1
        );
    }

    // A listing whose token the service DID issue still pages, so the refusal is about
    // the token and not about sending one.
    emulator.create_user(&server_id, "bob").await;
    let first = emulator
        .call("ListUsers", json!({"ServerId": &server_id, "MaxResults": 1}))
        .await;
    let issued = first
        .get("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("ListUsers answered no NextToken: {}", first.body))
        .to_string();
    let second = emulator
        .call(
            "ListUsers",
            json!({"ServerId": &server_id, "MaxResults": 1, "NextToken": issued}),
        )
        .await;
    assert_eq!(
        second.status, 200,
        "case \"a NextToken the service issued\": expected 200, got {} with body {}",
        second.status, second.body
    );
}

// ---------------------------------------------------------------------------
// Rule 4: one sentence for a web-app write the service could not carry out
// ---------------------------------------------------------------------------

/// The sentence a web-app update that could not be carried out is refused with, written
/// out as a literal rather than read off the crate under test, so the WORDING is what
/// these rows are held to. It is the same `Unable to <verb> WebApp …` sentence the two
/// writes of a web app's CUSTOMIZATION carry (`web_app_customization_conflict_message.rs`
/// pins those), with no sub-resource to name.
const WEB_APP_UPDATE_CONFLICT: &str = "Unable to update WebApp";

/// RULE: an `UpdateWebApp` the service could not carry out because another update of the
/// same web app was in flight is refused 409 `ConflictException` with that ONE sentence,
/// whatever the update carried and whichever web app it addressed.
#[tokio::test]
async fn a_web_app_update_that_lost_carries_the_services_conflict_sentence() {
    // (case, the members each racing writer carries beyond `WebAppId`)
    let rows: Vec<(&str, Vec<Value>)> = vec![
        (
            "two writers assigning different access endpoints",
            vec![
                json!({"AccessEndpoint": "https://racer-0.example.com"}),
                json!({"AccessEndpoint": "https://racer-1.example.com"}),
            ],
        ),
        (
            "two writers assigning different unit counts",
            vec![
                json!({"WebAppUnits": {"Provisioned": 2}}),
                json!({"WebAppUnits": {"Provisioned": 7}}),
            ],
        ),
        (
            "a writer carrying both members against one carrying nothing but the id",
            vec![
                json!({"AccessEndpoint": "https://both.example.com",
                       "WebAppUnits": {"Provisioned": 3}}),
                json!({}),
            ],
        ),
        (
            "three writers at once",
            vec![
                json!({"AccessEndpoint": "https://three-0.example.com"}),
                json!({"AccessEndpoint": "https://three-1.example.com"}),
                json!({"WebAppUnits": {"Provisioned": 5}}),
            ],
        ),
        (
            "five writers at once",
            vec![
                json!({"AccessEndpoint": "https://five-0.example.com"}),
                json!({"AccessEndpoint": "https://five-1.example.com"}),
                json!({"AccessEndpoint": "https://five-2.example.com"}),
                json!({"WebAppUnits": {"Provisioned": 9}}),
                json!({}),
            ],
        ),
    ];

    for (case, writers) in rows {
        let emulator = Emulator::new();
        // A SECOND web app, created first, so the one under test is not the only one the
        // store holds: a sentence that named the web app it was about would differ
        // between the two, and these rows would catch it.
        let other = create_web_app(&emulator).await;
        let web_app_id = create_web_app(&emulator).await;
        assert_ne!(
            other, web_app_id,
            "case {case:?}: the two web apps must have different ids"
        );

        // Every value the requests carried, none of which may reach the refusal.
        let mut submitted: Vec<String> = vec![web_app_id.clone(), other.clone()];
        for members in &writers {
            if let Some(endpoint) = member_of(members, "AccessEndpoint").as_str() {
                submitted.push(endpoint.to_string());
            }
        }
        let absent: Vec<&str> = submitted.iter().map(String::as_str).collect();

        let calls = writers.iter().map(|members| {
            let mut request = members.clone();
            request["WebAppId"] = json!(&web_app_id);
            emulator.call("UpdateWebApp", request)
        });
        let answers = futures_of(calls).await;

        let served = answers.iter().filter(|answer| answer.status == 200).count();
        assert_eq!(
            served,
            1,
            "case {case:?}: expected exactly one winning update, got the statuses {:?}",
            answers
                .iter()
                .map(|answer| answer.status)
                .collect::<Vec<_>>()
        );
        let mut refused = 0;
        for (index, answer) in answers.iter().enumerate() {
            if answer.status == 200 {
                continue;
            }
            // 409 is `ConflictException`'s modelled `@httpError` for `UpdateWebApp`.
            assert_message(
                &format!("{case} (writer {index} lost)"),
                answer,
                409,
                "ConflictException",
                WEB_APP_UPDATE_CONFLICT,
                &absent,
            );
            refused += 1;
        }
        assert_eq!(
            refused,
            writers.len() - 1,
            "case {case:?}: every writer but the winner must be refused"
        );

        // A writer that arrives AFTER the winner was answered overlaps nothing, so it is
        // served: the sentence above belongs to the condition two overlapping calls
        // produce, and the claim is released with the write that took it.
        let late = emulator
            .call(
                "UpdateWebApp",
                json!({"WebAppId": &web_app_id, "AccessEndpoint": "https://late.example.com"}),
            )
            .await;
        assert_eq!(
            late.status, 200,
            "case {case:?}: expected the writer that arrived after the winner's answer to be \
             served, got {} with body {}",
            late.status, late.body
        );

        // The OTHER web app was never written, so an update of its own is served: the
        // refusals above are about the web app that was applying one, not a mode the
        // service fell into.
        let untouched = emulator
            .call(
                "UpdateWebApp",
                json!({"WebAppId": &other, "AccessEndpoint": "https://other.example.com"}),
            )
            .await;
        assert_eq!(
            untouched.status, 200,
            "case {case:?}: expected the untouched web app's update served, got {} with \
             body {}",
            untouched.status, untouched.body
        );
    }
}

/// RULE (the same class, told apart): the sentence names the WRITE that could not be
/// carried out, so an update of the web app itself and a write of its customization are
/// different sentences for the one condition.
#[tokio::test]
async fn the_conflict_sentence_names_the_write_that_could_not_be_carried_out() {
    let emulator = Emulator::new();
    let web_app_id = create_web_app(&emulator).await;

    // Two updates of the web app ITSELF, overlapping: the loser carries the sentence for
    // a write of the web app.
    let updates: Vec<Value> = vec![
        json!({"AccessEndpoint": "https://first.example.com"}),
        json!({"AccessEndpoint": "https://second.example.com"}),
    ];
    let calls = updates.iter().map(|members| {
        let mut request = members.clone();
        request["WebAppId"] = json!(&web_app_id);
        emulator.call("UpdateWebApp", request)
    });
    let answers = futures_of(calls).await;
    let update_losers: Vec<&Answer> = answers
        .iter()
        .filter(|answer| answer.status != 200)
        .collect();
    assert_eq!(
        update_losers.len(),
        1,
        "expected exactly one losing update, got the statuses {:?}",
        answers
            .iter()
            .map(|answer| answer.status)
            .collect::<Vec<_>>()
    );
    let update_text = assert_message(
        "an update of the web app itself",
        update_losers[0],
        409,
        "ConflictException",
        WEB_APP_UPDATE_CONFLICT,
        &[web_app_id.as_str()],
    );

    // The two writes of its CUSTOMIZATION take a claim of their own, so of two that
    // overlap one loses — with the sentence for the write that lost, which is not the
    // one above.
    let customization: Vec<(&str, Value)> = vec![
        ("UpdateWebAppCustomization", json!({"Title": "assigned"})),
        ("DeleteWebAppCustomization", json!({})),
    ];
    let calls = customization.iter().map(|(op, members)| {
        let mut request = members.clone();
        request["WebAppId"] = json!(&web_app_id);
        emulator.call(op, request)
    });
    let answers = futures_of(calls).await;
    let losers: Vec<String> = answers
        .iter()
        .filter(|answer| answer.status != 200)
        .map(message_of)
        .collect();
    assert_eq!(
        losers.len(),
        1,
        "expected exactly one losing customization write, got the statuses {:?}",
        answers
            .iter()
            .map(|answer| answer.status)
            .collect::<Vec<_>>()
    );
    assert_ne!(
        losers[0], update_text,
        "case \"an update of the web app against a write of its customization\": expected \
         each write's own sentence, both are {update_text:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: naming both connector configurations, over the rest of the class
// ---------------------------------------------------------------------------

/// The sentence a request naming BOTH alternative configurations is refused with, and the
/// sibling sentence for naming NEITHER: two parallel statements of the one either/or over
/// the same two member names, told apart by which half the request broke.
const BOTH_CONFIGS: &str = "Only one of As2Config or SftpConfig must be specified";
const NEITHER_CONFIG: &str = "One of As2Config or SftpConfig must be specified";

/// Create one connector and hand back its `ConnectorId`.
async fn create_connector(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorId, got {}", answer.body))
        .to_string()
}

/// RULE (rule 1's both-half, over the rest of the class): a connector configuration that
/// names both `As2Config` and `SftpConfig` is refused 400 `InvalidRequestException` with
/// the service's sentence for that half of the either/or — whichever operation resolves
/// the configuration, whichever kind the stored connector was, and whatever else the
/// request carried.
#[tokio::test]
async fn naming_both_connector_configurations_states_that_only_one_may_be() {
    let emulator = Emulator::new();

    // Two stored connectors, one of each kind, so an UPDATE can break the pair from
    // either side.
    let sftp_connector = create_connector(
        &emulator,
        "a stored SFTP connector",
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": "sftp"},
        }),
    )
    .await;
    let as2_connector = create_connector(
        &emulator,
        "a stored AS2 connector",
        json!({
            "AccessRole": ROLE,
            "Url": "https://partner.example.com/as2",
            "As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "Compression": "DISABLED",
                "EncryptionAlgorithm": "NONE",
                "SigningAlgorithm": "NONE",
            },
        }),
    )
    .await;

    // (case, operation, request, the half of the rule the request broke)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "CreateConnector naming both configurations and no Url",
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
            BOTH_CONFIGS,
        ),
        (
            "CreateConnector naming both configurations and a connection bound",
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp", "MaxConcurrentConnections": 2},
            }),
            BOTH_CONFIGS,
        ),
        (
            "UpdateConnector adding an SftpConfig to the stored AS2 connector",
            "UpdateConnector",
            json!({
                "ConnectorId": &as2_connector,
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
            BOTH_CONFIGS,
        ),
        (
            "UpdateConnector naming both configurations at once",
            "UpdateConnector",
            json!({
                "ConnectorId": &sftp_connector,
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
            BOTH_CONFIGS,
        ),
        (
            "CreateConnector naming neither configuration: the sibling half",
            "CreateConnector",
            json!({"AccessRole": ROLE, "Url": "sftp://partner.example.com"}),
            NEITHER_CONFIG,
        ),
    ];

    for (case, op, request, expected) in rows {
        let answer = emulator.call(op, request).await;
        // 400 is `InvalidRequestException`'s modelled `@httpError`.
        assert_message(
            case,
            &answer,
            400,
            "InvalidRequestException",
            expected,
            &[
                ROLE,
                LOCAL_PROFILE,
                sftp_connector.as_str(),
                as2_connector.as_str(),
            ],
        );
    }

    // The two halves are parallel statements of the one rule over the same two member
    // names: the both-half is the neither-half with the "Only" that says which half of
    // the rule the request broke.
    let parallel = format!(
        "Only {}{}",
        NEITHER_CONFIG[..1].to_lowercase(),
        &NEITHER_CONFIG[1..]
    );
    assert_eq!(
        BOTH_CONFIGS, parallel,
        "case \"the two halves of one either/or\": expected the both-half to read \
         {parallel:?}, got {BOTH_CONFIGS:?}"
    );
    assert_ne!(
        BOTH_CONFIGS, NEITHER_CONFIG,
        "case \"the two halves of one either/or\": expected the condition reported to be \
         distinguishable"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the server that pins a workflow a delete addressed
// ---------------------------------------------------------------------------

/// The refusal a `DeleteWorkflow` of a pinned workflow answers: the fixed sentence plus
/// the one variable part, the referencing server's id — built here from the test's OWN
/// state, so each row proves where that part comes from.
fn workflow_in_use(server_id: &str) -> String {
    format!("Workflow is still in use by server {server_id}")
}

/// Create one workflow and hand back its `WorkflowId`.
async fn create_delete_workflow(emulator: &Emulator, case: &str, description: &str) -> String {
    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({
                "Description": description,
                "Steps": [{"Type": "DELETE", "DeleteStepDetails": {"Name": "drop"}}],
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowId, got {}", answer.body))
        .to_string()
}

/// RULE: deleting a workflow a server still attaches through its `WorkflowDetails` is
/// refused 400 `InvalidRequestException` with the service's sentence naming the server
/// that pins it, read from this service's STATE — the request names only the workflow.
///
/// The rows differ along the trigger the workflow is attached through (both members of
/// `WorkflowDetails`), how many servers attach it at once, and which workflow is
/// addressed.
#[tokio::test]
async fn deleting_a_pinned_workflow_names_the_server_that_pins_it() {
    // (case, the `WorkflowDetails` trigger each attaching server uses)
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("one server attaching through OnUpload", vec!["OnUpload"]),
        (
            "one server attaching through OnPartialUpload",
            vec!["OnPartialUpload"],
        ),
        (
            "two servers attaching it, through both triggers",
            vec!["OnUpload", "OnPartialUpload"],
        ),
        (
            "three servers attaching it",
            vec!["OnUpload", "OnUpload", "OnPartialUpload"],
        ),
    ];

    for (case, triggers) in rows {
        let emulator = Emulator::new();
        // A SECOND workflow, and a server attaching THAT one, so neither the workflow
        // under test nor a server that references it is the only candidate in the store.
        let other = create_delete_workflow(&emulator, case, "the other workflow").await;
        let workflow_id = create_delete_workflow(&emulator, case, "the pinned workflow").await;
        let other_server = emulator
            .create_server(json!({
                "WorkflowDetails": {"OnUpload": [{"WorkflowId": &other, "ExecutionRole": ROLE}]},
            }))
            .await;

        let mut attaching: Vec<String> = Vec::new();
        for &trigger in &triggers {
            attaching.push(
                emulator
                    .create_server(json!({
                        "WorkflowDetails": {
                            trigger: [{"WorkflowId": &workflow_id, "ExecutionRole": ROLE}],
                        },
                    }))
                    .await,
            );
        }
        // The server the refusal names is the first of them in `ServerId` order, so one
        // state always reports one server.
        attaching.sort();
        let expected = workflow_in_use(&attaching[0]);

        let answer = emulator
            .call("DeleteWorkflow", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_message(
            case,
            &answer,
            400,
            "InvalidRequestException",
            &expected,
            &[workflow_id.as_str(), other.as_str(), other_server.as_str()],
        );

        // Detaching every server that pinned it lets the delete through, which is what
        // the refusal asked for — so the sentence named a condition the caller can clear.
        for server_id in &attaching {
            let detached = emulator
                .call(
                    "UpdateServer",
                    json!({"ServerId": server_id,
                           "WorkflowDetails": {"OnUpload": [], "OnPartialUpload": []}}),
                )
                .await;
            assert_eq!(
                detached.status, 200,
                "case {case:?}: expected UpdateServer to detach the workflow, got {} with \
                 body {}",
                detached.status, detached.body
            );
        }
        let deleted = emulator
            .call("DeleteWorkflow", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the detached workflow to delete, got {} with body {}",
            deleted.status, deleted.body
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: what the absent identity-provider details are required FOR
// ---------------------------------------------------------------------------

/// The refusal a server configuration that requires `IdentityProviderDetails` and names
/// none answers: the fixed sentence plus the one variable part, the `IdentityProviderType`
/// the REQUEST chose.
fn details_required_for(identity_provider_type: &str) -> String {
    format!("IdentityProviderDetails required for IdentityProviderType: {identity_provider_type}")
}

/// Every `IdentityProviderType` the model documents `IdentityProviderDetails` as
/// "Required when `IdentityProviderType` is set to" — `AWS_DIRECTORY_SERVICE`,
/// `AWS_LAMBDA` and `API_GATEWAY`
/// (`smithy_query(query="doc",shape="CreateServerRequest",member="IdentityProviderDetails")`),
/// which is the whole modelled value set `[SERVICE_MANAGED, API_GATEWAY,
/// AWS_DIRECTORY_SERVICE, AWS_LAMBDA]` except the service-managed one. Each entry carries
/// the `IdentityProviderDetails` member that type IS configured through, for the control
/// that the same request with it is served.
const PROVIDERS_REQUIRING_DETAILS: [(&str, &str); 3] = [
    ("API_GATEWAY", "Url"),
    ("AWS_DIRECTORY_SERVICE", "DirectoryId"),
    ("AWS_LAMBDA", "Function"),
];

/// The `IdentityProviderDetails` configured through `member`, whose value satisfies that
/// member's modelled constraints.
fn provider_details(member: &str) -> Value {
    match member {
        "Url" => json!({"Url": "https://example.invalid/auth"}),
        "DirectoryId" => json!({"DirectoryId": DIRECTORY_ID}),
        "Function" => json!({"Function": LAMBDA_IDP}),
        other => panic!("{other} is not an IdentityProviderDetails member this suite configures"),
    }
}

/// RULE: a server configuration whose `IdentityProviderType` requires
/// `IdentityProviderDetails` and carries none is refused 400 `InvalidRequestException`
/// with the service's sentence naming what the details are required FOR — the deciding
/// member and the value the request chose.
///
/// The rows span every `IdentityProviderType` that requires the details, crossed with the
/// other members the request carries and with both operations that resolve a server
/// configuration.
#[tokio::test]
async fn a_server_missing_its_identity_provider_details_is_told_what_they_are_required_for() {
    let emulator = Emulator::new();

    // A stored server an UPDATE can re-type, so the same rule is reached through
    // `UpdateServer` as through `CreateServer`.
    let server_id = emulator.create_server(json!({})).await;

    for (identity_provider_type, details_member) in PROVIDERS_REQUIRING_DETAILS {
        let expected = details_required_for(identity_provider_type);

        // (case, operation, request, the values that must not reach the Message)
        let rows: Vec<(String, &str, Value, Vec<String>)> = vec![
            (
                format!("CreateServer as {identity_provider_type} with no details"),
                "CreateServer",
                json!({"IdentityProviderType": identity_provider_type}),
                vec![],
            ),
            (
                format!("CreateServer as {identity_provider_type} with a role and tags"),
                "CreateServer",
                json!({
                    "IdentityProviderType": identity_provider_type,
                    "LoggingRole": ROLE,
                    "Tags": [{"Key": "team", "Value": "files"}],
                }),
                vec![ROLE.to_string(), "team".to_string()],
            ),
            (
                format!("UpdateServer to {identity_provider_type} with no details"),
                "UpdateServer",
                json!({
                    "ServerId": &server_id,
                    "IdentityProviderType": identity_provider_type,
                }),
                vec![server_id.clone()],
            ),
        ];

        for (case, op, request, absent) in rows {
            let answer = emulator.call(op, request).await;
            let absent: Vec<&str> = absent.iter().map(String::as_str).collect();
            assert_message(
                &case,
                &answer,
                400,
                "InvalidRequestException",
                &expected,
                &absent,
            );
        }

        // The control: the SAME configuration carrying the details that type is
        // configured through is served, so the refusal is about the absent member rather
        // than about the type.
        let served = emulator
            .call(
                "CreateServer",
                json!({
                    "IdentityProviderType": identity_provider_type,
                    "IdentityProviderDetails": provider_details(details_member),
                }),
            )
            .await;
        assert_eq!(
            served.status, 200,
            "case \"CreateServer as {identity_provider_type} with its details\": expected \
             200, got {} with body {}",
            served.status, served.body
        );
    }

    // The deciding member's value is the only part that varies: the three sentences are
    // the one template filled from the request.
    let texts: Vec<String> = PROVIDERS_REQUIRING_DETAILS
        .iter()
        .map(|(identity_provider_type, _)| details_required_for(identity_provider_type))
        .collect();
    for (index, text) in texts.iter().enumerate() {
        for other in texts.iter().skip(index + 1) {
            assert_ne!(
                text, other,
                "case \"one sentence per identity-provider type\": expected the type to reach \
                 the text, got {texts:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 8: one condition, one sentence, over every refusal a server configuration
// can earn
// ---------------------------------------------------------------------------

/// Satisfies `EndpointDetails$VpcId`'s modelled form
/// (`smithy_query(query="shape",shape="EndpointDetails")`): the VPC the rows that DO name
/// the endpoint settings host their endpoint in.
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// The whole sentence the service states for a server whose endpoint would be hosted in a
/// VPC and names no `EndpointDetails`, written out here as the service words it. It is
/// FIXED for the condition in its entirety — no slot for the member, the `EndpointType` or
/// anything else the caller sent — so both endpoint types that host in a VPC answer it.
const ENDPOINT_DETAILS_REQUIRED: &str = "EndpointDetails required for VPC";

/// One row of the table: the condition the request offends (rows sharing a condition must
/// answer one sentence, and no two conditions may answer the same), the case name, the
/// request, the whole expected `Message`, and the values the request carried that must not
/// reach it.
struct DefectRow {
    condition: String,
    case: String,
    request: Value,
    expected: String,
    absent: Vec<String>,
}

/// RULE: every semantic refusal of a server configuration states the sentence the service
/// states for THAT condition — its fixed text with only that condition's variable part
/// filled from the request — so the three defects a `CreateServer` can be refused for are
/// told apart by their `Message` alone (they share one error code and one status), and a
/// refused configuration persists nothing.
///
/// The rows span the three defects, crossed with what each generalises over: every
/// `IdentityProviderType` that makes `IdentityProviderDetails` required, both
/// `EndpointType` values that host the endpoint in a VPC, both members of the FTP family,
/// and — around each — the other members the request carries (protocols, endpoint
/// settings, a logging role, tags), including configurations that offend a LATER rule too,
/// which must still be told the condition they have to clear first.
#[tokio::test]
async fn each_refused_server_configuration_states_its_own_condition_and_persists_nothing() {
    let emulator = Emulator::new();
    let vpc = json!({"VpcId": VPC_ID});
    let (tag_key, tag_value) = ("team", "files");

    let mut rows: Vec<DefectRow> = Vec::new();

    // ---- the identity-provider details defect ----
    for (identity_provider_type, _) in PROVIDERS_REQUIRING_DETAILS {
        let condition = format!("absent IdentityProviderDetails for {identity_provider_type}");
        let expected = details_required_for(identity_provider_type);
        rows.push(DefectRow {
            condition: condition.clone(),
            case: format!("{identity_provider_type} serving SFTP on a public endpoint"),
            request: json!({
                "IdentityProviderType": identity_provider_type,
                "Protocols": [st::PROTOCOL_SFTP],
                "EndpointType": "PUBLIC",
            }),
            expected: expected.clone(),
            absent: vec![],
        });
        rows.push(DefectRow {
            condition: condition.clone(),
            case: format!("{identity_provider_type} on a VPC endpoint whose settings ARE named"),
            request: json!({
                "IdentityProviderType": identity_provider_type,
                "EndpointType": st::ENDPOINT_TYPE_VPC,
                "EndpointDetails": vpc.clone(),
            }),
            expected: expected.clone(),
            absent: vec![VPC_ID.to_string()],
        });
        rows.push(DefectRow {
            condition: condition.clone(),
            case: format!("{identity_provider_type} asking for the FTP family with no certificate"),
            request: json!({
                "IdentityProviderType": identity_provider_type,
                "Protocols": [st::PROTOCOL_FTP, st::PROTOCOL_FTPS],
                "EndpointType": st::ENDPOINT_TYPE_VPC,
                "EndpointDetails": vpc.clone(),
            }),
            expected: expected.clone(),
            absent: vec![VPC_ID.to_string()],
        });
        rows.push(DefectRow {
            condition,
            case: format!("{identity_provider_type} with a logging role and tags"),
            request: json!({
                "IdentityProviderType": identity_provider_type,
                "LoggingRole": ROLE,
                "Tags": [{"Key": tag_key, "Value": tag_value}],
            }),
            expected,
            absent: vec![ROLE.to_string(), tag_key.to_string(), tag_value.to_string()],
        });
    }

    // ---- the VPC endpoint settings defect ----
    let endpoint_condition = "absent EndpointDetails for an endpoint hosted in a VPC";
    rows.push(DefectRow {
        condition: endpoint_condition.to_string(),
        case: "a VPC-hosted endpoint with no settings".to_string(),
        request: json!({"EndpointType": st::ENDPOINT_TYPE_VPC}),
        expected: ENDPOINT_DETAILS_REQUIRED.to_string(),
        absent: vec![],
    });
    rows.push(DefectRow {
        condition: endpoint_condition.to_string(),
        case: "the other VPC-hosted endpoint type, with no settings".to_string(),
        request: json!({
            "EndpointType": st::ENDPOINT_TYPE_VPC_ENDPOINT,
            "Protocols": [st::PROTOCOL_SFTP],
        }),
        expected: ENDPOINT_DETAILS_REQUIRED.to_string(),
        absent: vec![st::ENDPOINT_TYPE_VPC_ENDPOINT.to_string()],
    });

    // ---- the unservable protocol family defect ----
    // Both rows ask a SERVICE_MANAGED server for the family — one naming that provider,
    // one taking the documented default — so they are one condition and owe one sentence.
    // `protocol_family_gate.rs` states that sentence and pins the rule behind it.
    let family_condition = format!(
        "a protocol family {} cannot serve",
        st::IDENTITY_PROVIDER_SERVICE_MANAGED
    );
    rows.push(DefectRow {
        condition: family_condition.clone(),
        case: "the FTP family under the provider the model defaults to".to_string(),
        request: json!({"Protocols": [st::PROTOCOL_FTP]}),
        expected: family_gate_message(st::DEFAULT_IDENTITY_PROVIDER_TYPE),
        absent: vec![],
    });
    rows.push(DefectRow {
        condition: family_condition,
        case: "the family's other half beside SFTP, on the endpoint it would need".to_string(),
        request: json!({
            "Protocols": [st::PROTOCOL_SFTP, st::PROTOCOL_FTPS],
            "EndpointType": st::ENDPOINT_TYPE_VPC,
            "EndpointDetails": vpc.clone(),
            "IdentityProviderType": st::IDENTITY_PROVIDER_SERVICE_MANAGED,
        }),
        expected: family_gate_message(st::IDENTITY_PROVIDER_SERVICE_MANAGED),
        absent: vec![VPC_ID.to_string()],
    });

    // What the emulator ANSWERED each row, collected per condition.
    let mut answered: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for row in &rows {
        let answer = emulator.call("CreateServer", row.request.clone()).await;
        let absent: Vec<&str> = row.absent.iter().map(String::as_str).collect();
        let actual = assert_message(
            &row.case,
            &answer,
            400,
            "InvalidRequestException",
            &row.expected,
            &absent,
        );
        assert!(
            answer.body.get("ServerId").is_none(),
            "case {:?}: a refused create must answer no ServerId, got {}",
            row.case,
            answer.body
        );
        answered
            .entry(row.condition.clone())
            .or_default()
            .push((row.case.clone(), actual));
    }

    // Nothing in the table was served: a refusal of the configuration leaves no server
    // behind, whichever condition it reported.
    let (_, listed) =
        drain_pages(&emulator, "ListServers", &json!({}), "Servers", "ServerId", None).await;
    assert!(
        listed.is_empty(),
        "case \"the store after every refused create\": expected no server, got {listed:?}"
    );

    // One condition, one sentence — read off what the emulator answered above, not off
    // what this suite built: every row of a condition said the same thing whatever else
    // it configured, and no two conditions said the same, so the Message is what tells a
    // client WHICH member to repair.
    let sentences: Vec<(String, String)> = answered
        .iter()
        .map(|(condition, said)| {
            let (first_case, first) = said.first().expect("each condition has rows");
            for (case, actual) in said {
                assert_eq!(
                    actual, first,
                    "case {case:?} against {first_case:?}: expected one sentence for \
                     {condition:?}, got {actual:?} and {first:?}"
                );
            }
            (condition.clone(), first.clone())
        })
        .collect();
    for (index, (condition, sentence)) in sentences.iter().enumerate() {
        for (other_condition, other) in sentences.iter().skip(index + 1) {
            assert_ne!(
                sentence, other,
                "case \"{condition} against {other_condition}\": expected two conditions to \
                 state two sentences, got {sentence:?} for both"
            );
        }
    }

    // The refusals cost the service nothing: the shared catalog still answers, and the
    // SAME configuration carrying the details its provider is configured through is
    // created and reads back with them.
    let policy = emulator
        .call(
            "DescribeSecurityPolicy",
            json!({"SecurityPolicyName": st::DEFAULT_SECURITY_POLICY_NAME}),
        )
        .await;
    assert_eq!(
        policy.status, 200,
        "case \"the catalog after every refused create\": expected 200, got {} with body {}",
        policy.status, policy.body
    );
    assert_eq!(
        member_of(policy.get("SecurityPolicy"), "SecurityPolicyName").as_str(),
        Some(st::DEFAULT_SECURITY_POLICY_NAME),
        "case \"the catalog after every refused create\": expected the policy named, got {}",
        policy.body
    );

    for (identity_provider_type, details_member) in PROVIDERS_REQUIRING_DETAILS {
        let case = format!("{identity_provider_type} carrying its {details_member}");
        let details = provider_details(details_member);
        let server_id = emulator
            .create_server(json!({
                "IdentityProviderType": identity_provider_type,
                "IdentityProviderDetails": details.clone(),
            }))
            .await;
        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeServer 200, got {} with body {}",
            described.status, described.body
        );
        let server = described.get("Server");
        assert_eq!(
            member_of(server, "IdentityProviderType").as_str(),
            Some(identity_provider_type),
            "case {case:?}: expected the provider the create named, got {}",
            described.body
        );
        assert_eq!(
            member_of(member_of(server, "IdentityProviderDetails"), details_member),
            &details[details_member],
            "case {case:?}: expected the details the create carried, got {}",
            described.body
        );
    }

    // The same requirement over the OTHER operation that resolves a server
    // configuration, where "persists nothing" means "changes nothing": a stored server a
    // refused update tried to re-type still reports the provider it was created with.
    let stored = emulator.create_server(json!({})).await;
    for (identity_provider_type, _) in PROVIDERS_REQUIRING_DETAILS {
        let answer = emulator
            .call(
                "UpdateServer",
                json!({"ServerId": &stored, "IdentityProviderType": identity_provider_type}),
            )
            .await;
        assert_message(
            &format!("UpdateServer to {identity_provider_type} with no details"),
            &answer,
            400,
            "InvalidRequestException",
            &details_required_for(identity_provider_type),
            &[stored.as_str()],
        );
    }
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &stored}))
        .await;
    assert_eq!(
        member_of(described.get("Server"), "IdentityProviderType").as_str(),
        Some(st::DEFAULT_IDENTITY_PROVIDER_TYPE),
        "case \"the stored server after every refused update\": expected the provider it was \
         created with, got {}",
        described.body
    );
}
