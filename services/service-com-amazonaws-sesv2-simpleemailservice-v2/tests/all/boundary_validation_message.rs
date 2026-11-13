// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the constraint walk refused at the boundary,
//! end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the constraint walk and the error envelope are
//! all exercised.
//!
//! The rule these cases pin: a clause about a value's FORM — the enum value set or
//! the regular-expression pattern the member had to satisfy — names the member and
//! the requirement and WITHHOLDS the value that violated it, while a clause about a
//! value that falls SHORT of a length minimum keeps the value the framework quotes. The
//! envelope is AWS's own: one clause per violation, counted.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise the request boundary, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Value) -> Reply {
        let payload = bytes::Bytes::from(serde_json::to_vec(&body).expect("serialize body"));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// `CreateImportJob` with the data source the case supplies.
    async fn create_import_job(&self, data_source: Value) -> Reply {
        self.call(
            "POST",
            "/v2/email/import-jobs",
            json!({
                "ImportDataSource": data_source,
                "ImportDestination": {
                    "ContactListDestination": {
                        "ContactListName": "newsletter",
                        "ContactListImportAction": "PUT",
                    },
                },
            }),
        )
        .await
    }

    /// `CreateTenant`, whose `TenantName` carries a length constraint and no pattern.
    async fn create_tenant(&self, tenant_name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/tenants",
            json!({"TenantName": tenant_name}),
        )
        .await
    }
}

/// `ImportDataSource$S3Url`'s modelled pattern
/// (`smithy_query(query="shape",shape="S3Url")`), which the refusal states verbatim.
const S3_URL_PATTERN: &str = r"^s3:\/\/([^\/]+)\/(.*?([^\/]+)\/?)$";

/// The clause separator the framework renders, shared by every requirement.
const SEPARATOR: &str = " failed to satisfy constraint: ";

#[tokio::test]
async fn a_pattern_clause_withholds_the_value() {
    // The rows span the shapes a value outside the pattern can take: another scheme,
    // no scheme at all, a bucket with no object, the empty value, one carrying the
    // quote characters the framework would have wrapped it in, and a long one. None
    // of them appears in the message.
    let values = [
        "not-a-valid-s3-url",
        "https://bucket.example.com/object.csv",
        "s3://bucket",
        "s3:///object.csv",
        "",
        "s3://'quoted'",
        "file:///tmp/secret-contacts.csv",
    ];

    for value in values {
        let ses = Ses::new();
        let reply = ses
            .create_import_job(json!({"S3Url": value, "DataFormat": "CSV"}))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {value:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value at 'importDataSource.s3Url'{SEPARATOR}Member \
                 must satisfy regular expression pattern: {S3_URL_PATTERN}"
            ),
            "case {value:?}: expected the value-less clause naming the member and the pattern, got {:?}",
            reply.message()
        );
        assert!(
            !reply.message().contains(value) || value.is_empty(),
            "case {value:?}: expected the submitted value to be withheld, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_value_inside_the_pattern_is_not_refused_for_its_form() {
    // The same member with values the pattern admits: whatever the operation answers,
    // it is not the constraint refusal.
    for value in [
        "s3://bucket/object.csv",
        "s3://bucket/prefix/object.csv",
        "s3://bucket/prefix/",
    ] {
        let ses = Ses::new();
        let reply = ses
            .create_import_job(json!({"S3Url": value, "DataFormat": "CSV"}))
            .await;
        assert!(
            !reply.message().contains(SEPARATOR),
            "case {value:?}: expected no constraint refusal, got {} {:?}",
            reply.status,
            reply.message()
        );
    }
}

#[tokio::test]
async fn an_enum_clause_withholds_the_value_too() {
    // The other clause about a value's form, on another member of the same request.
    for value in ["NOT_A_FORMAT", "csv", ""] {
        let ses = Ses::new();
        let reply = ses
            .create_import_job(json!({"S3Url": "s3://bucket/object.csv", "DataFormat": value}))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {value:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            format!(
                "1 validation error detected: Value at 'importDataSource.dataFormat'{SEPARATOR}\
                 Member must satisfy enum value set: [CSV, JSON]"
            ),
            "case {value:?}: expected the value-less clause naming the member and the value set, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn two_clauses_are_counted_in_the_envelope() {
    let ses = Ses::new();
    let reply = ses
        .create_import_job(json!({"S3Url": "not-a-valid-s3-url", "DataFormat": "NOT_A_FORMAT"}))
        .await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "BadRequestException"),
        "expected 400 BadRequestException, got {} {:?}",
        reply.status,
        reply.body
    );
    assert!(
        reply.message().starts_with("2 validation errors detected: "),
        "expected both clauses to be counted, got {:?}",
        reply.message()
    );
    for member in ["importDataSource.s3Url", "importDataSource.dataFormat"] {
        assert!(
            reply.message().contains(&format!("Value at '{member}'")),
            "expected a value-less clause for {member}, got {:?}",
            reply.message()
        );
    }
    for value in ["not-a-valid-s3-url", "NOT_A_FORMAT"] {
        assert!(
            !reply.message().contains(value),
            "expected {value:?} to be withheld, got {:?}",
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_size_clause_keeps_the_value() {
    // The boundary of the rule, on its MINIMUM side: a clause about a value that is too
    // SHORT is the framework's own sentence, which quotes the value, as AWS's own
    // validation does for a length minimum. (A value past a length MAXIMUM is the other
    // side of that line and is stated without the value — pinned in the
    // email-template refusal-wording suite beside this one.) `TenantName` is modelled
    // with a minimum length of 1 and no pattern
    // (`smithy_query(query="operation",operation="CreateTenant")`).
    let ses = Ses::new();
    let reply = ses.create_tenant("").await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "BadRequestException"),
        "expected 400 BadRequestException, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.message(),
        format!(
            "1 validation error detected: Value '' at 'tenantName'{SEPARATOR}Member must have \
             length greater than or equal to 1"
        ),
        "expected the framework's own length clause, got {:?}",
        reply.message()
    );
}
