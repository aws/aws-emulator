// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IPAM external resource verification TOKEN: the
//! `CreateIpamExternalResourceVerificationToken` that mints one, the
//! `DescribeIpamExternalResourceVerificationTokens` that reads it back and the
//! `DeleteIpamExternalResourceVerificationToken` that takes it away — driven over the wire the
//! service actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a mint STORES one token against the IPAM the request names, taking that IPAM's own facts
//!   (its ARN and its home Region) from the stored IPAM and deriving everything else the
//!   service owns — the id, the ARN, the random value, the name, the expiration, the status and
//!   the state — so every member of the minted token reads back through the describe;
//! * the value is drawn rather than computed from the request, so no two mints of one IPAM
//!   carry the same value;
//! * an IPAM id the account holds no IPAM for is refused with that resource's own `NotFound`
//!   code and something that is no IPAM id at all with its `Malformed` code, and the same two
//!   questions are asked of a token id by the describe and the delete;
//! * one `ClientToken` names the one token it minted: a replay answers that token, and a replay
//!   asking for anything else is refused;
//! * the describe narrows by id, by every documented filter and by the page size, and refuses a
//!   filter it does not resolve;
//! * a delete leaves the token in the transient state the catalog carries and the clock then
//!   REMOVES it, a second delete is refused for the state it is in, and a token cannot outlive
//!   the IPAM that created it;
//! * a dry run of any of the three reports that outcome and changes nothing.
//!
//! Each table row is a distinct input along what those rules generalise over: how many tags the
//! request names, whether it carries a client token, which member is at fault, which filter
//! narrows, and how many tokens the store holds. Nothing here waits a fixed time: where the
//! clock owes a move, the condition is looked at AGAIN until it holds or a deadline far past the
//! service's own transition span passes.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime};

use ec2::ops::types::{IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN_STATE_VALUES, TOKEN_STATE_VALUES};
use ec2::state::{
    DRY_RUN_OPERATION, DRY_RUN_OPERATION_MESSAGE, DRY_RUN_OPERATION_STATUS,
    IDEMPOTENT_PARAMETER_MISMATCH, INCORRECT_STATE, INVALID_IPAM_ID_MALFORMED,
    INVALID_IPAM_ID_NOT_FOUND, INVALID_IPAM_VERIFICATION_TOKEN_ID_MALFORMED,
    INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND, INVALID_PARAMETER_VALUE,
    IPAM_VERIFICATION_TOKEN_ID_KIND, IPAM_VERIFICATION_TOKEN_ID_PREFIX,
    IPAM_VERIFICATION_TOKEN_KIND, IPAM_VERIFICATION_TOKEN_NAME,
    IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE,
    IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS, IPAM_VERIFICATION_TOKEN_STATUS_VALID,
    IPAM_VERIFICATION_TOKEN_VALIDITY, RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN,
    TRANSITION_DELAY, id_not_found_message, invalid_id_message, resource_state_message,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// The ec2Query `Action`s under test.
const CREATE: &str = "CreateIpamExternalResourceVerificationToken";
const DESCRIBE: &str = "DescribeIpamExternalResourceVerificationTokens";
const DELETE: &str = "DeleteIpamExternalResourceVerificationToken";

/// The HTTP status EC2 answers a refused request with.
const REFUSAL_STATUS: u16 = 400;

/// The deadline every poll below gives up at: a large multiple of the service's own transition
/// span, so an assertion is about the state the clock owes and never about how fast the host
/// runs.
fn removal_deadline() -> Duration {
    TRANSITION_DELAY * 60
}

/// How long one look waits before looking again — a fraction of that span, so a poll inside the
/// deadline above costs many looks rather than one.
fn poll_interval() -> Duration {
    TRANSITION_DELAY / 10
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The IPAM a suite mints its tokens against: what a request names it by, the ARN every token of
/// it reports, and the Region it is homed in.
struct CreatedIpam {
    /// The `IpamId` a `CreateIpamExternalResourceVerificationToken` request names.
    id: String,
    /// "ARN of the IPAM that created the token"
    /// (`IpamExternalResourceVerificationToken$IpamArn`).
    arn: String,
}

/// Create one IPAM through the operation that owns it, and answer what it is named by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| -> String {
        child_text(ipam, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
    }
}

/// One verification token as a response reports it: every member read off the wire, with the
/// tags flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedToken {
    token_id: Option<String>,
    token_arn: Option<String>,
    ipam_id: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    token_value: Option<String>,
    token_name: Option<String>,
    not_after: Option<String>,
    status: Option<String>,
    state: Option<String>,
    tags: Vec<(String, String)>,
}

/// The token element of a response as [`ReportedToken`].
fn read_token(node: roxmltree::Node<'_, '_>) -> ReportedToken {
    let tags = child(node, "tagSet")
        .map(|set| {
            set.children()
                .filter(|item| item.is_element())
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    ReportedToken {
        token_id: child_text(node, "ipamExternalResourceVerificationTokenId"),
        token_arn: child_text(node, "ipamExternalResourceVerificationTokenArn"),
        ipam_id: child_text(node, "ipamId"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        token_value: child_text(node, "tokenValue"),
        token_name: child_text(node, "tokenName"),
        not_after: child_text(node, "notAfter"),
        status: child_text(node, "status"),
        state: child_text(node, "state"),
        tags,
    }
}

/// The one token a create or a delete response carries.
fn reported_token(case: &str, body: &str) -> ReportedToken {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "ipamExternalResourceVerificationTokenId").is_some()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no verification token (no id element) in {body}")
        });
    read_token(node)
}

/// Every token a describe response reports, in the order it reported them.
fn described_tokens(case: &str, body: &str) -> Vec<ReportedToken> {
    let parsed = document(case, body);
    let Some(set) = parsed.descendants().find(|node| {
        node.is_element()
            && node.tag_name().name() == "ipamExternalResourceVerificationTokenSet"
    }) else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_token)
        .collect()
}

/// The ids a describe body reports, in that order.
fn described_ids(case: &str, body: &str) -> Vec<String> {
    described_tokens(case, body)
        .into_iter()
        .filter_map(|token| token.token_id)
        .collect()
}

/// The `nextToken` a describe body carries, if any — read as the envelope's own member, so a
/// value nested inside a reported token could never be mistaken for it.
fn next_token(body: &str) -> Option<String> {
    super::ec2_query::member(body, "nextToken").filter(|token| !token.is_empty())
}

/// The describe narrowed to one id, as the one token it reports.
async fn describe_one(case: &str, router: &MultiServiceRouter, token_id: &str) -> ReportedToken {
    let body = ok(
        case,
        router,
        DESCRIBE,
        &[param("IpamExternalResourceVerificationTokenId.1", token_id)],
    )
    .await;
    let mut tokens = described_tokens(case, &body);
    assert_eq!(
        tokens.len(),
        1,
        "case {case:?}: expected exactly the token {token_id} back, got {body}"
    );
    tokens.remove(0)
}

/// The form fields for one verification-token tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// Mint one token against `ipam_id` and answer it as the create reported it.
async fn mint(
    case: &str,
    router: &MultiServiceRouter,
    ipam_id: &str,
    tags: &[(&str, &str)],
) -> ReportedToken {
    let mut params = vec![param("IpamId", ipam_id)];
    params.extend(tag_params(tags));
    let body = ok(case, router, CREATE, &params).await;
    reported_token(case, &body)
}

/// The id of the token `token` reports, or a panic naming the case.
fn token_id(case: &str, token: &ReportedToken) -> String {
    token
        .token_id
        .clone()
        .unwrap_or_else(|| panic!("case {case:?}: the token carries no id: {token:?}"))
}

/// Whether `value` reads as the hyphenated lowercase-hex UUID form AWS issues a token value in:
/// the `8-4-4-4-12` groups, and nothing else.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(groups.iter())
            .all(|(wanted, group)| {
                group.len() == *wanted
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
}

/// Look at the token AGAIN until the describe of its id refuses it — the removal the clock owed
/// the delete — or a deadline far past the service's own transition span passes. The condition is
/// waited for, never a fixed time.
async fn poll_until_gone(case: &str, router: &MultiServiceRouter, id: &str) {
    let deadline = Instant::now() + removal_deadline();
    loop {
        let (status, body) = invoke(
            router,
            DESCRIBE,
            &[param("IpamExternalResourceVerificationTokenId.1", id)],
        )
        .await;
        if status == REFUSAL_STATUS {
            assert_eq!(
                error_code(&body),
                INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND,
                "case {case:?}: expected the removed token {id} to answer \
                 {INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                id_not_found_message(IPAM_VERIFICATION_TOKEN_ID_KIND, id),
                "case {case:?}: expected this resource's own not-found sentence for {id}, \
                 got {body}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {id} to stop resolving, got HTTP {status} with {body}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

// ---------------------------------------------------------------------------
// The words this suite asserts
// ---------------------------------------------------------------------------

/// Every state and status word written into an assertion below is a member of the modelled
/// catalog it belongs to, so no row can pin a value the API does not have.
#[test]
fn the_asserted_words_are_the_models_own() {
    for (case, word) in [
        ("a minted token", IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE),
        (
            "a token being deleted",
            IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS,
        ),
    ] {
        assert!(
            IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN_STATE_VALUES.contains(&word),
            "case {case:?}: expected {word:?} to be a member of the modelled \
             IpamExternalResourceVerificationTokenState catalog, which carries \
             {IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN_STATE_VALUES:?}"
        );
    }
    assert!(
        TOKEN_STATE_VALUES.contains(&IPAM_VERIFICATION_TOKEN_STATUS_VALID),
        "expected {IPAM_VERIFICATION_TOKEN_STATUS_VALID:?} to be a member of the modelled \
         TokenState catalog, which carries {TOKEN_STATE_VALUES:?}"
    );
}

// ---------------------------------------------------------------------------
// What the mint stores
// ---------------------------------------------------------------------------

/// One row of the mint table: the case and the tag payload the request names.
struct MintCase {
    case: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    client_token: Option<&'static str>,
}

/// The rule: a mint stores one token against the IPAM the request names, carrying exactly the
/// tags the request described, with that IPAM's own facts taken from the stored IPAM and every
/// member the service owns derived — and every one of those members reads back through the
/// describe.
#[tokio::test]
async fn create_stores_the_token_the_request_describes() {
    let router = emulator();
    let ipam = create_ipam("the IPAM the tokens are minted by", &router).await;

    let cases = [
        MintCase {
            case: "no optional member at all",
            tags: Vec::new(),
            client_token: None,
        },
        MintCase {
            case: "one tag",
            tags: vec![("Name", "token-alpha")],
            client_token: None,
        },
        MintCase {
            case: "three tags",
            tags: vec![
                ("Name", "token-beta"),
                ("Team", "network"),
                ("Env", "prod"),
            ],
            client_token: None,
        },
        MintCase {
            case: "a client token and one tag",
            tags: vec![("Env", "test")],
            client_token: Some("mint-with-a-token"),
        },
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut values: BTreeSet<String> = BTreeSet::new();
    for row in &cases {
        let case = row.case;
        let mut params = vec![param("IpamId", &ipam.id)];
        params.extend(tag_params(&row.tags));
        if let Some(client_token) = row.client_token {
            params.push(param("ClientToken", client_token));
        }

        let body = ok(case, &router, CREATE, &params).await;
        let token = reported_token(case, &body);
        let id = token_id(case, &token);

        assert!(
            is_long_id(IPAM_VERIFICATION_TOKEN_ID_PREFIX, &id),
            "case {case:?}: expected a minted {IPAM_VERIFICATION_TOKEN_ID_PREFIX} id, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: expected a token id no earlier mint was given, got {id:?}"
        );

        // What the service owns, derived rather than echoed.
        assert_eq!(
            token.token_arn.as_deref(),
            Some(
                engine::emulator_arn(
                    "ec2",
                    "",
                    &format!("{RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN}/{id}")
                )
                .as_str()
            ),
            "case {case:?}: the token reports its own ARN, got {token:?}"
        );
        assert_eq!(
            token.ipam_id.as_deref(),
            Some(ipam.id.as_str()),
            "case {case:?}: the token names the IPAM that created it, got {token:?}"
        );
        assert_eq!(
            token.ipam_arn.as_deref(),
            Some(ipam.arn.as_str()),
            "case {case:?}: the token reports that IPAM's ARN, got {token:?}"
        );
        assert_eq!(
            token.ipam_region.as_deref(),
            Some(REGION),
            "case {case:?}: the token reports that IPAM's home Region, got {token:?}"
        );
        assert_eq!(
            token.token_name.as_deref(),
            Some(IPAM_VERIFICATION_TOKEN_NAME),
            "case {case:?}: the token carries the name the service gives one, got {token:?}"
        );
        assert_eq!(
            token.status.as_deref(),
            Some(IPAM_VERIFICATION_TOKEN_STATUS_VALID),
            "case {case:?}: a freshly minted token may still be presented, got {token:?}"
        );
        assert_eq!(
            token.state.as_deref(),
            Some(IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE),
            "case {case:?}: the value is drawn by the call, so the mint is complete, \
             got {token:?}"
        );

        // The value: drawn, not computed from the request, so it is a fresh random value every
        // time and no two tokens share one.
        let value = token
            .token_value
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the token carries no value: {token:?}"));
        assert!(
            is_uuid(&value),
            "case {case:?}: expected a random value in the hyphenated UUID form, got {value:?}"
        );
        assert!(
            values.insert(value.clone()),
            "case {case:?}: expected a value no earlier mint drew, got {value:?}"
        );
        assert!(
            !value.contains(&ipam.id) && Some(value.as_str()) != row.client_token,
            "case {case:?}: the value is drawn rather than derived from the request, \
             got {value:?}"
        );

        // The expiration: one validity span from the mint, which is far enough out that no
        // assertion here depends on how fast the host runs.
        let not_after = token
            .not_after
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the token carries no expiration: {token:?}"));
        let expires_at = as_timestamp(&not_after).unwrap_or_else(|| {
            panic!("case {case:?}: the expiration must be a timestamp, got {not_after:?}")
        });
        let now = ::aws_smithy_types::DateTime::from(SystemTime::now()).as_secs_f64();
        let validity = IPAM_VERIFICATION_TOKEN_VALIDITY.as_secs_f64();
        let slack = Duration::from_secs(3600).as_secs_f64();
        assert!(
            expires_at > now + validity - slack && expires_at < now + validity + slack,
            "case {case:?}: expected an expiration about {validity}s out, got {expires_at} \
             against {now}"
        );

        // What the request decided, carried through.
        let wanted_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            token.tags, wanted_tags,
            "case {case:?}: the token carries the tags the request asked for, got {token:?}"
        );

        // The round trip: every member reads back through the describe.
        let read_back = describe_one(case, &router, &id).await;
        assert_eq!(
            read_back, token,
            "case {case:?}: the describe reports the token the create answered"
        );
    }

    // Every mint of the suite is still there, and the describe reports them in id order.
    let body = ok("every token the IPAM minted", &router, DESCRIBE, &[]).await;
    let reported = described_ids("every token the IPAM minted", &body);
    let wanted: Vec<String> = ids.iter().cloned().collect();
    assert_eq!(
        reported, wanted,
        "expected every minted token back in id order, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The ids a request names
// ---------------------------------------------------------------------------

/// The rule: an id that is no id of its kind is `.Malformed`, and a well-formed id the account
/// holds no resource for is `.NotFound` — asked in that order, of the IPAM a mint names and of
/// the token a describe or a delete names.
#[tokio::test]
async fn an_unresolvable_id_is_refused_in_its_own_wording() {
    let router = emulator();
    create_ipam("the IPAM the mint rows resolve against", &router).await;

    struct Row {
        case: &'static str,
        action: &'static str,
        params: Vec<Param>,
        code: &'static str,
        message: String,
    }

    let absent_ipam = "ipam-0123456789abcdef0";
    let absent_token = format!("{IPAM_VERIFICATION_TOKEN_ID_PREFIX}-0123456789abcdef0");
    let rows = vec![
        Row {
            case: "a mint naming something that is no IPAM id",
            action: CREATE,
            params: vec![param("IpamId", "not-an-ipam")],
            code: INVALID_IPAM_ID_MALFORMED,
            message: invalid_id_message("not-an-ipam"),
        },
        Row {
            case: "a mint naming an IPAM the account does not hold",
            action: CREATE,
            params: vec![param("IpamId", absent_ipam)],
            code: INVALID_IPAM_ID_NOT_FOUND,
            message: format!("The IPAM ({absent_ipam}) does not exist."),
        },
        Row {
            case: "a mint naming an IPAM id of the wrong family",
            action: CREATE,
            params: vec![param("IpamId", &absent_token)],
            code: INVALID_IPAM_ID_MALFORMED,
            message: invalid_id_message(&absent_token),
        },
        Row {
            case: "a describe naming something that is no token id",
            action: DESCRIBE,
            params: vec![param(
                "IpamExternalResourceVerificationTokenId.1",
                "not-a-token",
            )],
            code: INVALID_IPAM_VERIFICATION_TOKEN_ID_MALFORMED,
            message: invalid_id_message("not-a-token"),
        },
        Row {
            case: "a describe naming a token the account does not hold",
            action: DESCRIBE,
            params: vec![param(
                "IpamExternalResourceVerificationTokenId.1",
                &absent_token,
            )],
            code: INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND,
            message: id_not_found_message(IPAM_VERIFICATION_TOKEN_ID_KIND, &absent_token),
        },
        Row {
            case: "a delete naming something that is no token id",
            action: DELETE,
            params: vec![param(
                "IpamExternalResourceVerificationTokenId",
                "not-a-token",
            )],
            code: INVALID_IPAM_VERIFICATION_TOKEN_ID_MALFORMED,
            message: invalid_id_message("not-a-token"),
        },
        Row {
            case: "a delete naming a token the account does not hold",
            action: DELETE,
            params: vec![param(
                "IpamExternalResourceVerificationTokenId",
                &absent_token,
            )],
            code: INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND,
            message: id_not_found_message(IPAM_VERIFICATION_TOKEN_ID_KIND, &absent_token),
        },
    ];

    for row in &rows {
        let (status, body) = invoke(&router, row.action, &row.params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {:?}: expected HTTP {REFUSAL_STATUS} from {}, got {status} with {body}",
            row.case, row.action
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {:?}: expected the code {}, got {body}",
            row.case,
            row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {:?}: expected that refusal's own sentence, got {body}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// One client token names one minted token
// ---------------------------------------------------------------------------

/// The rule: a `ClientToken` names the one token it minted — a replay of the same request answers
/// that token and mints nothing, and a replay asking for anything else is refused.
#[tokio::test]
async fn a_client_token_names_the_one_token_it_minted() {
    let router = emulator();
    let ipam = create_ipam("the IPAM the tokens are minted by", &router).await;

    for (case, tags) in [
        ("a replay with no tags", Vec::new()),
        ("a replay with two tags", vec![("Name", "kept"), ("Env", "prod")]),
    ] {
        let client_token = format!("idempotent-{}", tags.len());
        let mut params = vec![param("IpamId", &ipam.id), param("ClientToken", &client_token)];
        params.extend(tag_params(&tags));

        let first = reported_token(case, &ok(case, &router, CREATE, &params).await);
        let again = reported_token(case, &ok(case, &router, CREATE, &params).await);
        assert_eq!(
            again, first,
            "case {case:?}: a replay answers the token that call minted"
        );

        // Nothing was minted twice: the describe holds one token for that id.
        let id = token_id(case, &first);
        let read_back = describe_one(case, &router, &id).await;
        assert_eq!(
            read_back, first,
            "case {case:?}: the store holds exactly the token the replay answered"
        );

        // The same token asking for something else is refused rather than answered with a token
        // the caller did not ask for.
        let mut other = vec![param("IpamId", &ipam.id), param("ClientToken", &client_token)];
        other.extend(tag_params(&[("Name", "different")]));
        let (status, body) = invoke(&router, CREATE, &other).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for a replay asking for other tags, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected {IDEMPOTENT_PARAMETER_MISMATCH}, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the describe narrows by
// ---------------------------------------------------------------------------

/// The rule: the describe reports every token the account holds, narrowed by the ids named, by
/// each DOCUMENTED filter and by the page size — and a filter it does not resolve is refused
/// rather than answered with no results.
#[tokio::test]
async fn the_describe_narrows_by_id_filter_and_page() {
    let router = emulator();
    let ipam = create_ipam("the IPAM the tokens are minted by", &router).await;
    let other = create_ipam("a second IPAM, whose tokens must not be reported", &router).await;

    let mut minted: Vec<ReportedToken> = Vec::new();
    for nth in 0..7 {
        let name = format!("token-{nth}");
        minted.push(mint("the tokens the filters narrow", &router, &ipam.id, &[("Name", &name)]).await);
    }
    let elsewhere = mint("the other IPAM's token", &router, &other.id, &[]).await;
    let elsewhere_id = token_id("the other IPAM's token", &elsewhere);

    let first = minted.first().cloned().expect("a minted token");
    let first_id = token_id("the first minted token", &first);

    // Every documented filter, each narrowing to exactly the token that carries the value.
    let documented: Vec<(&str, String, Vec<String>)> = vec![
        (
            "ipam-external-resource-verification-token-id",
            first_id.clone(),
            vec![first_id.clone()],
        ),
        (
            "ipam-external-resource-verification-token-arn",
            first.token_arn.clone().expect("an ARN"),
            vec![first_id.clone()],
        ),
        (
            "token-value",
            first.token_value.clone().expect("a value"),
            vec![first_id.clone()],
        ),
        (
            "ipam-id",
            other.id.clone(),
            vec![elsewhere_id.clone()],
        ),
        (
            "ipam-arn",
            other.arn.clone(),
            vec![elsewhere_id.clone()],
        ),
        ("tag:Name", "token-3".to_string(), {
            let wanted = minted
                .iter()
                .filter(|token| token.tags.iter().any(|(_, value)| value == "token-3"))
                .map(|token| token.token_id.clone().unwrap_or_default())
                .collect::<Vec<String>>();
            wanted
        }),
        ("tag-key", "Name".to_string(), {
            let mut wanted: Vec<String> = minted
                .iter()
                .map(|token| token.token_id.clone().unwrap_or_default())
                .collect();
            wanted.sort();
            wanted
        }),
    ];
    for (name, value, wanted) in documented {
        let case = format!("the filter {name}");
        let body = ok(
            &case,
            &router,
            DESCRIBE,
            &[param("Filter.1.Name", name), param("Filter.1.Value.1", &value)],
        )
        .await;
        let mut reported = described_ids(&case, &body);
        reported.sort();
        let mut wanted = wanted;
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the tokens carrying {value:?}, got {body}"
        );
    }

    // The filters every token of the account answers the same value for: each selects all of
    // them, so every documented name is exercised rather than only the narrowing ones.
    for (name, value) in [
        ("status", IPAM_VERIFICATION_TOKEN_STATUS_VALID),
        ("state", IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE),
        ("token-name", IPAM_VERIFICATION_TOKEN_NAME),
        ("ipam-region", REGION),
    ] {
        let case = format!("the filter {name}");
        let body = ok(
            &case,
            &router,
            DESCRIBE,
            &[param("Filter.1.Name", name), param("Filter.1.Value.1", value)],
        )
        .await;
        assert_eq!(
            described_ids(&case, &body).len(),
            minted.len() + 1,
            "case {case:?}: every token the account holds carries {value:?}, got {body}"
        );
    }

    // Two filters are joined with an AND, so one that holds and one that cannot select nothing.
    let case = "two filters, one of which cannot hold";
    let body = ok(
        case,
        &router,
        DESCRIBE,
        &[
            param("Filter.1.Name", "ipam-id"),
            param("Filter.1.Value.1", &ipam.id),
            param("Filter.2.Name", "state"),
            param(
                "Filter.2.Value.1",
                IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS,
            ),
        ],
    )
    .await;
    assert!(
        described_ids(case, &body).is_empty(),
        "case {case:?}: no token of that IPAM is being deleted, got {body}"
    );

    // A filter this operation does not resolve is refused.
    let case = "an undocumented filter";
    let (status, body) = invoke(
        &router,
        DESCRIBE,
        &[
            param("Filter.1.Name", "token-colour"),
            param("Filter.1.Value.1", "blue"),
        ],
    )
    .await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "The filter 'token-colour' is invalid",
        "case {case:?}: expected the filter's own sentence, got {body}"
    );

    // The page size, and the token that names where the next page starts.
    let case = "a page of five";
    let body = ok(
        case,
        &router,
        DESCRIBE,
        &[param("MaxResults", "5")],
    )
    .await;
    let page = described_ids(case, &body);
    assert_eq!(
        page.len(),
        5,
        "case {case:?}: expected five tokens in the page, got {body}"
    );
    let cursor = next_token(&body)
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken, got {body}"));
    let case = "the page after it";
    let rest = ok(
        case,
        &router,
        DESCRIBE,
        &[param("MaxResults", "5"), param("NextToken", &cursor)],
    )
    .await;
    let tail = described_ids(case, &rest);
    assert_eq!(
        tail.len(),
        3,
        "case {case:?}: expected the remaining three tokens, got {rest}"
    );
    assert!(
        next_token(&rest).is_none(),
        "case {case:?}: nothing follows the last page, got {rest}"
    );
    let mut every = page.clone();
    every.extend(tail);
    let mut wanted: Vec<String> = minted
        .iter()
        .map(|token| token.token_id.clone().unwrap_or_default())
        .collect();
    wanted.push(elsewhere_id);
    wanted.sort();
    assert_eq!(
        every, wanted,
        "case {case:?}: the two pages are every token the account holds, in id order"
    );
}

// ---------------------------------------------------------------------------
// What the delete records, and what the clock then removes
// ---------------------------------------------------------------------------

/// The rule: a delete leaves the token in the transient state the catalog carries, the clock then
/// REMOVES it, and a second delete of the same token is refused for the state it is in.
#[tokio::test]
async fn delete_records_the_move_and_the_clock_removes_the_token() {
    let router = emulator();
    let ipam = create_ipam("the IPAM the tokens are minted by", &router).await;

    for (case, tags) in [
        ("a token with no tags", Vec::new()),
        ("a token with one tag", vec![("Name", "going")]),
    ] {
        let minted = mint(case, &router, &ipam.id, &tags).await;
        let id = token_id(case, &minted);

        let body = ok(
            case,
            &router,
            DELETE,
            &[param("IpamExternalResourceVerificationTokenId", &id)],
        )
        .await;
        let deleted = reported_token(case, &body);
        assert_eq!(
            deleted.state.as_deref(),
            Some(IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS),
            "case {case:?}: the delete answers the token on its way out, got {deleted:?}"
        );
        assert_eq!(
            deleted.token_id, minted.token_id,
            "case {case:?}: the delete answers the token the request named, got {deleted:?}"
        );
        assert_eq!(
            deleted.token_value, minted.token_value,
            "case {case:?}: nothing about the token but its state changed, got {deleted:?}"
        );

        // A second delete has nothing left to delete.
        let (status, second) = invoke(
            &router,
            DELETE,
            &[param("IpamExternalResourceVerificationTokenId", &id)],
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for a second delete, got {status} \
             with {second}"
        );
        assert_eq!(
            error_code(&second),
            INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got {second}"
        );
        assert_eq!(
            error_message(&second),
            resource_state_message(
                IPAM_VERIFICATION_TOKEN_KIND,
                &id,
                IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS
            ),
            "case {case:?}: expected the state refusal's own sentence, got {second}"
        );

        // And the removal the clock owed it.
        poll_until_gone(case, &router, &id).await;
    }
}

/// The rule: a token cannot outlive the IPAM that created it — deleting the IPAM takes its
/// tokens with it.
#[tokio::test]
async fn a_token_does_not_outlive_its_ipam() {
    let case = "an IPAM holding two tokens";
    let router = emulator();
    let ipam = create_ipam(case, &router).await;
    let first = token_id(case, &mint(case, &router, &ipam.id, &[]).await);
    let second = token_id(case, &mint(case, &router, &ipam.id, &[("Name", "second")]).await);

    ok(case, &router, "DeleteIpam", &[param("IpamId", &ipam.id)]).await;
    poll_until_gone(case, &router, &first).await;
    poll_until_gone(case, &router, &second).await;
}

// ---------------------------------------------------------------------------
// The dry run
// ---------------------------------------------------------------------------

/// The rule: a dry run of a request that would have succeeded reports that outcome and changes
/// nothing — on the mint, on the describe and on the delete.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_changes_nothing() {
    let router = emulator();
    let ipam = create_ipam("the IPAM the dry runs name", &router).await;
    let minted = mint("the token the dry runs name", &router, &ipam.id, &[]).await;
    let id = token_id("the token the dry runs name", &minted);

    for (case, action, params) in [
        (
            "a dry-run mint",
            CREATE,
            vec![param("IpamId", &ipam.id), param("DryRun", "true")],
        ),
        (
            "a dry-run describe",
            DESCRIBE,
            vec![param("DryRun", "true")],
        ),
        (
            "a dry-run delete",
            DELETE,
            vec![
                param("IpamExternalResourceVerificationTokenId", &id),
                param("DryRun", "true"),
            ],
        ),
    ] {
        let (status, body) = invoke(&router, action, &params).await;
        assert_eq!(
            status, DRY_RUN_OPERATION_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_OPERATION_STATUS} from {action}, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            DRY_RUN_OPERATION_MESSAGE,
            "case {case:?}: expected the dry run's own sentence, got {body}"
        );
    }

    // Nothing the dry runs named changed: the one token is still there, exactly as it was.
    let body = ok("after the dry runs", &router, DESCRIBE, &[]).await;
    let reported = described_tokens("after the dry runs", &body);
    assert_eq!(
        reported,
        vec![minted],
        "expected the one token the mint stored, unchanged, got {body}"
    );
}
