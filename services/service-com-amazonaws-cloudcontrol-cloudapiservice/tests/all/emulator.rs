// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness this crate's suites drive the emulator through.
//!
//! Every suite calls the service the way a client does — `POST /` with
//! `X-Amz-Target: CloudApiService.<Operation>` on `awsJson1_0`, signed for the
//! `cloudcontrolapi` signing name, through the same `MultiServiceRouter::dispatch` the
//! socket front door uses — so the mounting rules live here once rather than in each
//! suite.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The sigv4 signing name the model gives this service.
pub(crate) const SIGNING_NAME: &str = "cloudcontrolapi";

/// The `X-Amz-Target` prefix a client writes: the local name of this service's Smithy
/// service shape.
pub(crate) const TARGET_PREFIX: &str = "CloudApiService";

/// The region the harness signs for.
pub(crate) const SIGNING_REGION: &str = "us-east-1";

/// The sigv4 signing name of the CloudWatch Logs service, whose own store holds the log
/// groups `AWS::Logs::LogGroup` names.
pub(crate) const LOGS_SIGNING_NAME: &str = "logs";

/// The `X-Amz-Target` prefix a client writes for that service: the local name of its
/// Smithy service shape.
pub(crate) const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The sigv4 signing name of the Amazon SNS service, whose own store holds the topics
/// `AWS::SNS::Topic` names.
pub(crate) const SNS_SIGNING_NAME: &str = "sns";

/// The API version an `awsQuery` call of that service carries, as its model declares it.
pub(crate) const SNS_API_VERSION: &str = "2010-03-31";

/// The sigv4 signing name of the Amazon SQS service, whose own store holds the queues
/// `AWS::SQS::Queue` names.
pub(crate) const SQS_SIGNING_NAME: &str = "sqs";

/// The sigv4 signing name of the Amazon S3 service, whose own store holds the buckets
/// `AWS::S3::Bucket` names.
pub(crate) const S3_SIGNING_NAME: &str = "s3";

/// The endpoint host a request to that service is addressed to: the REGIONAL one, which
/// carries no bucket label, so every request below is path-style.
pub(crate) const S3_ENDPOINT_HOST: &str = "s3.us-east-1.amazonaws.com";

/// The `X-Amz-Target` prefix a client writes for that service: the local name of its
/// Smithy service shape.
pub(crate) const SQS_TARGET_PREFIX: &str = "AmazonSQS";

/// The sigv4 signing name of the IAM service, whose own store holds the customer managed
/// policies `AWS::IAM::ManagedPolicy` names.
pub(crate) const IAM_SIGNING_NAME: &str = "iam";

/// The API version an `awsQuery` call of that service carries, as its model declares it.
pub(crate) const IAM_API_VERSION: &str = "2010-05-08";

/// The sigv4 signing name of the Step Functions service, whose own store holds the state
/// machines `AWS::StepFunctions::StateMachine` names.
pub(crate) const SFN_SIGNING_NAME: &str = "states";

/// The `X-Amz-Target` prefix a client writes for that service: the local name of its
/// Smithy service shape.
pub(crate) const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";

/// How long a wait for a resource operation request to complete runs before the case
/// fails.
///
/// An accepted request is answered in a non-terminal status, so the clock owes it the
/// rest of the progression; this is well past that span, because a case waits for the
/// STATUS it is after and never for a fixed span.
pub(crate) const SETTLED_BUDGET: Duration = Duration::from_secs(30);

/// How often that wait re-reads.
pub(crate) const SETTLED_INTERVAL: Duration = Duration::from_millis(20);

/// "`IN_PROGRESS`: The resource operation is currently in progress."
pub(crate) const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// "`SUCCESS`: The resource operation has successfully completed."
pub(crate) const STATUS_SUCCESS: &str = "SUCCESS";

/// "`FAILED`: The resource operation has failed. Refer to the error code and status
/// message for more information."
pub(crate) const STATUS_FAILED: &str = "FAILED";

/// "`CANCEL_IN_PROGRESS`: The resource operation is in the process of being canceled."
pub(crate) const STATUS_CANCEL_IN_PROGRESS: &str = "CANCEL_IN_PROGRESS";

/// "`CANCEL_COMPLETE`: The resource operation has been canceled."
pub(crate) const STATUS_CANCEL_COMPLETE: &str = "CANCEL_COMPLETE";

/// One emulator: a router with this service mounted over one shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// One emulator, mounted the way the assembled router mounts this service: this service
    /// over one shared engine, and whatever `owning` registers beside it on that SAME engine
    /// — which is what the assembled router is, and what a case about a resource created
    /// over an OWNING service's own API needs.
    ///
    /// The one router construction every constructor below goes through, so mounting a
    /// further owning service adds a registration rather than a copy of this block.
    fn mounted(owning: impl FnOnce(&mut MultiServiceRouter, Arc<engine::InMemoryEngine>)) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pinned off so the suites do not depend on the process-wide default; the
        // Authorization header below is still what carries the credential scope.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudcontrol_cloudapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        owning(&mut router, engine);
        Self { router }
    }

    /// One emulator holding this service alone.
    pub(crate) fn new() -> Self {
        Self::mounted(|_, _| {})
    }

    /// One emulator with the service that OWNS `AWS::Logs::LogGroup` mounted beside this
    /// one.
    pub(crate) fn with_cloudwatch_logs() -> Self {
        Self::mounted(|router, engine| {
            router.register_service(
                service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
            );
        })
    }

    /// One emulator with the service that OWNS `AWS::SQS::Queue` mounted beside this one.
    pub(crate) fn with_sqs() -> Self {
        Self::mounted(|router, engine| {
            router
                .register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(engine));
        })
    }

    /// One emulator with the service that OWNS `AWS::SNS::Topic` mounted beside this one.
    pub(crate) fn with_sns() -> Self {
        Self::mounted(|router, engine| {
            router.register_service(
                service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                    engine,
                ),
            );
        })
    }

    /// One operation of the Amazon SNS service — the OWNING service's own API, the one a
    /// client uses when it is not going through Cloud Control API.
    ///
    /// Amazon SNS speaks the `awsQuery` protocol: the call is a form-encoded `POST` naming
    /// the action in its own body, and the answer is an XML envelope, so it is handed back
    /// as the `(status, body)` text it is rather than as JSON.
    pub(crate) async fn sns(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version={SNS_API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{SIGNING_REGION}/\
             {SNS_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SNS_SIGNING_NAME}.localhost"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization)
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One emulator with the service that OWNS `AWS::IAM::ManagedPolicy` mounted beside this
    /// one.
    pub(crate) fn with_iam() -> Self {
        Self::mounted(|router, engine| {
            router.register_service(
                service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                    engine,
                ),
            );
        })
    }

    /// One operation of the IAM service — the OWNING service's own API, the one a client uses
    /// when it is not going through Cloud Control API.
    ///
    /// IAM speaks the `awsQuery` protocol, so the call is a form-encoded `POST` naming the
    /// action in its own body and the answer is an XML envelope, handed back as the
    /// `(status, body)` text it is rather than as JSON.
    pub(crate) async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{SIGNING_REGION}/\
             {IAM_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{IAM_SIGNING_NAME}.localhost"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization)
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// One emulator with the service that OWNS `AWS::StepFunctions::StateMachine` mounted
    /// beside this one.
    pub(crate) fn with_step_functions() -> Self {
        Self::mounted(|router, engine| {
            router.register_service(
                service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(engine),
            );
        })
    }

    /// One operation of the Step Functions service — the OWNING service's own API, the one a
    /// client uses when it is not going through Cloud Control API.
    pub(crate) async fn sfn(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            SFN_SIGNING_NAME,
            "application/x-amz-json-1.0",
            &format!("{SFN_TARGET_PREFIX}.{operation}"),
            body,
        )
        .await
    }

    /// One operation of the Step Functions service, required to succeed.
    pub(crate) async fn sfn_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.sfn(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// One emulator with the service that OWNS `AWS::S3::Bucket` mounted beside this one.
    pub(crate) fn with_s3() -> Self {
        Self::mounted(|router, engine| {
            router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(engine));
        })
    }

    /// One operation of this service, as a `(status, body)` pair.
    pub(crate) async fn cc(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            SIGNING_NAME,
            "application/x-amz-json-1.0",
            &format!("{TARGET_PREFIX}.{operation}"),
            body,
        )
        .await
    }

    /// One operation of the CloudWatch Logs service — the OWNING service's own API, the
    /// one a client uses when it is not going through Cloud Control API.
    pub(crate) async fn logs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            LOGS_SIGNING_NAME,
            "application/x-amz-json-1.1",
            &format!("{LOGS_TARGET_PREFIX}.{operation}"),
            body,
        )
        .await
    }

    /// One operation of the CloudWatch Logs service, required to succeed.
    pub(crate) async fn logs_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// One operation of the Amazon SQS service — the OWNING service's own API, the one a
    /// client uses when it is not going through Cloud Control API.
    pub(crate) async fn sqs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.post(
            SQS_SIGNING_NAME,
            "application/x-amz-json-1.0",
            &format!("{SQS_TARGET_PREFIX}.{operation}"),
            body,
        )
        .await
    }

    /// One operation of the Amazon SQS service, required to succeed.
    pub(crate) async fn sqs_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.sqs(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// One request to the Amazon S3 service's own API — the OWNING service's own API, the
    /// one a client uses when it is not going through Cloud Control API.
    ///
    /// S3 speaks restXml over a REST surface rather than a JSON RPC one, so a call is a
    /// method, a path (`/<bucket>` for a bucket, `/<bucket>/<key>` for an object) and a
    /// media type, and the answer is a status and a body of bytes.
    pub(crate) async fn s3(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
    ) -> (u16, Vec<u8>) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{SIGNING_REGION}/\
             {S3_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{S3_ENDPOINT_HOST}{path}"))
            .header("host", S3_ENDPOINT_HOST)
            .header("content-type", content_type)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.body().as_ref().to_vec(),
        )
    }

    /// One request to that service about a BUCKET or one of its subresources, as a
    /// `(status, body as text)` pair — which is how an XML document and an error document
    /// alike are read.
    pub(crate) async fn s3_bucket_call(&self, method: &str, path: &str) -> (u16, String) {
        let (status, answered) = self.s3(method, path, "application/xml", b"").await;
        (status, String::from_utf8_lossy(&answered).to_string())
    }

    /// One signed `POST /` through the router, as a `(status, body)` pair.
    async fn post(
        &self,
        signing_name: &str,
        content_type: &str,
        target: &str,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{SIGNING_REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-date;x-amz-target, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.localhost"))
            .header("content-type", content_type)
            .header("x-amz-target", target)
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// One operation of this service, required to succeed.
    pub(crate) async fn cc_ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.cc(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded:?}"
        );
        decoded
    }

    /// One `CreateResource` request body, handed back rather than posted — which is what
    /// sending the SAME request twice, as judging a repeated `ClientToken` takes, needs.
    pub(crate) fn create_request(
        type_name: &str,
        desired_state: &Value,
        client_token: Option<&str>,
    ) -> Value {
        let mut request = Map::new();
        request.insert("TypeName".to_string(), json!(type_name));
        // `DesiredState` is a JSON blob carried as a string member.
        request.insert("DesiredState".to_string(), json!(desired_state.to_string()));
        if let Some(token) = client_token {
            request.insert("ClientToken".to_string(), json!(token));
        }
        Value::Object(request)
    }

    /// `CreateResource`, as a `(status, body)` pair.
    pub(crate) async fn create(
        &self,
        type_name: &str,
        desired_state: &Value,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        self.cc(
            "CreateResource",
            Self::create_request(type_name, desired_state, client_token),
        )
        .await
    }

    /// `CreateResource`, required to succeed, answering the `ProgressEvent` it reports.
    pub(crate) async fn create_ok(
        &self,
        type_name: &str,
        desired_state: &Value,
        client_token: Option<&str>,
    ) -> Value {
        let created = self
            .cc_ok(
                "CreateResource",
                Self::create_request(type_name, desired_state, client_token),
            )
            .await;
        progress(&created)
    }

    /// The `ProgressEvent` `GetResourceRequestStatus` reports for `request_token`, as a
    /// `(status, body)` pair.
    pub(crate) async fn status(&self, request_token: &str) -> (u16, Value) {
        self.cc(
            "GetResourceRequestStatus",
            json!({ "RequestToken": request_token }),
        )
        .await
    }

    /// Wait until the request `request_token` names has completed, answering the
    /// `ProgressEvent` it reports then.
    pub(crate) async fn settled(&self, request_token: &str) -> Value {
        let event = self.terminal(request_token).await;
        assert_eq!(
            text(&event, "OperationStatus"),
            STATUS_SUCCESS,
            "request {request_token:?}: expected the operation to succeed, got {event:?}"
        );
        event
    }

    /// Wait until the request `request_token` names has reached a TERMINAL status,
    /// answering the `ProgressEvent` it reports then — whichever status that is, so a
    /// case can assert on a request the owning handler failed.
    pub(crate) async fn terminal(&self, request_token: &str) -> Value {
        self.reaching(request_token, &[
            STATUS_SUCCESS,
            STATUS_FAILED,
            STATUS_CANCEL_COMPLETE,
        ])
        .await
    }

    /// Wait until the request `request_token` names reports one of `wanted`, answering the
    /// `ProgressEvent` it reports then.
    ///
    /// The wait is for the STATUS and never for a span: it re-reads until the condition
    /// holds or the generous deadline above passes.
    pub(crate) async fn reaching(&self, request_token: &str, wanted: &[&str]) -> Value {
        let started = Instant::now();
        loop {
            let (status, body) = self.status(request_token).await;
            assert_eq!(
                status, 200,
                "request {request_token:?}: expected HTTP 200 from \
                 GetResourceRequestStatus, got {status} with body {body:?}"
            );
            let event = progress(&body);
            let reached = text(&event, "OperationStatus");
            if wanted.contains(&reached.as_str()) {
                return event;
            }
            assert!(
                started.elapsed() < SETTLED_BUDGET,
                "request {request_token:?}: expected the operation to report one of \
                 {wanted:?} within {SETTLED_BUDGET:?}, it is still {reached:?}"
            );
            tokio::time::sleep(SETTLED_INTERVAL).await;
        }
    }

    /// `CancelResourceRequest`, as a `(status, body)` pair.
    pub(crate) async fn cancel(&self, request_token: &str) -> (u16, Value) {
        self.cc(
            "CancelResourceRequest",
            json!({ "RequestToken": request_token }),
        )
        .await
    }

    /// Create one resource and wait until it is provisioned, answering its primary
    /// identifier — the server-resolved one, never a value the case chose.
    pub(crate) async fn provision(&self, type_name: &str, desired_state: &Value) -> String {
        let accepted = self.create_ok(type_name, desired_state, None).await;
        let settled = self.settled(&text(&accepted, "RequestToken")).await;
        text(&settled, "Identifier")
    }

    /// `GetResource`, as a `(status, body)` pair.
    pub(crate) async fn get(&self, type_name: &str, identifier: &str) -> (u16, Value) {
        self.cc(
            "GetResource",
            json!({ "TypeName": type_name, "Identifier": identifier }),
        )
        .await
    }

    /// The resource model `GetResource` reports for one resource, parsed back from the
    /// JSON string the `Properties` member carries.
    pub(crate) async fn properties(&self, type_name: &str, identifier: &str) -> Value {
        let fetched = self.cc_ok(
            "GetResource",
            json!({ "TypeName": type_name, "Identifier": identifier }),
        )
        .await;
        properties_of(&fetched["ResourceDescription"])
    }

    /// `UpdateResource`, as a `(status, body)` pair.
    pub(crate) async fn update(
        &self,
        type_name: &str,
        identifier: &str,
        patch_document: &Value,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = Map::new();
        request.insert("TypeName".to_string(), json!(type_name));
        request.insert("Identifier".to_string(), json!(identifier));
        // `PatchDocument` is a JSON Patch document carried as a string member.
        request.insert(
            "PatchDocument".to_string(),
            json!(patch_document.to_string()),
        );
        if let Some(token) = client_token {
            request.insert("ClientToken".to_string(), json!(token));
        }
        self.cc("UpdateResource", Value::Object(request)).await
    }

    /// Apply one patch and wait until the resource carries it.
    pub(crate) async fn update_and_settle(
        &self,
        type_name: &str,
        identifier: &str,
        patch_document: &Value,
    ) -> Value {
        let (status, body) = self.update(type_name, identifier, patch_document, None).await;
        assert_eq!(
            status, 200,
            "UpdateResource {identifier:?}: expected HTTP 200, got {status} with body {body:?}"
        );
        let accepted = progress(&body);
        self.settled(&text(&accepted, "RequestToken")).await
    }

    /// `DeleteResource`, as a `(status, body)` pair.
    pub(crate) async fn delete(&self, type_name: &str, identifier: &str) -> (u16, Value) {
        self.delete_with_token(type_name, identifier, None).await
    }

    /// `DeleteResource` carrying `client_token`, as a `(status, body)` pair.
    pub(crate) async fn delete_with_token(
        &self,
        type_name: &str,
        identifier: &str,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut request = Map::new();
        request.insert("TypeName".to_string(), json!(type_name));
        request.insert("Identifier".to_string(), json!(identifier));
        if let Some(token) = client_token {
            request.insert("ClientToken".to_string(), json!(token));
        }
        self.cc("DeleteResource", Value::Object(request)).await
    }

    /// Delete one resource and wait until it is gone.
    pub(crate) async fn delete_and_settle(&self, type_name: &str, identifier: &str) -> Value {
        let (status, body) = self.delete(type_name, identifier).await;
        assert_eq!(
            status, 200,
            "DeleteResource {identifier:?}: expected HTTP 200, got {status} with body {body:?}"
        );
        let accepted = progress(&body);
        self.settled(&text(&accepted, "RequestToken")).await
    }

    /// `ListResources` for one window, as the service reports it.
    pub(crate) async fn list(
        &self,
        type_name: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> Value {
        self.cc_ok(
            "ListResources",
            Self::list_request(type_name, max_results, next_token, None),
        )
        .await
    }

    /// One `ListResources` request body.
    pub(crate) fn list_request(
        type_name: &str,
        max_results: Option<i64>,
        next_token: Option<&str>,
        resource_model: Option<&Value>,
    ) -> Value {
        let mut request = Map::new();
        request.insert("TypeName".to_string(), json!(type_name));
        if let Some(max_results) = max_results {
            request.insert("MaxResults".to_string(), json!(max_results));
        }
        if let Some(token) = next_token {
            request.insert("NextToken".to_string(), json!(token));
        }
        if let Some(model) = resource_model {
            request.insert("ResourceModel".to_string(), json!(model.to_string()));
        }
        Value::Object(request)
    }

    /// Every identifier `ListResources` reports for `type_name`, walking the pages at
    /// `max_results` per page, in the order the pages reported them.
    pub(crate) async fn walk(&self, type_name: &str, max_results: Option<i64>) -> Vec<String> {
        let mut identifiers = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let listed = self.list(type_name, max_results, token.as_deref()).await;
            identifiers.extend(descriptions(&listed).iter().map(|d| text(d, "Identifier")));
            match listed["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => return identifiers,
            }
            assert!(
                identifiers.len() <= 1_000,
                "listing {type_name:?}: the pages never ran out, got {identifiers:?}"
            );
        }
    }
}

/// The `ProgressEvent` member of a mutating operation's response.
pub(crate) fn progress(body: &Value) -> Value {
    body["ProgressEvent"].clone()
}

/// The `ResourceDescriptions` of a listing.
pub(crate) fn descriptions(listed: &Value) -> Vec<Value> {
    listed["ResourceDescriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The resource model one `ResourceDescription` carries, parsed back from the JSON
/// string its `Properties` member holds.
pub(crate) fn properties_of(description: &Value) -> Value {
    let rendered = text(description, "Properties");
    serde_json::from_str(&rendered).unwrap_or(Value::Null)
}

/// A string member, or `""` when absent — so a missing member fails an assertion rather
/// than panicking the harness.
pub(crate) fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The wire error code of an error body.
pub(crate) fn code(body: &Value) -> String {
    text(body, "__type")
}

/// The `message` an error body carries, under either casing the envelope may use.
pub(crate) fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// One form value as an `awsQuery` body carries it: everything outside RFC 3986's
/// unreserved set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`) percent-encoded, which is what
/// an AWS SDK escapes in a form value — so an ARN's `:` reaches the service as part of the
/// value rather than as syntax.
pub(crate) fn form_encoded(value: &str) -> String {
    const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(value, UNRESERVED).to_string()
}

/// The text of the first `<tag>…</tag>` element of `block`, with the entities an XML
/// envelope escapes decoded back — or `""` when it carries none.
///
/// The ONE reader of an `awsQuery` answer in this test target, so every suite that drives a
/// service that OWNS a resource type over that wire reads its envelope the same way.
pub(crate) fn element(block: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let Some(start) = block.find(&open) else {
        return String::new();
    };
    let rest = &block[start + open.len()..];
    match rest.find(&format!("</{tag}>")) {
        Some(end) => rest[..end]
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
        None => String::new(),
    }
}

/// The value an `awsQuery` answer reports for the map entry keyed `name`, or `""` when it
/// reports none: such a map is one `<entry><key>…</key><value>…</value></entry>` per entry.
pub(crate) fn map_entry(block: &str, name: &str) -> String {
    let mut rest = block;
    while let Some(start) = rest.find("<entry>") {
        rest = &rest[start + "<entry>".len()..];
        let Some(end) = rest.find("</entry>") else {
            break;
        };
        let (entry, remainder) = rest.split_at(end);
        rest = remainder;
        if element(entry, "key") == name {
            return element(entry, "value");
        }
    }
    String::new()
}
