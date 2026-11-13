// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the emulator through.
//!
//! One copy, shared: a suite makes its calls the way a client does — `POST /` with
//! `X-Amz-Target: AWSGlue.<Operation>` and an `awsJson1_1` body — through the same
//! `MultiServiceRouter::dispatch` the production front door uses, so what a suite
//! asserts is HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_glue_awsglue::state::CATALOG_RESOURCE_FORM_TYPE_ID;
use smithy_server_runtime::MultiServiceRouter;

/// This emulator's account id, which is also the default `CatalogId`.
pub const ACCOUNT_ID: &str = "000000000000";

/// The access key id every request here is signed with. The router routes on the
/// SigV4 credential scope's service field (`glue`), so this one header is what gets a
/// request to this service. It is a credential id, NOT a principal: the catalog never
/// renders it into an ARN (see [`CALLER_PRINCIPAL`]).
pub const AKID: &str = "AKIDGLUETEST";

/// The principal ARN every request here speaks for: the emulator's single long-term
/// identity, the one `sts:GetCallerIdentity` reports (`arn:aws:iam::<account>:user/
/// emulator`). This is what the catalog records in `Table$CreatedBy`.
pub const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// The `__type` discriminator the Data Catalog writes onto a nested
/// `PrincipalPermissions` it returns: the structure's own Smithy shape name.
///
/// Source: the `__type` member of `com.amazonaws.glue#PrincipalPermissions` in this
/// crate's Smithy model (`smithy_query shape=PrincipalPermissions`), whose documentation
/// states the value is the structure's own shape name. Declared once here so every suite
/// that asserts the tag reads the same spelling.
pub const PRINCIPAL_PERMISSIONS_TAG: &str = "PrincipalPermissions";

/// The same discriminator on a nested `ErrorDetail`. Source: the `__type` member of
/// `com.amazonaws.glue#ErrorDetail` (`smithy_query shape=ErrorDetail`).
pub const ERROR_DETAIL_TAG: &str = "ErrorDetail";

/// A SigV4 `Authorization` header scoped to `glue`: the routing signal the router
/// reads first. Authentication is off on the test router, so the signature is not
/// verified — only the scope is read, including the Region field below, which is what
/// the service reads a request's Region from.
fn authorization_in(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{region}/glue/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The Region every call made through [`call`] is served in.
pub const REGION: &str = "us-east-1";

/// An emulator with only this service mounted, on one shared engine, with request
/// authentication off (the test sends no real signature).
pub fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same, on an engine the caller holds — so a test can seed the store directly
/// instead of driving thousands of requests to reach a capacity ceiling.
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        engine,
    ));
    router
}

/// An Amazon S3 client over this emulator's own in-process callback: the seam every Glue
/// reading of a table's data takes, so what a suite stages through this client is what the
/// run under test reads.
///
/// ONE copy, shared by every suite that stages the objects behind a table — the column
/// statistics computation, the crawl, a job script and the data quality evaluation — so the
/// client the suites stage through cannot drift from the one the service reads through. The
/// router must have Amazon S3 mounted on the same engine as Glue for it to reach anything.
pub fn s3_client(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves.
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Make one awsJson1_1 call and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The same, through another Region's endpoint: only the credential scope's Region
/// changes, which is the Region the service serves the request in.
pub async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, _headers, parsed) = call_reported(router, region, operation, body).await;
    (status, parsed)
}

/// The same ONE request builder, answering the response's HEADERS beside its status and
/// body — for a suite that asserts something a response reports in a header rather than in
/// its document, as the request a response names itself with is
/// (`tests/all/response_request_id.rs`).
pub async fn call_reported(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, smithy_server_runtime::http::HeaderMap, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header("authorization", authorization_in(region))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

/// Make one call that must succeed, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The awsJson error code of a failure body (`__type`), which may carry a namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` member of a failure body, which carries the refusal's wording.
pub fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// The `Id` a READ of the business-catalog asset type registered under `name` reports: the
/// catalog-qualified form `glue_<accountId>::<name>`, where the put that registered it
/// reports the bare `name`.
///
/// One copy, shared: every suite that reads an asset type back expects the same form, and
/// both forms address the record.
pub fn qualified_asset_type_id(name: &str) -> String {
    format!("glue_{ACCOUNT_ID}::{name}")
}

/// Assert that `form` is the catalog's OWN form on the asset `read` reports: it names the
/// catalog's own form type and carries, as its content, the catalog's resource document for
/// that asset — the `Name` and, when the asset has one, the `Description` the SAME read
/// reports.
///
/// One copy, shared: every suite that reads an asset back sees that form beside the forms
/// the asset's own puts stored, and what each of them checks is that the document agrees
/// with the record the same read reports. The exact TEXT of the document, member order
/// included, is pinned by `tests/all/asset_attachments.rs`.
pub fn assert_catalog_form(case: &str, read: &Value, form: Option<&Value>) {
    let form = form.unwrap_or_else(|| {
        panic!("case {case}: every read of an asset must report the catalog's own form, got {read}")
    });
    assert_eq!(
        form.get("FormTypeId").and_then(Value::as_str),
        Some(CATALOG_RESOURCE_FORM_TYPE_ID),
        "case {case}: the catalog's own form must name the catalog's own form type, got {form}"
    );

    let content = form
        .get("Content")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the catalog's own form must carry its document, got {form}"));
    let document: Value = serde_json::from_str(content).unwrap_or_else(|error| {
        panic!("case {case}: the document must be JSON, got {content:?} ({error})")
    });

    let mut expected = serde_json::Map::new();
    expected.insert(
        "name".to_string(),
        read.get("Name")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the read must report the asset's Name, got {read}")),
    );
    if let Some(description) = read.get("Description") {
        expected.insert("description".to_string(), description.clone());
    }
    assert_eq!(
        document,
        Value::Object(expected),
        "case {case}: the catalog's own form must carry the asset's resource document, got \
         {content:?} for {read}"
    );
}

/// The names in a `GetTables` `TableList`, in the order the service returned them.
///
/// One copy, shared: every suite that pages over tables reads the list the same way.
pub fn table_names(page: &Value) -> Vec<String> {
    page["TableList"]
        .as_array()
        .expect("TableList is a list")
        .iter()
        .map(|table| {
            table["Name"]
                .as_str()
                .expect("Table.Name is a string")
                .to_string()
        })
        .collect()
}

/// The strings of a top-level list member of a response body, which must be present even
/// when it is empty.
///
/// One copy, shared: every suite that reads a flat list of names off an answer — the
/// `*NotFound` list of a batch reader, say — reads it the same way.
pub fn string_list(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report {member}, got {body}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("{member} holds strings, got {name}"))
                .to_string()
        })
        .collect()
}

/// The `Name` of every element of a top-level list member, in the order the service
/// returned them.
///
/// One copy, shared: every suite that reads the names off a list of resource projections
/// (`Triggers`, `Jobs`, `Crawlers`) reads them the same way.
pub fn names_in(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report a {member} list, got {body}"))
        .iter()
        .map(|element| {
            element
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every element of {member} carries a Name, got {element}"))
                .to_string()
        })
        .collect()
}

/// Assert that every member of `sent` reappears, unchanged, on `got`. Recurses into
/// nested structures AND into the elements of a nested list, so a member the server
/// fills with its own default (an absent `Compressed`, say) or ADDS to a nested
/// structure (the `__type` tag the catalog writes onto a nested `PrincipalPermissions`)
/// is not read as a mismatch: the rule under test is that what the client sent comes
/// back, not that the server adds nothing.
///
/// A `null` expectation means the member carries no value: awsJson omits an absent
/// member, so absent or explicitly null both satisfy it.
pub fn assert_echoed(case: &str, sent: &Value, got: &Value) {
    match sent {
        Value::Null => assert!(
            got.is_null(),
            "case {case}: expected no value, got {got}"
        ),
        Value::Object(members) => {
            for (member, value) in members {
                let Some(actual) = got.get(member) else {
                    if value.is_null() {
                        continue;
                    }
                    if value.is_object() || value.is_array() {
                        panic!("case {case}: member {member} is absent from the response {got}");
                    }
                    panic!(
                        "case {case}: member {member} must round-trip; expected {value}, got \
                         nothing in {got}"
                    );
                };
                assert_echoed(case, value, actual);
            }
        }
        Value::Array(items) => {
            let actual = got.as_array().unwrap_or_else(|| {
                panic!("case {case}: expected a list to round-trip as a list, got {got}")
            });
            assert_eq!(
                items.len(),
                actual.len(),
                "case {case}: a list must round-trip element for element; expected {sent}, got \
                 {got}"
            );
            for (element, actual) in items.iter().zip(actual.iter()) {
                assert_echoed(case, element, actual);
            }
        }
        _ => assert_eq!(
            got, sent,
            "case {case}: a scalar must round-trip; expected {sent}, got {got}"
        ),
    }
}
