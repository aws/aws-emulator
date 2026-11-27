// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service lists an enumerated type's admissible values in when the
//! boundary refuses a value outside that type.
//!
//! RULE under test: a refusal of a value outside an enumerated type states the standard
//! constraint report — `N validation error(s) detected: Value '<value>' at '<member>'
//! failed to satisfy constraint: Member must satisfy enum value set: [<values>]` — with
//! that value set listed in the order this service's own TABLE of the admissible values
//! is walked (`service_common::validation_message::hashed_enum_order`, the fleet's one
//! home for that derivation), NOT in the order the model declares the type's members in.
//! The order is computed from the values themselves, so it belongs to the enumerated TYPE
//! and holds for every member targeting it: any operation, any binding (a query
//! parameter, a body member, a member nested inside one), any depth.
//!
//! Everything else about the refusal is the framework's and is unchanged: the counted
//! envelope, the member path, the echoed value, the status and the error code, and every
//! other constraint class's requirement.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use service_common::validation_message::hashed_enum_order;
use smithy_server_runtime::MultiServiceRouter;

/// `EntityStatusFilter`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="EntityStatusFilter")`).
const ENTITY_STATUS_FILTER_DECLARED: &[&str] = &["ACTIVE", "DRAFT", "DISABLED"];

/// The same three values in the order this service STATES them, which is recorded
/// behaviour: a clause rendered from the model alone would list the declaration order
/// above.
const ENTITY_STATUS_FILTER_STATED: &[&str] = &["DRAFT", "ACTIVE", "DISABLED"];

/// `SynchronizationStatus`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="SynchronizationStatus")`).
const SYNCHRONIZATION_STATUS_DECLARED: &[&str] = &["IN_SYNC", "OUT_OF_SYNC", "NOT_APPLICABLE"];

/// The same three values in the order this service STATES them — recorded behaviour, and
/// the second, independent set that pins the derivation below.
const SYNCHRONIZATION_STATUS_STATED: &[&str] = &["NOT_APPLICABLE", "OUT_OF_SYNC", "IN_SYNC"];

/// `PolicyFirewallType`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="PolicyFirewallType")`).
const POLICY_FIREWALL_TYPE_DECLARED: &[&str] = &["WAF", "SHIELD_ADVANCED", "IGW_FIREWALL"];

/// `ExistingCustomerWebACLResolution`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="ExistingCustomerWebACLResolution")`).
const WEB_ACL_RESOLUTION_DECLARED: &[&str] = &["RETROFIT", "OVERRIDE_ASSOCIATION", "NO_REMEDIATION"];

/// `RuleType`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="RuleType")`).
const RULE_TYPE_DECLARED: &[&str] = &["CONFIGURATION", "INSPECTION"];

/// The clause a refusal states about an enumerated member, over `values`.
fn value_set_clause(values: &[&str]) -> String {
    format!(
        "failed to satisfy constraint: Member must satisfy enum value set: [{}]",
        values.join(", ")
    )
}

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    json: serde_json::Value,
}

impl Reply {
    fn message(&self) -> String {
        self.json["message"]
            .as_str()
            .unwrap_or_else(|| panic!("a refusal states a message, got {:?}", self.json))
            .to_string()
    }
}

/// The emulator under test: this one service mounted on the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "network-security-manager.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/us-east-1/\
                 network-security-manager/aws4_request, SignedHeaders=host, Signature=0",
            )
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let json = if raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null)
        };
        Reply {
            status,
            error_code,
            json,
        }
    }
}

/// A `CreatePolicy` body whose members are all admissible but for what a row overrides.
fn policy_body(firewall_type: &str, web_acl_resolution: &str) -> serde_json::Value {
    serde_json::json!({
        "policyName": "enum-order-probe",
        "priority": 1,
        "firewallType": firewall_type,
        "policyConfiguration": {
            "remediationEnabled": true,
            "resourcesCleanUp": true,
            "wafConfig": {
                "existingCustomerWebACLResolution": web_acl_resolution,
                "conflictResolution": "MERGE_WHERE_APPLICABLE",
            },
        },
    })
}

/// RULE: the derivation this service's refusals are stated by lists each RECORDED value
/// set in the order that recording shows, whatever order the set is presented in.
///
/// Two independent sets pin it: `EntityStatusFilter` and `SynchronizationStatus`, neither
/// of which is stated in its declaration order and neither of which is stated sorted.
#[test]
fn the_stated_order_is_the_tables_order_for_every_recorded_set() {
    type Row = (&'static str, &'static [&'static str], &'static [&'static str]);
    let rows: [Row; 2] = [
        (
            "EntityStatusFilter",
            ENTITY_STATUS_FILTER_DECLARED,
            ENTITY_STATUS_FILTER_STATED,
        ),
        (
            "SynchronizationStatus",
            SYNCHRONIZATION_STATUS_DECLARED,
            SYNCHRONIZATION_STATUS_STATED,
        ),
    ];

    for (label, declared, stated) in rows {
        assert_eq!(
            hashed_enum_order(declared),
            stated,
            "case {label}: the derivation must state the recorded order"
        );
        assert_ne!(
            hashed_enum_order(declared),
            declared,
            "case {label}: the stated order is not the declaration order"
        );
        let mut sorted: Vec<&str> = declared.to_vec();
        sorted.sort_unstable();
        assert_ne!(
            hashed_enum_order(declared),
            sorted,
            "case {label}: the stated order is not a sort of the values"
        );
        // The order belongs to the TYPE, so presenting the same values in another order
        // cannot move them.
        let mut presented: Vec<&str> = declared.to_vec();
        presented.reverse();
        assert_eq!(
            hashed_enum_order(&presented),
            stated,
            "case {label}: the order must not depend on the order the values arrive in"
        );
    }
}

/// RULE: every member targeting an enumerated type is refused with that type's value set
/// in the stated order — in any operation, in any binding, at any depth.
#[tokio::test]
async fn an_out_of_enum_value_is_refused_with_the_stated_value_set() {
    let emulator = Emulator::new();

    /// One row: the case, the request, the value it submits, and the enum whose set the
    /// clause must list.
    struct Row {
        label: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<serde_json::Value>,
        /// The member path the clause names, when it is a top-level member and so known
        /// exactly; a nested member's path is left to the framework.
        member: Option<&'static str>,
        value: &'static str,
        declared: &'static [&'static str],
    }

    let rows = vec![
        // The two recorded rows: a query filter on two different operations, one enum.
        Row {
            label: "ListTemplates.status, the recorded case",
            method: "GET",
            path: "/templates?status=BOGUS",
            body: None,
            member: Some("status"),
            value: "BOGUS",
            declared: ENTITY_STATUS_FILTER_DECLARED,
        },
        Row {
            label: "ListAggregateResourceSynchronizationStatuses.synchronizationStatus",
            method: "GET",
            path: "/aggregate-sync-statuses?synchronizationStatus=NOT_A_REAL_STATUS",
            body: None,
            member: Some("synchronizationStatus"),
            value: "NOT_A_REAL_STATUS",
            declared: SYNCHRONIZATION_STATUS_DECLARED,
        },
        // The same enum on operations the recording does not cite, including one whose
        // handler is implemented and one whose handler is still a scaffold: the refusal
        // happens before any handler runs.
        Row {
            label: "ListDeployments.status",
            method: "GET",
            path: "/deployments?status=ACTIVE_DRAFT",
            body: None,
            member: Some("status"),
            value: "ACTIVE_DRAFT",
            declared: ENTITY_STATUS_FILTER_DECLARED,
        },
        Row {
            label: "ListPolicies.status",
            method: "GET",
            path: "/policies?status=active",
            body: None,
            member: Some("status"),
            value: "active",
            declared: ENTITY_STATUS_FILTER_DECLARED,
        },
        Row {
            label: "ListRules.status",
            method: "GET",
            path: "/rules?status=RETIRED",
            body: None,
            member: Some("status"),
            value: "RETIRED",
            declared: ENTITY_STATUS_FILTER_DECLARED,
        },
        // A BODY member, and a member nested two structures deep inside one: the same
        // rule, over enums the recording does not cite at all.
        Row {
            label: "CreatePolicy.firewallType, a body member",
            method: "POST",
            path: "/policies",
            body: Some(policy_body("FIREWALL", "RETROFIT")),
            member: Some("firewallType"),
            value: "FIREWALL",
            declared: POLICY_FIREWALL_TYPE_DECLARED,
        },
        Row {
            label: "CreatePolicy's nested wafConfig.existingCustomerWebACLResolution",
            method: "POST",
            path: "/policies",
            body: Some(policy_body("WAF", "REPLACE")),
            member: None,
            value: "REPLACE",
            declared: WEB_ACL_RESOLUTION_DECLARED,
        },
        Row {
            label: "CreateRule.ruleType",
            method: "POST",
            path: "/rules",
            body: Some(serde_json::json!({
                "ruleName": "enum-order-probe",
                "firewallType": "WAF",
                "ruleType": "INSPECTION_GROUP",
                "configuration": { "DefaultAction": "ALLOW" },
            })),
            member: Some("ruleType"),
            value: "INSPECTION_GROUP",
            declared: RULE_TYPE_DECLARED,
        },
    ];

    for row in rows {
        let label = row.label;
        let reply = emulator.call(row.method, row.path, row.body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {label}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {label}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );

        let message = reply.message();
        let stated = hashed_enum_order(row.declared);
        let clause = value_set_clause(&stated);
        assert!(
            message.ends_with(&clause),
            "case {label}: expected the message to end {clause:?}, got {message:?}"
        );
        // The declaration order is what a clause rendered from the model alone would
        // list, so a row whose stated order differs from it also proves the move
        // happened.
        if stated != row.declared {
            assert!(
                !message.contains(&value_set_clause(row.declared)),
                "case {label}: the value set must not be listed in declaration order, got \
                 {message:?}"
            );
        }
        if let Some(member) = row.member {
            assert_eq!(
                message,
                format!(
                    "1 validation error detected: Value '{}' at '{member}' {clause}",
                    row.value
                ),
                "case {label}: unexpected message"
            );
        } else {
            assert!(
                message.contains(&format!("Value '{}' at '", row.value)),
                "case {label}: the message must echo the submitted value, got {message:?}"
            );
        }
    }
}

/// RULE: only the ORDER inside an enum clause is this service's — a refusal of another
/// constraint class keeps the framework's own wording, and so states no value set at all.
#[tokio::test]
async fn a_refusal_of_another_constraint_class_is_unchanged() {
    let emulator = Emulator::new();

    // The rows differ along the constraint class broken: a `@range` minimum, a `@range`
    // maximum, a `@pattern`, and an absent `@required` member.
    let rows: Vec<(&str, &str, &str, Option<serde_json::Value>)> = vec![
        (
            "maxResults below its @range minimum",
            "GET",
            "/deployments?maxResults=0",
            None,
        ),
        (
            "maxResults above its @range maximum",
            "GET",
            "/deployments?maxResults=101",
            None,
        ),
        (
            "a nextToken outside its @pattern",
            "GET",
            "/deployments?nextToken=not%20a%20token",
            None,
        ),
        (
            "an absent @required member",
            "POST",
            "/deployments",
            Some(serde_json::json!({
                "deploymentConfiguration": { "enableCrossAccountVisibility": false },
            })),
        ),
    ];

    for (label, method, path, body) in rows {
        let reply = emulator.call(method, path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {label:?}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
        let message = reply.message();
        assert!(
            !message.contains("enum value set"),
            "case {label:?}: this refusal states no enum value set, got {message:?}"
        );
        assert!(
            message.contains("validation error"),
            "case {label:?}: the framework's envelope must be kept, got {message:?}"
        );
    }
}
