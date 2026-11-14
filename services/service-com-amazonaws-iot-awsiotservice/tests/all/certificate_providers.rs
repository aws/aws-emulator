// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of CERTIFICATE PROVIDERS: `CreateCertificateProvider`, the
//! `DescribeCertificateProvider` reader that proves what it persisted,
//! `DeleteCertificateProvider`, and the rule that an operation has ONE account
//! default.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rules under test:
//!
//! * a provider is created under its name, with the function it calls and the
//!   operations it is the account's default for, and a later read reports that record
//!   member for member;
//! * an operation has ONE account default: a provider claiming one another provider
//!   holds is refused `InvalidRequestException` (400) with nothing created, and the
//!   claim is free again as soon as the provider holding it is deleted;
//! * a name the account holds no provider under is `ResourceNotFoundException` (404).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::CERTIFICATE_PROVIDER_OPERATIONS;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region this suite signs for; the ARN assertions read it back.
const REGION: &str = "us-west-1";
/// The function a provider calls: its form is the caller's, so one well-formed ARN is
/// used throughout.
const PROVIDER_FUNCTION: &str = "arn:aws:lambda:us-west-1:000000000000:function:issuer";

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the certificate-provider request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn create_provider(&self, name: &str, body: &Value) -> Reply {
        self.dispatch(
            "POST",
            &format!("/certificate-providers/{name}"),
            Some(body),
        )
        .await
    }

    async fn describe_provider(&self, name: &str) -> Reply {
        self.dispatch("GET", &format!("/certificate-providers/{name}"), None)
            .await
    }

    async fn delete_provider(&self, name: &str) -> Reply {
        self.dispatch("DELETE", &format!("/certificate-providers/{name}"), None)
            .await
    }

    /// The request body creating a provider that claims `operations`.
    fn claim(operations: &[&str]) -> Value {
        json!({
            "lambdaFunctionArn": PROVIDER_FUNCTION,
            "accountDefaultForOperations": operations,
        })
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }

    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// RULE: a created provider is read back member for member, under whichever name and
/// operation it claims.
#[tokio::test]
async fn a_created_certificate_provider_is_read_back_member_for_member() {
    // One row per operation a provider may be the account default for, which is the
    // catalog the model declares
    // (`smithy_query(query="shape",shape="CertificateProviderOperation")`, reached
    // through `CERTIFICATE_PROVIDER_OPERATIONS`): each is created in its own account,
    // because the account holds one default per operation.
    for operation in CERTIFICATE_PROVIDER_OPERATIONS {
        let emulator = Emulator::new();
        // The names span the form the model admits (`^[\w=,@-]+$`): a plain name, one
        // using every character class the pattern allows, and the shortest legal one.
        for name in ["issuer", "issuer_2-A=,@", "a"] {
            let created = emulator
                .create_provider(name, &Emulator::claim(&[operation]))
                .await;
            assert_eq!(
                created.status,
                200,
                "case {name} claiming {operation}: expected 200, got {} body {}",
                created.status,
                created.text()
            );
            let expected_arn =
                format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:certificateprovider/{name}");
            assert_eq!(
                created.json()["certificateProviderArn"],
                json!(expected_arn),
                "case {name}: expected the minted ARN, got {}",
                created.text()
            );

            let read = emulator.describe_provider(name).await;
            assert_eq!(
                read.status,
                200,
                "case {name}: expected the read to answer 200, got {} body {}",
                read.status,
                read.text()
            );
            let described = read.json();
            assert_eq!(
                described["certificateProviderName"],
                json!(name),
                "case {name}: the read must name the provider, got {described}"
            );
            assert_eq!(
                described["certificateProviderArn"], created.json()["certificateProviderArn"],
                "case {name}: the read must report the ARN the create minted, got {described}"
            );
            assert_eq!(
                described["lambdaFunctionArn"],
                json!(PROVIDER_FUNCTION),
                "case {name}: the read must report the stored function, got {described}"
            );
            assert_eq!(
                described["accountDefaultForOperations"],
                json!([operation]),
                "case {name}: the read must report the claim, got {described}"
            );
            assert!(
                described["creationDate"].is_number()
                    && described["lastModifiedDate"].is_number(),
                "case {name}: the read must report when the provider was created, got {described}"
            );

            // The claim is freed by the delete, which is what lets the next row take it.
            assert_eq!(
                emulator.delete_provider(name).await.status,
                200,
                "case {name}: expected the delete to answer 200"
            );
        }
    }
}

/// RULE: an operation has ONE account default — a second claim is refused and leaves
/// no provider behind, and the claim is reclaimable once the holder is deleted.
#[tokio::test]
async fn one_provider_at_a_time_holds_an_operations_account_default() {
    let operation = CERTIFICATE_PROVIDER_OPERATIONS
        .first()
        .expect("the model declares at least one certificate provider operation");
    let emulator = Emulator::new();

    let first = emulator
        .create_provider("holder", &Emulator::claim(&[operation]))
        .await;
    assert_eq!(
        first.status,
        200,
        "case the first claim: expected 200, got {} body {}",
        first.status,
        first.text()
    );

    // A second provider claiming the same operation is refused, and nothing is created
    // for it: the read of the name it asked for is a 404.
    let second = emulator
        .create_provider("contender", &Emulator::claim(&[operation]))
        .await;
    assert_eq!(
        second.status,
        400,
        "case a second claim: expected 400, got {} body {}",
        second.status,
        second.text()
    );
    assert_eq!(
        second.error_code(),
        "InvalidRequestException",
        "case a second claim: got {}",
        second.text()
    );
    assert_eq!(
        second.error_message(),
        format!(
            "Cannot set provider [contender] as default for operation {operation} because \
             another provider was set to be default for the operation"
        ),
        "case a second claim: got {}",
        second.text()
    );
    let missing = emulator.describe_provider("contender").await;
    assert_eq!(
        missing.status,
        404,
        "case a second claim: the refused create must leave no provider behind, got {} body {}",
        missing.status,
        missing.text()
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "case a second claim: got {}",
        missing.text()
    );
    assert_eq!(
        missing.error_message(),
        "Certificate provider with name [contender] not found",
        "case a second claim: got {}",
        missing.text()
    );
    // The provider that holds the claim is untouched.
    assert_eq!(
        emulator.describe_provider("holder").await.status,
        200,
        "case a second claim: the holder must be left as it was"
    );

    // The name is the provider's identity, so a repeat create under the holder's own
    // name is the conflict that names it, not the default-exclusivity refusal.
    let duplicate = emulator
        .create_provider("holder", &Emulator::claim(&[operation]))
        .await;
    assert_eq!(
        duplicate.status,
        409,
        "case a name already taken: expected 409, got {} body {}",
        duplicate.status,
        duplicate.text()
    );
    assert_eq!(
        duplicate.error_code(),
        "ResourceAlreadyExistsException",
        "case a name already taken: got {}",
        duplicate.text()
    );

    // Deleting the holder frees the claim: the create that was refused now succeeds.
    assert_eq!(
        emulator.delete_provider("holder").await.status,
        200,
        "case the holder deleted: expected 200"
    );
    let reclaimed = emulator
        .create_provider("contender", &Emulator::claim(&[operation]))
        .await;
    assert_eq!(
        reclaimed.status,
        200,
        "case the claim reclaimed: expected 200, got {} body {}",
        reclaimed.status,
        reclaimed.text()
    );
    assert_eq!(
        emulator.describe_provider("contender").await.status,
        200,
        "case the claim reclaimed: the new provider must be readable"
    );

    // A delete of a provider the account does not hold is the same 404 the read is.
    let deleted_twice = emulator.delete_provider("holder").await;
    assert_eq!(
        deleted_twice.status,
        404,
        "case deleting a provider the account does not hold: expected 404, got {} body {}",
        deleted_twice.status,
        deleted_twice.text()
    );
    assert_eq!(
        deleted_twice.error_message(),
        "Certificate provider with name [holder] not found",
        "case deleting a provider the account does not hold: got {}",
        deleted_twice.text()
    );
}
