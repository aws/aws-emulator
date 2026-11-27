// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORGANIZATION ENROLLMENT CONFIGURATIONS
//! (`PutOrganizationEnrollmentConfiguration`,
//! `DeleteOrganizationEnrollmentConfiguration`,
//! `ListOrganizationEnrollmentConfigurations`).
//!
//! The rule under test: an organization enrollment configuration stored on one
//! target — an account, an organizational unit (OU) or the organization root —
//! supplies the EFFECTIVE organization rule mode of every account that target
//! covers (the configuration on the account itself, else the closest parent's,
//! else `NoneAllowed`), and every reader of an account reports that mode
//! together with where it came from. A configuration "take[s] precedence over
//! settings made by `AssociateAccounts` and `DisassociateAccounts`", and after a
//! delete "Automation stays enabled for affected accounts and earlier account
//! associations aren't restored".
//!
//! The rows span what the rule generalises over: both `OrganizationRuleMode`
//! values, both `OrganizationTargetType`s, an account the store already holds a
//! configuration for and one it does not, a target that carries an association
//! and one that does not, the whole life cycle put → read → delete → read, and
//! every page size of the listing.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns. The cases about the organization HIERARCHY (an OU inheriting from its
//! parent, the refusal of a configuration on the management account) need the
//! Organizations control plane mounted beside this one, so they live in the
//! cross-service suite `compute_optimizer_organization_enrollment_hierarchy`.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::account_association::{seed_enrollment, stored_enrollment};
use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, is_next_token, member, string_member,
};

/// Member account ids in the modelled `AccountId` form (`^[0-9]{12}$`) that are
/// not the calling account.
const MEMBER_A: &str = "111111111111";
const MEMBER_B: &str = "222222222222";
const MEMBER_C: &str = "333333333333";

/// Target ids of the OTHER `OrganizationTarget` member, each matching
/// `OrganizationalUnitId`'s modelled
/// `^(r-[0-9a-z]{4,32}|ou-[0-9a-z]{4,32}-[a-z0-9]{8,32})$`: an organization root
/// and two OUs. A configuration is stored on the id the request names, so these
/// need no organization to exist for the STORED-configuration cases; the cases
/// about what an OU INHERITS are the cross-service suite's.
const ROOT: &str = "r-abcd";
const OU_ONE: &str = "ou-abcd-11111111";
const OU_TWO: &str = "ou-abcd-22222222";

/// The one `OrganizationEnrollmentConfigurationType` the model declares
/// (`smithy_query(query="shape",shape="OrganizationEnrollmentConfigurationType")`),
/// spelled out here rather than imported so a wrong value in the implementation
/// cannot make this suite pass.
const CONFIGURATION_TYPE: &str = "OrganizationRuleMode";

/// The `OrganizationRuleMode` values, from
/// `smithy_query(query="shape",shape="OrganizationRuleMode")` — spelled out for
/// the same reason.
const RULE_MODES: [&str; 2] = ["AnyAllowed", "NoneAllowed"];

/// The `OrganizationTargetType` values, from
/// `smithy_query(query="shape",shape="OrganizationTargetType")`.
const TARGET_TYPE_ACCOUNT: &str = "Account";
const TARGET_TYPE_ORGANIZATIONAL_UNIT: &str = "OrganizationalUnit";

/// The `OrganizationRuleModeSourceType` values, from
/// `smithy_query(query="shape",shape="OrganizationRuleModeSourceType")`.
const SOURCE_APPLIED: &str = "Applied";
const SOURCE_DIRECT: &str = "Direct";
const SOURCE_DEFAULT: &str = "Default";

/// A `PutOrganizationEnrollmentConfiguration` request naming `target` and
/// `mode`.
fn put_request(target: &Value, mode: &str) -> Value {
    json!({"target": target, "organizationRuleMode": mode})
}

/// The `accountId` member of the `OrganizationTarget` union.
fn account_target(account_id: &str) -> Value {
    json!({"accountId": account_id})
}

/// The `organizationalUnitId` member of the `OrganizationTarget` union.
fn organizational_unit_target(id: &str) -> Value {
    json!({"organizationalUnitId": id})
}

/// Every stored configuration the listing reports, walked to the end so no case
/// depends on a page size. `target_type` is the request's filter.
async fn list_every_configuration(
    router: &MultiServiceRouter,
    target_type: Option<&str>,
    case: &str,
) -> Vec<Value> {
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(target_type) = target_type {
            request["targetType"] = json!(target_type);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(
            router,
            "ListOrganizationEnrollmentConfigurations",
            &request,
            case,
        )
        .await;
        walked.extend(
            member(&page, "organizationEnrollmentConfigurations")
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= state::MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS,
            "case {case:?}: the walk must terminate, walked {} configurations",
            walked.len()
        );
    }
    walked
}

/// The one configuration the listing reports for `target_id`, or `None`.
fn listed_configuration(listed: &[Value], target_id: &str) -> Option<Value> {
    listed
        .iter()
        .find(|row| string_member(row, "targetId") == target_id)
        .cloned()
}

/// The `AccountInfo` `ListAccounts` reports for `account_id` — the second,
/// independent reader of the effective organization rule mode.
async fn listed_account(router: &MultiServiceRouter, account_id: &str, case: &str) -> Value {
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = call_ok(router, "ListAccounts", &request, case).await;
        let rows = member(&page, "accounts").as_array().cloned().unwrap_or_default();
        if let Some(found) = rows
            .iter()
            .find(|row| string_member(row, "accountId") == account_id)
        {
            return found.clone();
        }
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            panic!("case {case:?}: {account_id} must be listed");
        }
    }
}

/// Assert `account` reports `mode` as its organization rule mode, supplied by
/// `source_type` from `source_target_id`.
fn assert_effective_mode(
    account: &Value,
    mode: &str,
    source_type: &str,
    source_target_id: Option<&str>,
    case: &str,
) {
    assert_eq!(
        string_member(account, "organizationRuleMode"),
        mode,
        "case {case:?}: expected the effective organization rule mode {mode}, got {account}"
    );
    let source = member(account, "organizationRuleModeSource");
    assert_eq!(
        string_member(source, "sourceType"),
        source_type,
        "case {case:?}: expected sourceType {source_type}, got {account}"
    );
    assert_eq!(
        source.get("sourceTargetId").and_then(Value::as_str),
        source_target_id,
        "case {case:?}: expected sourceTargetId {source_target_id:?}, got {account}"
    );
}

// ---------------------------------------------------------------------------
// What a put stores, and what reads it back
// ---------------------------------------------------------------------------

/// A put stores the configuration its target and mode name, and both readers
/// report it: the listing reports one row per stored target with the mode and
/// target type as stored, and `ListAccounts` reports that mode as the effective
/// one of an account target, sourced `Applied` from the target itself.
///
/// The rows span both rule modes, both target types, and an account that
/// carries an association against one that does not.
#[tokio::test]
async fn a_put_stores_the_configuration_every_reader_then_reports() {
    // (case, target, target type, mode) — both modes against both target types,
    // so neither the mode nor the type is reported from the other.
    let rows: Vec<(String, Value, &str, &str)> = vec![
        (
            "an-associated-account-any-allowed".to_string(),
            account_target(MEMBER_A),
            TARGET_TYPE_ACCOUNT,
            RULE_MODES[0],
        ),
        (
            "an-unassociated-account-none-allowed".to_string(),
            account_target(MEMBER_B),
            TARGET_TYPE_ACCOUNT,
            RULE_MODES[1],
        ),
        (
            "an-account-the-store-never-saw".to_string(),
            account_target(MEMBER_C),
            TARGET_TYPE_ACCOUNT,
            RULE_MODES[0],
        ),
        (
            "the-organization-root".to_string(),
            organizational_unit_target(ROOT),
            TARGET_TYPE_ORGANIZATIONAL_UNIT,
            RULE_MODES[1],
        ),
        (
            "an-organizational-unit".to_string(),
            organizational_unit_target(OU_ONE),
            TARGET_TYPE_ORGANIZATIONAL_UNIT,
            RULE_MODES[0],
        ),
    ];

    let (router, engine) = emulator_with_engine();
    // MEMBER_A carries an association, MEMBER_B does not, MEMBER_C is unknown to
    // the store: the put must not depend on any of that.
    seed_enrollment(
        &engine,
        MEMBER_A,
        state::ENROLLED_STATUS,
        None,
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
    );
    seed_enrollment(&engine, MEMBER_B, state::ENROLLED_STATUS, None, None);

    for (case, target, _, mode) in &rows {
        let stored = call_ok(
            &router,
            "PutOrganizationEnrollmentConfiguration",
            &put_request(target, mode),
            case,
        )
        .await;
        assert_eq!(
            stored,
            json!({}),
            "case {case:?}: the response models no member, got {stored}"
        );
    }

    let listed = list_every_configuration(&router, None, "every-stored-configuration").await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case \"every-stored-configuration\": one row per stored target, got {listed:?}"
    );
    for (case, target, target_type, mode) in &rows {
        let target_id = target
            .as_object()
            .and_then(|object| object.values().next())
            .and_then(Value::as_str)
            .unwrap_or_default();
        let row = listed_configuration(&listed, target_id)
            .unwrap_or_else(|| panic!("case {case:?}: {target_id} must be listed, got {listed:?}"));
        assert_eq!(
            string_member(&row, "targetType"),
            *target_type,
            "case {case:?}: the target type is the union member the request named, got {row}"
        );
        assert_eq!(
            string_member(&row, "organizationRuleMode"),
            *mode,
            "case {case:?}: the stored mode must be reported as stored, got {row}"
        );
    }

    // An account target's configuration is the effective mode of that account,
    // sourced from the account itself.
    for (case, account_id, mode) in [
        ("an-associated-account-any-allowed", MEMBER_A, RULE_MODES[0]),
        (
            "an-unassociated-account-none-allowed",
            MEMBER_B,
            RULE_MODES[1],
        ),
    ] {
        let account = listed_account(&router, account_id, case).await;
        assert_effective_mode(&account, mode, SOURCE_APPLIED, Some(account_id), case);
        assert_eq!(
            string_member(&account, "status"),
            state::ENROLLED_STATUS,
            "case {case:?}: a configuration changes no account's enrollment status, got {account}"
        );
    }

    // A target no configuration covers is reported by nothing, and an account
    // nothing has set a mode for is at the default.
    assert!(
        listed_configuration(&listed, OU_TWO).is_none(),
        "case \"an-unconfigured-target\": only stored targets are listed, got {listed:?}"
    );
    seed_enrollment(&engine, "444444444444", state::ENROLLED_STATUS, None, None);
    let untouched = listed_account(&router, "444444444444", "an-unconfigured-account").await;
    assert_effective_mode(
        &untouched,
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        SOURCE_DEFAULT,
        None,
        "an-unconfigured-account",
    );
}

/// An account whose mode was set by an association reports it as `Direct` until
/// a configuration applies; the configuration then takes precedence; and the
/// delete leaves the account with no mode of its own — "earlier account
/// associations aren't restored" — while its enrollment status stays enabled.
#[tokio::test]
async fn a_configuration_overrides_an_association_and_a_delete_does_not_restore_it() {
    let (router, engine) = emulator_with_engine();

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [MEMBER_A]}),
        "associate",
    )
    .await;
    let associated = listed_account(&router, MEMBER_A, "associated").await;
    assert_effective_mode(
        &associated,
        state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
        SOURCE_DIRECT,
        None,
        "associated",
    );

    // The configuration takes precedence over what the association set.
    call_ok(
        &router,
        "PutOrganizationEnrollmentConfiguration",
        &put_request(&account_target(MEMBER_A), "NoneAllowed"),
        "configured",
    )
    .await;
    let configured = listed_account(&router, MEMBER_A, "configured").await;
    assert_effective_mode(
        &configured,
        "NoneAllowed",
        SOURCE_APPLIED,
        Some(MEMBER_A),
        "configured",
    );
    assert_eq!(
        string_member(&configured, "status"),
        state::ENROLLED_STATUS,
        "case \"configured\": the association's enrollment stays enabled, got {configured}"
    );

    // The management account may no longer reach into the account the
    // configuration forbids, so the member's analysed actions leave the
    // retrieval scope.
    let retrievable = call_ok(
        &router,
        "ListRecommendedActions",
        &json!({"filters": [{"name": "AccountId", "values": [MEMBER_A]}]}),
        "configured",
    )
    .await;
    assert_eq!(
        member(&retrievable, "recommendedActions")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case \"configured\": a NoneAllowed configuration takes the member out of the retrieval \
         scope, got {retrievable}"
    );

    // The delete removes the configuration and restores nothing.
    call_ok(
        &router,
        "DeleteOrganizationEnrollmentConfiguration",
        &json!({
            "target": account_target(MEMBER_A),
            "configurationTypes": [CONFIGURATION_TYPE],
        }),
        "deleted",
    )
    .await;
    assert!(
        list_every_configuration(&router, None, "deleted")
            .await
            .is_empty(),
        "case \"deleted\": the delete removes the stored configuration"
    );
    let deleted = listed_account(&router, MEMBER_A, "deleted").await;
    assert_effective_mode(
        &deleted,
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        SOURCE_DEFAULT,
        None,
        "deleted",
    );
    assert_eq!(
        stored_enrollment(&engine, MEMBER_A).status,
        state::ENROLLED_STATUS,
        "case \"deleted\": Automation stays enabled for the affected account"
    );

    // A delete of a target that carries no configuration removes nothing and is
    // not refused: the model declares no not-found error for it.
    call_ok(
        &router,
        "DeleteOrganizationEnrollmentConfiguration",
        &json!({
            "target": organizational_unit_target(OU_ONE),
            "configurationTypes": [CONFIGURATION_TYPE],
        }),
        "delete-an-unconfigured-target",
    )
    .await;
}

// ---------------------------------------------------------------------------
// The listing's filter and its pages
// ---------------------------------------------------------------------------

/// `targetType` selects which stored targets the listing reports — `Account` the
/// account targets, `OrganizationalUnit` the OUs and the organization root, and
/// a request naming none every target type — over one stable sequence that every
/// page size walks identically.
#[tokio::test]
async fn the_listing_filters_on_target_type_and_pages_one_stable_sequence() {
    let (router, _engine) = emulator_with_engine();

    // (target, the type the listing must report it under).
    let targets: Vec<(Value, &str, &str)> = vec![
        (account_target(MEMBER_A), MEMBER_A, TARGET_TYPE_ACCOUNT),
        (account_target(MEMBER_B), MEMBER_B, TARGET_TYPE_ACCOUNT),
        (account_target(MEMBER_C), MEMBER_C, TARGET_TYPE_ACCOUNT),
        (
            organizational_unit_target(ROOT),
            ROOT,
            TARGET_TYPE_ORGANIZATIONAL_UNIT,
        ),
        (
            organizational_unit_target(OU_ONE),
            OU_ONE,
            TARGET_TYPE_ORGANIZATIONAL_UNIT,
        ),
        (
            organizational_unit_target(OU_TWO),
            OU_TWO,
            TARGET_TYPE_ORGANIZATIONAL_UNIT,
        ),
    ];
    for (target, target_id, _) in &targets {
        call_ok(
            &router,
            "PutOrganizationEnrollmentConfiguration",
            &put_request(target, RULE_MODES[0]),
            target_id,
        )
        .await;
    }

    // (case, the filter, the target ids it must report).
    let expected_accounts: Vec<String> = {
        let mut ids: Vec<String> = targets
            .iter()
            .filter(|(_, _, target_type)| *target_type == TARGET_TYPE_ACCOUNT)
            .map(|(_, id, _)| (*id).to_string())
            .collect();
        ids.sort();
        ids
    };
    let expected_units: Vec<String> = {
        let mut ids: Vec<String> = targets
            .iter()
            .filter(|(_, _, target_type)| *target_type == TARGET_TYPE_ORGANIZATIONAL_UNIT)
            .map(|(_, id, _)| (*id).to_string())
            .collect();
        ids.sort();
        ids
    };
    let mut expected_all: Vec<String> = expected_accounts
        .iter()
        .chain(expected_units.iter())
        .cloned()
        .collect();
    expected_all.sort();

    let filters: Vec<(&str, Option<&str>, &Vec<String>)> = vec![
        ("no-filter", None, &expected_all),
        (
            "account-targets",
            Some(TARGET_TYPE_ACCOUNT),
            &expected_accounts,
        ),
        (
            "organizational-unit-targets",
            Some(TARGET_TYPE_ORGANIZATIONAL_UNIT),
            &expected_units,
        ),
    ];

    for (case, filter, expected) in &filters {
        let listed = list_every_configuration(&router, *filter, case).await;
        let ids: Vec<String> = listed
            .iter()
            .map(|row| string_member(row, "targetId"))
            .collect();
        assert_eq!(
            &ids, *expected,
            "case {case:?}: the filter selects the stored targets of that type, in one stable \
             order, got {listed:?}"
        );
    }

    // Every page size walks the unfiltered sequence identically, and the last
    // page carries no token.
    for page_size in 1..=expected_all.len() + 1 {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = call_ok(
                &router,
                "ListOrganizationEnrollmentConfigurations",
                &request,
                &case,
            )
            .await;
            let rows = member(&page, "organizationEnrollmentConfigurations")
                .as_array()
                .cloned()
                .unwrap_or_default();
            let remaining = expected_all.len() - walked.len();
            assert_eq!(
                rows.len(),
                remaining.min(page_size),
                "case {case:?}: a page holds min(maxResults, remaining) rows, got {page}"
            );
            walked.extend(rows.iter().map(|row| string_member(row, "targetId")));
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if let Some(token) = &token {
                assert!(
                    is_next_token(token),
                    "case {case:?}: nextToken must match ^[A-Za-z0-9+/=]+$, got {token:?}"
                );
            }
            pages += 1;
            assert!(
                pages <= expected_all.len() + 1,
                "case {case:?}: the walk must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected_all,
            "case {case:?}: the walk reports every stored target once, in the one order"
        );
    }

    // A token taken for one filter cannot resume the walk under another, and a
    // token this listing never issued is refused rather than answered with a
    // page of something else.
    let first = call_ok(
        &router,
        "ListOrganizationEnrollmentConfigurations",
        &json!({"maxResults": 1, "targetType": TARGET_TYPE_ACCOUNT}),
        "cross-filter",
    )
    .await;
    let account_token = string_member(&first, "nextToken");
    assert!(
        is_next_token(&account_token),
        "case \"cross-filter\": a one-row page of three leaves a further page, got {first}"
    );
    for (case, request) in [
        (
            "another-filters-token",
            json!({"targetType": TARGET_TYPE_ORGANIZATIONAL_UNIT, "nextToken": account_token}),
        ),
        ("not-a-token-at-all", json!({"nextToken": "notatokenofmine"})),
    ] {
        let answer = call(
            &router,
            "ListOrganizationEnrollmentConfigurations",
            &request,
        )
        .await;
        let message = answer
            .1
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_refused(answer, 400, "InvalidParameterValueException", case);
        assert!(
            message.contains("nextToken"),
            "case {case:?}: the refusal must name the member it is about, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals and idempotency
// ---------------------------------------------------------------------------

/// A put naming no configuration at all is refused ("You must specify at least
/// one configuration in the request") and stores nothing; a value outside one of
/// the new operations' enum PARAMETER types is refused with that type's own
/// rejection; and the modelled `maxResults` range of the listing is enforced at
/// each published bound.
#[tokio::test]
async fn a_put_naming_no_configuration_is_refused_and_stores_nothing() {
    let (router, _engine) = emulator_with_engine();

    for (case, target) in [
        ("an-account-target", account_target(MEMBER_A)),
        ("an-organizational-unit-target", organizational_unit_target(OU_ONE)),
    ] {
        let answer = call(
            &router,
            "PutOrganizationEnrollmentConfiguration",
            &json!({"target": target}),
        )
        .await;
        let message = answer
            .1
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_refused(answer, 400, "InvalidParameterValueException", case);
        assert!(
            message.contains("organizationRuleMode"),
            "case {case:?}: the refusal must name the member it is about, got {message:?}"
        );
    }

    // A value outside an enum parameter type's modelled value set is refused
    // with the rejection that type's own: the type, the value as sent, and the
    // type's modelled values. `OrganizationRuleMode`, `OrganizationTargetType`
    // and `OrganizationEnrollmentConfigurationType` are the enum types the
    // organization-configuration operations take.
    let enum_rows: Vec<(&str, &str, Value, &str, &[&str])> = vec![
        (
            "an-unknown-rule-mode",
            "PutOrganizationEnrollmentConfiguration",
            json!({"target": account_target(MEMBER_A), "organizationRuleMode": "SomeAllowed"}),
            "OrganizationRuleMode",
            &RULE_MODES,
        ),
        (
            "an-unknown-target-type",
            "ListOrganizationEnrollmentConfigurations",
            json!({"targetType": "Organization"}),
            "OrganizationTargetType",
            &[TARGET_TYPE_ACCOUNT, TARGET_TYPE_ORGANIZATIONAL_UNIT],
        ),
        (
            "an-unknown-configuration-type",
            "DeleteOrganizationEnrollmentConfiguration",
            json!({
                "target": account_target(MEMBER_A),
                "configurationTypes": ["OrganizationAutoEnroll"],
            }),
            "OrganizationEnrollmentConfigurationType",
            &[CONFIGURATION_TYPE],
        ),
    ];
    for (case, operation, request, enum_type, values) in enum_rows {
        let answer = call(&router, operation, &request).await;
        let message = answer
            .1
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_refused(answer, 400, "InvalidParameterValueException", case);
        assert!(
            message.starts_with(&format!("Invalid {enum_type} value: ")),
            "case {case:?}: the refusal must name the enum type whose set the value missed, got \
             {message:?}"
        );
        for value in values {
            assert!(
                message.contains(value),
                "case {case:?}: the refusal must enumerate the type's modelled values, got \
                 {message:?}"
            );
        }
    }

    assert!(
        list_every_configuration(&router, None, "after-the-refusals")
            .await
            .is_empty(),
        "case \"after-the-refusals\": a refused put stores nothing"
    );

    // `ListOrganizationEnrollmentConfigurationsRequest$maxResults` carries
    // `@range(min: 1, max: 1000)`
    // (`smithy_query(query="operation",operation="ListOrganizationEnrollmentConfigurations")`),
    // enforced by the generated request walk: each published bound is served and
    // a value past either end is refused.
    for (case, max_results, expected) in [
        ("at-the-range-minimum", 1, 200),
        ("at-the-range-maximum", 1000, 200),
        ("below-the-range-minimum", 0, 400),
        ("above-the-range-maximum", 1001, 400),
    ] {
        let (status, body) = call(
            &router,
            "ListOrganizationEnrollmentConfigurations",
            &json!({"maxResults": max_results}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: maxResults={max_results} must answer HTTP {expected}, got {status} \
             and {body}"
        );
    }
}

/// One row of the caller's own-enrollment table: the case name, the mode a put
/// configured for the caller (or none), and the mode, source type and source
/// target the readers must then report.
type OwnEnrollmentRow<'a> = (
    &'a str,
    Option<&'a str>,
    &'a str,
    &'a str,
    Option<&'a str>,
);

/// The caller's OWN account reads its effective organization rule mode back
/// through `GetEnrollmentConfiguration`, and that read omits
/// `organizationEnrollmentConfigurationStatus` for an account that is in no
/// organization — the member "is omitted for management accounts and accounts
/// that aren't in an organization".
#[tokio::test]
async fn the_callers_own_enrollment_read_reports_the_effective_mode() {
    let (router, _engine) = emulator_with_engine();
    let caller = state::caller_account_id();

    // (case, what is stored for the caller, the mode and source it must report).
    let rows: Vec<OwnEnrollmentRow<'_>> = vec![
        (
            "nothing-configured",
            None,
            state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
            SOURCE_DEFAULT,
            None,
        ),
        (
            "configured-any-allowed",
            Some(RULE_MODES[0]),
            RULE_MODES[0],
            SOURCE_APPLIED,
            Some(caller.as_str()),
        ),
        (
            "configured-none-allowed",
            Some(RULE_MODES[1]),
            RULE_MODES[1],
            SOURCE_APPLIED,
            Some(caller.as_str()),
        ),
    ];

    for (case, configured, mode, source_type, source_target_id) in rows {
        if let Some(configured) = configured {
            call_ok(
                &router,
                "PutOrganizationEnrollmentConfiguration",
                &put_request(&account_target(&caller), configured),
                case,
            )
            .await;
        }
        let read = call_ok(&router, "GetEnrollmentConfiguration", &json!({}), case).await;
        assert_eq!(
            string_member(&read, "organizationRuleMode"),
            mode,
            "case {case:?}: the read must report the effective rule mode, got {read}"
        );
        assert_eq!(
            read.get("organizationEnrollmentConfigurationStatus"),
            None,
            "case {case:?}: the member is omitted for an account that is in no organization, \
             got {read}"
        );
        // The listing of accounts reports the same mode and source for the same
        // account, so the two readers cannot drift apart.
        assert_effective_mode(
            &listed_account(&router, &caller, case).await,
            mode,
            source_type,
            source_target_id,
            case,
        );
    }
}

/// A replayed `clientToken` makes the put and the delete idempotent, the same
/// token with different parameters is refused
/// `IdempotentParameterMismatchException`, and a token another operation used is
/// refused `IdempotencyTokenInUseException`.
#[tokio::test]
async fn a_client_token_makes_the_put_and_the_delete_idempotent() {
    let (router, engine) = emulator_with_engine();
    seed_enrollment(&engine, MEMBER_A, state::ENROLLED_STATUS, None, None);

    let token = "organization-configuration-token";
    let mut request = put_request(&account_target(MEMBER_A), RULE_MODES[0]);
    request["clientToken"] = json!(token);

    call_ok(
        &router,
        "PutOrganizationEnrollmentConfiguration",
        &request,
        "first-put",
    )
    .await;
    let after_first = listed_account(&router, MEMBER_A, "first-put").await;
    call_ok(
        &router,
        "PutOrganizationEnrollmentConfiguration",
        &request,
        "replayed-put",
    )
    .await;
    assert_eq!(
        listed_account(&router, MEMBER_A, "replayed-put").await,
        after_first,
        "case \"replayed-put\": a replay stores nothing again, so no timestamp moves"
    );

    // The same token, a different request.
    let mut mismatched = put_request(&account_target(MEMBER_A), RULE_MODES[1]);
    mismatched["clientToken"] = json!(token);
    assert_refused(
        call(
            &router,
            "PutOrganizationEnrollmentConfiguration",
            &mismatched,
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        "a-different-request-under-one-token",
    );

    // The same token, a different operation.
    assert_refused(
        call(
            &router,
            "DeleteOrganizationEnrollmentConfiguration",
            &json!({
                "target": account_target(MEMBER_A),
                "configurationTypes": [CONFIGURATION_TYPE],
                "clientToken": token,
            }),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "another-operation-under-one-token",
    );

    // The refused calls left the stored configuration exactly where it was.
    assert_effective_mode(
        &listed_account(&router, MEMBER_A, "after-the-refusals").await,
        RULE_MODES[0],
        SOURCE_APPLIED,
        Some(MEMBER_A),
        "after-the-refusals",
    );

    // The delete under its OWN token is idempotent too: the replay removes
    // nothing again and the target stays unconfigured.
    let delete = json!({
        "target": account_target(MEMBER_A),
        "configurationTypes": [CONFIGURATION_TYPE],
        "clientToken": "organization-configuration-delete-token",
    });
    for case in ["first-delete", "replayed-delete"] {
        call_ok(
            &router,
            "DeleteOrganizationEnrollmentConfiguration",
            &delete,
            case,
        )
        .await;
        assert!(
            list_every_configuration(&router, None, case).await.is_empty(),
            "case {case:?}: the target carries no configuration"
        );
    }
}
