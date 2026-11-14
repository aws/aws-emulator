// SPDX-License-Identifier: CC0-1.0

//! The shared wire harness every suite in this crate drives the emulator through.
//!
//! One emulator over one shared engine, called the way a real restJson1 SDK client
//! calls it (`MultiServiceRouter::dispatch`), and one reduced view of a response.
//! Both suites use it, so a request is built in exactly one place.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53profiles_route53profiles::EmulatedService;

/// This service's sigv4 signing name, from the model (`OPERATIONS.md`, "Wire protocol").
pub(crate) const SIGNING_NAME: &str = "route53profiles";

/// The account id every generated emulator presents, as `engine` defines it.
pub(crate) const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// One response, reduced to what the assertions read.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The wire error code (`x-amzn-errortype`, else the body's `__type`), with any
    /// `namespace#` prefix stripped. `None` on success.
    pub(crate) error: Option<String>,
    /// The `x-amzn-errortype` header exactly as it arrived, for a suite that asserts
    /// on the HEADER rather than on whichever of the two carriers answered.
    pub(crate) error_type_header: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    /// The response's top-level `member`, panicking with the whole body when it is
    /// not there — a failure that is otherwise invisible behind `Value::Null`.
    pub(crate) fn member(&self, name: &str) -> &Value {
        let value = &self.body[name];
        assert!(
            !value.is_null(),
            "member {name} is absent from {}",
            self.body
        );
        value
    }

    /// The string at `member.field`, for the single-structure outputs of this
    /// service (`Profile`, `ProfileAssociation`).
    pub(crate) fn nested_string(&self, member: &str, field: &str) -> String {
        self.member(member)[field]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "{member}.{field} is not a string in {}",
                    self.body
                )
            })
            .to_string()
    }

    /// A list member as a slice, panicking when it is absent or not a list.
    pub(crate) fn list(&self, name: &str) -> &[Value] {
        self.body[name]
            .as_array()
            .unwrap_or_else(|| panic!("member {name} is not a list in {}", self.body))
    }

    /// The error body's `Message`, panicking when the response carries none — the
    /// member every modelled error of this service declares.
    pub(crate) fn message(&self) -> String {
        self.body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("no Message in {}", self.body))
            .to_string()
    }

    /// The error body's lowercase `message`, which is the member the front door's
    /// own constraint refusal carries (as against the modelled errors' `Message`).
    pub(crate) fn lower_message(&self) -> String {
        self.body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.body))
            .to_string()
    }
}

/// One emulator over one shared engine: every call sees the same store.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    region: String,
}

impl Emulator {
    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub(crate) fn new() -> Self {
        Self::in_region("us-east-1")
    }

    pub(crate) async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host, Signature=00",
            self.region
        );
        let payload = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://{SIGNING_NAME}.{}.amazonaws.com{path_and_query}",
                self.region
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let header_error_header = header_error.clone();
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error = if status < 400 {
            None
        } else {
            header_error
                .or_else(|| body["__type"].as_str().map(str::to_string))
                .map(|code| {
                    code.rsplit('#')
                        .next()
                        .unwrap_or(code.as_str())
                        .split('/')
                        .next()
                        .unwrap_or(code.as_str())
                        .to_string()
                })
        };
        Reply {
            status,
            error,
            error_type_header: header_error_header,
            body,
        }
    }

    pub(crate) async fn create_profile(&self, body: Value) -> Reply {
        self.call("POST", "/profile", Some(body)).await
    }

    /// A create with a token derived from the name, for the cases that do not test
    /// idempotency themselves.
    pub(crate) async fn create_named(&self, name: &str) -> Reply {
        self.create_profile(json!({ "Name": name, "ClientToken": format!("tok-{name}") }))
            .await
    }

    pub(crate) async fn get_profile(&self, id: &str) -> Reply {
        self.call("GET", &format!("/profile/{id}"), None).await
    }

    pub(crate) async fn delete_profile(&self, id: &str) -> Reply {
        self.call("DELETE", &format!("/profile/{id}"), None).await
    }

    pub(crate) async fn list_profiles(&self, query: &str) -> Reply {
        self.call("GET", &format!("/profiles{query}"), None).await
    }

    pub(crate) async fn associate(
        &self,
        profile_id: &str,
        resource_id: &str,
        name: &str,
    ) -> Reply {
        self.call(
            "POST",
            "/profileassociation",
            Some(json!({
                "ProfileId": profile_id,
                "ResourceId": resource_id,
                "Name": name,
            })),
        )
        .await
    }

    pub(crate) async fn get_association(&self, id: &str) -> Reply {
        self.call("GET", &format!("/profileassociation/{id}"), None)
            .await
    }

    pub(crate) async fn list_associations(&self, query: &str) -> Reply {
        self.call("GET", &format!("/profileassociations{query}"), None)
            .await
    }

    /// `AssociateResourceToProfile`, with `ResourceProperties` on the request only
    /// when the case names a document.
    pub(crate) async fn associate_resource(
        &self,
        profile_id: &str,
        resource_arn: &str,
        name: &str,
        properties: Option<&str>,
    ) -> Reply {
        let mut body = json!({
            "ProfileId": profile_id,
            "ResourceArn": resource_arn,
            "Name": name,
        });
        if let Some(properties) = properties {
            body["ResourceProperties"] = json!(properties);
        }
        self.call("POST", "/profileresourceassociation", Some(body))
            .await
    }

    pub(crate) async fn get_resource_association(&self, id: &str) -> Reply {
        self.call("GET", &format!("/profileresourceassociation/{id}"), None)
            .await
    }

    /// `ListProfileResourceAssociations`: the Profile is a path label and
    /// `ResourceType`, `MaxResults` and `NextToken` are query parameters, so
    /// `query` is appended as given (`""` for none).
    pub(crate) async fn list_resource_associations(&self, profile_id: &str, query: &str) -> Reply {
        self.call(
            "GET",
            &format!("/profileresourceassociations/profileid/{profile_id}{query}"),
            None,
        )
        .await
    }

    pub(crate) async fn disassociate(&self, profile_id: &str, resource_id: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/profileassociation/Profileid/{profile_id}/resourceid/{resource_id}"),
            None,
        )
        .await
    }
}
