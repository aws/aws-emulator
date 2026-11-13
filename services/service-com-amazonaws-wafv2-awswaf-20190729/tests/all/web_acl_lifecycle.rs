// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 WebACL + IPSet lifecycle, end to end over the wire.
//!
//! Every call here goes through the same front door a real client uses: a
//! `POST /` carrying `X-Amz-Target: AWSWAF_20190729.<Operation>` and an
//! `awsJson1_1` body, dispatched by the multi-service router onto one shared
//! engine. Nothing reaches into the state struct, so what these tests observe is
//! exactly what an SDK observes.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    MAX_IP_SETS_PER_SCOPE, MAX_IP_SET_ADDRESSES, MAX_STATEMENT_NESTING_DEPTH,
    MAX_WEB_ACLS_PER_SCOPE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for unless a case asks for another one.
pub(crate) const TEST_REGION: &str = "us-west-2";

/// The ONE region endpoint the CLOUDFRONT scope is served at: "API and SDKs - For
/// all calls, use the Region endpoint us-east-1" (`Scope`'s own doc). A conforming
/// client has to make a CLOUDFRONT-scoped call there, so [`Waf::call`] signs one for
/// this region and everything else for [`TEST_REGION`].
pub(crate) const CLOUDFRONT_ENDPOINT_REGION: &str = "us-east-1";

/// One emulator with the wafv2 service mounted on a single shared engine.
///
/// Shared with the sibling suites in `tests/all/`, so every suite drives the
/// service through the same front door.
pub(crate) struct Waf {
    router: MultiServiceRouter,
}

impl Waf {
    pub(crate) fn new() -> Self {
        Self::sharing_engine().0
    }

    /// [`Self::new`], handing the shared engine back as well.
    ///
    /// For a suite that must seed state NO operation writes — the per-web-ACL
    /// request log `GetTopPathStatisticsByTraffic` aggregates, which only WAF's data
    /// plane appends to (see `crate::top_path_statistics`).
    pub(crate) fn sharing_engine() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise the service, not SigV4: the `Authorization` header
        // below is present so the request ROUTES and names a region, and is never
        // verified.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(engine.clone()),
        );
        (Self { router }, engine)
    }

    /// POST one awsJson1_1 call, credential-scoped for `region`, and answer its
    /// HTTP status and parsed body.
    async fn call_in(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLEWAFV2/20260101/{region}/wafv2/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=00"
        );
        let request = ::http::Request::builder()
            .method("POST")
            .uri("https://wafv2.example.com/")
            .header("host", "wafv2.example.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSWAF_20190729.{operation}"))
            .header("authorization", authorization)
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(response.body()).into_owned())
            })
        };
        (status, parsed)
    }

    /// [`Self::call_in`] for the endpoint a conforming client would use: a request
    /// naming the CLOUDFRONT scope goes to [`CLOUDFRONT_ENDPOINT_REGION`], every
    /// other to [`TEST_REGION`].
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let region = match body.get("Scope").and_then(Value::as_str) {
            Some("CLOUDFRONT") => CLOUDFRONT_ENDPOINT_REGION,
            _ => TEST_REGION,
        };
        self.call_in(region, operation, body).await
    }

    /// [`Self::call_in`] for a region the case names, whatever scope the body
    /// carries: how a suite drives a call from an endpoint that does not serve the
    /// scope it names.
    pub(crate) async fn call_from(
        &self,
        region: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call_in(region, operation, body).await
    }

    /// The call must answer 200; answers its parsed body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got status {status} and body {parsed}"
        );
        parsed
    }

    /// The call must be refused with HTTP 400 and the error code `code`.
    pub(crate) async fn refused(
        &self,
        operation: &str,
        body: Value,
        code: &str,
        case: &str,
    ) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} must answer 400 {code}, got status {status} \
             and body {parsed}"
        );
        assert_error_code(&parsed, code, case);
        parsed
    }
}

/// The awsJson error code of a refusal lives in the body's `__type`.
fn assert_error_code(body: &Value, expected: &str, case: &str) {
    let actual = body["__type"].as_str().unwrap_or_default();
    assert!(
        actual.ends_with(expected),
        "case {case:?}: expected error code {expected}, got {actual:?} in body {body}"
    );
}

/// `^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$` — the form the model pins for
/// `EntityId` and `LockToken` (`smithy_query(query="shape",shape="LockToken")`),
/// checked by hand so the test target needs no regex engine.
pub(crate) fn is_model_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
}

/// A minimal valid `VisibilityConfig` for a rule or web ACL.
pub(crate) fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A blocking rule wrapping `statement`.
pub(crate) fn rule(name: &str, statement: Value) -> Value {
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": statement,
        "Action": { "Block": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// `rules` with each rule's `Priority` set to its position in the list.
///
/// `Rule$Priority` is the evaluation order and "the priorities don't need to be
/// consecutive, but they must all be different", so a list of SEVERAL rules built
/// from [`rule`] — which gives every rule the same priority — has to be ordered
/// before it can be saved. The order is the one the list is written in, so a row
/// that reads its rules back sees them as it submitted them.
pub(crate) fn ordered(rules: Value) -> Value {
    let Value::Array(mut rules) = rules else {
        panic!("ordered expects a list of rules, got {rules}");
    };
    for (position, rule) in rules.iter_mut().enumerate() {
        rule["Priority"] = json!(position);
    }
    Value::Array(rules)
}

/// A blob member — `ByteMatchStatement$SearchString` is one — travels base64-encoded
/// in an awsJson body. One copy for every suite that writes such a statement.
pub(crate) fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// A `SqliMatchStatement` over the query string.
pub(crate) fn sqli_statement() -> Value {
    json!({
        "SqliMatchStatement": {
            "FieldToMatch": { "QueryString": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    })
}

/// An `XssMatchStatement` over the query string.
pub(crate) fn xss_statement() -> Value {
    json!({
        "XssMatchStatement": {
            "FieldToMatch": { "QueryString": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    })
}

/// Create an IPSet in `scope` and answer `(id, arn, lock token)`.
pub(crate) async fn create_ip_set(
    waf: &Waf,
    scope: &str,
    name: &str,
) -> (String, String, String) {
    let created = waf
        .ok(
            "CreateIPSet",
            json!({
                "Name": name,
                "Scope": scope,
                "IPAddressVersion": "IPV4",
                "Addresses": ["10.0.0.0/8"],
                "Description": "boot ip set",
            }),
        )
        .await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// Create a WebACL in `scope` with `rules` and answer `(id, arn, lock token)`.
pub(crate) async fn create_web_acl(
    waf: &Waf,
    scope: &str,
    name: &str,
    rules: Value,
) -> (String, String, String) {
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": scope,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name),
                "Rules": rules,
            }),
        )
        .await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// `IPSet$Addresses` requires CIDR notation and `IPAddressVersion` fixes the
/// family: "All addresses must be specified using Classless Inter-Domain Routing
/// (CIDR) notation. WAF supports all IPv4 and IPv6 CIDR ranges except for /0".
///
/// One row per way an address can differ — family, each published prefix-length
/// bound, one past each bound, the wrong family, and a string that is not CIDR at
/// all. An accepted row must read back in the notation its family is stored in (an
/// IPv4 address as submitted, an IPv6 one canonicalized — see
/// `crate::ip_set_canonicalization`); a refused one must store nothing.
#[tokio::test]
async fn create_ip_set_accepts_cidr_of_the_declared_family_and_refuses_everything_else() {
    // `(family, submitted address, the notation a read reports, accepted)`.
    let cases: &[(&str, &str, &str, bool)] = &[
        ("IPV4", "10.0.0.0/8", "10.0.0.0/8", true),
        ("IPV4", "192.0.2.44/32", "192.0.2.44/32", true),
        ("IPV4", "0.0.0.0/1", "0.0.0.0/1", true),
        ("IPV4", "192.0.2.0/0", "", false),
        ("IPV4", "192.0.2.0/33", "", false),
        ("IPV4", "192.0.2.44", "", false),
        ("IPV4", "1111::/64", "", false),
        ("IPV4", "not-an-address/24", "", false),
        (
            "IPV6",
            "1111:0000:0000:0000:0000:0000:0000:0111/128",
            "1111:0000:0000:0000:0000:0000:0000:0111/128",
            true,
        ),
        (
            "IPV6",
            "1111::/64",
            "1111:0000:0000:0000:0000:0000:0000:0000/64",
            true,
        ),
        (
            "IPV6",
            "::/1",
            "0000:0000:0000:0000:0000:0000:0000:0000/1",
            true,
        ),
        ("IPV6", "1111::/0", "", false),
        ("IPV6", "1111::/129", "", false),
        ("IPV6", "192.0.2.0/24", "", false),
    ];

    let waf = Waf::new();
    for (index, (version, address, stored, accepted)) in cases.iter().enumerate() {
        let name = format!("cidr-case-{index}");
        let case = format!("{version} {address:?}");
        let (status, body) = waf
            .call(
                "CreateIPSet",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "IPAddressVersion": version,
                    "Addresses": [address],
                }),
            )
            .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got status {status} and body {body}"
            );
            let id = body["Summary"]["Id"].as_str().unwrap_or_default().to_string();
            let read_back = waf
                .ok(
                    "GetIPSet",
                    json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
                )
                .await;
            assert_eq!(
                read_back["IPSet"]["Addresses"],
                json!([stored]),
                "case {case:?}: GetIPSet must answer the stored addresses, got {read_back}"
            );
            assert_eq!(
                read_back["IPSet"]["IPAddressVersion"],
                json!(version),
                "case {case:?}: GetIPSet must answer the stored version, got {read_back}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got status {status} and body {body}"
            );
            assert_error_code(&body, "WAFInvalidParameterException", &case);
            // A refused create stores nothing, so the name it asked for is still
            // free: the same name with a valid address is a create, not a
            // WAFDuplicateItemException.
            let (retry_status, retry_body) = waf
                .call(
                    "CreateIPSet",
                    json!({
                        "Name": name,
                        "Scope": "REGIONAL",
                        "IPAddressVersion": "IPV4",
                        "Addresses": ["10.0.0.0/8"],
                    }),
                )
                .await;
            assert_eq!(
                retry_status, 200,
                "case {case:?}: a refused CreateIPSet must store nothing, so the name \
                 stays free — got status {retry_status} and body {retry_body}"
            );
            // And what it stores is the retry's address, not the refused one.
            let retry_id = retry_body["Summary"]["Id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let read_back = waf
                .ok(
                    "GetIPSet",
                    json!({ "Name": name, "Scope": "REGIONAL", "Id": retry_id }),
                )
                .await;
            assert_eq!(
                read_back["IPSet"]["Addresses"],
                json!(["10.0.0.0/8"]),
                "case {case:?}: the refused address must not be stored, got {read_back}"
            );
        }
    }
}

/// The lifecycle the issue describes: create, read back, page, update under the
/// optimistic lock, and delete.
#[tokio::test]
async fn web_acl_round_trips_through_get_update_and_delete() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _ip_set_lock) = create_ip_set(&waf, "REGIONAL", "boot-ips").await;

    let rules = json!([rule(
        "block-listed-ips",
        json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } })
    )]);
    let (acl_id, acl_arn, create_lock) =
        create_web_acl(&waf, "REGIONAL", "boot-acl", rules.clone()).await;

    assert!(
        is_model_uuid(&acl_id),
        "CreateWebACL must mint a UUID Id, got {acl_id:?}"
    );
    assert!(
        is_model_uuid(&create_lock),
        "CreateWebACL must mint a UUID LockToken, got {create_lock:?}"
    );

    // GetWebACL answers exactly what was stored, with the current lock token.
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "boot-acl", "Scope": "REGIONAL", "Id": acl_id }),
        )
        .await;
    assert_eq!(
        got["WebACL"]["Rules"], rules,
        "GetWebACL must answer the rules CreateWebACL stored, got {got}"
    );
    assert_eq!(
        got["WebACL"]["ARN"],
        json!(acl_arn),
        "GetWebACL must answer the stored ARN, got {got}"
    );
    assert_eq!(
        got["WebACL"]["DefaultAction"],
        json!({ "Allow": {} }),
        "GetWebACL must answer the stored DefaultAction, got {got}"
    );
    assert_eq!(
        got["WebACL"]["LabelNamespace"],
        json!(format!(
            "awswaf:{}:webacl:boot-acl:",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "GetWebACL must answer the documented label namespace prefix, got {got}"
    );
    assert_eq!(
        got["LockToken"],
        json!(create_lock),
        "GetWebACL must answer the current lock token, got {got}"
    );

    // The same web ACL is reachable by ARN alone.
    let by_arn = waf.ok("GetWebACL", json!({ "ARN": acl_arn })).await;
    assert_eq!(
        by_arn["WebACL"], got["WebACL"],
        "GetWebACL by ARN must answer the same web ACL, got {by_arn}"
    );

    // Naming neither form is a parameter error, not a 404 and not a 501.
    waf.refused(
        "GetWebACL",
        json!({ "Name": "boot-acl" }),
        "WAFInvalidParameterException",
        "GetWebACL with only a Name",
    )
    .await;

    // A stale lock token refuses the update and changes nothing.
    let stale_lock = "00000000-0000-4000-8000-000000000000";
    let update_body = |lock: &str, description: &str, rules: Value| {
        json!({
            "Name": "boot-acl",
            "Scope": "REGIONAL",
            "Id": acl_id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("boot-acl"),
            "LockToken": lock,
            "Description": description,
            "Rules": rules,
        })
    };
    let updated_rules = json!([rule("block-sqli", sqli_statement())]);
    waf.refused(
        "UpdateWebACL",
        update_body(stale_lock, "should-not-apply", updated_rules.clone()),
        "WAFOptimisticLockException",
        "UpdateWebACL with a stale LockToken",
    )
    .await;
    let unchanged = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "boot-acl", "Scope": "REGIONAL", "Id": acl_id }),
        )
        .await;
    assert_eq!(
        unchanged["WebACL"], got["WebACL"],
        "a refused UpdateWebACL must leave the stored web ACL untouched, got {unchanged}"
    );

    // The current token applies the update and rotates to a fresh one.
    let update = waf
        .ok(
            "UpdateWebACL",
            update_body(&create_lock, "second-generation", updated_rules.clone()),
        )
        .await;
    let next_lock = update["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    assert!(
        is_model_uuid(&next_lock),
        "UpdateWebACL must answer a UUID NextLockToken, got {next_lock:?}"
    );
    assert_ne!(
        next_lock, create_lock,
        "UpdateWebACL must rotate the lock token, got {next_lock:?} again"
    );

    let after_update = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "boot-acl", "Scope": "REGIONAL", "Id": acl_id }),
        )
        .await;
    assert_eq!(
        after_update["WebACL"]["Rules"], updated_rules,
        "GetWebACL must answer the updated rules, got {after_update}"
    );
    assert_eq!(
        after_update["WebACL"]["Description"],
        json!("second-generation"),
        "GetWebACL must answer the updated description, got {after_update}"
    );
    assert_eq!(
        after_update["LockToken"],
        json!(next_lock),
        "GetWebACL must answer the rotated lock token, got {after_update}"
    );

    // The token the update consumed no longer works for anything.
    waf.refused(
        "UpdateWebACL",
        update_body(&create_lock, "third-generation", updated_rules.clone()),
        "WAFOptimisticLockException",
        "UpdateWebACL replayed with the consumed LockToken",
    )
    .await;
    waf.refused(
        "DeleteWebACL",
        json!({
            "Name": "boot-acl",
            "Scope": "REGIONAL",
            "Id": acl_id,
            "LockToken": create_lock,
        }),
        "WAFOptimisticLockException",
        "DeleteWebACL with the consumed LockToken",
    )
    .await;

    // Deleting with the current token removes it from Get and from List.
    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": "boot-acl",
            "Scope": "REGIONAL",
            "Id": acl_id,
            "LockToken": next_lock,
        }),
    )
    .await;
    waf.refused(
        "GetWebACL",
        json!({ "Name": "boot-acl", "Scope": "REGIONAL", "Id": acl_id }),
        "WAFNonexistentItemException",
        "GetWebACL after DeleteWebACL",
    )
    .await;
    waf.refused(
        "GetWebACL",
        json!({ "ARN": acl_arn }),
        "WAFNonexistentItemException",
        "GetWebACL by ARN after DeleteWebACL",
    )
    .await;
    let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
    assert_eq!(
        listed["WebACLs"],
        json!([]),
        "ListWebACLs must not show a deleted web ACL, got {listed}"
    );
}

/// `Scope` is a namespace: the same name in CLOUDFRONT and in REGIONAL are two
/// resources, a repeated name within one scope is a duplicate, and the scope
/// decides the ARN's region and resource prefix — a CLOUDFRONT resource is reached
/// through one endpoint ("API and SDKs - For all calls, use the Region endpoint
/// us-east-1", the `Scope` member doc).
#[tokio::test]
async fn scopes_are_independent_namespaces_with_their_own_arn_form() {
    let cases: &[(&str, &str, &str)] = &[
        ("REGIONAL", TEST_REGION, "regional"),
        ("CLOUDFRONT", "us-east-1", "global"),
    ];

    let waf = Waf::new();
    let mut ids = BTreeSet::new();
    for (scope, arn_region, arn_prefix) in cases {
        let (ip_set_id, ip_set_arn, _lock) = create_ip_set(&waf, scope, "shared-name").await;
        let (acl_id, acl_arn, _acl_lock) = create_web_acl(
            &waf,
            scope,
            "shared-name",
            json!([rule(
                "block-listed-ips",
                json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } })
            )]),
        )
        .await;

        let account = engine::EMULATOR_ACCOUNT_ID;
        assert_eq!(
            ip_set_arn,
            format!("arn:aws:wafv2:{arn_region}:{account}:{arn_prefix}/ipset/shared-name/{ip_set_id}"),
            "case {scope}: the IPSet ARN must carry the scope's region and resource prefix"
        );
        assert_eq!(
            acl_arn,
            format!("arn:aws:wafv2:{arn_region}:{account}:{arn_prefix}/webacl/shared-name/{acl_id}"),
            "case {scope}: the WebACL ARN must carry the scope's region and resource prefix"
        );
        assert!(
            ids.insert(acl_id.clone()),
            "case {scope}: every scope's web ACL needs its own id, saw {acl_id:?} twice"
        );

        // A second create of the same name in the SAME scope is a duplicate.
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": "shared-name",
                    "Scope": scope,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility("shared-name"),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {scope}: a duplicate CreateWebACL must answer 400, got {status} and {body}"
        );
        assert_error_code(&body, "WAFDuplicateItemException", scope);
    }

    // Each scope lists only its own web ACL.
    for (scope, _region, _prefix) in cases {
        let listed = waf.ok("ListWebACLs", json!({ "Scope": scope })).await;
        let names: Vec<&Value> = listed["WebACLs"]
            .as_array()
            .expect("WebACLs")
            .iter()
            .map(|summary| &summary["Name"])
            .collect();
        assert_eq!(
            names,
            vec![&json!("shared-name")],
            "case {scope}: ListWebACLs must enumerate only this scope, got {listed}"
        );
    }
}

/// `ListWebACLs` pages: at most `Limit` summaries per call, resumed with the
/// `NextMarker` of the previous page. WAF answers every non-empty page with a
/// `NextMarker` — the name of its last summary, whether or not the page is full —
/// so the paginator's loop ends on the empty page that follows the last full one.
/// One row per page size, including a page size equal to and larger than the whole
/// collection.
#[tokio::test]
async fn list_web_acls_pages_by_limit_and_next_marker() {
    let waf = Waf::new();
    let all_names: Vec<String> = (1..=5).map(|index| format!("acl-{index}")).collect();
    for name in &all_names {
        create_web_acl(&waf, "REGIONAL", name, json!([])).await;
    }

    for limit in [1_usize, 2, 3, 5, 8] {
        let case = format!("Limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_names.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({ "Scope": "REGIONAL", "Limit": limit });
            if let Some(marker) = marker.as_deref() {
                request["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListWebACLs", request).await;
            let summaries = page["WebACLs"].as_array().expect("WebACLs").clone();
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                summaries.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) summaries, \
                 got {page}"
            );
            for summary in &summaries {
                let name = summary["Name"].as_str().expect("Name").to_string();
                assert!(
                    is_model_uuid(summary["Id"].as_str().unwrap_or_default()),
                    "case {case:?}: every summary carries a UUID Id, got {summary}"
                );
                seen.push(name);
            }
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                summaries
                    .last()
                    .map(|summary| summary["Name"].as_str().expect("Name")),
                "case {case:?}: page {pages} must carry the last summary's name as its \
                 NextMarker, got {page}"
            );
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every web ACL exactly once, in order"
        );
        assert_eq!(
            pages,
            all_names.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil(5 / limit) full pages plus the empty page \
             that ends the loop, got {pages}"
        );
    }

    // No `Limit` at all: one page with everything on it — and still a marker.
    let page = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
    let names: Vec<String> = page["WebACLs"]
        .as_array()
        .expect("WebACLs")
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names, all_names,
        "an unlimited ListWebACLs must answer every web ACL, got {page}"
    );
    assert_eq!(
        page["NextMarker"],
        json!(all_names.last().expect("a last name")),
        "an unlimited ListWebACLs must still carry its last summary's name as \
         NextMarker, got {page}"
    );
}

/// One way of nesting a statement inside a rule's statement tree: the case name
/// and the wrapper that places the given statement there.
type Nesting = (&'static str, fn(Value) -> Value);

/// An IPSet a live web ACL references cannot be deleted, wherever in the rule tree
/// the `IPSetReferenceStatement` sits: at the top level or nested inside a logical
/// or rate-based statement. Once the web ACL is gone the IP set deletes.
#[tokio::test]
async fn delete_ip_set_refuses_while_any_rule_tree_references_it() {
    let nestings: &[Nesting] = &[
        ("top level", |reference| reference),
        ("inside AndStatement", |reference| {
            json!({ "AndStatement": { "Statements": [reference, sqli_statement()] } })
        }),
        ("inside OrStatement", |reference| {
            json!({ "OrStatement": { "Statements": [sqli_statement(), reference] } })
        }),
        ("inside NotStatement", |reference| {
            json!({ "NotStatement": { "Statement": reference } })
        }),
        ("inside a rate-based scope-down", |reference| {
            json!({
                "RateBasedStatement": {
                    "Limit": 1000,
                    "AggregateKeyType": "IP",
                    "ScopeDownStatement": reference,
                }
            })
        }),
    ];

    for (case, nest) in nestings {
        let waf = Waf::new();
        let (ip_set_id, ip_set_arn, ip_set_lock) =
            create_ip_set(&waf, "REGIONAL", "referenced-ips").await;
        let statement = nest(json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } }));
        let (acl_id, _acl_arn, acl_lock) =
            create_web_acl(&waf, "REGIONAL", "referring-acl", json!([rule("r", statement)])).await;

        waf.refused(
            "DeleteIPSet",
            json!({
                "Name": "referenced-ips",
                "Scope": "REGIONAL",
                "Id": ip_set_id,
                "LockToken": ip_set_lock,
            }),
            "WAFAssociatedItemException",
            case,
        )
        .await;

        // A stale token is refused for the lock, not the association.
        waf.refused(
            "DeleteIPSet",
            json!({
                "Name": "referenced-ips",
                "Scope": "REGIONAL",
                "Id": ip_set_id,
                "LockToken": "00000000-0000-4000-8000-000000000000",
            }),
            "WAFOptimisticLockException",
            case,
        )
        .await;

        waf.ok(
            "DeleteWebACL",
            json!({
                "Name": "referring-acl",
                "Scope": "REGIONAL",
                "Id": acl_id,
                "LockToken": acl_lock,
            }),
        )
        .await;
        waf.ok(
            "DeleteIPSet",
            json!({
                "Name": "referenced-ips",
                "Scope": "REGIONAL",
                "Id": ip_set_id,
                "LockToken": ip_set_lock,
            }),
        )
        .await;
        waf.refused(
            "GetIPSet",
            json!({ "Name": "referenced-ips", "Scope": "REGIONAL", "Id": ip_set_id }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }
}

/// `WebACL$Capacity` is "the web ACL capacity units (WCUs) currently being used by
/// this web ACL", so it is derived from the stored rules: each statement's own WCU
/// cost with the modifiers it carries, summed, with a logical statement costing what
/// it nests. One row per statement kind and per nesting form. Three rows are the
/// costs the recorded corpus pins: a web ACL of three `CONTAINS` byte-match rules
/// reports 30 (`state::BYTE_MATCH_SCANNING_WCU`), one holding a bare `IP`-aggregated
/// rate-based rule reports 2 (`state::RATE_BASED_WCU`), and the same rule
/// aggregating on one custom key reports 32 (plus
/// `state::RATE_BASED_CUSTOM_KEY_WCU`, charged per key). The whole cost model — every
/// base cost and every modifier — is spanned by `crate::wcu_cost_model`.
#[tokio::test]
async fn web_acl_capacity_is_derived_from_the_stored_rules() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _lock) = create_ip_set(&waf, "REGIONAL", "capacity-ips").await;
    let ip_set_reference = json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } });
    let byte_match = json!({
        "ByteMatchStatement": {
            "SearchString": "YQ==",
            "FieldToMatch": { "UriPath": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
            "PositionalConstraint": "CONTAINS",
        }
    });

    let cases: Vec<(&str, Value, i64)> = vec![
        ("no rules", json!([]), 0),
        (
            "one IP set reference",
            json!([rule("a", ip_set_reference.clone())]),
            1,
        ),
        ("one byte match", json!([rule("a", byte_match.clone())]), 10),
        (
            "three byte matches",
            ordered(json!([
                rule("a", byte_match.clone()),
                rule("b", byte_match.clone()),
                rule("c", byte_match.clone()),
            ])),
            30,
        ),
        (
            "one bare rate-based statement",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 2000,
                        "AggregateKeyType": "IP",
                    }
                })
            )]),
            2,
        ),
        (
            "a rate-based statement aggregating on one custom key",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 2000,
                        "AggregateKeyType": "CUSTOM_KEYS",
                        "CustomKeys": [{ "HTTPMethod": {} }],
                    }
                })
            )]),
            32,
        ),
        (
            "a rate-based statement aggregating on three custom keys",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 2000,
                        "AggregateKeyType": "CUSTOM_KEYS",
                        "CustomKeys": [
                            { "HTTPMethod": {} },
                            { "QueryString": { "TextTransformations": [{ "Priority": 0, "Type": "NONE" }] } },
                            { "UriPath": { "TextTransformations": [{ "Priority": 0, "Type": "NONE" }] } },
                        ],
                    }
                })
            )]),
            // Every aggregation key is extracted and hashed per request, so each
            // one is charged.
            92,
        ),
        ("one SQLi match", json!([sqli_rule("a")]), 20),
        ("one XSS match", json!([rule("a", xss_statement())]), 40),
        (
            "two rules",
            ordered(json!([rule("a", ip_set_reference.clone()), sqli_rule("b")])),
            21,
        ),
        (
            "an AndStatement of two cheap statements",
            json!([rule(
                "a",
                json!({
                    "AndStatement": {
                        "Statements": [ip_set_reference.clone(), ip_set_reference.clone()]
                    }
                })
            )]),
            2,
        ),
        (
            "an AndStatement of two SQLi matches",
            json!([rule(
                "a",
                json!({
                    "AndStatement": {
                        "Statements": [sqli_statement(), sqli_statement()]
                    }
                })
            )]),
            40,
        ),
        (
            "a NotStatement",
            json!([rule(
                "a",
                json!({ "NotStatement": { "Statement": sqli_statement() } })
            )]),
            20,
        ),
        (
            "a rate-based statement with a cheap scope-down",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 1000,
                        "AggregateKeyType": "IP",
                        "ScopeDownStatement": ip_set_reference.clone(),
                    }
                })
            )]),
            3,
        ),
        (
            "a rate-based statement with an XSS scope-down",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 1000,
                        "AggregateKeyType": "IP",
                        "ScopeDownStatement": xss_statement(),
                    }
                })
            )]),
            42,
        ),
        (
            "custom keys and a scope-down together",
            json!([rule(
                "a",
                json!({
                    "RateBasedStatement": {
                        "Limit": 1000,
                        "AggregateKeyType": "CUSTOM_KEYS",
                        "CustomKeys": [{ "HTTPMethod": {} }],
                        "ScopeDownStatement": ip_set_reference.clone(),
                    }
                })
            )]),
            33,
        ),
    ];

    for (index, (case, rules, expected)) in cases.into_iter().enumerate() {
        let name = format!("capacity-acl-{index}");
        let (acl_id, _arn, _lock) = create_web_acl(&waf, "REGIONAL", &name, rules).await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(expected),
            "case {case:?}: expected Capacity {expected}, got {got}"
        );
    }
}

/// A SQLi rule named `name`, for the capacity table.
fn sqli_rule(name: &str) -> Value {
    rule(name, sqli_statement())
}

/// Every id and lock token this service mints is fresh: no two resources share
/// one, and each has the form the model pins.
#[tokio::test]
async fn ids_and_lock_tokens_are_fresh_and_modelled() {
    let waf = Waf::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for index in 0..4 {
        let (ip_set_id, _arn, ip_set_lock) =
            create_ip_set(&waf, "REGIONAL", &format!("ips-{index}")).await;
        let (acl_id, _acl_arn, acl_lock) =
            create_web_acl(&waf, "REGIONAL", &format!("acl-{index}"), json!([])).await;
        for value in [ip_set_id, ip_set_lock, acl_id, acl_lock] {
            assert!(
                is_model_uuid(&value),
                "every minted id and lock token is a UUID, got {value:?}"
            );
            assert!(
                minted.insert(value.clone()),
                "every minted id and lock token is fresh, saw {value:?} twice"
            );
        }
    }

    assert_eq!(
        minted.len(),
        16,
        "4 IP sets and 4 web ACLs mint 16 distinct values, got {}",
        minted.len()
    );
}

/// A statement nesting `depth` levels deep: `depth - 1` `NotStatement`s wrapped
/// around `innermost`.
pub(crate) fn nested_statement(depth: u32, innermost: Value) -> Value {
    let mut statement = innermost;
    for _ in 1..depth {
        statement = json!({ "NotStatement": { "Statement": statement } });
    }
    statement
}

/// Rule statements nest, and the tree arrives verbatim from the request, so the
/// nesting depth is bounded on ingress: at the bound a tree is stored, one level
/// past it the request is refused with the modelled parameter error and nothing is
/// stored or measured. One row at each published bound and one past it.
#[tokio::test]
async fn rule_statement_nesting_is_bounded_on_ingress() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _lock) = create_ip_set(&waf, "REGIONAL", "nesting-ips").await;
    let reference = json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } });

    let cases: &[(u32, bool)] = &[
        (1, true),
        (MAX_STATEMENT_NESTING_DEPTH / 2, true),
        (MAX_STATEMENT_NESTING_DEPTH, true),
        (MAX_STATEMENT_NESTING_DEPTH + 1, false),
        (MAX_STATEMENT_NESTING_DEPTH * 3, false),
    ];

    for (depth, accepted) in cases {
        let name = format!("nesting-{depth}");
        let case = format!("depth {depth}");
        let rules = json!([rule("r", nested_statement(*depth, reference.clone()))]);
        let (status, body) = waf
            .call(
                "CreateWebACL",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                    "Rules": rules,
                }),
            )
            .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a tree within the bound must be stored, got status \
                 {status} and body {body}"
            );
            let id = body["Summary"]["Id"].as_str().unwrap_or_default().to_string();
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Rules"], rules,
                "case {case:?}: the stored tree must round-trip, got {got}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a tree past the bound must be refused, got status \
                 {status} and body {body}"
            );
            assert_error_code(&body, "WAFInvalidParameterException", &case);
            let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
            let names: Vec<&str> = listed["WebACLs"]
                .as_array()
                .expect("WebACLs")
                .iter()
                .filter_map(|summary| summary["Name"].as_str())
                .collect();
            assert!(
                !names.contains(&name.as_str()),
                "case {case:?}: a refused CreateWebACL must store nothing, got {listed}"
            );
        }
    }

    // UpdateWebACL applies the same bound, and refusing leaves the stored web ACL
    // alone.
    let (acl_id, _arn, acl_lock) = create_web_acl(&waf, "REGIONAL", "update-nesting", json!([])).await;
    waf.refused(
        "UpdateWebACL",
        json!({
            "Name": "update-nesting",
            "Scope": "REGIONAL",
            "Id": acl_id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("update-nesting"),
            "LockToken": acl_lock,
            "Rules": [rule(
                "r",
                nested_statement(MAX_STATEMENT_NESTING_DEPTH + 1, reference.clone())
            )],
        }),
        "WAFInvalidParameterException",
        "UpdateWebACL past the nesting bound",
    )
    .await;
    let unchanged = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "update-nesting", "Scope": "REGIONAL", "Id": acl_id }),
        )
        .await;
    assert_eq!(
        unchanged["WebACL"]["Rules"],
        json!([]),
        "a refused UpdateWebACL must leave the stored rules alone, got {unchanged}"
    );
}

/// The per-scope resource quotas WAF publishes are enforced, so the store cannot
/// grow without limit: the last resource inside the quota is created, the next one
/// is refused with the modelled limits error, and the other scope still has its own
/// room (the quota is per scope).
#[tokio::test]
async fn per_scope_resource_quotas_are_enforced() {
    let waf = Waf::new();

    for index in 0..MAX_WEB_ACLS_PER_SCOPE {
        create_web_acl(&waf, "REGIONAL", &format!("quota-acl-{index}"), json!([])).await;
    }
    waf.refused(
        "CreateWebACL",
        json!({
            "Name": "quota-acl-overflow",
            "Scope": "REGIONAL",
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("quota-acl-overflow"),
        }),
        "WAFLimitsExceededException",
        "CreateWebACL one past the per-scope quota",
    )
    .await;
    // The quota is per scope, so the other namespace is unaffected.
    create_web_acl(&waf, "CLOUDFRONT", "quota-acl-overflow", json!([])).await;

    for index in 0..MAX_IP_SETS_PER_SCOPE {
        create_ip_set(&waf, "REGIONAL", &format!("quota-ips-{index}")).await;
    }
    waf.refused(
        "CreateIPSet",
        json!({
            "Name": "quota-ips-overflow",
            "Scope": "REGIONAL",
            "IPAddressVersion": "IPV4",
            "Addresses": ["10.0.0.0/8"],
        }),
        "WAFLimitsExceededException",
        "CreateIPSet one past the per-scope quota",
    )
    .await;
    create_ip_set(&waf, "CLOUDFRONT", "quota-ips-overflow").await;
}

/// WAF publishes a maximum number of addresses per IP set, so one stored entry is
/// bounded too: a set holding exactly the maximum is stored and reads back, one
/// address past it is refused with the modelled limits error.
#[tokio::test]
async fn ip_set_address_count_is_bounded() {
    let waf = Waf::new();
    let addresses: Vec<String> = (0..=MAX_IP_SET_ADDRESSES)
        .map(|index| {
            format!(
                "10.{}.{}.{}/32",
                (index / 65536) % 256,
                (index / 256) % 256,
                index % 256
            )
        })
        .collect();

    let at_the_bound = &addresses[..MAX_IP_SET_ADDRESSES];
    let created = waf
        .ok(
            "CreateIPSet",
            json!({
                "Name": "bounded-ips",
                "Scope": "REGIONAL",
                "IPAddressVersion": "IPV4",
                "Addresses": at_the_bound,
            }),
        )
        .await;
    let id = created["Summary"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let got = waf
        .ok(
            "GetIPSet",
            json!({ "Name": "bounded-ips", "Scope": "REGIONAL", "Id": id }),
        )
        .await;
    assert_eq!(
        got["IPSet"]["Addresses"].as_array().map(Vec::len),
        Some(MAX_IP_SET_ADDRESSES),
        "a set at the address bound must read back whole, got {} addresses",
        got["IPSet"]["Addresses"]
            .as_array()
            .map_or(0, Vec::len)
    );

    waf.refused(
        "CreateIPSet",
        json!({
            "Name": "overfull-ips",
            "Scope": "REGIONAL",
            "IPAddressVersion": "IPV4",
            "Addresses": addresses,
        }),
        "WAFLimitsExceededException",
        "CreateIPSet one address past the bound",
    )
    .await;
}

/// `GetWebACL` by ARN separates the two ARN failures the model names: an ARN that
/// is malformed is a parameter error ("Your request references an ARN that is
/// malformed"), while a well-formed ARN naming nothing stored — including one
/// naming a resource of another type — is a nonexistent item. One row per way an
/// ARN can be wrong.
#[tokio::test]
async fn get_web_acl_by_arn_separates_malformed_from_unknown() {
    let waf = Waf::new();
    let (_ip_set_id, ip_set_arn, _lock) = create_ip_set(&waf, "REGIONAL", "arn-ips").await;
    let (_acl_id, acl_arn, _acl_lock) = create_web_acl(&waf, "REGIONAL", "arn-acl", json!([])).await;

    let found = waf.ok("GetWebACL", json!({ "ARN": acl_arn })).await;
    assert_eq!(
        found["WebACL"]["Name"],
        json!("arn-acl"),
        "a stored web ACL's own ARN must resolve to it, got {found}"
    );

    // Well formed, but naming nothing this service holds as a web ACL.
    let unknown: &[(&str, &str)] = &[
        ("the ARN of an IP set", ip_set_arn.as_str()),
        (
            "a web ACL ARN that was never created",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/absent/\
             11111111-2222-4333-8444-555555555555",
        ),
    ];
    for (case, arn) in unknown {
        waf.refused(
            "GetWebACL",
            json!({ "ARN": arn }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // Malformed: not an ARN, the wrong service, an unknown scope segment, or too
    // few resource segments. Each is at least the modelled 20 characters long, so
    // the boundary's length constraint is not what refuses it.
    let malformed: &[(&str, &str)] = &[
        ("not an ARN at all", "webacl/absent/1111111111111111"),
        (
            "another service's ARN",
            "arn:aws:s3:::a-bucket-named-like-this",
        ),
        (
            "an unknown scope segment",
            "arn:aws:wafv2:us-west-2:000000000000:worldwide/webacl/absent/1111",
        ),
        (
            "too few resource segments",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl/absent",
        ),
        (
            "an empty resource segment",
            "arn:aws:wafv2:us-west-2:000000000000:regional/webacl//1111",
        ),
    ];
    for (case, arn) in malformed {
        waf.refused(
            "GetWebACL",
            json!({ "ARN": arn }),
            "WAFInvalidParameterException",
            case,
        )
        .await;
    }
}

/// A `Limit` outside the model's `PaginationLimit` range is refused, not clamped:
/// a caller never receives a silently empty terminal page. One row per bound and
/// one past each.
#[tokio::test]
async fn list_web_acls_refuses_a_limit_outside_the_modelled_range() {
    let waf = Waf::new();
    create_web_acl(&waf, "REGIONAL", "limit-acl", json!([])).await;

    for limit in [1_i64, 100] {
        let page = waf
            .ok("ListWebACLs", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            page["WebACLs"].as_array().map(Vec::len),
            Some(1),
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call("ListWebACLs", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        assert!(
            body["WebACLs"].is_null(),
            "case Limit={limit}: a refusal must carry no page, got {body}"
        );
    }
}

/// A buffer the audit subscriber below writes one line per record into, so a test
/// can read back what the handlers recorded.
#[derive(Clone)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

/// Renders one event's fields as ` name="value"` pairs, the form the assertions
/// below match on.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

/// A `tracing` subscriber that keeps this service's audit records and nothing
/// else. Hand-written on `tracing` itself so the test target needs no formatting
/// subscriber crate.
impl tracing::Subscriber for CapturedLog {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "wafv2::audit"
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // This capture reads events only; spans are not recorded, so every span
        // gets the same non-zero id (`Id` refuses zero).
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(String::new());
        event.record(&mut line);
        line.0.push('\n');
        self.0
            .lock()
            .expect("the capture buffer lock")
            .extend_from_slice(line.0.as_bytes());
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The audit capture for this test binary, installed ONCE as the process-wide
/// default subscriber.
///
/// It has to be global rather than thread-local: `tracing` caches each callsite's
/// interest globally, computed from whatever subscriber the first thread to reach
/// that callsite happened to have — and with the harness running tests in
/// parallel, that may have been none, which caches the callsite as "never". One
/// global capture plus an interest-cache rebuild removes the ordering dependency.
/// Records from the other tests land here too, so the assertions below pin each
/// record by the resource it names.
fn audit_capture() -> &'static CapturedLog {
    static CAPTURE: std::sync::OnceLock<CapturedLog> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let buffer = CapturedLog(Arc::new(Mutex::new(Vec::new())));
        tracing::subscriber::set_global_default(buffer.clone())
            .expect("this test binary installs no other global tracing subscriber");
        tracing::callsite::rebuild_interest_cache();
        buffer
    })
}

/// Every state-changing call leaves an audit record naming the operation, the
/// resource and the outcome — the applied mutations and the refused ones alike.
/// The record carries no part of the caller's credential. One row per
/// state-changing operation and per kind of outcome.
#[tokio::test]
async fn state_changing_calls_leave_an_audit_record() {
    let capture = audit_capture();
    {
        let waf = Waf::new();
        let (ip_set_id, ip_set_arn, ip_set_lock) =
            create_ip_set(&waf, "REGIONAL", "audited-ips").await;
        let (acl_id, _acl_arn, acl_lock) = create_web_acl(
            &waf,
            "REGIONAL",
            "audited-acl",
            json!([rule(
                "block-listed-ips",
                json!({ "IPSetReferenceStatement": { "ARN": ip_set_arn } })
            )]),
        )
        .await;

        // A refused mutation: the IP set is still referenced.
        waf.refused(
            "DeleteIPSet",
            json!({
                "Name": "audited-ips",
                "Scope": "REGIONAL",
                "Id": ip_set_id,
                "LockToken": ip_set_lock,
            }),
            "WAFAssociatedItemException",
            "audited DeleteIPSet",
        )
        .await;

        let updated = waf
            .ok(
                "UpdateWebACL",
                json!({
                    "Name": "audited-acl",
                    "Scope": "REGIONAL",
                    "Id": acl_id,
                    "DefaultAction": { "Block": {} },
                    "VisibilityConfig": visibility("audited-acl"),
                    "LockToken": acl_lock,
                }),
            )
            .await;
        waf.ok(
            "DeleteWebACL",
            json!({
                "Name": "audited-acl",
                "Scope": "REGIONAL",
                "Id": acl_id,
                "LockToken": updated["NextLockToken"],
            }),
        )
        .await;
        waf.ok(
            "DeleteIPSet",
            json!({
                "Name": "audited-ips",
                "Scope": "REGIONAL",
                "Id": ip_set_id,
                "LockToken": ip_set_lock,
            }),
        )
        .await;
    }

    let recorded = String::from_utf8(capture.0.lock().expect("the capture buffer lock").clone())
        .expect("the captured log is UTF-8");

    // (operation, resource, outcome) rows: a create, an update, two deletes and a
    // refusal.
    let expected: &[(&str, &str, &str)] = &[
        ("CreateIPSet", "audited-ips", "created"),
        ("CreateWebACL", "audited-acl", "created"),
        ("UpdateWebACL", "audited-acl", "updated"),
        ("DeleteWebACL", "audited-acl", "deleted"),
        ("DeleteIPSet", "audited-ips", "deleted"),
        ("DeleteIPSet", "audited-ips", "WAFAssociatedItemException"),
    ];
    for (operation, resource, outcome) in expected {
        let record = recorded.lines().find(|line| {
            line.contains(operation) && line.contains(resource) && line.contains(outcome)
        });
        assert!(
            record.is_some(),
            "case {operation} / {resource} / {outcome}: no audit record names all three; \
             captured:\n{recorded}"
        );
        let record = record.unwrap_or_default();
        assert!(
            record.contains("scope=\"REGIONAL\""),
            "case {operation} / {resource} / {outcome}: the record must name the scope, \
             got {record:?}"
        );
    }

    // No part of the caller's credential is ever recorded: neither the signature
    // nor the access key id the request presented.
    assert!(
        !recorded.contains("Signature="),
        "an audit record must never carry signing material; captured:\n{recorded}"
    );
    assert!(
        !recorded.contains("AKIDEXAMPLEWAFV2"),
        "an audit record must never carry the presented access key id; \
         captured:\n{recorded}"
    );
}

/// A rule statement that references another entity is resolved when the rule is
/// SAVED: the reference must name an entity this service holds, in the same scope,
/// or the save is refused with the modelled `WAFNonexistentItemException` and
/// nothing is stored — a dangling reference never reaches a later `GetWebACL`, and
/// a reference whose cost `WebACL$Capacity` cannot be computed from is never
/// priced as free.
///
/// One row per reference-bearing statement the model declares
/// (`smithy_query(query="shape",shape="Statement")` → `IPSetReferenceStatement`,
/// `RuleGroupReferenceStatement`, `RegexPatternSetReferenceStatement`,
/// `ManagedRuleGroupStatement`), plus a reference reached only through a nested
/// statement and one naming an entity that exists in the OTHER scope. Rule groups
/// are covered by [`crate::rule_group_lifecycle`], which references one that
/// EXISTS; the row here is a rule group ARN naming nothing stored. The managed
/// rule groups a save MAY reference are covered by
/// [`crate::managed_rule_groups`]; the rows here name a group the catalogue does
/// not list, and one that it does.
#[tokio::test]
async fn rule_references_must_name_a_stored_entity() {
    let waf = Waf::new();
    let (_ip_set_id, regional_arn, _lock) = create_ip_set(&waf, "REGIONAL", "referenced-ips").await;
    let (_other_id, cloudfront_arn, _other_lock) =
        create_ip_set(&waf, "CLOUDFRONT", "cloudfront-ips").await;
    let account = engine::EMULATOR_ACCOUNT_ID;
    // Well-formed wafv2 ARNs of the form this service mints, naming nothing stored.
    let unknown_ip_set = format!(
        "arn:aws:wafv2:{TEST_REGION}:{account}:regional/ipset/absent-ips/\
         11111111-2222-3333-4444-555555555555"
    );
    let unknown_rule_group = format!(
        "arn:aws:wafv2:{TEST_REGION}:{account}:regional/rulegroup/absent-group/\
         22222222-3333-4444-5555-666666666666"
    );
    let unknown_regex_set = format!(
        "arn:aws:wafv2:{TEST_REGION}:{account}:regional/regexpatternset/absent-set/\
         33333333-4444-5555-6666-777777777777"
    );

    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "an IPSet stored in this scope",
            json!({ "IPSetReferenceStatement": { "ARN": &regional_arn } }),
            true,
        ),
        (
            "an IPSet ARN naming nothing",
            json!({ "IPSetReferenceStatement": { "ARN": &unknown_ip_set } }),
            false,
        ),
        (
            "an IPSet stored in the other scope",
            json!({ "IPSetReferenceStatement": { "ARN": &cloudfront_arn } }),
            false,
        ),
        (
            "an unknown IPSet reached only through an AndStatement",
            json!({
                "AndStatement": {
                    "Statements": [
                        { "IPSetReferenceStatement": { "ARN": &regional_arn } },
                        { "IPSetReferenceStatement": { "ARN": &unknown_ip_set } },
                    ]
                }
            }),
            false,
        ),
        (
            "a RuleGroupReferenceStatement naming nothing stored",
            json!({ "RuleGroupReferenceStatement": { "ARN": &unknown_rule_group } }),
            false,
        ),
        (
            "a RegexPatternSetReferenceStatement, with no pattern set store",
            json!({
                "RegexPatternSetReferenceStatement": {
                    "ARN": &unknown_regex_set,
                    "FieldToMatch": { "QueryString": {} },
                    "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                }
            }),
            false,
        ),
        (
            "a ManagedRuleGroupStatement naming a group no vendor publishes",
            json!({
                "ManagedRuleGroupStatement": {
                    "VendorName": "AWS",
                    "Name": "AWSManagedRulesNoSuchRuleSet",
                }
            }),
            false,
        ),
        (
            "a ManagedRuleGroupStatement naming a group of an unknown vendor",
            json!({
                "ManagedRuleGroupStatement": {
                    "VendorName": "NoSuchVendor",
                    "Name": "AWSManagedRulesCommonRuleSet",
                }
            }),
            false,
        ),
        (
            "a ManagedRuleGroupStatement naming a catalogued group",
            json!({
                "ManagedRuleGroupStatement": {
                    "VendorName": "AWS",
                    "Name": "AWSManagedRulesCommonRuleSet",
                }
            }),
            true,
        ),
    ];

    for (index, (case, statement, accepted)) in cases.into_iter().enumerate() {
        let name = format!("reference-acl-{index}");
        let request = json!({
            "Name": &name,
            "Scope": "REGIONAL",
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(&name),
            "Rules": [rule("reference", statement.clone())],
        });
        if accepted {
            let created = waf.ok("CreateWebACL", request).await;
            let acl_id = created["Summary"]["Id"]
                .as_str()
                .expect("Summary.Id")
                .to_string();
            let got = waf
                .ok(
                    "GetWebACL",
                    json!({ "Name": &name, "Scope": "REGIONAL", "Id": acl_id }),
                )
                .await;
            assert_eq!(
                got["WebACL"]["Rules"][0]["Statement"], statement,
                "case {case:?}: the stored rule must read back verbatim, got {got}"
            );
        } else {
            waf.refused(
                "CreateWebACL",
                request,
                "WAFNonexistentItemException",
                case,
            )
            .await;
            // A refused create stores nothing, so the name is still absent.
            let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
            assert!(
                !listed.to_string().contains(name.as_str()),
                "case {case:?}: a refused CreateWebACL must store nothing, got {listed}"
            );
        }
    }

    // An update carrying a dangling reference is refused too, and leaves the stored
    // web ACL and its lock token exactly as they were.
    let (acl_id, _acl_arn, acl_lock) = create_web_acl(
        &waf,
        "REGIONAL",
        "reference-update",
        json!([rule(
            "keep",
            json!({ "IPSetReferenceStatement": { "ARN": &regional_arn } })
        )]),
    )
    .await;
    waf.refused(
        "UpdateWebACL",
        json!({
            "Name": "reference-update",
            "Scope": "REGIONAL",
            "Id": &acl_id,
            "DefaultAction": { "Block": {} },
            "VisibilityConfig": visibility("reference-update"),
            "LockToken": &acl_lock,
            "Rules": [rule(
                "dangling",
                json!({ "IPSetReferenceStatement": { "ARN": &unknown_ip_set } })
            )],
        }),
        "WAFNonexistentItemException",
        "UpdateWebACL with a dangling reference",
    )
    .await;
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "reference-update", "Scope": "REGIONAL", "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        got["LockToken"],
        json!(&acl_lock),
        "case {:?}: a refused update must not rotate the lock token, got {got}",
        "UpdateWebACL with a dangling reference"
    );
    assert_eq!(
        got["WebACL"]["Rules"][0]["Name"],
        json!("keep"),
        "case {:?}: a refused update must not replace the stored rules, got {got}",
        "UpdateWebACL with a dangling reference"
    );
    assert_eq!(
        got["WebACL"]["DefaultAction"],
        json!({ "Allow": {} }),
        "case {:?}: a refused update must not replace the stored default action, \
         got {got}",
        "UpdateWebACL with a dangling reference"
    );
}
