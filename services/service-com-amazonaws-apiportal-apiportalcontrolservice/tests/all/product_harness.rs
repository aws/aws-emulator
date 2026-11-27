// SPDX-License-Identifier: CC0-1.0

//! The harness the product suites share: one emulator over one shared engine, driven
//! over HTTP through the same router the emulator serves with.
//!
//! Every assertion in those suites reads the response the service computed from its
//! store; no suite reaches into the state struct, so an echoed or canned answer fails.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_apiportal_apiportalcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every suite signs for, so the ARNs the service mints must carry it.
pub const REGION: &str = "us-west-2";

/// The account segment every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT_ID: &str = "000000000000";

/// `ProductId`'s modelled `@length`, from
/// `smithy_query(query="shape", shape="ProductId")`.
pub const PRODUCT_ID_MIN: usize = 10;
pub const PRODUCT_ID_MAX: usize = 30;

/// `ProductName`'s modelled `@length`, from
/// `smithy_query(query="operation", operation="CreateProduct")`.
pub const DISPLAY_NAME_MIN: usize = 1;
pub const DISPLAY_NAME_MAX: usize = 255;

/// `ProductDescription`'s modelled `@length`, from the same operation.
pub const DESCRIPTION_MAX: usize = 1024;

/// `MaxResults`' modelled `@range`, from
/// `smithy_query(query="operation", operation="ListProducts")`.
pub const MAX_RESULTS_MIN: i64 = 1;
pub const MAX_RESULTS_MAX: i64 = 100;

/// One emulator: a router with only this service mounted, over one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; routing, handlers and state are.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Dispatch a request and hand back the status and the parsed JSON body
    /// (`Value::Null` when the answer carries none, as a 204 does).
    pub async fn call(&self, method: &str, uri: &str, body: &Value) -> Answer {
        let payload = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "apiportal.us-west-2.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/apiportal/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        Answer { status, body }
    }

    pub async fn create_product(&self, request: &Value) -> Answer {
        self.call("POST", "/products", request).await
    }

    pub async fn get_product(&self, product_id: &str) -> Answer {
        self.call(
            "GET",
            &format!("/products/{}", encode(product_id)),
            &Value::Null,
        )
        .await
    }

    pub async fn update_product(&self, product_id: &str, request: &Value) -> Answer {
        self.call("PATCH", &format!("/products/{}", encode(product_id)), request)
            .await
    }

    pub async fn delete_product(&self, product_id: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/products/{}", encode(product_id)),
            &Value::Null,
        )
        .await
    }

    /// `ListProducts` with the query string `query` (already assembled, without `?`).
    pub async fn list_products(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/products".to_string()
        } else {
            format!("/products?{query}")
        };
        self.call("GET", &uri, &Value::Null).await
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

/// What the emulator answered: the HTTP status and the JSON body.
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub fn error_code(&self) -> &str {
        // restJson1 renders `__type` as a shape id; a client reads the part after `#`.
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The error `message`, or the empty string when there is none.
    pub fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// A string member of the body, or the empty string when absent.
    pub fn string(&self, member: &str) -> &str {
        self.body[member].as_str().unwrap_or_default()
    }

    /// `updatedAt` as the epoch seconds restJson1 renders a timestamp as.
    pub fn updated_at(&self) -> f64 {
        self.body["updatedAt"].as_f64().unwrap_or_default()
    }

    /// The `items` of a `ListProducts` answer.
    pub fn items(&self) -> Vec<&Value> {
        self.body["items"]
            .as_array()
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// The `productId`s of a `ListProducts` answer, in the order it reported them.
    pub fn item_ids(&self) -> Vec<String> {
        self.items()
            .iter()
            .map(|item| item["productId"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The `nextToken` of a `ListProducts` answer, or `None` when it carries none.
    pub fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

/// Percent-encode a value for one URI path segment, the way an AWS SDK encodes an
/// `@httpLabel` member.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Whether `product_id` satisfies `ProductId`: `^[a-z0-9]+$`, 10..30 characters
/// (`smithy_query(query="shape", shape="ProductId")`).
pub fn is_well_formed_product_id(product_id: &str) -> bool {
    (PRODUCT_ID_MIN..=PRODUCT_ID_MAX).contains(&product_id.len())
        && product_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `arn` satisfies `ProductArn`:
/// `^arn:aws[a-z\-]*:apiportal:[a-z0-9\-]+:[0-9]{12}:products/[a-z0-9]{10,30}$`
/// (`smithy_query(query="shape", shape="ProductArn")`).
pub fn is_well_formed_product_arn(arn: &str) -> bool {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = segments.as_slice() else {
        return false;
    };
    let partition_is_well_formed = partition.strip_prefix("aws").is_some_and(|suffix| {
        suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    });
    let region_is_well_formed = !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let account_is_well_formed =
        account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit());
    let resource_is_well_formed = resource
        .strip_prefix("products/")
        .is_some_and(is_well_formed_product_id);

    *prefix == "arn"
        && partition_is_well_formed
        && *service == "apiportal"
        && region_is_well_formed
        && account_is_well_formed
        && resource_is_well_formed
}

/// A string of exactly `length` characters, for a case that sits on a published bound.
pub fn string_of_length(length: usize) -> String {
    "a".repeat(length)
}
