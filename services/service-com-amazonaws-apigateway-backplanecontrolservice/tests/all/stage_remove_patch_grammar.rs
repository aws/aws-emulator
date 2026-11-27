// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `remove` GRAMMAR of `UpdateStage`, for the pointer ENUMERATION
//! its method-setting-path refusal prints, and for the sentence a method setting refuses an
//! unauthorized-cache-control strategy outside its declared set with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `CreateRestApi`/`CreateDeployment`
//! helpers, the patch-entry spellings and the `GetStage` reader are this crate's existing
//! harness.
//!
//! The rules under test, each over a TABLE of inputs rather than one call:
//!
//! * a `remove` names a part of a stage that can go AWAY — a method's settings, a nested
//!   settings structure, an entry of a collection the stage keys by name — so a pointer
//!   naming none of them, every single-valued member of the `Stage` shape among them, is
//!   refused `BadRequestException` (HTTP 400) with the one pointer template the operation
//!   publishes for that op, and the stage is left exactly as it was. The same member takes
//!   a `replace`, which is how a caller changes it — and how a caller takes an optional one
//!   back off, by replacing it with no value at all;
//! * the enumeration a refused method-setting path prints is the SERVICE's own fixed text:
//!   it names the `/*/*` stage-wide method settings and both `accessLogSettings` pointers,
//!   runs its throttling/caching entries together the way the service prints them, and names
//!   no `/canarySettings` pointer and no `/documentationVersion` — none of which follows from
//!   which pointers the operation can apply;
//! * a method setting's `unauthorizedCacheControlHeaderStrategy` takes the values its enum
//!   declares and refuses every other with one fixed sentence of the service's own.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES;

use crate::client_certificates::generated;
use crate::patch_entry_precedence::stage_invalid_patch_path;
use crate::patch_updates::{op, patch, patch_ok, replace};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};

/// The one pointer TEMPLATE `UpdateStage` publishes for a `remove`: a method's settings path,
/// `{resource_path}/{http_method}` as `Stage$methodSettings` states one.
///
/// Stated here rather than read from the crate, so the assertion is independent of the
/// constant under test (`METHOD_SETTINGS_PATH_TEMPLATE` in src/ops/update_stage.rs).
const METHOD_SETTINGS_PATH_TEMPLATE: &str = "/{resourcePath}/{httpMethod}";

/// The sentence a stage refuses a `remove` naming no removable part of it with: the fixed
/// frame that quotes the rejected path and the op that named it, and then the one template
/// above — the two spaces after `path` are the service's own.
///
/// `pub(crate)` so the sibling `stage_canary_and_patch_grammar` suite asserts the one
/// sentence rather than keeping a copy of it.
pub(crate) fn remove_outside_the_grammar(path: &str) -> String {
    format!(
        "Invalid patch path  '{path}' specified for op 'remove'. Must be one of: \
         {METHOD_SETTINGS_PATH_TEMPLATE}"
    )
}

/// `UpdateStage` for the `prod` stage of `api` with `operations`.
async fn patch_stage(gateway: &Gateway, api: &str, operations: Vec<Value>) -> Answer {
    patch(gateway, &format!("/restapis/{api}/stages/prod"), operations).await
}

/// `UpdateStage` for that stage, asserting the modelled 200 and answering the body.
async fn patch_stage_ok(
    gateway: &Gateway,
    api: &str,
    case: &str,
    operations: Vec<Value>,
) -> Value {
    patch_ok(gateway, case, &format!("/restapis/{api}/stages/prod"), operations).await
}

/// The stage body `GetStage` reports, asserting the modelled 200.
async fn stage(gateway: &Gateway, api: &str, case: &str) -> Value {
    let read = get_stage(gateway, api, "prod").await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from GetStage, got {} with {:?}",
        read.status, read.body
    );
    read.body
}

/// One row of the `remove` table: the pointer, and the `replace` that gives the member the
/// pointer names a value first — so a `remove` that was applied would show as the loss of
/// that value rather than as nothing at all.
struct Removal<'a> {
    /// The pointer the `remove` names.
    path: &'static str,
    /// The value a `replace` of the same pointer sets, when the pointer names a member a
    /// `replace` takes a value for.
    set: Option<&'a str>,
}

#[tokio::test]
async fn a_remove_naming_no_removable_part_of_a_stage_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-remove-grammar").await;
    let deployment = create_deployed_stage(&gateway, &api, "prod").await;
    // A certificate of the ACCOUNT, so the row that assigns one to the stage names a record
    // this service holds.
    let certificate = string_member(
        &generated(&gateway, "GenerateClientCertificate", json!({})).await,
        "clientCertificateId",
        "GenerateClientCertificate",
    );

    // Rows span what the rule generalises over: every single-valued member of the `Stage`
    // shape this operation patches — the deployment it serves, three strings, two booleans
    // and the cache capacity — plus a member the SERVICE assigns and no patch writes, a
    // pointer no shape declares at all, and the collection member named with no entry after
    // it, which names no entry to take away either. Each is spelled as the one reference
    // token it is: a pointer carrying more cannot be refused this way, because it can spell
    // a method path (see the rows of `patch_entry_precedence`).
    let rows: Vec<Removal<'_>> = vec![
        Removal {
            path: "/deploymentId",
            set: Some(deployment.as_str()),
        },
        Removal {
            path: "/description",
            set: Some("the stage this suite patches"),
        },
        Removal {
            path: "/documentationVersion",
            set: None,
        },
        Removal {
            path: "/clientCertificateId",
            set: Some(certificate.as_str()),
        },
        Removal {
            path: "/tracingEnabled",
            set: Some("true"),
        },
        Removal {
            path: "/cacheClusterEnabled",
            set: None,
        },
        Removal {
            path: "/cacheClusterSize",
            set: Some("1.6"),
        },
        Removal {
            path: "/stageName",
            set: None,
        },
        Removal {
            path: "/sim2realUndeclaredMember",
            set: None,
        },
        Removal {
            path: "/variables",
            set: None,
        },
    ];

    for row in &rows {
        // The other side of the rule first, where the member takes one: a `replace` of the
        // SAME pointer is applied, so what the `remove` earns below is the op and not the
        // pointer.
        if let Some(value) = row.set {
            let case = format!("UpdateStage(replace {} = {value:?})", row.path);
            patch_stage_ok(&gateway, &api, &case, vec![replace(row.path, value)]).await;
        }
    }

    let before = stage(&gateway, &api, "GetStage before the refused removes").await;
    for row in &rows {
        let case = format!("UpdateStage(remove {})", row.path);
        let answer = patch_stage(&gateway, &api, vec![op("remove", row.path, None)]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let expected = remove_outside_the_grammar(row.path);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {:?}",
            answer.body["message"]
        );
        let after = stage(&gateway, &api, &case).await;
        assert_eq!(
            after, before,
            "case {case}: a refused remove must leave the stage as it was ({before:?}), got \
             {after:?}"
        );
    }

    // And a list carrying one such `remove` after ops the stage WOULD apply leaves the stage
    // untouched too: the whole patch list is refused rather than half applied.
    let case = "UpdateStage(an applied replace followed by a refused remove)";
    let answer = patch_stage(
        &gateway,
        &api,
        vec![
            replace("/description", "changed by the refused list"),
            op("remove", "/description", None),
        ],
    )
    .await;
    assert_failure(&answer, 400, "BadRequestException", case);
    let after = stage(&gateway, &api, case).await;
    assert_eq!(
        after, before,
        "case {case}: the refused list must leave the stage as it was ({before:?}), got \
         {after:?}"
    );
}

#[tokio::test]
async fn an_optional_member_a_remove_cannot_clear_is_cleared_by_a_valueless_replace() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-remove-alternative").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // The optional single-valued members a caller takes back off a stage, and the two
    // spellings of a `replace` that names no value — the documented unset — which is how
    // that is done where the `remove` above is refused.
    let members = [
        ("/description", "description", "set by this suite"),
        ("/documentationVersion", "documentationVersion", "v1"),
    ];
    for (path, member, value) in members {
        for (spelling, unset) in [
            ("an empty value", replace(path, "")),
            ("no value member at all", op("replace", path, None)),
        ] {
            let case = format!("UpdateStage(replace {path} with {spelling})");
            patch_stage_ok(&gateway, &api, &case, vec![replace(path, value)]).await;
            let set = stage(&gateway, &api, &case).await;
            assert_eq!(
                set[member],
                json!(value),
                "case {case}: expected {member} to be {value:?} before the unset, got {:?}",
                set[member]
            );

            let cleared = patch_stage_ok(&gateway, &api, &case, vec![unset]).await;
            assert_eq!(
                cleared[member],
                Value::Null,
                "case {case}: expected {member} gone, got {:?}",
                cleared[member]
            );
            let read = stage(&gateway, &api, &case).await;
            assert_eq!(
                read[member],
                Value::Null,
                "case {case}: GetStage must read back {member} as gone, got {:?}",
                read[member]
            );
        }
    }
}

#[tokio::test]
async fn the_parts_of_a_stage_a_remove_takes_away_are_applied_and_read_back() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-remove-applied").await;
    let deployment = create_deployed_stage(&gateway, &api, "prod").await;

    // Everything a `remove` of this stage can take away is configured first, over the parts
    // the rule names: an entry of each collection the stage keys by name, a sub-member and
    // the whole of each nested structure, one setting of a method and the method's whole
    // entry.
    let configured = vec![
        op("add", "/variables/table", Some("pets")),
        op(
            "add",
            "/canarySettings/stageVariableOverrides/table",
            Some("pets-canary"),
        ),
        replace("/canarySettings/deploymentId", &deployment),
        replace(
            "/accessLogSettings/destinationArn",
            "arn:aws:logs:us-east-1:000000000000:log-group:api-access",
        ),
        replace("/accessLogSettings/format", "$context.requestId"),
        replace("/*/*/caching/ttlInSeconds", "60"),
        replace("/~1pets/GET/metrics/enabled", "true"),
    ];
    patch_stage_ok(
        &gateway,
        &api,
        "UpdateStage(configure every removable part)",
        configured,
    )
    .await;

    // Rows differ along the KIND of part the pointer names, and each states the pointer of
    // the reported stage that must be gone afterwards — spelled with `/` inside one key
    // escaped the way a JSON Pointer spells it.
    let rows: Vec<(&str, &str)> = vec![
        ("/variables/table", "/variables/table"),
        (
            "/canarySettings/stageVariableOverrides/table",
            "/canarySettings/stageVariableOverrides",
        ),
        ("/accessLogSettings/destinationArn", "/accessLogSettings/destinationArn"),
        ("/accessLogSettings", "/accessLogSettings"),
        ("/~1pets/GET", "/methodSettings/~01pets~1GET"),
        ("/canarySettings", "/canarySettings"),
    ];
    for (path, pointer) in rows {
        let case = format!("UpdateStage(remove {path})");
        let updated = patch_stage_ok(&gateway, &api, &case, vec![op("remove", path, None)]).await;
        let reported = updated.pointer(pointer).cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported,
            Value::Null,
            "case {case}: expected {pointer} gone, got {reported:?}"
        );
        let read = stage(&gateway, &api, &case).await;
        let reported = read.pointer(pointer).cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported,
            Value::Null,
            "case {case}: GetStage must read back {pointer} as gone, got {reported:?}"
        );
    }

    // One setting of a method the stage still carries settings for goes back to the value a
    // stage carries for a setting nobody configured — the documented default TTL of 300
    // seconds — rather than out of the entry.
    let case = "UpdateStage(remove one setting of a method)";
    let updated = patch_stage_ok(
        &gateway,
        &api,
        case,
        vec![op("remove", "/*/*/caching/ttlInSeconds", None)],
    )
    .await;
    assert_eq!(
        updated["methodSettings"]["*/*"]["cacheTtlInSeconds"],
        json!(300),
        "case {case}: expected the documented default TTL, got {:?}",
        updated["methodSettings"]
    );
}

#[tokio::test]
async fn a_refused_method_setting_path_prints_the_services_own_pointer_enumeration() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-method-setting-enumeration").await;
    create_deployed_stage(&gateway, &api, "prod").await;

    // Rows differ along the FORM of the rejected pointer: a member no shape declares, a
    // member the service assigns, a method path that escapes the separator before the method
    // too (so it names the method in one token and the setting after it is no setting's own
    // name), a setting with no method path before it, and a method path with no setting after
    // it. The `op` is the one a stage publishes a path set for.
    let paths = [
        "/sim2realUndeclaredMember",
        "/stageName",
        "/~1pets~1GET/throttling/rateLimit",
        "/metrics/enabled",
        "/~1pets/GET/throttling",
    ];
    for path in paths {
        let case = format!("UpdateStage(replace {path})");
        let answer = patch_stage(&gateway, &api, vec![replace(path, "1")]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        let expected = stage_invalid_patch_path(path);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {:?}",
            answer.body["message"]
        );
    }

    // The enumeration is the SERVICE's fixed text rather than a mirror of what the operation
    // applies, so these properties of it are asserted on their own: the stage-wide `/*/*`
    // method settings and both `accessLogSettings` pointers are named, the throttling and
    // caching entries of each method-path family run TOGETHER with no comma between them,
    // and the pointers this operation applies but the service's list does not carry — the
    // canary's and the documentation version's — are absent.
    let printed = stage_invalid_patch_path("/sim2realUndeclaredMember");
    for named in [
        "/*/*/metrics/enabled",
        "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
        "/accessLogSettings/destinationArn",
        "/accessLogSettings/format",
        "/variables/{variable_name}",
        "/{resourcePath}/{httpMethod}/throttling/burstLimit/{resourcePath}/{httpMethod}/throttling/rateLimit",
        "/*/*/throttling/burstLimit /*/*/throttling/rateLimit /*/*/caching/ttlInSeconds",
    ] {
        assert!(
            printed.contains(named),
            "case the printed enumeration: expected it to name {named:?}, got {printed:?}"
        );
    }
    for absent in ["/canarySettings", "/documentationVersion"] {
        assert!(
            !printed.contains(absent),
            "case the printed enumeration: expected it to carry no {absent:?}, got {printed:?}"
        );
    }

    // And the canary pointers the list does NOT name are applied all the same: the printed
    // list and what the operation applies are separate things.
    let case = "UpdateStage(a pointer the printed list does not name)";
    let updated = patch_stage_ok(
        &gateway,
        &api,
        case,
        vec![
            replace("/canarySettings/percentTraffic", "5"),
            replace("/documentationVersion", "v1"),
        ],
    )
    .await;
    assert_eq!(
        updated["canarySettings"]["percentTraffic"],
        json!(5.0),
        "case {case}: expected the canary's percentTraffic applied, got {updated:?}"
    );
    assert_eq!(
        updated["documentationVersion"],
        json!("v1"),
        "case {case}: expected the documentation version applied, got {updated:?}"
    );
}

#[tokio::test]
async fn a_method_settings_cache_control_strategy_takes_only_its_declared_values() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "stage-cache-control-strategy").await;
    create_deployed_stage(&gateway, &api, "prod").await;
    let path = "/*/*/caching/unauthorizedCacheControlHeaderStrategy";
    let refusal = "Invalid unauthorized cache control header strategy";

    // Rows differ along HOW the value misses the declared set: a word outside it, a declared
    // value in the wrong case, one with a trailing character, a number, and the strategy of
    // another member's set.
    for value in [
        "BOGUS",
        "fail_with_403",
        "SUCCEED_WITH_RESPONSE_HEADERS",
        "403",
        "FAIL_WITH_404",
    ] {
        let case = format!("UpdateStage(replace {path} = {value:?})");
        let answer = patch_stage(&gateway, &api, vec![replace(path, value)]).await;
        assert_failure(&answer, 400, "BadRequestException", &case);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(refusal),
            "case {case}: expected {refusal:?}, got {:?}",
            answer.body["message"]
        );
        let read = stage(&gateway, &api, &case).await;
        assert_eq!(
            read["methodSettings"],
            json!({}),
            "case {case}: a refused strategy must configure no method setting, got {:?}",
            read["methodSettings"]
        );
    }

    // The other side of the rule: every value the modelled enum declares is accepted and
    // read back, over the whole set rather than the one the refusals above were measured
    // against.
    for value in UNAUTHORIZED_CACHE_CONTROL_HEADER_STRATEGY_VALUES {
        let case = format!("UpdateStage(replace {path} = {value:?})");
        let updated = patch_stage_ok(&gateway, &api, &case, vec![replace(path, value)]).await;
        assert_eq!(
            updated["methodSettings"]["*/*"]["unauthorizedCacheControlHeaderStrategy"],
            json!(value),
            "case {case}: expected the strategy applied, got {:?}",
            updated["methodSettings"]
        );
        let read = stage(&gateway, &api, &case).await;
        assert_eq!(
            read["methodSettings"]["*/*"]["unauthorizedCacheControlHeaderStrategy"],
            json!(value),
            "case {case}: GetStage must read the strategy back, got {:?}",
            read["methodSettings"]
        );
    }
}
