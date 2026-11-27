// SPDX-License-Identifier: CC0-1.0

//! The federated identity an Azure Blob Storage or object storage transfer location
//! reaches its storage through, driven end to end over HTTP.
//!
//! `FederatedIdentity` is a request member of `CreateLocationAzureBlob`,
//! `CreateLocationObjectStorage` and `UpdateLocationObjectStorage`, and a response member
//! of the two matching describes, so what a create (or an update) stores is what the
//! describe reports back — nested members included, and only the members the request
//! named. Both members document the same dependency: the federated identity is what
//! DataSync uses INSTEAD of the location's long-term credential (a shared access
//! signature for Azure Blob Storage, an access key and secret key for object storage), so
//! a request naming both is refused.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router. Nothing reaches into the state struct, so a response that is not
//! computed from persisted state cannot pass.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// An IAM role DataSync exchanges a web identity token through, as the modelled
/// `IamRoleArn` spells one (`^arn:…:iam::[0-9]{12}:role/.*$`).
const AWS_IAM_ROLE: &str = "arn:aws:iam::000000000000:role/datasync-federated-identity";

/// A second one, so a case can prove an update replaces the identity rather than merging
/// into it.
const OTHER_AWS_IAM_ROLE: &str = "arn:aws:iam::000000000000:role/datasync-other-identity";

/// An Azure tenant id, in the modelled `AzureGuidString` shape (36 characters,
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const AZURE_TENANT_ID: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";

/// An Azure client id, in the same modelled shape.
const AZURE_CLIENT_ID: &str = "6fa459ea-ee8a-3ca4-894e-db77e160355e";

/// A shared access signature token, shaped like the one the modelled `AzureBlobSasToken`
/// member documents.
const SAS_TOKEN: &str = "sp=r&st=2026-01-01T00:00:00Z&se=2026-01-02T00:00:00Z&sv=2021-06-08&sr=c&sig=ZXhhbXBsZQ%3D%3D";

/// A router with just this service mounted on a fresh engine.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("FmrsService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{SIGNED_REGION}\
                 /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|_| {
            serde_json::json!({ "__unparsed": String::from_utf8_lossy(&payload) })
        })
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    case: &str,
) -> serde_json::Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 `InvalidRequestException` whose message is
/// `expected_message`.
async fn invalid_request(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
    expected_message: &str,
    case: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("InvalidRequestException"),
        "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
    );
    let message = parsed["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the refusal {expected_message:?} from {operation}, got {parsed}"
    );
}

/// The `LocationArn` a create answered with.
fn created_arn(created: &serde_json::Value, case: &str) -> String {
    created["LocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer a LocationArn, got {created}"))
        .to_string()
}

/// One Azure Blob Storage location this suite creates: the federated identity its request
/// carries (if any), and the authentication method it declares.
struct AzureRow {
    case: &'static str,
    container_url: &'static str,
    authentication_type: &'static str,
    federated_identity: Option<serde_json::Value>,
}

/// `CreateLocationAzureBlob` stores the `FederatedIdentity` its request names, and
/// `DescribeLocationAzureBlob` reports it back exactly as stored.
///
/// The rows span what the rule generalises over: a federated identity carrying both of
/// its members, each of them on its own (both are optional), and a location created
/// without one at all.
#[tokio::test]
async fn azure_blob_locations_report_the_federated_identity_they_hold() {
    let rows = [
        AzureRow {
            case: "an oidc location naming the role and the azure identity",
            container_url: "https://oidcstore.blob.core.windows.net/federated-in",
            authentication_type: "OIDC",
            federated_identity: Some(serde_json::json!({
                "AwsIamRole": AWS_IAM_ROLE,
                "AzureOidc": { "TenantId": AZURE_TENANT_ID, "ClientId": AZURE_CLIENT_ID },
            })),
        },
        AzureRow {
            case: "an oidc location naming only the role",
            container_url: "https://rolestore.blob.core.windows.net/federated-role",
            authentication_type: "OIDC",
            federated_identity: Some(serde_json::json!({ "AwsIamRole": AWS_IAM_ROLE })),
        },
        AzureRow {
            case: "an oidc location naming only the azure identity",
            container_url: "https://tenantstore.blob.core.windows.net/federated-tenant",
            authentication_type: "OIDC",
            federated_identity: Some(serde_json::json!({
                "AzureOidc": { "TenantId": AZURE_TENANT_ID, "ClientId": AZURE_CLIENT_ID },
            })),
        },
        AzureRow {
            case: "a location created with no federated identity at all",
            container_url: "https://openstore.blob.core.windows.net/public-container",
            authentication_type: "NONE",
            federated_identity: None,
        },
    ];

    let router = service();

    for row in &rows {
        let mut request = serde_json::json!({
            "ContainerUrl": row.container_url,
            "AuthenticationType": row.authentication_type,
        });
        if let Some(identity) = &row.federated_identity {
            request["FederatedIdentity"] = identity.clone();
        }

        let created = ok(&router, "CreateLocationAzureBlob", request, row.case).await;
        let arn = created_arn(&created, row.case);

        let described = ok(
            &router,
            "DescribeLocationAzureBlob",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;

        match &row.federated_identity {
            Some(identity) => assert_eq!(
                &described["FederatedIdentity"], identity,
                "case {:?}: expected the federated identity the request named, got {described}",
                row.case
            ),
            None => assert!(
                described["FederatedIdentity"].is_null(),
                "case {:?}: a location created with no federated identity reports no member, \
                 got {described}",
                row.case
            ),
        }
        assert_eq!(
            described["AuthenticationType"].as_str().unwrap_or_default(),
            row.authentication_type,
            "case {:?}: expected the authentication method the request named, got {described}",
            row.case
        );
        // A federated identity is not a credential, so no secret is kept for one.
        if row.federated_identity.is_some() {
            for member in ["ManagedSecretConfig", "CmkSecretConfig", "CustomSecretConfig"] {
                assert!(
                    described[member].is_null(),
                    "case {:?}: a federated location keeps no secret, got {member} in {described}",
                    row.case
                );
            }
        }

        // A read reports; it never changes what the next reader sees.
        let again = ok(
            &router,
            "DescribeLocationAzureBlob",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            again, described,
            "case {:?}: two reads of one location must agree, got {again} then {described}",
            row.case
        );
    }
}

/// One object storage location this suite creates: the federated identity its request
/// carries (if any), and the long-term access key it names instead.
struct ObjectStorageRow {
    case: &'static str,
    bucket_name: &'static str,
    access_key: Option<&'static str>,
    federated_identity: Option<serde_json::Value>,
}

/// `CreateLocationObjectStorage` stores the `FederatedIdentity` its request names, and
/// `DescribeLocationObjectStorage` reports it back exactly as stored.
///
/// The rows span what the rule generalises over: a federated identity carrying both of
/// its members (the nested Google identity with all four of its own), each of them on its
/// own, and a location created with the long-term access key instead.
#[tokio::test]
async fn object_storage_locations_report_the_federated_identity_they_hold() {
    let rows = [
        ObjectStorageRow {
            case: "a federated location naming the role and the google identity",
            bucket_name: "federated-bucket",
            access_key: None,
            federated_identity: Some(serde_json::json!({
                "AwsIamRole": AWS_IAM_ROLE,
                "ExternalIdentity": { "GoogleOidc": google_oidc() },
            })),
        },
        ObjectStorageRow {
            case: "a federated location naming only the role",
            bucket_name: "role-only-bucket",
            access_key: None,
            federated_identity: Some(serde_json::json!({ "AwsIamRole": AWS_IAM_ROLE })),
        },
        ObjectStorageRow {
            case: "a federated location naming only the google identity",
            bucket_name: "google-only-bucket",
            access_key: None,
            federated_identity: Some(serde_json::json!({
                "ExternalIdentity": { "GoogleOidc": google_oidc() },
            })),
        },
        ObjectStorageRow {
            case: "a location created with a long-term access key instead",
            bucket_name: "access-key-bucket",
            access_key: Some("AKIDOBJECTSTORAGE"),
            federated_identity: None,
        },
    ];

    let router = service();

    for row in &rows {
        let mut request = serde_json::json!({
            "ServerHostname": "object.storage.example.com",
            "BucketName": row.bucket_name,
        });
        if let Some(access_key) = row.access_key {
            request["AccessKey"] = serde_json::json!(access_key);
        }
        if let Some(identity) = &row.federated_identity {
            request["FederatedIdentity"] = identity.clone();
        }

        let created = ok(&router, "CreateLocationObjectStorage", request, row.case).await;
        let arn = created_arn(&created, row.case);

        let described = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;

        match &row.federated_identity {
            Some(identity) => assert_eq!(
                &described["FederatedIdentity"], identity,
                "case {:?}: expected the federated identity the request named, got {described}",
                row.case
            ),
            None => assert!(
                described["FederatedIdentity"].is_null(),
                "case {:?}: a location created with no federated identity reports no member, \
                 got {described}",
                row.case
            ),
        }
        assert_eq!(
            described["AccessKey"].as_str(),
            row.access_key,
            "case {:?}: expected the access key the request named, got {described}",
            row.case
        );

        // A read reports; it never changes what the next reader sees.
        let again = ok(
            &router,
            "DescribeLocationObjectStorage",
            serde_json::json!({ "LocationArn": arn }),
            row.case,
        )
        .await;
        assert_eq!(
            again, described,
            "case {:?}: two reads of one location must agree, got {again} then {described}",
            row.case
        );
    }
}

/// A Google Cloud Storage external identity, every member in the modelled shape:
/// `ProjectName` (len 6..30), `ProjectNumber` (digits), `IdentityPoolName` and
/// `IdentityProviderName` (len 4..32), all four `@required`.
fn google_oidc() -> serde_json::Value {
    serde_json::json!({
        "ProjectName": "example-project",
        "ProjectNumber": "123456789012",
        "IdentityPoolName": "datasync-pool",
        "IdentityProviderName": "datasync-provider",
    })
}

/// An update names the federated identity a location reaches its bucket through from then
/// on; an update naming none leaves the one the location already holds.
#[tokio::test]
async fn an_object_storage_update_replaces_the_federated_identity_it_names() {
    let router = service();
    let case = "a federated location updated twice";

    let created = ok(
        &router,
        "CreateLocationObjectStorage",
        serde_json::json!({
            "ServerHostname": "object.storage.example.com",
            "BucketName": "updated-bucket",
            "FederatedIdentity": { "AwsIamRole": AWS_IAM_ROLE },
        }),
        case,
    )
    .await;
    let arn = created_arn(&created, case);

    let replacement = serde_json::json!({
        "AwsIamRole": OTHER_AWS_IAM_ROLE,
        "ExternalIdentity": { "GoogleOidc": google_oidc() },
    });
    ok(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn, "FederatedIdentity": replacement }),
        case,
    )
    .await;

    let described = ok(
        &router,
        "DescribeLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn }),
        case,
    )
    .await;
    assert_eq!(
        described["FederatedIdentity"], replacement,
        "case {case:?}: expected the federated identity the update named, got {described}"
    );

    // An update that names no federated identity leaves the one the location holds.
    ok(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn, "ServerProtocol": "HTTP" }),
        case,
    )
    .await;
    let after = ok(
        &router,
        "DescribeLocationObjectStorage",
        serde_json::json!({ "LocationArn": arn }),
        case,
    )
    .await;
    assert_eq!(
        after["FederatedIdentity"], replacement,
        "case {case:?}: an update naming no federated identity leaves the stored one, got {after}"
    );
}

/// The refusal a request earns by naming a federated identity BESIDE the long-term
/// credential it stands in for.
///
/// The rows span what the rule generalises over: both location kinds, each of the
/// long-term credential members the rule names, and the create as well as the update.
#[tokio::test]
async fn naming_a_federated_identity_and_a_long_term_credential_is_refused() {
    let router = service();

    let azure_rule = "Provide either FederatedIdentity or SasConfiguration, but not both.";
    let object_storage_rule =
        "Provide either FederatedIdentity or the AccessKey/SecretKey credentials, but not both.";

    let azure_identity = serde_json::json!({
        "AwsIamRole": AWS_IAM_ROLE,
        "AzureOidc": { "TenantId": AZURE_TENANT_ID, "ClientId": AZURE_CLIENT_ID },
    });
    let object_storage_identity = serde_json::json!({ "AwsIamRole": AWS_IAM_ROLE });

    invalid_request(
        &router,
        "CreateLocationAzureBlob",
        serde_json::json!({
            "ContainerUrl": "https://bothstore.blob.core.windows.net/both-credentials",
            "AuthenticationType": "SAS",
            "SasConfiguration": { "Token": SAS_TOKEN },
            "FederatedIdentity": azure_identity,
        }),
        azure_rule,
        "an azure blob create naming a sas token and a federated identity",
    )
    .await;

    for (case, credential) in [
        (
            "an object storage create naming an access key and a federated identity",
            serde_json::json!({ "AccessKey": "AKIDOBJECTSTORAGE" }),
        ),
        (
            "an object storage create naming a secret key and a federated identity",
            serde_json::json!({ "SecretKey": "object-storage-secret-key" }),
        ),
        (
            "an object storage create naming both keys and a federated identity",
            serde_json::json!({
                "AccessKey": "AKIDOBJECTSTORAGE",
                "SecretKey": "object-storage-secret-key",
            }),
        ),
    ] {
        let mut request = serde_json::json!({
            "ServerHostname": "object.storage.example.com",
            "BucketName": "both-credentials-bucket",
            "FederatedIdentity": object_storage_identity,
        });
        for (member, value) in credential.as_object().expect("the credential is a map") {
            request[member] = value.clone();
        }
        invalid_request(
            &router,
            "CreateLocationObjectStorage",
            request,
            object_storage_rule,
            case,
        )
        .await;
    }

    // The same rule on the update, against a location that already exists.
    let case = "an object storage update naming an access key and a federated identity";
    let created = ok(
        &router,
        "CreateLocationObjectStorage",
        serde_json::json!({
            "ServerHostname": "object.storage.example.com",
            "BucketName": "refusal-bucket",
            "FederatedIdentity": object_storage_identity,
        }),
        case,
    )
    .await;
    let arn = created_arn(&created, case);
    invalid_request(
        &router,
        "UpdateLocationObjectStorage",
        serde_json::json!({
            "LocationArn": arn,
            "AccessKey": "AKIDOBJECTSTORAGE",
            "FederatedIdentity": object_storage_identity,
        }),
        object_storage_rule,
        case,
    )
    .await;

    // Nothing was stored by any refused create: the refused configurations are not
    // locations this account holds.
    let listed = ok(&router, "ListLocations", serde_json::json!({}), "the refusals").await;
    let locations = listed["Locations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        locations.len(),
        1,
        "case {:?}: only the location the update case created may exist, got {listed}",
        "the refusals"
    );
}
