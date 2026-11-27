// SPDX-License-Identifier: CC0-1.0

//! The template member that says who owns a template, and the listing filter over it.
//!
//! RULE under test: a template's `templateOwner` is `CUSTOMER` — the model: "CUSTOMER – The
//! template is in your account" — for every template this service holds, because every one
//! of them was created in the caller's own account; the other value, `AWS`, names an AWS
//! managed template, which no operation of this service creates. So `ListTemplates` reports
//! `CUSTOMER` on every summary, its `templateOwner` filter reports the same templates when
//! it asks for that owner and none when it asks for the other, and the filter composes with
//! the `status` filter and with paging.
//!
//! The rows differ along what the rule generalises over: the owner asked for, whether the
//! template is published, how many rules it associates, and the page size the listing is
//! walked with.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `TemplateOwner`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="TemplateOwner")`).
const TEMPLATE_OWNERS: &[&str] = &["CUSTOMER", "AWS"];

/// What a caller sees: the HTTP facts it branches on plus the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    json: serde_json::Value,
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

    /// Walk a `ListTemplates` query to its end, returning every summary it reported.
    async fn list_templates(&self, query: &[(&str, &str)], max_results: Option<u32>) -> Vec<serde_json::Value> {
        let mut collected: Vec<serde_json::Value> = Vec::new();
        let mut next: Option<String> = None;
        // Generous: every page carries at least one row, so a terminating walk is far
        // inside this bound and a listing that never terminates fails instead of hanging.
        for _ in 0..512 {
            let mut rendered: Vec<String> = query
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            if let Some(limit) = max_results {
                rendered.push(format!("maxResults={limit}"));
            }
            if let Some(token) = &next {
                rendered.push(format!(
                    "nextToken={}",
                    ::percent_encoding::utf8_percent_encode(
                        token,
                        ::percent_encoding::NON_ALPHANUMERIC
                    )
                ));
            }
            let path = if rendered.is_empty() {
                "/templates".to_string()
            } else {
                format!("/templates?{}", rendered.join("&"))
            };
            let reply = self.call("GET", &path, None).await;
            assert_eq!(
                reply.status, 200,
                "case walking {query:?}: expected 200, got {} {:?}",
                reply.status, reply.json
            );
            let page = reply.json["templates"]
                .as_array()
                .unwrap_or_else(|| panic!("case walking {query:?}: templates is required, got {:?}", reply.json));
            collected.extend(page.iter().cloned());
            match reply.json["nextToken"].as_str() {
                Some(token) => next = Some(token.to_string()),
                None => return collected,
            }
        }
        panic!("case walking {query:?}: the walk never terminated, collected {collected:?}");
    }
}

/// `count` distinct rule references, as a `CreateTemplate` body carries them: the model
/// documents `ruleIdentifier` as the rule's ARN, built with the helper the handlers mint
/// ARNs with rather than from a literal.
fn rule_references(count: usize) -> serde_json::Value {
    serde_json::Value::Array(
        (0..count)
            .map(|index| {
                serde_json::json!({
                    "ruleIdentifier": engine::emulator_arn(
                        "network-security-manager",
                        "us-east-1",
                        &format!("rule/ownedrule{index}"),
                    ),
                })
            })
            .collect(),
    )
}

/// Create the templates the rows below are listed from: they differ along how many rules
/// each associates and whether it is published.
async fn create_templates(emulator: &Emulator) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, (rules, published)) in [(1usize, true), (3, false), (2, true)]
        .into_iter()
        .enumerate()
    {
        let reply = emulator
            .call(
                "POST",
                "/templates",
                Some(serde_json::json!({
                    "templateName": format!("owned-template-{index}"),
                    "firewallType": "WAF",
                    "isPublished": published,
                    "associatedRuleList": rule_references(rules),
                })),
            )
            .await;
        assert_eq!(
            reply.status, 201,
            "creating template {index} expected 201, got {} {:?}",
            reply.status, reply.json
        );
        ids.push(
            reply.json["templateId"]
                .as_str()
                .unwrap_or_else(|| panic!("templateId is required, got {:?}", reply.json))
                .to_string(),
        );
    }
    ids
}

/// RULE: every template this service holds is listed as owned by the customer.
#[tokio::test]
async fn every_stored_template_is_listed_as_customer_owned() {
    let emulator = Emulator::new();
    let created = create_templates(&emulator).await;

    let listed = emulator.list_templates(&[], None).await;
    assert_eq!(
        listed.len(),
        created.len(),
        "every created template is listed, got {listed:?}"
    );
    for summary in &listed {
        assert_eq!(
            summary["templateOwner"].as_str(),
            Some(TEMPLATE_OWNERS[0]),
            "case {:?}: a template in this account is customer-owned, got {:?}",
            summary["templateId"],
            summary["templateOwner"]
        );
    }
}

/// RULE: the `templateOwner` filter reports the templates with that owner — every stored
/// template for `CUSTOMER`, none for `AWS` — and composes with `status` and with paging.
#[tokio::test]
async fn the_template_owner_filter_reports_the_templates_with_that_owner() {
    let emulator = Emulator::new();
    let created = create_templates(&emulator).await;

    /// One row: the case, the query it asks with, and how many summaries it must report.
    struct Row {
        label: &'static str,
        query: Vec<(&'static str, &'static str)>,
        expected: usize,
    }

    let rows = vec![
        Row {
            label: "no owner filter",
            query: vec![],
            expected: created.len(),
        },
        Row {
            label: "templateOwner CUSTOMER",
            query: vec![("templateOwner", "CUSTOMER")],
            expected: created.len(),
        },
        Row {
            label: "templateOwner AWS, which this store holds none of",
            query: vec![("templateOwner", "AWS")],
            expected: 0,
        },
        Row {
            label: "templateOwner CUSTOMER and status ACTIVE",
            query: vec![("templateOwner", "CUSTOMER"), ("status", "ACTIVE")],
            expected: 2,
        },
        Row {
            label: "templateOwner CUSTOMER and status DRAFT",
            query: vec![("templateOwner", "CUSTOMER"), ("status", "DRAFT")],
            expected: 1,
        },
        Row {
            label: "templateOwner AWS and status ACTIVE",
            query: vec![("templateOwner", "AWS"), ("status", "ACTIVE")],
            expected: 0,
        },
    ];

    for row in rows {
        let label = row.label;
        // The page sizes: the listing's own default, one row per page, and the modelled
        // maximum of `MaxResults` (range 1..100).
        for max_results in [None, Some(1), Some(100)] {
            let listed = emulator.list_templates(&row.query, max_results).await;
            assert_eq!(
                listed.len(),
                row.expected,
                "case {label} with maxResults={max_results:?}: expected {} summary(ies), got \
                 {listed:?}",
                row.expected
            );
            for summary in &listed {
                assert_eq!(
                    summary["templateOwner"].as_str(),
                    Some("CUSTOMER"),
                    "case {label}: a reported summary is customer-owned, got {:?}",
                    summary["templateOwner"]
                );
            }
        }
    }
}

/// RULE: a `templateOwner` outside the modelled value set is refused at the boundary.
#[tokio::test]
async fn a_template_owner_outside_the_modelled_set_is_refused() {
    let emulator = Emulator::new();
    create_templates(&emulator).await;

    // The rows differ along how the value misses the set: the right word in the wrong
    // case, another service's word, and the empty string.
    for value in ["customer", "AMAZON", ""] {
        let reply = emulator
            .call("GET", &format!("/templates?templateOwner={value}"), None)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {value:?}: expected 400, got {} {:?}",
            reply.status, reply.json
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ValidationException"),
            "case {value:?}: unexpected x-amzn-errortype, got {:?}",
            reply.error_code
        );
    }
}
