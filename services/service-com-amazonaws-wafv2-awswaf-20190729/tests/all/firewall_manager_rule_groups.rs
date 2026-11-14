// SPDX-License-Identifier: CC0-1.0

//! `DeleteFirewallManagerRuleGroups`, end to end over the wire.
//!
//! The operation "deletes all rule groups that are managed by Firewall Manager from
//! the specified WebACL. You can only use this if ManagedByFirewallManager and
//! RetrofittedByFirewallManager are both false in the web ACL", so the RULE these
//! suites assert is: the call clears BOTH Firewall Manager rule group lists of the
//! web ACL its ARN names, leaves every other member of that web ACL alone, rotates
//! the web ACL's optimistic-lock token and answers the rotated token — and it is
//! refused, changing nothing, when the ARN is malformed, when the ARN names no
//! stored web ACL, when the token is not the current one, or when either Firewall
//! Manager flag is set on the web ACL.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: both scopes, a Firewall Manager list that is absent, empty, one entry
//! long and several entries long, in each position (pre-process, post-process or
//! both), every way an ARN can fail, and each of the two flags alone and together.
//!
//! Only Firewall Manager writes a web ACL's Firewall Manager rule groups and its two
//! Firewall Manager flags, through a Firewall Manager policy, and this emulator
//! serves the WAFv2 API alone — so no operation here fills them, and the cases seed
//! them through the state API ([`seed_firewall_manager`], the same seam
//! `crate::top_path_statistics` uses) before driving every assertion through the
//! router's front door, the door an SDK uses.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::ops::types::{
    FirewallManagerRuleGroup, FirewallManagerStatement, ManagedRuleGroupStatement, NoneAction,
    OverrideAction, VisibilityConfig,
};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ComAmazonawsWafv2Awswaf20190729State, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, is_model_uuid, rule, sqli_statement, visibility, Waf,
};

/// The operation under test, named once.
const OPERATION: &str = "DeleteFirewallManagerRuleGroups";

/// A `LockToken`-shaped token (the model's
/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$`) that no create minted, so it is
/// never the current token of anything stored.
const STALE_LOCK: &str = "99999999-8888-7777-6666-555555555555";

/// One Firewall Manager rule group, as a Firewall Manager policy would have put it
/// on a web ACL: the four members `FirewallManagerRuleGroup` requires beside its
/// name, with a managed-rule-group statement as its `FirewallManagerStatement`.
fn fm_group(name: &str, priority: i32) -> FirewallManagerRuleGroup {
    FirewallManagerRuleGroup {
        name: name.to_string(),
        priority,
        firewall_manager_statement: FirewallManagerStatement {
            managed_rule_group_statement: Some(ManagedRuleGroupStatement {
                vendor_name: "AWS".to_string(),
                name: "AWSManagedRulesCommonRuleSet".to_string(),
                ..Default::default()
            }),
            rule_group_reference_statement: None,
        },
        override_action: OverrideAction {
            count: None,
            none: Some(NoneAction {}),
        },
        visibility_config: VisibilityConfig {
            sampled_requests_enabled: true,
            cloud_watch_metrics_enabled: true,
            metric_name: name.to_string(),
        },
    }
}

/// `count` Firewall Manager rule groups, named `<prefix>-<n>` and prioritised in
/// the order they are listed.
fn fm_groups(prefix: &str, count: usize) -> Vec<FirewallManagerRuleGroup> {
    (0..count)
        .map(|index| {
            fm_group(
                &format!("{prefix}-{index}"),
                i32::try_from(index).expect("a table row holds a handful of groups"),
            )
        })
        .collect()
}

/// Put `pre`/`post` Firewall Manager rule groups, and the two Firewall Manager
/// flags, on the stored web ACL `(scope, id)`.
///
/// The one thing these suites cannot do over the wire: a Firewall Manager policy is
/// what writes these members on a web ACL, and this emulator serves only the WAFv2
/// API, so no operation fills them.
fn seed_firewall_manager(
    engine: &engine::InMemoryEngine,
    scope: &str,
    id: &str,
    pre: Option<Vec<FirewallManagerRuleGroup>>,
    post: Option<Vec<FirewallManagerRuleGroup>>,
    managed: bool,
    retrofitted: bool,
) {
    let state = engine.state::<ComAmazonawsWafv2Awswaf20190729State>();
    let mut state = state.lock();
    let entry = state
        .web_acls
        .get_mut(&(scope.to_string(), id.to_string()))
        .expect("the web ACL the case created is stored");
    entry.web_acl.pre_process_firewall_manager_rule_groups = pre;
    entry.web_acl.post_process_firewall_manager_rule_groups = post;
    entry.web_acl.managed_by_firewall_manager = managed;
    entry.web_acl.retrofitted_by_firewall_manager = retrofitted;
}

/// The stored web ACL, as `GetWebACL` answers it for `arn`, paired with its current
/// lock token.
///
/// Read at the endpoint of the region the ARN itself names — which is the endpoint a
/// conforming client uses, because a resource is reached through the endpoint of its own
/// region: a CLOUDFRONT-scoped web ACL's ARN carries the one region the global namespace
/// lives in, and a REGIONAL one carries the region it was created at.
async fn read_web_acl(waf: &Waf, arn: &str) -> (Value, String) {
    let region = arn
        .split(':')
        .nth(3)
        .expect("a wafv2 resource ARN names its region");
    let (status, body) = waf
        .call_from(region, "GetWebACL", json!({ "ARN": arn }))
        .await;
    assert_eq!(
        status, 200,
        "GetWebACL({arn}) at {region} must succeed, got status {status} and body {body}"
    );
    let lock_token = body["LockToken"]
        .as_str()
        .expect("GetWebACL answers a LockToken")
        .to_string();
    (body["WebACL"].clone(), lock_token)
}

/// How many entries a Firewall Manager list member holds: `None` when the member is
/// absent from the answer, so an absent list and an empty one stay distinguishable.
fn list_len(web_acl: &Value, member: &str) -> Option<usize> {
    web_acl.get(member).map(|value| {
        value
            .as_array()
            .unwrap_or_else(|| panic!("{member} must be a list, got {value}"))
            .len()
    })
}

/// One success row: the scope, and how each Firewall Manager list stands before the
/// call — absent (`None`), or a list of that many entries.
struct Seeded {
    scope: &'static str,
    pre: Option<usize>,
    post: Option<usize>,
}

/// The call clears both Firewall Manager rule group lists of the web ACL its ARN
/// names, rotates the lock token, answers the rotated token, and touches nothing
/// else about the web ACL.
///
/// The rows differ along what the rule generalises over: the scope the web ACL lives
/// in, which list carries Firewall Manager rule groups, and how many it carries —
/// absent, empty, one, several — because a clear must reach every one of those
/// states alike.
#[tokio::test]
async fn deleting_firewall_manager_rule_groups_clears_both_lists_and_rotates_the_token() {
    let (waf, engine) = Waf::sharing_engine();

    let cases = [
        Seeded {
            scope: SCOPE_REGIONAL,
            pre: None,
            post: None,
        },
        Seeded {
            scope: SCOPE_REGIONAL,
            pre: Some(0),
            post: Some(0),
        },
        Seeded {
            scope: SCOPE_REGIONAL,
            pre: Some(1),
            post: None,
        },
        Seeded {
            scope: SCOPE_REGIONAL,
            pre: None,
            post: Some(2),
        },
        Seeded {
            scope: SCOPE_REGIONAL,
            pre: Some(2),
            post: Some(3),
        },
        Seeded {
            scope: SCOPE_CLOUDFRONT,
            pre: Some(3),
            post: Some(1),
        },
        Seeded {
            scope: SCOPE_CLOUDFRONT,
            pre: Some(0),
            post: Some(2),
        },
        Seeded {
            scope: SCOPE_CLOUDFRONT,
            pre: None,
            post: None,
        },
    ];

    for (index, seeded) in cases.iter().enumerate() {
        let case = format!(
            "{} web ACL with pre {:?} and post {:?}",
            seeded.scope, seeded.pre, seeded.post
        );
        let name = format!("fm-acl-{index}");
        let (id, arn, created_lock) = create_web_acl(
            &waf,
            seeded.scope,
            &name,
            json!([rule("block-sqli", sqli_statement())]),
        )
        .await;
        seed_firewall_manager(
            &engine,
            seeded.scope,
            &id,
            seeded.pre.map(|count| fm_groups("fm-pre", count)),
            seeded.post.map(|count| fm_groups("fm-post", count)),
            false,
            false,
        );

        // What the web ACL looks like before the call, so the assertions below can
        // say which members changed and which did not.
        let (before, lock_before) = read_web_acl(&waf, &arn).await;
        assert_eq!(
            lock_before, created_lock,
            "case {case:?}: seeding must not have rotated the token, expected \
             {created_lock:?}, got {lock_before:?}"
        );
        assert_eq!(
            list_len(&before, "PreProcessFirewallManagerRuleGroups"),
            seeded.pre,
            "case {case:?}: the seeded pre-process list must read back, got {before}"
        );
        assert_eq!(
            list_len(&before, "PostProcessFirewallManagerRuleGroups"),
            seeded.post,
            "case {case:?}: the seeded post-process list must read back, got {before}"
        );

        let answer = waf
            .ok(
                OPERATION,
                json!({ "WebACLArn": arn, "WebACLLockToken": created_lock }),
            )
            .await;
        let next = answer["NextWebACLLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_model_uuid(&next),
            "case {case:?}: NextWebACLLockToken must have the modelled LockToken \
             form, got {next:?} in {answer}"
        );
        assert_ne!(
            next, created_lock,
            "case {case:?}: the token must rotate, got {next:?} again"
        );

        let (after, lock_after) = read_web_acl(&waf, &arn).await;
        assert_eq!(
            lock_after, next,
            "case {case:?}: the stored token must be the one the call answered \
             ({next:?}), got {lock_after:?}"
        );
        assert_eq!(
            list_len(&after, "PreProcessFirewallManagerRuleGroups"),
            None,
            "case {case:?}: the pre-process list must be gone, got {after}"
        );
        assert_eq!(
            list_len(&after, "PostProcessFirewallManagerRuleGroups"),
            None,
            "case {case:?}: the post-process list must be gone, got {after}"
        );

        // Everything else the web ACL carries is untouched: the operation deletes
        // Firewall Manager rule groups, not the customer's own configuration.
        for member in [
            "Name",
            "Id",
            "ARN",
            "DefaultAction",
            "Description",
            "Rules",
            "VisibilityConfig",
            "Capacity",
            "LabelNamespace",
            "ManagedByFirewallManager",
            "RetrofittedByFirewallManager",
            "OnSourceDDoSProtectionConfig",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: {member} must be unchanged, was {:?}, now {:?}",
                before.get(member),
                after.get(member)
            );
        }

        // The token the caller just spent no longer opens the web ACL, and the one
        // the call answered does: a second clear is a no-op that rotates again.
        waf.refused(
            OPERATION,
            json!({ "WebACLArn": arn, "WebACLLockToken": created_lock }),
            "WAFOptimisticLockException",
            &format!("{case}, the spent token"),
        )
        .await;
        let repeated = waf
            .ok(
                OPERATION,
                json!({ "WebACLArn": arn, "WebACLLockToken": next }),
            )
            .await;
        let after_repeat = repeated["NextWebACLLockToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_model_uuid(&after_repeat) && after_repeat != next,
            "case {case:?}: repeating the clear must rotate the token again, got \
             {after_repeat:?} after {next:?}"
        );
        let (twice, _) = read_web_acl(&waf, &arn).await;
        assert_eq!(
            list_len(&twice, "PreProcessFirewallManagerRuleGroups"),
            None,
            "case {case:?}: both lists must still be gone, got {twice}"
        );
        assert_eq!(
            list_len(&twice, "PostProcessFirewallManagerRuleGroups"),
            None,
            "case {case:?}: both lists must still be gone, got {twice}"
        );
    }
}

/// Every way the call is refused, with the Firewall Manager rule groups of a stored
/// web ACL left exactly as they were.
///
/// The rows span the refusal class: an ARN that is not a wafv2 resource ARN at all
/// and one malformed in each segment that matters, a well-formed ARN naming no
/// stored web ACL in each scope path, an ARN naming a stored resource of another
/// kind, and a token that is not the web ACL's current one.
#[tokio::test]
async fn deleting_firewall_manager_rule_groups_refuses_a_bad_arn_or_a_stale_token() {
    let (waf, engine) = Waf::sharing_engine();

    let (acl_id, acl_arn, acl_lock) =
        create_web_acl(&waf, SCOPE_REGIONAL, "fm-guard-acl", json!([])).await;
    seed_firewall_manager(
        &engine,
        SCOPE_REGIONAL,
        &acl_id,
        Some(fm_groups("fm-pre", 2)),
        Some(fm_groups("fm-post", 1)),
        false,
        false,
    );
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, SCOPE_REGIONAL, "fm-guard-ips").await;

    // `WebACLArn` is `length 20..2048, pattern \S`, so every ARN below is at least
    // 20 characters long and carries no whitespace: each one reaches the handler and
    // is judged by it, not by the modelled constraints.
    let refusals: &[(&str, String, &str)] = &[
        (
            "a string that is not an ARN at all",
            "fm-not-an-arn-but-long-enough".to_string(),
            "WAFInvalidParameterException",
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::fm-guard-bucket-name".to_string(),
            "WAFInvalidParameterException",
        ),
        (
            "a wafv2 ARN missing the name and id segments",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl".to_string(),
            "WAFInvalidParameterException",
        ),
        (
            "a wafv2 ARN whose scope path is neither global nor regional",
            "arn:aws:wafv2:us-west-2:000000000000:zonal/webacl/fm-guard-acl/\
             00000000-1111-2222-3333-444444444444"
                .to_string(),
            "WAFInvalidParameterException",
        ),
        (
            "a wafv2 ARN with an empty partition",
            "arn::wafv2:us-west-2:000000000000:regional/webacl/fm-guard-acl/\
             00000000-1111-2222-3333-444444444444"
                .to_string(),
            "WAFInvalidParameterException",
        ),
        (
            "a well-formed regional web ACL ARN naming nothing stored",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/fm-absent-acl/\
             00000000-1111-2222-3333-444444444444"
                .to_string(),
            "WAFNonexistentItemException",
        ),
        (
            "a well-formed global web ACL ARN naming nothing stored",
            "arn:aws:wafv2:us-east-1:000000000000:global/webacl/fm-absent-acl/\
             00000000-1111-2222-3333-444444444444"
                .to_string(),
            "WAFNonexistentItemException",
        ),
        (
            "the ARN of a stored IP set, which is not a web ACL",
            ip_set_arn.clone(),
            "WAFNonexistentItemException",
        ),
    ];

    for (case, arn, exception) in refusals {
        waf.refused(
            OPERATION,
            json!({ "WebACLArn": arn, "WebACLLockToken": acl_lock }),
            exception,
            case,
        )
        .await;
    }

    // A token that is not the web ACL's current one, whatever else the request got
    // right.
    for (case, token) in [
        ("a token no create minted", STALE_LOCK.to_string()),
        ("the web ACL's own id used as a token", acl_id.clone()),
    ] {
        waf.refused(
            OPERATION,
            json!({ "WebACLArn": acl_arn, "WebACLLockToken": token }),
            "WAFOptimisticLockException",
            case,
        )
        .await;
    }

    // Not one of those refusals touched the web ACL: both lists stand, and so does
    // the token the create minted.
    let (web_acl, lock_token) = read_web_acl(&waf, &acl_arn).await;
    assert_eq!(
        lock_token, acl_lock,
        "a refused call must not rotate the token, expected {acl_lock:?}, got \
         {lock_token:?}"
    );
    assert_eq!(
        list_len(&web_acl, "PreProcessFirewallManagerRuleGroups"),
        Some(2),
        "a refused call must leave the pre-process list alone, got {web_acl}"
    );
    assert_eq!(
        list_len(&web_acl, "PostProcessFirewallManagerRuleGroups"),
        Some(1),
        "a refused call must leave the post-process list alone, got {web_acl}"
    );
}

/// While Firewall Manager manages the web ACL — `ManagedByFirewallManager` or
/// `RetrofittedByFirewallManager` set — the call is not a valid operation on it, and
/// its Firewall Manager rule groups stay.
///
/// One row per combination of the two flags: either alone, and both together. The
/// row with both clear is the success suite above.
#[tokio::test]
async fn deleting_firewall_manager_rule_groups_refuses_a_web_acl_firewall_manager_manages() {
    let (waf, engine) = Waf::sharing_engine();

    let flag_rows = [
        (SCOPE_REGIONAL, true, false),
        (SCOPE_REGIONAL, false, true),
        (SCOPE_REGIONAL, true, true),
        (SCOPE_CLOUDFRONT, true, false),
        (SCOPE_CLOUDFRONT, false, true),
        (SCOPE_CLOUDFRONT, true, true),
    ];

    for (index, (scope, managed, retrofitted)) in flag_rows.iter().enumerate() {
        let case =
            format!("{scope} web ACL, ManagedByFirewallManager {managed}, RetrofittedByFirewallManager {retrofitted}");
        let name = format!("fm-managed-acl-{index}");
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": name,
                    "Scope": scope,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();
        let arn = created["Summary"]["ARN"]
            .as_str()
            .expect("Summary.ARN")
            .to_string();
        let lock = created["Summary"]["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string();
        seed_firewall_manager(
            &engine,
            scope,
            &id,
            Some(fm_groups("fm-pre", 1)),
            Some(fm_groups("fm-post", 2)),
            *managed,
            *retrofitted,
        );

        waf.refused(
            OPERATION,
            json!({ "WebACLArn": arn, "WebACLLockToken": lock }),
            "WAFInvalidOperationException",
            &case,
        )
        .await;

        let (web_acl, lock_token) = read_web_acl(&waf, &arn).await;
        assert_eq!(
            lock_token, lock,
            "case {case:?}: the refusal must not rotate the token, expected \
             {lock:?}, got {lock_token:?}"
        );
        assert_eq!(
            list_len(&web_acl, "PreProcessFirewallManagerRuleGroups"),
            Some(1),
            "case {case:?}: the pre-process list must stand, got {web_acl}"
        );
        assert_eq!(
            list_len(&web_acl, "PostProcessFirewallManagerRuleGroups"),
            Some(2),
            "case {case:?}: the post-process list must stand, got {web_acl}"
        );
    }
}
