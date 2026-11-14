// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteDhcpOptions`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteDhcpOptions`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rules under test are:
//!
//! 1. the delete TAKES THE OPTION SET THE REQUEST NAMES OUT OF THE ONE STORE, so every later
//!    caller sees it gone: `DescribeDhcpOptions` no longer reports it, every other set of the
//!    account is left where it was, and the bytes the removed record held go back to the
//!    store's budget;
//! 2. this delete READS the id before it resolves anything, so a value this service could
//!    never have ISSUED as a DHCP options set id is refused BEFORE the lookup with
//!    `InvalidDhcpOptionsId.Malformed` naming it, while an id it DID issue that the account
//!    holds no set under — the id of a set an earlier call deleted, or one the mint spent on
//!    another resource — is refused with this operation's own `InvalidDhcpOptionID.NotFound`;
//!    either way nothing is removed;
//! 3. `DryRun` asks whether the request WOULD have succeeded, so it is answered only once the
//!    set the request names has been resolved: a dry run of a delete that would have worked
//!    reports `DryRunOperation` and removes nothing.
//!
//! Each table row is a distinct input along what the rules generalise over — which options
//! the deleted set carries and which of several sets of the account is deleted, and each way
//! a value can fail to name a set this service issued (both published suffix lengths, the
//! all-zero placeholder, a leading digit the issued long form never carries, upper case,
//! non-hexadecimal text, an identifier above everything the mint reached, another resource's
//! prefix, free text, XML metacharacters) — so no one recorded call is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DHCP_OPTIONS_ID_PREFIX, DHCP_OPTION_DOMAIN_NAME,
    DHCP_OPTION_DOMAIN_NAME_SERVERS, DHCP_OPTION_NTP_SERVERS, DRY_RUN_OPERATION,
    INVALID_DHCP_OPTIONS_ID_MALFORMED, INVALID_DHCP_OPTION_ID_NOT_FOUND,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{document, param, Param};

/// The Region every call here is signed for: a DHCP option set belongs to the account rather
/// than to one Region, so the Region only selects the ec2Query envelope.
const REGION: &str = ec2::state::DEFAULT_REGION;

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `DeleteDhcpOptions` call naming `dhcp_options_id`.
async fn delete_set(router: &MultiServiceRouter, dhcp_options_id: &str) -> (u16, String) {
    api(
        router,
        "DeleteDhcpOptions",
        &[param("DhcpOptionsId", dhcp_options_id)],
    )
    .await
}

/// The `DhcpConfiguration.N.Key` / `DhcpConfiguration.N.Value.M` form fields for a list of
/// options, as ec2Query flattens a list of structures into indexed keys.
fn configuration_params(options: &[(&str, Vec<&str>)]) -> Vec<Param> {
    let mut fields = Vec::new();
    for (index, (key, values)) in options.iter().enumerate() {
        let nth = index + 1;
        fields.push(param(&format!("DhcpConfiguration.{nth}.Key"), key));
        fields.extend(indexed(&format!("DhcpConfiguration.{nth}.Value"), values));
    }
    fields
}

/// A DHCP option set `CreateDhcpOptions` mints carrying `options`, as its id.
async fn create_set(case: &str, router: &MultiServiceRouter, options: &[(&str, Vec<&str>)]) -> String {
    let body = ok(
        case,
        router,
        "CreateDhcpOptions",
        &configuration_params(options),
    )
    .await;
    element(&body, "dhcpOptionsId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no dhcpOptionsId: {body}"))
}

/// Every `dhcpOptionsId` `DescribeDhcpOptions` reports for the account, sorted so two reads
/// compare by membership rather than by the order the store happens to walk.
async fn held_sets(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(case, router, "DescribeDhcpOptions", &[]).await;
    let mut ids = elements(&body, "dhcpOptionsId");
    ids.sort();
    ids
}

/// The caller-chosen bytes the store is holding, read from the state the emulator serves
/// from: the budget a delete has to give back.
fn stored_bytes(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().stored_bytes()
}

/// The `<Message>` of a refusal, read out of the PARSED XML envelope: the body has to be
/// well-formed XML and the element's own text is what is answered, so a caller-controlled
/// value the service wrote into the markup unescaped cannot read back as the message it
/// belongs to.
fn refusal_message(case: &str, body: &str) -> String {
    document(case, body)
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "Message")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"))
        .to_string()
}

/// The refusal a value this service could never have ISSUED as a DHCP options set id gets.
fn malformed_message(dhcp_options_id: &str) -> String {
    format!("The dhcp-options ID '{dhcp_options_id}' is malformed")
}

/// The refusal an id this service DID issue that names no option set of the account gets.
fn absent_message(dhcp_options_id: &str) -> String {
    format!("The dhcpOption ID '{dhcp_options_id}' does not exist")
}

/// One row of [`option_rows`]: the case it names and the options the set it creates carries,
/// as (option name, values).
type OptionRow = (&'static str, Vec<(&'static str, Vec<&'static str>)>);

/// What makes two stored option sets differ in the caller-chosen bytes they hold, which is
/// what the delete has to give back: the options the set carries and how many values each of
/// them has. The option names are the ones `CreateDhcpOptions` publishes
/// (`smithy_query(query="doc",shape="CreateDhcpOptions")`, read through
/// [`ec2::state::DHCP_OPTION_DOMAIN_NAME`] and its neighbours).
fn option_rows() -> Vec<OptionRow> {
    vec![
        (
            "a set with one option and one value",
            vec![(DHCP_OPTION_DOMAIN_NAME, vec!["one.internal"])],
        ),
        (
            "a set with one option and several values",
            vec![(
                DHCP_OPTION_DOMAIN_NAME_SERVERS,
                vec!["10.40.0.2", "10.40.0.3", "10.40.0.4"],
            )],
        ),
        (
            "a set with several options",
            vec![
                (DHCP_OPTION_DOMAIN_NAME, vec!["many.internal"]),
                (DHCP_OPTION_DOMAIN_NAME_SERVERS, vec!["10.41.0.2"]),
                (DHCP_OPTION_NTP_SERVERS, vec!["10.41.0.3", "10.41.0.4"]),
            ],
        ),
    ]
}

/// Each way a value can be one this service could never have ISSUED as a DHCP options set
/// id: the all-zero placeholder a client reaches for in both published EC2 id suffix lengths,
/// a suffix of neither length, the published SHORT form (this service's mint counts in the
/// long one), a suffix whose leading digit is not `0` (the long form carries 68 bits for a
/// 64-bit identifier, so an issued id always reads `dopt-0…`), an upper-case suffix, a
/// non-hexadecimal suffix, a prefix with no suffix at all, two well-formed long-form ids
/// astronomically above everything the mint has counted, another resource type's id, free
/// text, and a value carrying the characters that are significant in the XML envelope the
/// refusal is written into.
fn unissued_ids() -> Vec<(&'static str, String)> {
    let prefix = DHCP_OPTIONS_ID_PREFIX;
    vec![
        ("the all-zero long form", format!("{prefix}-00000000000000000")),
        ("the all-zero short form", format!("{prefix}-00000000")),
        ("neither length", format!("{prefix}-0000000000000000")),
        ("the published short form", format!("{prefix}-0000beef")),
        (
            "a suffix outside the issued long form",
            format!("{prefix}-123456789abcdef01"),
        ),
        ("an upper-case suffix", format!("{prefix}-0000000000000BEEF")),
        ("a non-hex suffix", format!("{prefix}-zzzzzzzz")),
        ("no suffix at all", format!("{prefix}-")),
        (
            "an id the mint has not reached",
            format!("{prefix}-0123456789abcdef0"),
        ),
        (
            "another id the mint has not reached",
            format!("{prefix}-0fedcba987654321f"),
        ),
        ("another resource's id", "vpc-0123456789abcdef1".to_string()),
        ("free text", "not-a-dhcp-options-id".to_string()),
        ("XML metacharacters", "dopt-<tag>&'\"end".to_string()),
    ]
}

// ---------------------------------------------------------------------------
// RULE 1: the store is what is deleted
// ---------------------------------------------------------------------------

/// RULE 1: deleting a set removes exactly it — the set is gone for every later caller, every
/// other set of the account is left where it was, and the caller-chosen bytes it held go back
/// to the store's budget. One row per option shape, each deleted out of an account that holds
/// a second set beside it.
#[tokio::test]
async fn deleting_an_option_set_takes_exactly_it_out_of_the_store() {
    for (case, options) in option_rows() {
        let (router, engine) = super::ec2_query::emulator();
        let before = stored_bytes(&engine);
        let kept = create_set(case, &router, &[(DHCP_OPTION_DOMAIN_NAME, vec!["kept.internal"])]).await;
        let kept_held = stored_bytes(&engine);
        let doomed = create_set(case, &router, &options).await;

        let held = held_sets(case, &router).await;
        assert!(
            held.contains(&doomed) && held.contains(&kept),
            "case {case:?}: expected the account to hold {doomed} and {kept} before the \
             delete, got {held:?}"
        );

        let (status, body) = delete_set(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {doomed}, got {status} with body {body}"
        );

        let after = held_sets(case, &router).await;
        assert!(
            !after.contains(&doomed),
            "case {case:?}: expected {doomed} to be gone, got {after:?}"
        );
        assert!(
            after.contains(&kept),
            "case {case:?}: expected {kept} to be left where it was, got {after:?}"
        );
        // Read back through the id too: a describe that NAMES the deleted set must refuse it
        // rather than report an empty set, which a caller could not tell from a set with no
        // options.
        let (status, body) = api(
            &router,
            "DescribeDhcpOptions",
            &indexed("DhcpOptionsId", &[doomed.as_str()]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the describe of the deleted {doomed} to be refused, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_DHCP_OPTION_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_DHCP_OPTION_ID_NOT_FOUND} describing the \
             deleted {doomed}, got {body}"
        );

        let left = stored_bytes(&engine);
        assert_eq!(
            left, kept_held,
            "case {case:?}: expected the bytes {doomed} held back in the budget (the account \
             holds only {kept} now), got {left} where {kept_held} was held before it and \
             {before} by the empty store"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a value the mint never issued, against an id it did
// ---------------------------------------------------------------------------

/// RULE 2, first half: a value this service could never have ISSUED is refused BEFORE the
/// lookup — HTTP 400 `InvalidDhcpOptionsId.Malformed`, naming the value the request sent —
/// and nothing is removed. One row per way a value can be one the mint could never have
/// handed out.
#[tokio::test]
async fn a_dhcp_options_id_the_mint_never_issued_is_refused_as_malformed() {
    let (router, _engine) = super::ec2_query::emulator();
    let held = create_set(
        "the malformed table's set",
        &router,
        &[(DHCP_OPTION_DOMAIN_NAME, vec!["malformed.internal"])],
    )
    .await;
    let before = held_sets("before the malformed table", &router).await;

    for (case, dhcp_options_id) in unissued_ids() {
        let (status, body) = delete_set(&router, &dhcp_options_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_DHCP_OPTIONS_ID_MALFORMED,
            "case {case:?}: expected {INVALID_DHCP_OPTIONS_ID_MALFORMED}, got {code:?} with \
             body {body}"
        );
        // Read the message out of the PARSED envelope rather than by substring: a value whose
        // metacharacters reached the markup verbatim would either stop the body from parsing
        // or cut the element's text short at that character, so this is what pins that a
        // caller-controlled id is written into the envelope escaped.
        let message = refusal_message(case, &body);
        assert_eq!(
            message,
            malformed_message(&dhcp_options_id),
            "case {case:?}: expected this operation's malformed sentence, got {message:?}"
        );
        assert_eq!(
            message,
            error_message(&body),
            "case {case:?}: the parsed message and the one on the wire must agree, got \
             {message:?} and {:?}",
            error_message(&body)
        );
    }

    let after = held_sets("after the malformed table", &router).await;
    assert_eq!(
        after, before,
        "case \"after the malformed table\": a refused delete must remove nothing, so the \
         account must still hold {before:?} (including {held}), got {after:?}"
    );
}

/// RULE 2, second half: a delete is not idempotent — the set is gone after the first call, so
/// a second call names an id this service DID issue that the account holds no set under, and
/// is answered with this operation's own not-found code naming that id. One row per option
/// shape.
#[tokio::test]
async fn deleting_the_same_option_set_twice_refuses_the_second_call_as_absent() {
    for (case, options) in option_rows() {
        let (router, _engine) = super::ec2_query::emulator();
        let dhcp_options_id = create_set(case, &router, &options).await;

        let (status, body) = delete_set(&router, &dhcp_options_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete expected HTTP 200, got {status} with body {body}"
        );

        let (status, body) = delete_set(&router, &dhcp_options_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the second delete expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_DHCP_OPTION_ID_NOT_FOUND,
            "case {case:?}: the second delete expected {INVALID_DHCP_OPTION_ID_NOT_FOUND}, \
             got {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            absent_message(&dhcp_options_id),
            "case {case:?}: the second delete must name {dhcp_options_id}, got {message:?}"
        );
    }
}

/// RULE 2, the line between its two halves: what separates "not an options set id" from "no
/// such options set" is what the MINT has issued, not how the value is spelt — an id whose
/// identifier the mint spent on another resource of the account is spelt exactly like an
/// option set's own and is answered as absent, where the same spelling above everything the
/// mint reached is answered as malformed.
#[tokio::test]
async fn an_id_the_mint_spent_on_another_resource_is_refused_as_absent() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "an identifier the mint issued to a VPC";
    let body = ok(
        case,
        &router,
        "CreateVpc",
        &[param("CidrBlock", "10.42.0.0/16")],
    )
    .await;
    let vpc_id = element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no vpcId: {body}"));
    let identifier = vpc_id
        .rsplit_once('-')
        .map(|(_prefix, suffix)| suffix.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: {vpc_id} carries no identifier"));
    let spent = format!("{DHCP_OPTIONS_ID_PREFIX}-{identifier}");

    let (status, body) = delete_set(&router, &spent).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_DHCP_OPTION_ID_NOT_FOUND,
        "case {case:?}: expected {INVALID_DHCP_OPTION_ID_NOT_FOUND} for {spent}, got {code:?} \
         with body {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        absent_message(&spent),
        "case {case:?}: expected the absent sentence naming {spent}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the dry run
// ---------------------------------------------------------------------------

/// RULE 3: a dry run of a delete that WOULD have succeeded reports `DryRunOperation` and
/// removes nothing, while a dry run naming a value the operation cannot resolve is refused
/// with THAT reason instead — the order every operation that acts on the one resource it
/// names keeps.
#[tokio::test]
async fn a_dry_run_reports_the_permitted_outcome_without_deleting() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "a dry run of a delete that would have succeeded";
    let dhcp_options_id = create_set(
        case,
        &router,
        &[(DHCP_OPTION_DOMAIN_NAME, vec!["dry.internal"])],
    )
    .await;

    let (status, body) = api(
        &router,
        "DeleteDhcpOptions",
        &[
            param("DhcpOptionsId", &dhcp_options_id),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, DRY_RUN_OPERATION,
        "case {case:?}: expected {DRY_RUN_OPERATION}, got {code:?} with body {body}"
    );
    let held = held_sets(case, &router).await;
    assert!(
        held.contains(&dhcp_options_id),
        "case {case:?}: expected the dry run to remove nothing, so the account must still \
         hold {dhcp_options_id}, got {held:?}"
    );

    // The same dry run over a value the operation cannot resolve at all: the refusal is the
    // one the value earns, not the permission report.
    let unresolvable = format!("{DHCP_OPTIONS_ID_PREFIX}-0123456789abcdef0");
    let (status, body) = api(
        &router,
        "DeleteDhcpOptions",
        &[
            param("DhcpOptionsId", &unresolvable),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dry run naming a value the mint never issued\": expected HTTP 400, got \
         {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_DHCP_OPTIONS_ID_MALFORMED,
        "case \"a dry run naming a value the mint never issued\": expected \
         {INVALID_DHCP_OPTIONS_ID_MALFORMED}, got {code:?} with body {body}"
    );
}
