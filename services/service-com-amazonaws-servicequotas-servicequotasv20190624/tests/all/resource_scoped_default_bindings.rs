// SPDX-License-Identifier: CC0-1.0

//! The default-quota rows a service publishes as quotas OF A RESOURCE, and the code each
//! of them is published under.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes, so the row served under one
//! is a claim about the quota the service publishes UNDER THAT CODE. The rules driven here
//! over a TABLE of codes whose binding a source available here states:
//!
//! * the row published under a code carries that quota's own `QuotaName`, `Value`,
//!   `Unit`, `Adjustable`, `GlobalQuota` and `Description`, and nothing of a neighbouring
//!   quota's — identically through `ListAWSDefaultServiceQuotas` and through
//!   `GetAWSDefaultServiceQuota`;
//! * a quota the service publishes as a quota OF A RESOURCE carries a `QuotaContext` that
//!   names the plane it is counted on (`ContextScope` `RESOURCE`), the resource type that
//!   plane applies to, and the level an increase may be asked for at; a quota managed for
//!   the account carries no `QuotaContext` at all, so a client can tell an account-wide
//!   limit from a per-resource one;
//! * `Adjustable` is a fact of the code rather than of the quota's class: the row under a
//!   code the service publishes as fixed refuses `RequestServiceQuotaIncrease` while an
//!   adjustable one takes it, so a misbound row sends a client down a path the service
//!   does not serve;
//! * and one quota is published under ONE code: the name a code publishes is published
//!   under no other code of that catalog.
//!
//! The table spans what the rules generalise over: three `ServiceCode`s; both levels a
//! per-resource quota may be adjustable at (`ALL` and `ACCOUNT`) and three resource
//! types; resource-scoped rows beside account-managed ones; adjustable rows beside fixed
//! ones; rows that publish a sentence beside rows whose published sentence no source
//! available here states; and metered rows beside unmetered ones. The harness (`client`,
//! `emulator`, `default_quotas`) is the one `quota_workflow.rs` owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    LEVEL_ACCOUNT, LEVEL_ALL,
};

use crate::quota_workflow::{client, default_quotas as defaults, emulator, Client, ACCOUNT};
use crate::recorded_bindings_sns_through_xray::context_json;

/// One published binding: the code, and the members of the quota published under it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The sentence the service publishes for the quota, or `None` where no source
    /// available here states it: `Description` is optional (`@length 0..350`), so such a
    /// row publishes none rather than one this emulator words itself.
    description: Option<&'static str>,
    /// `(ContextScopeType, AdjustableAtLevel)` for a quota published as a quota OF A
    /// RESOURCE; `None` for one managed for the account, which publishes no context.
    resource_plane: Option<(&'static str, &'static str)>,
}

/// The bindings this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each `ServiceCode`,
/// which is the reference for which quota a code publishes, for the level and resource
/// type its per-resource plane is published with, and for the wording of each row's
/// `Description`. The catalog these are seeded from is
/// `src/state/quota_rows/<service>.rs`, which names the source of every member beside it.
const ROWS: &[Row] = &[
    // The custom response bodies of one regional web ACL or rule group: published as
    // ADJUSTABLE and as a quota of the web ACL, at the `ALL` level, where the catalog
    // served the per-rule statement nesting depth as a fixed, account-managed row.
    Row {
        service_code: "wafv2",
        quota_code: "L-71C2E81B",
        quota_name: "Maximum number of custom response bodies per web ACL or rule group for \
                     regional",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: None,
        resource_plane: Some(("AWS::WAFv2::WebACL", LEVEL_ALL)),
    },
    // The two association planes of one regional web ACL, published the same way: the
    // rule holds for a class of rows, not for one of them.
    Row {
        service_code: "wafv2",
        quota_code: "L-5B2F9B11",
        quota_name: "Maximum number of AWS AppSync GraphQL API associations per web ACL in WAF \
                     for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        resource_plane: Some(("AWS::WAFv2::WebACL", LEVEL_ALL)),
    },
    Row {
        service_code: "wafv2",
        quota_code: "L-B32E8672",
        quota_name: "Maximum number of Amazon API Gateway REST API associations per web ACL in \
                     WAF for regional",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        resource_plane: Some(("AWS::WAFv2::WebACL", LEVEL_ALL)),
    },
    // The plane the `L-71C2E81B` binding displaced, under the code it is carried by: a
    // FIXED row managed for the account, so it publishes no context and takes no increase.
    Row {
        service_code: "wafv2",
        quota_code: "L-0224FEE0",
        quota_name: "Statement nesting depth per rule",
        value: 10.0,
        unit: "None",
        adjustable: false,
        description: Some("The maximum nesting depth of the statements of one rule."),
        resource_plane: None,
    },
    // Another service's per-resource planes, published at the ACCOUNT level: the value is
    // applied to each resource while an increase is asked for account-wide.
    Row {
        service_code: "vpc",
        quota_code: "L-0EA8095F",
        quota_name: "Inbound or outbound rules per security group",
        value: 60.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of inbound or outbound rules per security group. This quota \
             counts the inbound rules and the outbound rules of a security group separately.",
        ),
        resource_plane: Some(("AWS::EC2::SecurityGroup", LEVEL_ACCOUNT)),
    },
    Row {
        service_code: "vpc",
        quota_code: "L-93826ACB",
        quota_name: "Routes per route table",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of non-propagated routes per route table. This quota can be \
             increased up to a maximum of 1000; however, network performance might be impacted.",
        ),
        resource_plane: Some(("AWS::EC2::RouteTable", LEVEL_ACCOUNT)),
    },
    // An account-managed, fixed, measured row of a third service: no context, no
    // increase, and a unit other than `None`.
    Row {
        service_code: "xray",
        quota_code: "L-D781C0FD",
        quota_name: "Segment document size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: false,
        description: Some("The maximum size for segment documents."),
        resource_plane: None,
    },
];

/// The members a binding fixes, as the row must publish them.
///
/// A narrower projection than the thirteen-member one
/// `recorded_bindings_sns_through_xray.rs` compares a whole recorded row with: the rules
/// here are about the identity, the limit, the adjustability and the PLANE of a row, so
/// the window and the usage metric are that suite's to pin and are left out of the
/// comparison rather than restated in this table.
fn published(row: &Row) -> Value {
    json!({
        "QuotaCode": row.quota_code,
        "ServiceCode": row.service_code,
        "QuotaName": row.quota_name,
        "Value": row.value,
        "Unit": row.unit,
        "Adjustable": row.adjustable,
        "GlobalQuota": false,
        "Description": row.description.map_or(Value::Null, |text| json!(text)),
        "QuotaContext": context_json(row.resource_plane),
    })
}

/// The same members, read off a served record.
fn served(quota: &Value) -> Value {
    json!({
        "QuotaCode": quota["QuotaCode"],
        "ServiceCode": quota["ServiceCode"],
        "QuotaName": quota["QuotaName"],
        "Value": quota["Value"],
        "Unit": quota["Unit"],
        "Adjustable": quota["Adjustable"],
        "GlobalQuota": quota["GlobalQuota"],
        "Description": quota["Description"],
        "QuotaContext": quota["QuotaContext"],
    })
}

/// Every default quota of the services the table names, read once per service.
async fn catalogs(caller: &Client) -> BTreeMap<&'static str, Vec<Value>> {
    let mut catalogs = BTreeMap::new();
    for row in ROWS {
        if !catalogs.contains_key(row.service_code) {
            catalogs.insert(row.service_code, defaults(caller, row.service_code).await);
        }
    }
    catalogs
}

/// The table spans the classes the rules generalise over, so no row can be dropped
/// without the suite saying so.
#[test]
fn the_table_spans_the_published_row_classes() {
    let services: Vec<&str> = {
        let mut seen: Vec<&str> = ROWS.iter().map(|row| row.service_code).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };
    let scope_types: Vec<&str> = {
        let mut seen: Vec<&str> = ROWS
            .iter()
            .filter_map(|row| row.resource_plane.map(|(scope_type, _)| scope_type))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };
    let levels: Vec<&str> = {
        let mut seen: Vec<&str> = ROWS
            .iter()
            .filter_map(|row| row.resource_plane.map(|(_, level)| level))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    };
    let actual = (
        services.len(),
        scope_types.len(),
        levels.len(),
        ROWS.iter().any(|row| row.resource_plane.is_none()),
        ROWS.iter().any(|row| row.adjustable),
        ROWS.iter().any(|row| !row.adjustable),
        ROWS.iter().any(|row| row.description.is_none()),
        ROWS.iter().any(|row| row.unit != "None"),
    );
    assert_eq!(
        actual,
        (3, 3, 2, true, true, true, true, true),
        "case table: expected three services {services:?}, three resource types \
         {scope_types:?}, both adjustable levels {levels:?}, and account-managed, fixed, \
         sentenceless and measured rows among them, got {actual:?}"
    );
}

/// RULE: the row published under a code carries that quota's own members, including the
/// context of its per-resource plane, through the listing and through the single read
/// alike.
#[tokio::test]
async fn each_code_publishes_its_own_quota_with_its_own_context() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");
    let catalogs = catalogs(&caller).await;

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let wanted = published(row);
        let listed: Vec<&Value> = catalogs[row.service_code]
            .iter()
            .filter(|quota| quota["QuotaCode"] == json!(row.quota_code))
            .collect();
        assert_eq!(
            listed.len(),
            1,
            "case {case}: the listing must publish exactly one row under the code, got \
             {listed:?}"
        );
        assert_eq!(
            served(listed[0]),
            wanted,
            "case {case}: expected {wanted}, got {}",
            listed[0]
        );

        // The single read answers the same record for the same key.
        let read = caller
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            served(&read),
            wanted,
            "case {case} through GetAWSDefaultServiceQuota: expected {wanted}, got {read}"
        );

        // One quota, one code: the name is published under no other code of the catalog.
        let elsewhere: Vec<&Value> = catalogs[row.service_code]
            .iter()
            .filter(|quota| {
                quota["QuotaName"] == json!(row.quota_name)
                    && quota["QuotaCode"] != json!(row.quota_code)
            })
            .collect();
        assert!(
            elsewhere.is_empty(),
            "case {case}: {} must be published under this code only, also found under \
             {elsewhere:?}",
            row.quota_name
        );
    }
}

/// RULE: the adjustability published under a code decides whether an increase is taken,
/// and a `ContextId` is honoured exactly for a quota published as a quota of a resource.
#[tokio::test]
async fn the_published_plane_decides_what_the_quota_accepts() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        // Above the value in force, so the only thing left to refuse the request for is
        // the quota's published adjustability.
        let desired = row.value.max(1.0) * 2.0;
        let increase = json!({
            "ServiceCode": row.service_code,
            "QuotaCode": row.quota_code,
            "DesiredValue": desired,
        });
        if row.adjustable {
            let accepted = caller.ok("RequestServiceQuotaIncrease", increase).await;
            let requested = &accepted["RequestedQuota"];
            assert_eq!(
                (
                    requested["QuotaCode"].as_str(),
                    requested["QuotaName"].as_str(),
                    requested["DesiredValue"].as_f64(),
                ),
                (
                    Some(row.quota_code),
                    Some(row.quota_name),
                    Some(desired)
                ),
                "case {case}: the accepted request must name the quota published under the \
                 code, got {requested}"
            );
        } else {
            let (status, error) = caller.refused("RequestServiceQuotaIncrease", increase).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "IllegalArgumentException"),
                "case {case}: a quota the service publishes as fixed refuses an increase"
            );
        }

        // A read naming a resource is served exactly where an increase may be asked for
        // at the resource level, and refused where the quota is adjustable account-wide
        // only — which is every row that publishes no plane, and the per-resource planes
        // whose published level is the ACCOUNT.
        let context = format!("arn:aws:{}:eu-central-1:{ACCOUNT}:probe", row.service_code);
        let scoped = json!({
            "ServiceCode": row.service_code,
            "QuotaCode": row.quota_code,
            "ContextId": context,
        });
        if row
            .resource_plane
            .is_some_and(|(_, level)| level == LEVEL_ALL)
        {
            let record = caller.ok("GetServiceQuota", scoped).await["Quota"].clone();
            assert_eq!(
                record["QuotaContext"]["ContextId"],
                json!(context),
                "case {case}: the record must name the resource asked for, got {record}"
            );
        } else {
            let (status, error) = caller.refused("GetServiceQuota", scoped).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "IllegalArgumentException"),
                "case {case}: a quota adjustable account-wide only refuses a ContextId"
            );
        }
    }
}
