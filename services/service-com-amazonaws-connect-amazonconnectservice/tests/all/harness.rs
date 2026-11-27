// SPDX-License-Identifier: CC0-1.0

//! The fixture every Connect suite in this target drives, and the helpers more than one
//! of them needs.
//!
//! One emulator, one region, with the Connect service mounted, reached over HTTP through
//! the same `MultiServiceRouter` the emulator serves with. The region travels in the
//! SigV4 credential scope, which is also how the handlers learn which region to mint
//! ARNs in.
//!
//! A suite adds its own operation helpers in its own file, as an `impl Connect` block
//! there: this module holds only what two or more suites share, so no suite stands up a
//! second copy of the emulator fixture.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the HTTP status, the `x-amzn-errortype` error code restJson1
/// names a modelled error shape with (empty on a success), and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

/// The mounted emulator a suite sends requests to.
pub(crate) struct Connect {
    router: MultiServiceRouter,
    region: String,
    engine: Arc<InMemoryEngine>,
}

impl Default for Connect {
    fn default() -> Self {
        Self::in_region("us-east-1")
    }
}

impl Connect {
    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            region: region.to_string(),
            engine,
        }
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The engine this emulator was built over — the single shared store its handlers
    /// read, so a suite can put a record there the way another service's fixture does
    /// and then read it back over HTTP.
    pub(crate) fn engine(&self) -> &InMemoryEngine {
        &self.engine
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let answer = self.send_answer(method, uri, body).await;
        (answer.status, answer.body)
    }

    /// The same call as [`Connect::send`], keeping the `x-amzn-errortype` header a
    /// suite needs to tell two modelled errors of the same status apart.
    pub(crate) async fn send_answer(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "connect.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{}/connect/aws4_request, \
                     SignedHeaders=host, Signature=abc",
                    self.region
                ),
            )
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body: parsed,
        }
    }

    /// `CreateInstance`, asserting success and handing back `(Id, Arn)`.
    pub(crate) async fn create_instance(&self, body: Value) -> (String, String) {
        let (status, out) = self.send("PUT", "/instance", Some(body.clone())).await;
        assert_eq!(status, 200, "CreateInstance {body}: got {out}");
        (
            out["Id"].as_str().expect("Id").to_string(),
            out["Arn"].as_str().expect("Arn").to_string(),
        )
    }

    /// `ListTagsForResource`: `GET /tags/{resourceArn}`.
    pub(crate) async fn list_tags_for_resource(&self, resource_arn: &str) -> Answer {
        self.send_answer("GET", &format!("/tags/{}", label(resource_arn)), None)
            .await
    }

    /// `CreateQueue`, asserting success and handing back the queue's ARN.
    pub(crate) async fn queue_arn(&self, instance_id: &str, body: Value) -> String {
        let created = self
            .send_answer("PUT", &format!("/queues/{instance_id}"), Some(body.clone()))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateQueue {body}: got {} {}",
            created.status, created.body
        );
        created.body["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue answers a QueueArn, got {}", created.body))
            .to_string()
    }

    /// `CreateContactFlow`, asserting success and handing back the flow's ARN.
    pub(crate) async fn contact_flow_arn(&self, instance_id: &str, body: Value) -> String {
        let created = self
            .send_answer(
                "PUT",
                &format!("/contact-flows/{instance_id}"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateContactFlow {body}: got {} {}",
            created.status, created.body
        );
        created.body["ContactFlowArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "CreateContactFlow answers a ContactFlowArn, got {}",
                    created.body
                )
            })
            .to_string()
    }

    /// `CreateHoursOfOperation`, asserting success and handing back its ARN.
    pub(crate) async fn hours_of_operation_arn(&self, instance_id: &str, body: Value) -> String {
        let created = self
            .send_answer(
                "PUT",
                &format!("/hours-of-operations/{instance_id}"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateHoursOfOperation {body}: got {} {}",
            created.status, created.body
        );
        created.body["HoursOfOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "CreateHoursOfOperation answers a HoursOfOperationArn, got {}",
                    created.body
                )
            })
            .to_string()
    }

    /// `ClaimPhoneNumber` of the first number the inventory advertises for this instance,
    /// handing back the claim's ARN.
    pub(crate) async fn claimed_phone_number_arn(&self, instance_id: &str, tags: Value) -> String {
        let search = self
            .send_answer(
                "POST",
                "/phone-number/search-available",
                Some(json!({
                    "InstanceId": instance_id,
                    "PhoneNumberCountryCode": "US",
                    "PhoneNumberType": "TOLL_FREE",
                })),
            )
            .await;
        assert_eq!(
            search.status, 200,
            "SearchAvailablePhoneNumbers: got {} {}",
            search.status, search.body
        );
        let number = search.body["AvailableNumbersList"][0]["PhoneNumber"]
            .as_str()
            .unwrap_or_else(|| panic!("the search advertises a number, got {}", search.body))
            .to_string();
        let claimed = self
            .send_answer(
                "POST",
                "/phone-number/claim",
                Some(json!({
                    "InstanceId": instance_id,
                    "PhoneNumber": number,
                    "Tags": tags,
                })),
            )
            .await;
        assert_eq!(
            claimed.status, 200,
            "ClaimPhoneNumber {number}: got {} {}",
            claimed.status, claimed.body
        );
        claimed.body["PhoneNumberArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "ClaimPhoneNumber answers a PhoneNumberArn, got {}",
                    claimed.body
                )
            })
            .to_string()
    }
}

/// The modelled `TagMap` maximum: `smithy_query(query="shape",shape="TagMap")` -> length
/// min 1 max 50.
pub(crate) const TAG_MAP_MAXIMUM: usize = 50;

/// The sentence this service's `ResourceNotFoundException` carries, written out here
/// rather than read off the implementation so the suites pin the text itself.
pub(crate) const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// An identifier of the form this service mints that names nothing, so a row can build
/// the ARN of a resource that was never stored.
pub(crate) const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A flow content of the documented form: the JSON string of a flow definition.
pub(crate) const FLOW_CONTENT: &str = r#"{"Version":"2019-10-30","StartAction":"a1","Actions":[{"Identifier":"a1","Type":"DisconnectParticipant","Parameters":{}}]}"#;

/// An ARN as ONE URI path segment: `/tags/{resourceArn}` binds the ARN to a non-greedy
/// label, so its `:` and `/` travel as `%3A` and `%2F` or the request addresses a
/// different route.
pub(crate) fn label(arn: &str) -> String {
    arn.replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// A minimal valid `CreateQueue` body carrying `tags`.
pub(crate) fn queue_with_tags(name: &str, tags: Value) -> Value {
    json!({ "Name": name, "HoursOfOperationId": ABSENT_ID, "Tags": tags })
}

/// The `tags` member of an answer, as a plain map, or `None` when the answer carries no
/// `tags` member at all.
pub(crate) fn tags_of(answer: &Answer) -> Option<BTreeMap<String, String>> {
    let tags = answer.body.get("tags")?;
    Some(
        tags.as_object()
            .unwrap_or_else(|| panic!("tags is a map, got {tags}"))
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("a tag value is a string, got {value}"))
                        .to_string(),
                )
            })
            .collect(),
    )
}

/// `tags` as the request that stored them sent it.
pub(crate) fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A tag map of `count` distinct entries, for the size rows.
pub(crate) fn tag_map(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
            .collect(),
    )
}

/// Build the `maxResults`/`nextToken` query string the `List*` models bind, plus any
/// number of repetitions of one list-valued query parameter (`queueTypes`,
/// `contactFlowTypes`). Pass an empty `values` for a `List*` that binds no such member.
pub(crate) fn query(
    max_results: Option<i32>,
    token: Option<&str>,
    repeated: &str,
    values: &[&str],
) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        params.push(format!("maxResults={max}"));
    }
    if let Some(token) = token {
        params.push(format!("nextToken={token}"));
    }
    for value in values {
        params.push(format!("{repeated}={value}"));
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// A minimal valid `CreateInstance` body.
pub(crate) fn connect_managed(alias: &str) -> Value {
    json!({
        "IdentityManagementType": "CONNECT_MANAGED",
        "InstanceAlias": alias,
        "InboundCallsEnabled": true,
        "OutboundCallsEnabled": true,
    })
}

/// `8-4-4-4-12` hex: the shape of the identifiers Connect mints. The service itself owns
/// that predicate — its handlers refuse a path label of any other form — so this asks it
/// rather than keeping a second copy of the shape.
pub(crate) fn looks_like_a_uuid(value: &str) -> bool {
    service_com_amazonaws_connect_amazonconnectservice::state::is_resource_id(value)
}

/// Pull one member out of every entry of a summary list.
pub(crate) fn ids_of(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .expect("a summary list")
        .iter()
        .map(|entry| entry[key].as_str().expect("an id").to_string())
        .collect()
}
