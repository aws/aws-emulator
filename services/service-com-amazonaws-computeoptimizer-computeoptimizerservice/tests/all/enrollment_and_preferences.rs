// SPDX-License-Identifier: CC0-1.0

//! The enrollment + recommendation-preference workflow, end to end over the wire.
//!
//! Every call here goes through the shared wire harness (`tests/all/harness.rs`), i.e. the
//! same `MultiServiceRouter::dispatch` the emulator's front door uses. Nothing reaches
//! into the state struct, so what these tests prove is what a client sees.
//!
//! The tables at the end of this file pin WHICH CALLER may create a preference at the
//! organization level: "for management accounts of an organization only"
//! (`PutRecommendationPreferences$scope`).

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    MemberAccountEnrollment, DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS, DEFAULT_LOOK_BACK_PERIOD,
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MAX_PREFERENCE_RECORDS, ORGANIZATION_SCOPE_VALUE,
    SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN,
};

use crate::harness::{
    auto_scaling_group_arn, call, effective_for, emulator, emulator_with_engine, error_code,
    get_preferences, instance_arn, management_account_emulator, opt_in, put_preference,
    record_organization, ACCOUNT,
};

/// `GetEnrollmentStatus` reports the documented default before any write, and afterwards
/// exactly what the last accepted `UpdateEnrollmentStatus` stored — while `Pending` and
/// `Failed`, which the model says are server-assigned, are refused.
#[tokio::test]
async fn enrollment_defaults_to_inactive_and_round_trips_every_settable_status() {
    let router = emulator();

    let (status, body) = call(&router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(
        status, 200,
        "untouched enrollment: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body["status"], "Inactive",
        "untouched enrollment: expected status Inactive, got {body}"
    );
    assert_eq!(
        body["memberAccountsEnrolled"], false,
        "untouched enrollment: expected memberAccountsEnrolled false, got {body}"
    );
    assert!(
        body["lastUpdatedTimestamp"].is_null(),
        "untouched enrollment: nothing has been written, so there is no \
         lastUpdatedTimestamp, got {body}"
    );

    // The rows span the model's `Status` enum (Active | Failed | Inactive | Pending —
    // `crate::ops::types::STATUS_VALUES`) crossed with `includeMemberAccounts`. Per the
    // `status` member's documentation only Active and Inactive may be supplied by a
    // client; Pending and Failed "cannot be used to update the enrollment status". And
    // `includeMemberAccounts` enrolls "member accounts of the organization if the account is
    // the management account of an organization", which this emulator's standalone account is
    // not — so every row that asks for them is refused too (`include_member_accounts.rs`
    // pins that rule across every operation taking the member).
    let cases = [
        ("Active", true, false),
        ("Active", false, true),
        ("Pending", false, false),
        ("Inactive", true, false),
        ("Failed", true, false),
        ("Inactive", false, true),
    ];
    for (wanted, include_member_accounts, accepted) in cases {
        let case = format!("status={wanted} includeMemberAccounts={include_member_accounts}");
        let (status, updated) = call(
            &router,
            "UpdateEnrollmentStatus",
            json!({"status": wanted, "includeMemberAccounts": include_member_accounts}),
        )
        .await;

        if !accepted {
            assert_eq!(
                status, 400,
                "case {case:?}: a server-assigned status, or member accounts a standalone \
                 account cannot ask for, must be refused, got {status}: {updated}"
            );
            assert!(
                error_code(&updated).contains("InvalidParameterValueException"),
                "case {case:?}: expected InvalidParameterValueException, got {updated}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["status"], wanted,
            "case {case:?}: the response must carry the stored status, got {updated}"
        );
        assert!(
            updated["statusReason"].is_string(),
            "case {case:?}: expected a statusReason, got {updated}"
        );

        let (status, got) = call(&router, "GetEnrollmentStatus", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read-back, got {status}: {got}"
        );
        assert_eq!(
            got["status"], wanted,
            "case {case:?}: the read-back must report the stored status, got {got}"
        );
        assert_eq!(
            got["memberAccountsEnrolled"], include_member_accounts,
            "case {case:?}: the read-back must report the stored memberAccountsEnrolled, got {got}"
        );
        assert_eq!(
            got["statusReason"], updated["statusReason"],
            "case {case:?}: both operations report one stored statusReason, got {got}"
        );
        assert!(
            got["lastUpdatedTimestamp"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: a write sets lastUpdatedTimestamp, got {got}"
        );
        // The opted-in member-account count is a count, reported whatever it is: this
        // organization holds no member accounts, so it is `0` on every row — never an
        // absent member a client cannot tell from "none opted in".
        assert_eq!(
            got["numberOfMemberAccountsOptedIn"],
            json!(0),
            "case {case:?}: expected the member-account count 0, got {got}"
        );
    }

    // The refusals above changed nothing: the last accepted row is still in force.
    let (_, body) = call(&router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(
        body["status"], "Inactive",
        "a refused update must not move the stored status, got {body}"
    );
    assert_eq!(
        body["memberAccountsEnrolled"], false,
        "a refused update must not move the stored memberAccountsEnrolled, got {body}"
    );
}

/// Every preference a `PutRecommendationPreferences` call stores comes back from
/// `GetRecommendationPreferences` at its own resource type and scope, and a second put
/// merges into the record rather than replacing it.
#[tokio::test]
async fn a_written_preference_is_read_back_at_its_own_resource_type_and_scope() {
    // The rows below include a write at the organization-level scope, which the model admits
    // "for management accounts of an organization only" (`PutRecommendationPreferences$scope`),
    // so the caller here manages an organization.
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    let instance = instance_arn("i-1234567890abcdef0");
    let group = auto_scaling_group_arn("web-tier");

    // Rows differ along resource type, scope level, and which preference fields are set.
    // Every value is from the model's own enums; `Idle`/`NotApplicable` resource types
    // carry no settable preference of their own and are left out.
    let cases: Vec<(&str, &str, Option<Value>, Value)> = vec![
        (
            "ec2 account default, no scope",
            "Ec2Instance",
            None,
            json!({"enhancedInfrastructureMetrics": "Active"}),
        ),
        (
            "ec2 at the account scope",
            "Ec2Instance",
            Some(json!({"name": "AccountId", "value": ACCOUNT})),
            json!({"lookBackPeriod": "DAYS_32", "inferredWorkloadTypes": "Inactive"}),
        ),
        (
            "ec2 at the resource scope",
            "Ec2Instance",
            Some(json!({"name": "ResourceArn", "value": instance})),
            json!({
                "enhancedInfrastructureMetrics": "Inactive",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
        ),
        (
            "ec2 at the organization scope",
            "Ec2Instance",
            Some(json!({"name": "Organization", "value": "ALL_ACCOUNTS"})),
            json!({
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"threshold": "P95", "headroom": "PERCENT_10"},
                }],
                "savingsEstimationMode": "AfterDiscounts",
            }),
        ),
        (
            "ebs volume at the account scope",
            "EbsVolume",
            Some(json!({"name": "AccountId", "value": ACCOUNT})),
            json!({"lookBackPeriod": "DAYS_93"}),
        ),
        (
            "auto scaling group at the resource scope",
            "AutoScalingGroup",
            Some(json!({"name": "ResourceArn", "value": group})),
            json!({
                "preferredResources": [{
                    "name": "Ec2InstanceTypes",
                    "includeList": ["m5.large", "m5.xlarge"],
                }],
            }),
        ),
        (
            "lambda function account default",
            "LambdaFunction",
            None,
            json!({"savingsEstimationMode": "BeforeDiscounts"}),
        ),
    ];

    for (name, resource_type, scope, preferences) in &cases {
        let mut request = json!({"resourceType": resource_type});
        for (field, value) in preferences.as_object().expect("an object of preferences") {
            request[field] = value.clone();
        }
        if let Some(scope) = scope {
            request["scope"] = scope.clone();
        }
        put_preference(&router, &request).await;

        let body = get_preferences(&router, resource_type, scope.as_ref()).await;
        let details = body["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected a detail list, got {body}"));
        assert_eq!(
            details.len(),
            1,
            "case {name:?}: expected exactly the record just written, got {body}"
        );
        let detail = &details[0];
        assert_eq!(
            detail["resourceType"], *resource_type,
            "case {name:?}: expected resourceType {resource_type}, got {detail}"
        );
        match scope {
            Some(scope) => assert_eq!(
                detail["scope"], *scope,
                "case {name:?}: expected the stored scope {scope}, got {detail}"
            ),
            None => assert!(
                detail["scope"].is_null(),
                "case {name:?}: nothing was written at a scope, got {detail}"
            ),
        }
        for (field, written) in preferences.as_object().expect("an object of preferences") {
            if field == "preferredResources" {
                // Reported as `EffectivePreferredResource`: the stored lists plus
                // `effectiveIncludeList`, which resolves wildcards — the identity here,
                // since these include lists carry none.
                for (index, want) in written.as_array().expect("a list").iter().enumerate() {
                    let got = &detail[field][index];
                    assert_eq!(
                        got["name"], want["name"],
                        "case {name:?}: expected preferred-resource name {want}, got {got}"
                    );
                    assert_eq!(
                        got["includeList"], want["includeList"],
                        "case {name:?}: expected includeList {want}, got {got}"
                    );
                    assert_eq!(
                        got["effectiveIncludeList"], want["includeList"],
                        "case {name:?}: a wildcard-free includeList resolves to itself, got {got}"
                    );
                }
                continue;
            }
            assert_eq!(
                detail[field], *written,
                "case {name:?}: expected {field} = {written}, got {detail}"
            );
        }
    }

    // Reading by resource type alone returns every record of that type and nothing else.
    for (resource_type, expected) in [
        ("Ec2Instance", 4),
        ("EbsVolume", 1),
        ("AutoScalingGroup", 1),
        ("LambdaFunction", 1),
        ("EcsService", 0),
    ] {
        let body = get_preferences(&router, resource_type, None).await;
        let details = body["recommendationPreferencesDetails"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            details, expected,
            "resourceType {resource_type}: expected {expected} record(s), got {body}"
        );
    }

    // A second put UPDATES the record for that key: the field it names changes, the
    // fields it does not are left where they were.
    let account_scope = json!({"name": "AccountId", "value": ACCOUNT});
    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": account_scope,
            "lookBackPeriod": "DAYS_93",
        }),
    )
    .await;
    let body = get_preferences(&router, "Ec2Instance", Some(&account_scope)).await;
    let detail = &body["recommendationPreferencesDetails"][0];
    assert_eq!(
        detail["lookBackPeriod"], "DAYS_93",
        "an upsert must store the new value, got {detail}"
    );
    assert_eq!(
        detail["inferredWorkloadTypes"], "Inactive",
        "an upsert must leave a field it does not name alone, got {detail}"
    );
}

/// `maxResults` caps a page and the opaque `nextToken` resumes exactly where the previous
/// page stopped, whatever the page size: paging always yields the unpaged sequence.
#[tokio::test]
async fn preferences_page_through_an_opaque_next_token() {
    // One of the scopes paged over is the organization level, which only a management account
    // may create a preference at.
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    let scopes = [
        None,
        Some(json!({"name": "AccountId", "value": ACCOUNT})),
        Some(json!({"name": "Organization", "value": "ALL_ACCOUNTS"})),
        Some(json!({"name": "ResourceArn", "value": instance_arn("i-0abc")})),
        Some(json!({"name": "ResourceArn", "value": instance_arn("i-0def")})),
    ];
    for (index, scope) in scopes.iter().enumerate() {
        let mut request = json!({
            "resourceType": "Ec2Instance",
            "enhancedInfrastructureMetrics": if index % 2 == 0 { "Active" } else { "Inactive" },
        });
        if let Some(scope) = scope {
            request["scope"] = scope.clone();
        }
        put_preference(&router, &request).await;
    }

    let unpaged = get_preferences(&router, "Ec2Instance", None).await;
    let expected = unpaged["recommendationPreferencesDetails"]
        .as_array()
        .expect("a detail list")
        .clone();
    assert_eq!(
        expected.len(),
        scopes.len(),
        "the unpaged read must return every record written, got {unpaged}"
    );
    assert!(
        unpaged["nextToken"].is_null(),
        "an unpaged read has no further page, got {unpaged}"
    );

    // One row per page size: every size must walk the same sequence, and only the final
    // page may omit the token. 1 is the smallest page, 5 is exactly the record count, and
    // 6 is one past it.
    for page_size in [1_u64, 2, 3, 5, 6] {
        let case = format!("maxResults={page_size}");
        let mut collected: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"resourceType": "Ec2Instance", "maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetRecommendationPreferences", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );
            let page = body["recommendationPreferencesDetails"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a detail list, got {body}"));
            assert!(
                page.len() as u64 <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {} entries",
                page.len()
            );
            collected.extend(page.iter().cloned());
            pages += 1;
            assert!(
                pages <= scopes.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case {case:?}: paging must yield the unpaged sequence"
        );
    }

    // A token this service did not issue names no page boundary.
    let (status, body) = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "Ec2Instance", "nextToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a forged nextToken must be refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("InvalidParameterValueException"),
        "a forged nextToken: expected InvalidParameterValueException, got {body}"
    );

    // `maxResults` is modelled 0..1000. Zero is the published lower bound and carries no
    // documented meaning; a cap of zero could never advance through the pages, so it is
    // read as "no size asked for" — the same answer as leaving maxResults out, which is
    // the emulator's own DEFAULT_PAGE_SIZE and so holds all five records here.
    let (status, body) = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "Ec2Instance", "maxResults": 0}),
    )
    .await;
    assert_eq!(status, 200, "maxResults=0: expected 200, got {status}: {body}");
    assert_eq!(
        body["recommendationPreferencesDetails"],
        Value::Array(expected.clone()),
        "maxResults=0: expected the unpaged sequence, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "maxResults=0: expected no further page, got {body}"
    );

    // Each published bound is accepted, and one past the top is refused by the modelled
    // range before the handler runs.
    for (page_size, accepted) in [(1000_u64, true), (1001, false)] {
        let (status, body) = call(
            &router,
            "GetRecommendationPreferences",
            json!({"resourceType": "Ec2Instance", "maxResults": page_size}),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "maxResults={page_size}: inside the modelled range, got {status}: {body}"
            );
            assert_eq!(
                body["recommendationPreferencesDetails"],
                Value::Array(expected.clone()),
                "maxResults={page_size}: expected every record on one page, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "maxResults={page_size}: outside the modelled range 0..1000, got {status}: {body}"
            );
        }
    }
}

/// Each effective preference is the value set by the most specific scope that sets it, so
/// a resource-level record wins field by field and inherits the rest; with nothing set,
/// the look-back period is the documented default.
#[tokio::test]
async fn effective_preferences_resolve_by_scope_precedence() {
    // The least specific record in the chain below is the organization-level one, which only a
    // management account may create.
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    let instance = instance_arn("i-1234567890abcdef0");
    let other_instance = instance_arn("i-00000000000000000");
    let group = auto_scaling_group_arn("web-tier");

    // Nothing stored: only the documented default applies ("When this preference isn't
    // specified, we use the default value DAYS_14").
    let (status, body) = call(
        &router,
        "GetEffectiveRecommendationPreferences",
        json!({"resourceArn": instance}),
    )
    .await;
    assert_eq!(
        status, 200,
        "no preferences stored: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body["lookBackPeriod"], "DAYS_14",
        "no preferences stored: expected the default look-back period, got {body}"
    );
    assert_eq!(
        body["enhancedInfrastructureMetrics"], "Inactive",
        "no preferences stored: the status in force is the documented default, got {body}"
    );

    // Each row adds a more specific record and states what must then be in effect.
    let organization_scope = json!({"name": "Organization", "value": "ALL_ACCOUNTS"});
    let account_scope = json!({"name": "AccountId", "value": ACCOUNT});
    let resource_scope = json!({"name": "ResourceArn", "value": instance});

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": organization_scope,
            "enhancedInfrastructureMetrics": "Active",
            "lookBackPeriod": "DAYS_93",
        }),
    )
    .await;
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Active",
        "organization level only: expected it to apply, got {effective}"
    );
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_93",
        "organization level only: expected it to apply, got {effective}"
    );

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": account_scope,
            "lookBackPeriod": "DAYS_32",
        }),
    )
    .await;
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_32",
        "account over organization: the account level wins, got {effective}"
    );
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Active",
        "account over organization: a field the account level does not set is inherited, \
         got {effective}"
    );

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": resource_scope,
            "enhancedInfrastructureMetrics": "Inactive",
            "externalMetricsPreference": {"source": "Dynatrace"},
        }),
    )
    .await;
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Inactive",
        "resource over account: the resource level wins, got {effective}"
    );
    assert_eq!(
        effective["externalMetricsPreference"]["source"], "Dynatrace",
        "resource over account: the resource level value is reported, got {effective}"
    );
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_32",
        "resource over account: the look-back period is inherited from the account scope, \
         got {effective}"
    );

    // The resource-level record applies to ITS resource only; another instance of the
    // same account still resolves to the account and organization levels.
    let effective = effective_for(&router, &other_instance).await;
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Active",
        "another instance: a resource-level record must not leak to it, got {effective}"
    );
    assert!(
        effective["externalMetricsPreference"].is_null(),
        "another instance: a resource-level record must not leak to it, got {effective}"
    );

    // An Auto Scaling group resolves against AutoScalingGroup records, not EC2 ones.
    put_preference(
        &router,
        &json!({
            "resourceType": "AutoScalingGroup",
            "scope": {"name": "ResourceArn", "value": group},
            "preferredResources": [{"name": "Ec2InstanceTypes", "excludeList": ["t2"]}],
        }),
    )
    .await;
    let effective = effective_for(&router, &group).await;
    assert_eq!(
        effective["preferredResources"][0]["excludeList"][0], "t2",
        "auto scaling group: expected its own resource-level record, got {effective}"
    );
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Inactive",
        "auto scaling group: an Ec2Instance record must not apply to it, so the documented \
         default is in force, got {effective}"
    );
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_14",
        "auto scaling group: no AutoScalingGroup record sets one, so the default applies, \
         got {effective}"
    );

    // Only EC2 instance and Auto Scaling group ARNs are supported.
    for unsupported in [
        "arn:aws:s3:::my-bucket",
        "arn:aws:ec2:us-east-1:111122223333:volume/vol-123",
        "not-an-arn",
    ] {
        let (status, body) = call(
            &router,
            "GetEffectiveRecommendationPreferences",
            json!({"resourceArn": unsupported}),
        )
        .await;
        assert_eq!(
            status, 400,
            "resourceArn {unsupported:?}: expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "resourceArn {unsupported:?}: expected InvalidParameterValueException, got {body}"
        );
    }
}

/// `DeleteRecommendationPreferences` removes exactly the preferences it names, from
/// exactly the record it addresses; a record with nothing left is gone, and a later read
/// falls back to the next scope or to the default.
#[tokio::test]
async fn delete_removes_only_the_named_preferences() {
    let router = emulator();
    opt_in(&router).await;

    let instance = instance_arn("i-1234567890abcdef0");
    let account_scope = json!({"name": "AccountId", "value": ACCOUNT});
    let resource_scope = json!({"name": "ResourceArn", "value": instance});

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": account_scope,
            "enhancedInfrastructureMetrics": "Active",
            "lookBackPeriod": "DAYS_32",
        }),
    )
    .await;

    // Every name is a `RecommendationPreferenceName` (the model's enum:
    // EnhancedInfrastructureMetrics | InferredWorkloadTypes | ExternalMetricsPreference |
    // LookBackPeriodPreference | PreferredResources | UtilizationPreferences), each
    // written at the resource scope and then deleted one at a time.
    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": resource_scope,
            "enhancedInfrastructureMetrics": "Inactive",
            "inferredWorkloadTypes": "Inactive",
            "externalMetricsPreference": {"source": "Instana"},
            "lookBackPeriod": "DAYS_93",
            "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5.large"]}],
            "utilizationPreferences": [{
                "metricName": "MemoryUtilization",
                "metricParameters": {"headroom": "PERCENT_0"},
            }],
        }),
    )
    .await;

    // An unrecognised preference name is refused, and nothing is cleared: a refused
    // request must not have deleted the names that came before the bad one.
    let (status, body) = call(
        &router,
        "DeleteRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": resource_scope,
            "recommendationPreferenceNames": ["EnhancedInfrastructureMetrics", "NotAPreference"],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unrecognised preference name must be refused, got {status}: {body}"
    );
    let read = get_preferences(&router, "Ec2Instance", Some(&resource_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"][0]["enhancedInfrastructureMetrics"], "Inactive",
        "a refused delete must leave every preference where it was, got {read}"
    );

    let deletions = [
        ("EnhancedInfrastructureMetrics", "enhancedInfrastructureMetrics"),
        ("InferredWorkloadTypes", "inferredWorkloadTypes"),
        ("ExternalMetricsPreference", "externalMetricsPreference"),
        ("LookBackPeriodPreference", "lookBackPeriod"),
        ("PreferredResources", "preferredResources"),
        ("UtilizationPreferences", "utilizationPreferences"),
    ];
    let mut remaining: Vec<&str> = deletions.iter().map(|(_, field)| *field).collect();
    for (name, field) in deletions {
        let (status, body) = call(
            &router,
            "DeleteRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "scope": resource_scope,
                "recommendationPreferenceNames": [name],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "deleting {name}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "deleting {name}: DeleteRecommendationPreferences has an empty output, got {body}"
        );

        remaining.retain(|candidate| *candidate != field);
        let read = get_preferences(&router, "Ec2Instance", Some(&resource_scope)).await;
        let details = read["recommendationPreferencesDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("deleting {name}: expected a detail list, got {read}"));
        if remaining.is_empty() {
            assert!(
                details.is_empty(),
                "deleting {name}: the record holds no preference any more, so it is gone, \
                 got {read}"
            );
            continue;
        }
        assert_eq!(
            details.len(),
            1,
            "deleting {name}: the record still holds {remaining:?}, got {read}"
        );
        assert!(
            details[0][field].is_null(),
            "deleting {name}: expected {field} to be gone, got {}",
            details[0]
        );
        for survivor in &remaining {
            assert!(
                !details[0][survivor].is_null(),
                "deleting {name}: {survivor} was not named, so it must survive, got {}",
                details[0]
            );
        }
    }

    // The account-scoped record was never addressed, so it is untouched — and with the
    // resource-level record gone the effective value falls back to it.
    let read = get_preferences(&router, "Ec2Instance", Some(&account_scope)).await;
    let detail = &read["recommendationPreferencesDetails"][0];
    assert_eq!(
        detail["enhancedInfrastructureMetrics"], "Active",
        "a delete at one scope must not touch another scope's record, got {read}"
    );
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Active",
        "with the resource-level record gone, the account level applies, got {effective}"
    );
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_32",
        "with the resource-level record gone, the account level applies, got {effective}"
    );

    // Deleting the account-scoped record's last preferences drops it too, so the
    // documented default applies again.
    let (status, body) = call(
        &router,
        "DeleteRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": account_scope,
            "recommendationPreferenceNames": [
                "EnhancedInfrastructureMetrics", "LookBackPeriodPreference",
            ],
        }),
    )
    .await;
    assert_eq!(status, 200, "deleting both names: got {status}: {body}");
    let effective = effective_for(&router, &instance).await;
    assert_eq!(
        effective["lookBackPeriod"], "DAYS_14",
        "with every record gone, the default look-back period applies, got {effective}"
    );
    assert_eq!(
        effective["enhancedInfrastructureMetrics"], "Inactive",
        "with every record gone, the documented default status applies, got {effective}"
    );
}

/// A request that breaks a documented preference rule is refused with
/// `InvalidParameterValueException`, and one that keeps every rule is stored.
#[tokio::test]
async fn preference_requests_are_checked_against_the_documented_rules() {
    // These rows pin the rules a request's own members have to keep, so the caller manages an
    // organization: WHICH caller may create a preference at the organization level is a
    // separate rule, pinned by the tables at the end of this suite.
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    let instance = instance_arn("i-1234567890abcdef0");
    let group = auto_scaling_group_arn("web-tier");

    // `accepted` says whether the request keeps every rule the model documents for these
    // members. The refusals cover each rule in turn; the acceptances pin that the rule is
    // no wider than documented.
    let cases: Vec<(&str, bool, Value)> = vec![
        // `externalMetricsPreference` "can only be set for the Ec2Instance resource type".
        (
            "external metrics on Ec2Instance",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "externalMetricsPreference": {"source": "NewRelic"},
            }),
        ),
        (
            "external metrics on EbsVolume",
            false,
            json!({
                "resourceType": "EbsVolume",
                "externalMetricsPreference": {"source": "NewRelic"},
            }),
        ),
        (
            "external metrics on RdsDBInstance",
            false,
            json!({
                "resourceType": "RdsDBInstance",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
        ),
        // `preferredResources`: "You must specify either an includeList or excludeList.
        // If the preference is an empty set of resource type values, an error occurs."
        (
            "preferred resources with an include list",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5"]}],
            }),
        ),
        (
            "preferred resources with an exclude list",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "preferredResources": [{"name": "Ec2InstanceTypes", "excludeList": ["t2"]}],
            }),
        ),
        (
            "preferred resources with neither list",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "preferredResources": [{"name": "Ec2InstanceTypes"}],
            }),
        ),
        (
            "preferred resources with two empty lists",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "preferredResources": [{
                    "name": "Ec2InstanceTypes", "includeList": [], "excludeList": [],
                }],
            }),
        ),
        // `utilizationPreferences`: "You can only set CPU and memory utilization
        // preferences for the Amazon EC2 instance resource type" and "The threshold
        // setting isn't available for memory utilization."
        (
            "cpu utilization with a threshold",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"threshold": "P90", "headroom": "PERCENT_30"},
                }],
            }),
        ),
        (
            "memory utilization with a headroom only",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "utilizationPreferences": [{
                    "metricName": "MemoryUtilization",
                    "metricParameters": {"headroom": "PERCENT_20"},
                }],
            }),
        ),
        (
            "memory utilization with a threshold",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "utilizationPreferences": [{
                    "metricName": "MemoryUtilization",
                    "metricParameters": {"threshold": "P99_5"},
                }],
            }),
        ),
        (
            "utilization preferences on EcsService",
            false,
            json!({
                "resourceType": "EcsService",
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"headroom": "PERCENT_20"},
                }],
            }),
        ),
        // `lookBackPeriod` "can only be set for the Amazon EC2 instance, Auto Scaling
        // group, Amazon EBS volume, Amazon ECS service on Fargate, Amazon RDS DB
        // instance, and Aurora DB cluster storage resource types" — every one of them,
        // and nothing else.
        (
            "look-back period on Ec2Instance",
            true,
            json!({"resourceType": "Ec2Instance", "lookBackPeriod": "DAYS_14"}),
        ),
        (
            "look-back period on EbsVolume",
            true,
            json!({"resourceType": "EbsVolume", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on EcsService",
            true,
            json!({"resourceType": "EcsService", "lookBackPeriod": "DAYS_93"}),
        ),
        (
            "look-back period on RdsDBInstance",
            true,
            json!({"resourceType": "RdsDBInstance", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on AuroraDBClusterStorage",
            true,
            json!({"resourceType": "AuroraDBClusterStorage", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on AutoScalingGroup at the resource scope",
            true,
            json!({
                "resourceType": "AutoScalingGroup",
                "scope": {"name": "ResourceArn", "value": group},
                "lookBackPeriod": "DAYS_32",
            }),
        ),
        (
            "look-back period on LambdaFunction",
            false,
            json!({"resourceType": "LambdaFunction", "lookBackPeriod": "DAYS_32"}),
        ),
        (
            "look-back period on License",
            false,
            json!({"resourceType": "License", "lookBackPeriod": "DAYS_32"}),
        ),
        // `scope`: an Auto Scaling group preference can be created "only at the resource
        // level by specifying a scope name of ResourceArn".
        (
            "auto scaling group at the account scope",
            false,
            json!({
                "resourceType": "AutoScalingGroup",
                "scope": {"name": "AccountId", "value": ACCOUNT},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "auto scaling group with no scope",
            false,
            json!({"resourceType": "AutoScalingGroup", "inferredWorkloadTypes": "Active"}),
        ),
        // `Scope$value`: Organization -> ALL_ACCOUNTS, AccountId -> a 12-digit account id,
        // ResourceArn -> an EC2 instance or Auto Scaling group ARN.
        (
            "organization scope with ALL_ACCOUNTS",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "Organization", "value": "ALL_ACCOUNTS"},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "organization scope with an account id",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "Organization", "value": ACCOUNT},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "account scope with an 11-digit account id",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "AccountId", "value": "11112222333"},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "account scope with a non-numeric account id",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "AccountId", "value": "1111-2222-333"},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "resource scope with an instance ARN",
            true,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "ResourceArn", "value": instance},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "resource scope with a bucket ARN",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "ResourceArn", "value": "arn:aws:s3:::my-bucket"},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "a scope name with no value",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"name": "AccountId"},
                "inferredWorkloadTypes": "Active",
            }),
        ),
        (
            "a scope value with no name",
            false,
            json!({
                "resourceType": "Ec2Instance",
                "scope": {"value": ACCOUNT},
                "inferredWorkloadTypes": "Active",
            }),
        ),
    ];

    for (name, accepted, request) in &cases {
        let (status, body) = call(&router, "PutRecommendationPreferences", request.clone()).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {name:?}: this request keeps every documented rule, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {name:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // A refused request stores nothing: the refused Lambda look-back period above left no
    // LambdaFunction record behind.
    let body = get_preferences(&router, "LambdaFunction", None).await;
    assert_eq!(
        body["recommendationPreferencesDetails"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "a refused request must store nothing, got {body}"
    );
}

/// Every preference operation that models `OptInRequiredException` answers it while the
/// account is not opted in, and the enrollment operations keep working so the caller can
/// opt in. Opting back in finds the records still there: the store is shared, not
/// per-request.
#[tokio::test]
async fn preference_operations_require_the_account_to_be_opted_in() {
    let router = emulator();

    let instance = instance_arn("i-1234567890abcdef0");
    let put = json!({
        "resourceType": "Ec2Instance",
        "scope": {"name": "AccountId", "value": ACCOUNT},
        "enhancedInfrastructureMetrics": "Active",
    });
    let requests: Vec<(&str, Value)> = vec![
        ("PutRecommendationPreferences", put.clone()),
        (
            "GetRecommendationPreferences",
            json!({"resourceType": "Ec2Instance"}),
        ),
        (
            "GetEffectiveRecommendationPreferences",
            json!({"resourceArn": instance}),
        ),
        (
            "DeleteRecommendationPreferences",
            json!({
                "resourceType": "Ec2Instance",
                "recommendationPreferenceNames": ["EnhancedInfrastructureMetrics"],
            }),
        ),
    ];

    // The default enrollment is Inactive, so every one of them is refused.
    for (operation, request) in &requests {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 403,
            "{operation} while not opted in: expected 403, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "{operation} while not opted in: expected OptInRequiredException, got {body}"
        );
    }

    // Opt in; the same calls now work.
    opt_in(&router).await;
    put_preference(&router, &put).await;
    let body = get_preferences(&router, "Ec2Instance", None).await;
    assert_eq!(
        body["recommendationPreferencesDetails"][0]["enhancedInfrastructureMetrics"], "Active",
        "after opting in the write must be stored, got {body}"
    );

    // Opt out again: the preference operations go back to refusing.
    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive", "includeMemberAccounts": false}),
    )
    .await;
    assert_eq!(status, 200, "opting out: got {status}: {body}");
    for (operation, request) in &requests {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            status, 403,
            "{operation} after opting out: expected 403, got {status}: {body}"
        );
    }

    // And the record written earlier is still in the shared store when the account opts
    // back in — no request got a private copy of it.
    opt_in(&router).await;
    let body = get_preferences(&router, "Ec2Instance", None).await;
    assert_eq!(
        body["recommendationPreferencesDetails"][0]["enhancedInfrastructureMetrics"], "Active",
        "the store is shared across requests, got {body}"
    );
}

/// The store and the page it can produce are BOUNDED: the store holds at most
/// `MAX_PREFERENCE_RECORDS` records, so a request for one more NEW key is refused (while
/// an update to a key already held still works), and a read that names no page size gets
/// `DEFAULT_PAGE_SIZE` records plus a token rather than the whole store in one response.
#[tokio::test]
async fn the_preference_store_and_its_pages_are_bounded() {
    let router = emulator();
    opt_in(&router).await;

    // Fill the store through the public API: each distinct ResourceArn scope is a distinct
    // key, which is exactly how a caller could grow it without limit.
    for index in 0..MAX_PREFERENCE_RECORDS {
        let request = json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": "ResourceArn", "value": instance_arn(&format!("i-{index:016x}"))},
            "enhancedInfrastructureMetrics": "Active",
        });
        let (status, body) = call(&router, "PutRecommendationPreferences", request).await;
        assert_eq!(
            status, 200,
            "record {index} of {MAX_PREFERENCE_RECORDS}: must be accepted, got {status}: {body}"
        );
    }

    // One past the bound: a NEW key is refused with the modelled server-capacity error,
    // and nothing is stored for it.
    let past_bound = instance_arn("i-ffffffffffffffff");
    let (status, body) = call(
        &router,
        "PutRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": {"name": "ResourceArn", "value": past_bound},
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;
    assert_eq!(
        status, 503,
        "a new key past {MAX_PREFERENCE_RECORDS} records: expected 503, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ServiceUnavailableException"),
        "a new key past the bound: expected ServiceUnavailableException, got {body}"
    );
    let at_bound = json!({"name": "ResourceArn", "value": past_bound});
    let body = get_preferences(&router, "Ec2Instance", Some(&at_bound)).await;
    assert_eq!(
        body["recommendationPreferencesDetails"]
            .as_array()
            .map_or(0, Vec::len),
        0,
        "the refused key must not have been stored, got {body}"
    );

    // An UPDATE to a key the full store already holds allocates no record, so it is still
    // accepted — and it is really stored.
    let existing = json!({"name": "ResourceArn", "value": instance_arn("i-0000000000000007")});
    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": existing.clone(),
            "enhancedInfrastructureMetrics": "Inactive",
        }),
    )
    .await;
    let body = get_preferences(&router, "Ec2Instance", Some(&existing)).await;
    assert_eq!(
        body["recommendationPreferencesDetails"][0]["enhancedInfrastructureMetrics"], "Inactive",
        "an update to an existing key in a full store must be stored, got {body}"
    );

    // A read that names no page size is bounded by the emulator's default page, not by the
    // store's size, and hands back a token for the rest.
    let body = get_preferences(&router, "Ec2Instance", None).await;
    assert_eq!(
        body["recommendationPreferencesDetails"]
            .as_array()
            .map_or(0, Vec::len),
        DEFAULT_PAGE_SIZE,
        "a read naming no maxResults must return one default-sized page, got {body}"
    );
    assert!(
        body["nextToken"].is_string(),
        "a bounded page over a larger store must carry a nextToken, got {body}"
    );

    // Walking those pages still visits every stored record exactly once.
    let mut seen = 0_usize;
    let mut next_token = body["nextToken"].as_str().map(str::to_string);
    while let Some(token) = next_token {
        seen += DEFAULT_PAGE_SIZE;
        let (status, body) = call(
            &router,
            "GetRecommendationPreferences",
            json!({"resourceType": "Ec2Instance", "nextToken": token}),
        )
        .await;
        assert_eq!(status, 200, "paging a full store: got {status}: {body}");
        let page = body["recommendationPreferencesDetails"]
            .as_array()
            .map_or(0, Vec::len);
        assert!(
            page <= DEFAULT_PAGE_SIZE,
            "paging a full store: a page must not exceed {DEFAULT_PAGE_SIZE}, got {page}"
        );
        next_token = body["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            seen += page;
        }
    }
    assert_eq!(
        seen, MAX_PREFERENCE_RECORDS,
        "paging must visit every one of the {MAX_PREFERENCE_RECORDS} stored records, saw {seen}"
    );

    // And no `maxResults` a request may carry can exceed the page ceiling.
    let (status, body) = call(
        &router,
        "GetRecommendationPreferences",
        json!({"resourceType": "Ec2Instance", "maxResults": 1000}),
    )
    .await;
    assert_eq!(status, 200, "maxResults=1000: got {status}: {body}");
    let page = body["recommendationPreferencesDetails"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(
        page <= MAX_PAGE_SIZE,
        "maxResults=1000: a page must not exceed the {MAX_PAGE_SIZE} ceiling, got {page}"
    );
}

// --- which caller may create a preference at the organization level -----------------------
//
// "You can create recommendation preferences at the organization level (for management accounts
// of an organization only), account level, and resource level"
// (`PutRecommendationPreferences$scope`): a request stating an organization-level scope is
// admitted only from a caller that MANAGES an organization, and any other caller is refused with
// the service's one sentence for the condition, storing nothing. The tables below span what that
// rule generalises over — the resource type, which preference member the request sets, how many
// members ride along, and, on the admitting side, the organization the management account is
// recorded with — because what a recording happens to carry is one row of the class.

/// The sentence this service refuses an organization-wide request from a caller that manages no
/// organization with: the wording it already answers the organization-wide
/// `includeMemberAccounts` with, which names neither the member nor the account — it states the
/// two reasons the caller cannot ask for the organization. Spelled out here rather than imported
/// so the WORDING is what these tables pin.
const NOT_A_MANAGEMENT_ACCOUNT_MESSAGE: &str = "Cannot include member accounts. This could be \
     because you are not an AWS Organization user or you are not the master account in the \
     organization.";

/// The organization-level scope every row below states: the one value the model admits for it,
/// "The `value` must be `ALL_ACCOUNTS`" (`Scope$value`).
fn organization_scope() -> Value {
    json!({"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE})
}

/// One row: the resource type the preference is written for, and the preference members the
/// request sets — each settable for that resource type, so the caller's role is the only thing
/// that can refuse the request.
struct ScopeRow {
    case: &'static str,
    resource_type: &'static str,
    preferences: Value,
}

/// One row per preference member kind, spread over the resource types each is settable for: the
/// two status switches (`enhancedInfrastructureMetrics`, `inferredWorkloadTypes`), the scalar
/// (`lookBackPeriod`), the mode (`savingsEstimationMode`), the object
/// (`externalMetricsPreference`) and the two lists (`utilizationPreferences`,
/// `preferredResources`) — alone, and several in one request. An Auto Scaling group is absent:
/// its preference "can only be set at the resource level" whoever asks, which
/// `tests/all/client_refusal_wording.rs` pins.
fn organization_scope_rows() -> Vec<ScopeRow> {
    vec![
        ScopeRow {
            case: "a look-back period for EC2 instances",
            resource_type: "Ec2Instance",
            preferences: json!({"lookBackPeriod": "DAYS_93"}),
        },
        ScopeRow {
            case: "a look-back period for EBS volumes",
            resource_type: "EbsVolume",
            preferences: json!({"lookBackPeriod": "DAYS_32"}),
        },
        ScopeRow {
            case: "a status switch and an object in one request",
            resource_type: "Ec2Instance",
            preferences: json!({
                "enhancedInfrastructureMetrics": "Active",
                "externalMetricsPreference": {"source": "Datadog"},
            }),
        },
        ScopeRow {
            case: "a savings estimation mode for RDS DB instances",
            resource_type: "RdsDBInstance",
            preferences: json!({"savingsEstimationMode": "AfterDiscounts"}),
        },
        ScopeRow {
            case: "the inferred workload types switch for Lambda functions",
            resource_type: "LambdaFunction",
            preferences: json!({"inferredWorkloadTypes": "Inactive"}),
        },
        ScopeRow {
            case: "a preferred-resource list for ECS services",
            resource_type: "EcsService",
            preferences: json!({
                "preferredResources": [{"name": "Ec2InstanceTypes", "includeList": ["m5"]}],
            }),
        },
        ScopeRow {
            case: "a utilization-preference list and a look-back period together",
            resource_type: "Ec2Instance",
            preferences: json!({
                "utilizationPreferences": [{
                    "metricName": "CpuUtilization",
                    "metricParameters": {"threshold": "P95", "headroom": "PERCENT_20"},
                }],
                "lookBackPeriod": "DAYS_14",
            }),
        },
        ScopeRow {
            case: "a savings estimation mode for idle resources",
            resource_type: "Idle",
            preferences: json!({"savingsEstimationMode": "BeforeDiscounts"}),
        },
    ]
}

/// The `PutRecommendationPreferences` request a row sends, at `scope`.
fn scope_row_request(row: &ScopeRow, scope: Option<&Value>) -> Value {
    let mut request = json!({"resourceType": row.resource_type});
    for (member, value) in row
        .preferences
        .as_object()
        .expect("the row's preferences are an object")
    {
        request[member] = value.clone();
    }
    if let Some(scope) = scope {
        request["scope"] = scope.clone();
    }
    request
}

/// How many preference records a `GetRecommendationPreferences` response reports.
fn reported_records(read: &Value) -> usize {
    read["recommendationPreferencesDetails"]
        .as_array()
        .map_or(0, Vec::len)
}

/// A caller that manages no organization — which this emulator presents while its store records
/// no member account — cannot create a preference at the organization level: every row is
/// refused with the service's sentence for the condition, nothing is stored at the key or
/// anywhere in its resource type, and no resource inherits what the request asked for.
#[tokio::test]
async fn an_organization_level_preference_is_refused_to_a_caller_that_manages_no_organization() {
    let scope = organization_scope();
    for row in organization_scope_rows() {
        // A fresh emulator per row: what the reads report is this row's request alone.
        let router = emulator();
        opt_in(&router).await;

        let asked = scope_row_request(&row, Some(&scope));
        let (status, body) = call(&router, "PutRecommendationPreferences", asked.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected {asked} to be refused with 400, got {status}: {body}",
            row.case
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {:?}: expected InvalidParameterValueException, got {body}",
            row.case
        );
        assert_eq!(
            body["message"].as_str(),
            Some(NOT_A_MANAGEMENT_ACCOUNT_MESSAGE),
            "case {:?}: expected the service's sentence for a caller that manages no \
             organization, got {body}",
            row.case
        );

        // A refused request stores nothing: neither at the key it addressed nor anywhere else
        // in its resource type.
        for (what, at) in [
            ("the key it addressed", Some(&scope)),
            ("the whole resource type", None),
        ] {
            let read = get_preferences(&router, row.resource_type, at).await;
            assert_eq!(
                reported_records(&read),
                0,
                "case {:?}: a refused request must leave nothing for a read of {what} to \
                 report, got {read}",
                row.case
            );
        }

        // And nothing to inherit: an organization-level record is what every resource of the
        // account would otherwise resolve its effective preferences from, so the documented
        // defaults must still be in force.
        let effective = effective_for(&router, &instance_arn("i-0ab0c0d0e0f000011")).await;
        assert_eq!(
            effective["lookBackPeriod"],
            json!(DEFAULT_LOOK_BACK_PERIOD),
            "case {:?}: expected the documented default lookBackPeriod, got {effective}",
            row.case
        );
        assert_eq!(
            effective["enhancedInfrastructureMetrics"],
            json!(DEFAULT_ENHANCED_INFRASTRUCTURE_METRICS),
            "case {:?}: expected the documented default status, got {effective}",
            row.case
        );
    }
}

/// The organizations a management account can be recorded with — the organization's SIZE and the
/// enrollment its members are in, neither of which the rule depends on: one member account,
/// several, and one whose enrollment is already settled at a status.
fn organizations() -> Vec<(&'static str, Vec<MemberAccountEnrollment>)> {
    let settled = |account_id: &str, status: &str| {
        let mut member = MemberAccountEnrollment::new(account_id);
        member.status = status.to_string();
        member
    };
    vec![
        (
            "one member account",
            vec![MemberAccountEnrollment::new("200000000001")],
        ),
        (
            "three member accounts",
            vec![
                MemberAccountEnrollment::new("200000000001"),
                MemberAccountEnrollment::new("200000000002"),
                MemberAccountEnrollment::new("200000000003"),
            ],
        ),
        (
            "a member account already opted in",
            vec![settled("200000000004", "Active")],
        ),
        (
            "a member account opted out",
            vec![settled("200000000005", "Inactive")],
        ),
    ]
}

/// The same requests are ACCEPTED from a management account, whatever organization it manages,
/// and the record each creates is read back at its own key: what the refusal above turns on is
/// the caller's role, not the organization level itself.
#[tokio::test]
async fn an_organization_level_preference_is_created_by_a_management_account() {
    let scope = organization_scope();
    let organizations = organizations();
    for (index, row) in organization_scope_rows().into_iter().enumerate() {
        // Each row under a different organization, so no row's acceptance depends on one shape
        // of organization; a fresh emulator per row keeps the reads to this row's write.
        let (organization_case, members) = &organizations[index % organizations.len()];
        let case = format!("{} for {organization_case}", row.case);
        let (router, engine) = emulator_with_engine();
        record_organization(&engine, members);
        opt_in(&router).await;

        let asked = scope_row_request(&row, Some(&scope));
        // `put_preference` asserts HTTP 200 and the modelled empty output body.
        put_preference(&router, &asked).await;

        let read = get_preferences(&router, row.resource_type, Some(&scope)).await;
        assert_eq!(
            reported_records(&read),
            1,
            "case {case:?}: expected exactly the record just created, got {read}"
        );
        let detail = &read["recommendationPreferencesDetails"][0];
        assert_eq!(
            detail["scope"], scope,
            "case {case:?}: expected the detail to carry the organization-level scope, got {read}"
        );
        for (member, written) in row
            .preferences
            .as_object()
            .expect("the row's preferences are an object")
        {
            if member == "preferredResources" {
                // Reported as `EffectivePreferredResource`: the stored lists plus the
                // `effectiveIncludeList` expansion, so the stored list is what round-trips.
                assert_eq!(
                    detail[member][0]["includeList"], written[0]["includeList"],
                    "case {case:?}: expected the stored includeList {written}, got {read}"
                );
                continue;
            }
            assert_eq!(
                detail[member], *written,
                "case {case:?}: expected {member} = {written} to be read back, got {read}"
            );
        }
    }
}

/// The record a management account creates at the organization level is what every resource of
/// the account then inherits: the admitted write is a real preference, not a 200 over an empty
/// store.
#[tokio::test]
async fn a_created_organization_level_preference_is_inherited_by_the_accounts_resources() {
    let (router, _engine) = management_account_emulator();
    opt_in(&router).await;

    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": organization_scope(),
            "lookBackPeriod": "DAYS_93",
            "enhancedInfrastructureMetrics": "Active",
        }),
    )
    .await;

    for instance_id in ["i-0ab0c0d0e0f000011", "i-0ab0c0d0e0f000022"] {
        let arn = instance_arn(instance_id);
        let effective = effective_for(&router, &arn).await;
        assert_eq!(
            effective["lookBackPeriod"], "DAYS_93",
            "case {arn:?}: expected the organization-level lookBackPeriod in force, got \
             {effective}"
        );
        assert_eq!(
            effective["enhancedInfrastructureMetrics"], "Active",
            "case {arn:?}: expected the organization-level status in force, got {effective}"
        );
    }
}

/// The other scope levels name the caller's OWN account or one of its resources, so a caller
/// that manages no organization still creates a preference at every one of them: the refusal is
/// no wider than the organization level.
#[tokio::test]
async fn the_other_scope_levels_are_created_by_a_caller_that_manages_no_organization() {
    let instance = instance_arn("i-0ab0c0d0e0f000033");
    let scopes: Vec<(&str, &str, Option<Value>)> = vec![
        ("no scope at all", "Ec2Instance", None),
        (
            "the account level",
            "Ec2Instance",
            Some(json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})),
        ),
        (
            "an instance at the resource level",
            "Ec2Instance",
            Some(json!({"name": SCOPE_RESOURCE_ARN, "value": instance})),
        ),
        (
            "an Auto Scaling group at the resource level",
            "AutoScalingGroup",
            Some(json!({
                "name": SCOPE_RESOURCE_ARN,
                "value": auto_scaling_group_arn("batch-tier"),
            })),
        ),
    ];

    for (case, resource_type, scope) in &scopes {
        let router = emulator();
        opt_in(&router).await;

        let mut asked = json!({"resourceType": resource_type, "lookBackPeriod": "DAYS_32"});
        if let Some(scope) = scope {
            asked["scope"] = scope.clone();
        }
        put_preference(&router, &asked).await;

        let read = get_preferences(&router, resource_type, scope.as_ref()).await;
        assert_eq!(
            reported_records(&read),
            1,
            "case {case:?}: expected the record created at this scope, got {read}"
        );
        assert_eq!(
            read["recommendationPreferencesDetails"][0]["lookBackPeriod"], "DAYS_32",
            "case {case:?}: expected the written lookBackPeriod to be read back, got {read}"
        );
    }
}

/// Reading and deleting at the organization level stay SERVED to a caller that manages no
/// organization: the rule is about creating that record, and such a caller holds none — so the
/// read reports nothing at that scope and the delete removes nothing, while the records it does
/// hold at its own scopes are untouched.
#[tokio::test]
async fn an_organization_level_read_and_delete_report_the_records_the_caller_holds() {
    let router = emulator();
    opt_in(&router).await;

    let account_scope = json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT});
    put_preference(
        &router,
        &json!({
            "resourceType": "Ec2Instance",
            "scope": account_scope.clone(),
            "lookBackPeriod": "DAYS_93",
        }),
    )
    .await;

    let read = get_preferences(&router, "Ec2Instance", Some(&organization_scope())).await;
    assert_eq!(
        reported_records(&read),
        0,
        "case \"the organization-level read\": a caller that manages no organization holds no \
         record at that scope, got {read}"
    );

    let (status, body) = call(
        &router,
        "DeleteRecommendationPreferences",
        json!({
            "resourceType": "Ec2Instance",
            "scope": organization_scope(),
            "recommendationPreferenceNames": ["LookBackPeriodPreference"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the organization-level delete\": expected 200, got {status}: {body}"
    );

    let read = get_preferences(&router, "Ec2Instance", Some(&account_scope)).await;
    assert_eq!(
        read["recommendationPreferencesDetails"][0]["lookBackPeriod"],
        json!("DAYS_93"),
        "case \"the account-level record\": a delete at another scope must leave it alone, got \
         {read}"
    );
}
