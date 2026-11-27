// SPDX-License-Identifier: CC0-1.0

//! The scope member that says what a scope is specialized for.
//!
//! RULE under test: `specializedScopeType` is set when a scope is created and never
//! changed — "Omit this value to create a standard scope that you can associate with a
//! deployment" — so every read of the scope answers with exactly the value the create
//! submitted, and a scope created without one carries none at all. A value outside the
//! modelled `SpecializedScopeType` set is refused at the boundary, before the scope exists.
//!
//! The rows differ along what the rule generalises over: whether the member is submitted,
//! which read answers it back (`CreateScope`, `GetScope`, `ListScopes`), whether the scope
//! is published, and how the scope's configuration selects.

use std::sync::Arc;

use service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `SpecializedScopeType`'s values as the MODEL declares them
/// (`smithy_query(query="shape",shape="SpecializedScopeType")`).
const SPECIALIZED_SCOPE_TYPES: &[&str] = &["IGW_FIREWALL_SUBNET_CONNECTIVITY_OVERRIDE"];

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
}

/// `value` as one URI path segment: percent-encoded by the maintained encoder, the way an
/// SDK encodes an `@httpLabel` value.
fn escape(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}

/// RULE: the value a create submits is the value every read of the scope answers with, and
/// a create that submits none leaves the member absent everywhere.
#[tokio::test]
async fn a_scopes_specialized_type_is_what_its_create_submitted() {
    let emulator = Emulator::new();

    /// One row: the case, the `specializedScopeType` the create submits, whether the scope
    /// is published, and the resource selection it carries.
    struct Row {
        label: &'static str,
        specialized: Option<&'static str>,
        published: bool,
        resource_scopes: serde_json::Value,
    }

    let rows = vec![
        Row {
            label: "a standard scope, the member omitted",
            specialized: None,
            published: true,
            resource_scopes: serde_json::json!({ "AWS::EC2::EIP": { "includeAll": true } }),
        },
        Row {
            label: "a specialized scope, the one modelled value",
            specialized: Some(SPECIALIZED_SCOPE_TYPES[0]),
            published: true,
            resource_scopes: serde_json::json!({ "AWS::EC2::Subnet": { "includeAll": true } }),
        },
        Row {
            label: "a specialized DRAFT scope selecting explicit ARNs",
            specialized: Some(SPECIALIZED_SCOPE_TYPES[0]),
            published: false,
            resource_scopes: serde_json::json!({
                "AWS::EC2::Subnet": {
                    "include": {
                        "explicitArns": [
                            "arn:aws:ec2:us-east-1:111122223333:subnet/subnet-0123456789abcdef0",
                        ],
                    },
                },
            }),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let label = row.label;
        let mut body = serde_json::json!({
            "scopeName": format!("specialized-probe-{index}"),
            "isPublished": row.published,
            "scopeConfiguration": { "resourceScopes": row.resource_scopes },
        });
        if let Some(specialized) = row.specialized {
            body["specializedScopeType"] = serde_json::json!(specialized);
        }

        let created = emulator.call("POST", "/scopes", Some(body)).await;
        assert_eq!(
            created.status, 201,
            "case {label}: expected 201, got {} {:?}",
            created.status, created.json
        );
        assert_eq!(
            created.json["specializedScopeType"].as_str(),
            row.specialized,
            "case {label}: the create must answer with the submitted value, got {:?}",
            created.json["specializedScopeType"]
        );
        let scope_arn = created.json["scopeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: scopeArn is required, got {:?}", created.json))
            .to_string();
        let scope_id = created.json["scopeId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label}: scopeId is required, got {:?}", created.json))
            .to_string();

        // Read back through the implemented reader, by ARN: the stored record, not the
        // create's answer.
        let read = emulator
            .call("GET", &format!("/scopes/{}", escape(&scope_arn)), None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {label}: the read expected 200, got {} {:?}",
            read.status, read.json
        );
        assert_eq!(
            read.json["specializedScopeType"].as_str(),
            row.specialized,
            "case {label}: GetScope must answer with the stored value, got {:?}",
            read.json["specializedScopeType"]
        );

        // And in the listing's summary, which carries the member too.
        let listed = emulator.call("GET", "/scopes", None).await;
        assert_eq!(
            listed.status, 200,
            "case {label}: the listing expected 200, got {} {:?}",
            listed.status, listed.json
        );
        let summaries = listed.json["scopes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: scopes is required, got {:?}", listed.json));
        let summary = summaries
            .iter()
            .find(|summary| summary["scopeId"].as_str() == Some(scope_id.as_str()))
            .unwrap_or_else(|| panic!("case {label}: the created scope must be listed, got {summaries:?}"));
        assert_eq!(
            summary["specializedScopeType"].as_str(),
            row.specialized,
            "case {label}: the summary must carry the stored value, got {:?}",
            summary["specializedScopeType"]
        );
    }
}

/// RULE: a `specializedScopeType` outside the modelled value set is refused, and no scope
/// is created for it.
#[tokio::test]
async fn a_specialized_type_outside_the_modelled_set_is_refused() {
    let emulator = Emulator::new();

    // The rows differ along how the value misses the set: another service's word, the
    // modelled value in the wrong case, and the empty string.
    for (index, value) in [
        "IGW_FIREWALL",
        "igw_firewall_subnet_connectivity_override",
        "",
    ]
    .into_iter()
    .enumerate()
    {
        let reply = emulator
            .call(
                "POST",
                "/scopes",
                Some(serde_json::json!({
                    "scopeName": format!("specialized-refusal-{index}"),
                    "specializedScopeType": value,
                    "scopeConfiguration": {
                        "resourceScopes": { "AWS::EC2::Subnet": { "includeAll": true } },
                    },
                })),
            )
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

    let listed = emulator.call("GET", "/scopes", None).await;
    assert_eq!(
        listed.json["scopes"],
        serde_json::json!([]),
        "a refused create stores nothing, got {:?}",
        listed.json["scopes"]
    );
}
