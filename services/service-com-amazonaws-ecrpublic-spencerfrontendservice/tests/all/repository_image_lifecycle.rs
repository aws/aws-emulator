// SPDX-License-Identifier: CC0-1.0

//! The ECR Public repository + image lifecycle, driven end to end over the wire.
//!
//! Every case here goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"), dispatched by the multi-service router. Nothing
//! reaches into the state struct, so a response that is not computed from persisted
//! state cannot pass.
//!
//! The tables below span the classes the implementation generalises over — every form
//! the modelled `repositoryName` pattern admits, every manifest shape and size, every
//! page size, every way an `ImageIdentifier` can name or fail to name an image — rather
//! than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::API_REGION;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`),
/// which is also this registry's `registryId`.
pub(crate) const REGISTRY_ID: &str = "000000000000";

/// The ARN prefix of every repository in this partition-wide (region-less) registry.
pub(crate) const ARN_PREFIX: &str = "arn:aws:ecr-public::000000000000:repository/";

/// The universal gallery prefix plus this registry's alias: the leading segments of
/// every `repositoryUri`.
const URI_PREFIX: &str = "public.ecr.aws/emulator/";

/// A router with just this service mounted on a fresh engine.
///
/// `pub(crate)` so a sibling suite in this test target drives the same front door
/// rather than standing up a second harness.
pub(crate) fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope
    // below still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
///
/// Signed for a Region this registry's whole surface — including the Region-scoped
/// catalog write — is served from (`state::API_REGION`), which is what every case but the
/// Region-gate suite wants.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    call_signed_for(router, operation, body, API_REGION).await
}

/// One `awsJson1_1` call whose SigV4 credential scope names `region`.
///
/// The credential scope is the only place a request carries the Region it addressed, and
/// its service field (`ecr-public`) is also what routes the request to this service
/// (`emulator_server::resolve_route_key`). `pub(crate)` so the Region-gate suite drives
/// the same front door rather than building a second request.
pub(crate) async fn call_signed_for(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDECRPUBLIC/20260101/{region}\
                 /ecr-public/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
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
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
pub(crate) async fn ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A call that must be refused with HTTP 400 and the named modelled error code.
pub(crate) async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    case: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400 {expected_code}, \
         got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
    assert!(
        wire_type.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {wire_type:?} in {parsed}"
    );
    parsed
}

/// A required string member of a (possibly nested) response object.
pub(crate) fn text(body: &Value, path: &[&str], case: &str) -> String {
    let mut cursor = body;
    for step in path {
        cursor = &cursor[*step];
    }
    cursor
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {path:?}, got {body}"))
        .to_string()
}

/// The `sha256:<hex>` digest of a manifest, computed independently of the emulator.
pub(crate) fn digest_of(manifest: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()))
}

/// The URL a stored logo payload is reported under, computed independently of the
/// emulator.
///
/// `RepositoryCatalogData$logoUrl` is "the URL that contains the logo that's associated
/// with the repository", so it exists only once a payload has been stored, and its last
/// segment is that payload's own sha256 digest: the same bytes always yield the same URL
/// and different bytes never do. The host is the endpoint THIS emulator is served at,
/// which for an in-process router is the unserved base
/// (`smithy_server_runtime::UNSERVED_URL_BASE`), and the path is the asset plane the
/// service serves the logo on, under the registry that published it.
///
/// `pub(crate)` so the sibling catalog-data suite asserts the same derivation instead of
/// keeping a second copy of it.
pub(crate) fn logo_url(repository: &str, blob: &[u8]) -> String {
    format!(
        "{}/_aws/ecr-public-logos/{REGISTRY_ID}/{repository}/{:x}",
        smithy_server_runtime::UNSERVED_URL_BASE,
        Sha256::digest(blob)
    )
}

/// Every `repositoryName` form the modelled pattern
/// `^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$` admits, plus both
/// published length bounds (`RepositoryName`, len 2..205). Source: the model, via
/// `smithy_query(query="operation", operation="CreateRepository")`.
fn valid_repository_names() -> Vec<String> {
    vec![
        // The shortest name the length bound admits.
        "ab".to_string(),
        "nginx".to_string(),
        "nginx-web-app".to_string(),
        "team.app".to_string(),
        "team_app".to_string(),
        "project-a/nginx-web-app".to_string(),
        "org/team/app".to_string(),
        "0mixed9/with.0-1_2".to_string(),
        // The longest name the length bound admits.
        "z".repeat(205),
    ]
}

#[tokio::test]
async fn create_repository_inserts_a_repository_every_valid_name_form_can_be_read_back() {
    let router = service();

    for name in valid_repository_names() {
        let case = format!("create+describe {name:?}");
        let created = ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            &case,
        )
        .await;

        let arn = text(&created, &["repository", "repositoryArn"], &case);
        let uri = text(&created, &["repository", "repositoryUri"], &case);
        assert_eq!(
            arn,
            format!("{ARN_PREFIX}{name}"),
            "case {case:?}: expected the repository ARN to name the repository, got {arn:?}"
        );
        assert_eq!(
            uri,
            format!("{URI_PREFIX}{name}"),
            "case {case:?}: expected the repository URI to be gallery-host/alias/name, \
             got {uri:?}"
        );
        assert_eq!(
            text(&created, &["repository", "registryId"], &case),
            REGISTRY_ID,
            "case {case:?}: expected the registry id of this account, got {created}"
        );
        let created_at = created["repository"]["createdAt"].as_f64().unwrap_or(0.0);
        assert!(
            created_at > 0.0,
            "case {case:?}: expected a populated createdAt, got {created}"
        );

        // The round trip: the reader returns exactly the object the create stored.
        let described = ok(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            &case,
        )
        .await;
        let repositories = described["repositories"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a repositories array, {described}"));
        assert_eq!(
            repositories.len(),
            1,
            "case {case:?}: expected exactly the named repository, got {described}"
        );
        assert_eq!(
            repositories[0], created["repository"],
            "case {case:?}: expected DescribeRepositories to report what CreateRepository \
             stored, got {described}"
        );

        // Re-creating the same name is refused, and leaves the stored repository alone.
        refused(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            "RepositoryAlreadyExistsException",
            &case,
        )
        .await;
    }

    // Every name created above is in the unfiltered listing exactly once.
    let all = ok(&router, "DescribeRepositories", json!({}), "all").await;
    let listed: Vec<String> = all["repositories"]
        .as_array()
        .expect("a repositories array")
        .iter()
        .map(|repository| repository["repositoryName"].as_str().unwrap().to_string())
        .collect();
    let mut expected = valid_repository_names();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected every created repository, in repositoryName order, got {listed:?}"
    );
}

#[tokio::test]
async fn a_repository_name_outside_the_modelled_constraint_is_an_invalid_parameter() {
    let router = service();

    // One row per way the modelled `repositoryName` constraint can be broken: each
    // published length bound exceeded by one, and the pattern broken in each position it
    // constrains (case, leading/trailing separator, doubled separator, empty namespace).
    let cases = [
        ("one character, below the 2..205 minimum", "a".to_string()),
        ("206 characters, above the maximum", "z".repeat(206)),
        ("upper case", "Nginx".to_string()),
        ("leading hyphen", "-nginx".to_string()),
        ("trailing dot", "nginx.".to_string()),
        ("doubled separator", "ng--inx".to_string()),
        ("trailing slash", "team/".to_string()),
        ("empty namespace", "team//app".to_string()),
        ("space", "my app".to_string()),
    ];

    for (case, name) in cases {
        refused(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            "InvalidParameterException",
            case,
        )
        .await;
        // The refusal stored nothing.
        refused(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            "InvalidParameterException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn describing_a_repository_that_was_never_created_is_not_found() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/present"}),
        "seed",
    )
    .await;

    for case in ["team/absent", "absent", "other/team/absent"] {
        refused(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [case]}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        refused(
            &router,
            "DescribeImages",
            json!({"repositoryName": case}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        refused(
            &router,
            "DeleteRepository",
            json!({"repositoryName": case}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        refused(
            &router,
            "BatchDeleteImage",
            json!({"repositoryName": case, "imageIds": [{"imageTag": "v1"}]}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
    }

    // A name that is absent alongside one that is present still fails: the subset is
    // exact, not best-effort.
    refused(
        &router,
        "DescribeRepositories",
        json!({"repositoryNames": ["team/present", "team/absent"]}),
        "RepositoryNotFoundException",
        "mixed subset",
    )
    .await;
}

#[tokio::test]
async fn create_repository_reports_the_catalog_data_it_stored() {
    let router = service();

    // `logoImageBlob` travels as base64 on awsJson1_1; these are the bytes 0x00 0x01
    // 0x02, so the logo URL the service derives from them is checkable here.
    let logo_bytes = [0u8, 1, 2];
    let created = ok(
        &router,
        "CreateRepository",
        json!({
            "repositoryName": "team/catalog",
            "catalogData": {
                "description": "a short description",
                "architectures": ["ARM 64", "x86-64"],
                "operatingSystems": ["Linux", "Windows"],
                "aboutText": "# about",
                "usageText": "docker pull",
                "logoImageBlob": "AAEC",
            },
        }),
        "catalog data",
    )
    .await;

    let catalog = &created["catalogData"];
    assert_eq!(
        catalog["description"], "a short description",
        "expected the stored description, got {catalog}"
    );
    assert_eq!(
        catalog["architectures"],
        json!(["ARM 64", "x86-64"]),
        "expected the stored architectures, got {catalog}"
    );
    assert_eq!(
        catalog["operatingSystems"],
        json!(["Linux", "Windows"]),
        "expected the stored operating systems, got {catalog}"
    );
    assert_eq!(
        catalog["aboutText"], "# about",
        "expected the stored about text, got {catalog}"
    );
    assert_eq!(
        catalog["usageText"], "docker pull",
        "expected the stored usage text, got {catalog}"
    );
    assert_eq!(
        catalog["marketplaceCertified"],
        Value::Null,
        "expected no Marketplace certification statement about a repository created \
         through the API, got {catalog}"
    );
    assert_eq!(
        catalog["logoUrl"].as_str().unwrap_or_default(),
        logo_url("team/catalog", &logo_bytes),
        "expected the logo URL to be derived from the logo payload, got {catalog}"
    );

    // A create that supplies no catalog data still reports the shape, with no logo.
    let bare = ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/bare"}),
        "no catalog data",
    )
    .await;
    assert_eq!(
        bare["catalogData"]["logoUrl"],
        Value::Null,
        "expected no logo URL for a repository with no logo payload, got {bare}"
    );
    assert_eq!(
        bare["catalogData"]["marketplaceCertified"],
        Value::Null,
        "expected marketplaceCertified to be absent, got {bare}"
    );
}

#[tokio::test]
async fn create_repository_refuses_more_tags_than_a_repository_may_carry() {
    let router = service();

    // `TooManyTagsException` documents the limit: "the maximum number of tags that can
    // be applied to a repository is 50". 50 is accepted, 51 is not.
    let tags = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|i| json!({"Key": format!("k{i}"), "Value": format!("v{i}")}))
                .collect(),
        )
    };

    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/fifty", "tags": tags(50)}),
        "50 tags",
    )
    .await;
    refused(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/fiftyone", "tags": tags(51)}),
        "TooManyTagsException",
        "51 tags",
    )
    .await;
    // A tag with no key names no category, so there is nothing to store.
    refused(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/keyless", "tags": [{"Value": "v"}]}),
        "InvalidTagParameterException",
        "tag without a key",
    )
    .await;
    // Neither refusal stored a repository.
    for name in ["team/fiftyone", "team/keyless"] {
        refused(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            "RepositoryNotFoundException",
            name,
        )
        .await;
    }
}

/// One manifest to push, and what the service should derive from it.
struct ManifestCase {
    case: &'static str,
    manifest: String,
    /// The `imageManifestMediaType` the manifest itself states, if any.
    manifest_media_type: Option<&'static str>,
    /// The `artifactMediaType` the manifest's `config` states, if any.
    artifact_media_type: Option<&'static str>,
    /// The `imageSizeInBytes` the blobs the manifest's descriptors name add up to, or
    /// `None` when the document references no sized content. Covered exhaustively by
    /// `tests/all/response_field_fidelity.rs`.
    image_size: Option<i64>,
}

/// Manifests spanning what a pushed document can be: an OCI image manifest carrying both
/// media types, a Docker manifest list carrying only its own, a JSON document carrying
/// neither, a JSON array rather than an object, and one large enough that its byte length
/// is not its character count by accident. (A body that is not JSON at all is refused
/// rather than stored — see `tests/all/json_document_members.rs`.)
fn manifest_cases() -> Vec<ManifestCase> {
    vec![
        ManifestCase {
            case: "oci image manifest with a config media type",
            manifest: json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "size": 7},
                "layers": [
                    {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "size": 12},
                    {"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip", "size": 34},
                ],
            })
            .to_string(),
            manifest_media_type: Some("application/vnd.oci.image.manifest.v1+json"),
            artifact_media_type: Some("application/vnd.oci.image.config.v1+json"),
            // The config blob (7) plus the two layers (12 and 34).
            image_size: Some(53),
        },
        ManifestCase {
            case: "docker manifest list, no config",
            manifest: json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
                "manifests": [],
            })
            .to_string(),
            manifest_media_type: Some(
                "application/vnd.docker.distribution.manifest.list.v2+json",
            ),
            artifact_media_type: None,
            image_size: None,
        },
        ManifestCase {
            case: "json with no media types at all",
            manifest: json!({"schemaVersion": 1, "name": "plain"}).to_string(),
            manifest_media_type: None,
            artifact_media_type: None,
            image_size: None,
        },
        ManifestCase {
            case: "a json array rather than an object",
            manifest: json!([{"schemaVersion": 2}]).to_string(),
            manifest_media_type: None,
            artifact_media_type: None,
            image_size: None,
        },
        ManifestCase {
            case: "large manifest with multi-byte characters",
            manifest: json!({
                "schemaVersion": 2,
                "note": "é".repeat(50_000),
            })
            .to_string(),
            manifest_media_type: None,
            artifact_media_type: None,
            image_size: None,
        },
    ]
}

#[tokio::test]
async fn put_image_digests_the_manifest_and_describe_images_reports_the_stored_bytes() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    for (index, manifest_case) in manifest_cases().into_iter().enumerate() {
        let case = manifest_case.case;
        let tag = format!("v{index}");
        let expected_digest = digest_of(&manifest_case.manifest);

        let pushed = ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": "team/app",
                "imageManifest": manifest_case.manifest,
                "imageTag": tag,
            }),
            case,
        )
        .await;

        assert_eq!(
            text(&pushed, &["image", "imageId", "imageDigest"], case),
            expected_digest,
            "case {case:?}: expected the sha256 of the manifest bytes, got {pushed}"
        );
        assert_eq!(
            text(&pushed, &["image", "imageId", "imageTag"], case),
            tag,
            "case {case:?}: expected the pushed tag, got {pushed}"
        );
        assert_eq!(
            text(&pushed, &["image", "imageManifest"], case),
            manifest_case.manifest,
            "case {case:?}: expected the manifest to be stored verbatim, got a different one"
        );
        assert_eq!(
            pushed["image"]["imageManifestMediaType"],
            manifest_case
                .manifest_media_type
                .map_or(Value::Null, |media| json!(media)),
            "case {case:?}: expected the manifest's own media type, got {}",
            pushed["image"]["imageManifestMediaType"]
        );

        // Read the image back: every reported detail comes from the stored manifest.
        let described = ok(
            &router,
            "DescribeImages",
            json!({
                "repositoryName": "team/app",
                "imageIds": [{"imageDigest": expected_digest}],
            }),
            case,
        )
        .await;
        let details = described["imageDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected imageDetails, got {described}"));
        assert_eq!(
            details.len(),
            1,
            "case {case:?}: expected exactly the filtered image, got {described}"
        );
        let detail = &details[0];
        assert_eq!(
            detail["imageDigest"], json!(expected_digest),
            "case {case:?}: expected the stored digest, got {detail}"
        );
        assert_eq!(
            detail["imageSizeInBytes"].as_i64(),
            manifest_case.image_size,
            "case {case:?}: expected the size of the layer content the manifest \
             references, got {detail}"
        );
        assert_eq!(
            detail["imageTags"], json!([tag]),
            "case {case:?}: expected the stored tag set, got {detail}"
        );
        assert_eq!(
            detail["repositoryName"], json!("team/app"),
            "case {case:?}: expected the owning repository, got {detail}"
        );
        assert_eq!(
            detail["registryId"], json!(REGISTRY_ID),
            "case {case:?}: expected this registry's id, got {detail}"
        );
        assert!(
            detail["imagePushedAt"].as_f64().unwrap_or(0.0) > 0.0,
            "case {case:?}: expected a populated imagePushedAt, got {detail}"
        );
        assert_eq!(
            detail["artifactMediaType"],
            manifest_case
                .artifact_media_type
                .map_or(Value::Null, |media| json!(media)),
            "case {case:?}: expected the artifact media type from the manifest config, \
             got {detail}"
        );
    }

    // Declaring `imageManifestMediaType` overrides what the manifest states.
    let declared = ok(
        &router,
        "PutImage",
        json!({
            "repositoryName": "team/app",
            "imageManifest": json!({"mediaType": "application/from-manifest"}).to_string(),
            "imageManifestMediaType": "application/from-request",
            "imageTag": "declared",
        }),
        "declared media type",
    )
    .await;
    assert_eq!(
        declared["image"]["imageManifestMediaType"],
        json!("application/from-request"),
        "expected the request's declared media type to win, got {declared}"
    );
}

#[tokio::test]
async fn put_image_checks_a_supplied_digest_and_refuses_a_duplicate_push() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    let manifest = json!({"schemaVersion": 2, "name": "one"}).to_string();
    let other = json!({"schemaVersion": 2, "name": "two"}).to_string();
    let digest = digest_of(&manifest);
    let other_digest = digest_of(&other);

    // A supplied digest that matches the bytes is accepted.
    let pushed = ok(
        &router,
        "PutImage",
        json!({
            "repositoryName": "team/app",
            "imageManifest": manifest,
            "imageDigest": digest,
            "imageTag": "v1",
        }),
        "matching digest",
    )
    .await;
    assert_eq!(
        text(&pushed, &["image", "imageId", "imageDigest"], "matching digest"),
        digest,
        "expected the supplied digest to be the stored one, got {pushed}"
    );

    // One that does not is refused, for every way it can differ.
    for (case, claimed) in [
        ("another manifest's digest", digest.clone()),
        (
            "a well-formed digest of nothing stored",
            format!("sha256:{}", "0".repeat(64)),
        ),
        ("a digest with no algorithm", "deadbeef".to_string()),
    ] {
        assert_ne!(
            claimed, other_digest,
            "case {case:?}: the test row must differ from the manifest's real digest"
        );
        refused(
            &router,
            "PutImage",
            json!({
                "repositoryName": "team/app",
                "imageManifest": other,
                "imageDigest": claimed,
                "imageTag": "mismatch",
            }),
            "ImageDigestDoesNotMatchException",
            case,
        )
        .await;
    }

    // Pushing the identical digest and tag again is a duplicate.
    refused(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": manifest, "imageTag": "v1"}),
        "ImageAlreadyExistsException",
        "same digest, same tag",
    )
    .await;
    // So is pushing it with no tag at all.
    refused(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": manifest}),
        "ImageAlreadyExistsException",
        "same digest, no tag",
    )
    .await;

    // But the same digest under a NEW tag adds that tag to the stored image.
    ok(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": manifest, "imageTag": "latest"}),
        "same digest, new tag",
    )
    .await;
    let described = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app", "imageIds": [{"imageDigest": digest}]}),
        "two tags",
    )
    .await;
    assert_eq!(
        described["imageDetails"][0]["imageTags"],
        json!(["latest", "v1"]),
        "expected both tags on the one stored image, got {described}"
    );

    // Pushing an existing tag onto a different manifest moves the tag.
    ok(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": other, "imageTag": "latest"}),
        "tag moves",
    )
    .await;
    let moved = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app"}),
        "tag moves",
    )
    .await;
    let tags_by_digest: Vec<(String, Value)> = moved["imageDetails"]
        .as_array()
        .expect("imageDetails")
        .iter()
        .map(|detail| {
            (
                detail["imageDigest"].as_str().unwrap().to_string(),
                detail["imageTags"].clone(),
            )
        })
        .collect();
    for (stored_digest, tags) in &tags_by_digest {
        if *stored_digest == digest {
            assert_eq!(
                *tags,
                json!(["v1"]),
                "expected the moved tag to leave the first manifest, got {moved}"
            );
        } else if *stored_digest == other_digest {
            assert_eq!(
                *tags,
                json!(["latest"]),
                "expected the moved tag on the second manifest, got {moved}"
            );
        }
    }
    assert_eq!(
        tags_by_digest.len(),
        2,
        "expected exactly the two pushed manifests, got {moved}"
    );
}

#[tokio::test]
async fn describe_images_resolves_an_identifier_by_digest_or_by_tag() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;
    let manifest = json!({"schemaVersion": 2, "name": "one"}).to_string();
    let digest = digest_of(&manifest);
    ok(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": manifest, "imageTag": "v1"}),
        "seed",
    )
    .await;

    // Every form of identifier that names the stored image resolves to it.
    for (case, id) in [
        ("by digest", json!({"imageDigest": digest})),
        ("by tag", json!({"imageTag": "v1"})),
        ("by digest and tag", json!({"imageDigest": digest, "imageTag": "v1"})),
    ] {
        let described = ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": "team/app", "imageIds": [id]}),
            case,
        )
        .await;
        assert_eq!(
            described["imageDetails"][0]["imageDigest"], json!(digest),
            "case {case:?}: expected the stored image, got {described}"
        );
    }

    // Every form that names nothing is not found.
    for (case, id) in [
        (
            "unknown digest",
            json!({"imageDigest": format!("sha256:{}", "1".repeat(64))}),
        ),
        ("unknown tag", json!({"imageTag": "nope"})),
        (
            "digest and a tag that points elsewhere",
            json!({"imageDigest": format!("sha256:{}", "1".repeat(64)), "imageTag": "v1"}),
        ),
    ] {
        refused(
            &router,
            "DescribeImages",
            json!({"repositoryName": "team/app", "imageIds": [id]}),
            "ImageNotFoundException",
            case,
        )
        .await;
    }

    // An identifier that names neither is an invalid parameter, not a not-found.
    refused(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app", "imageIds": [{}]}),
        "InvalidParameterException",
        "neither digest nor tag",
    )
    .await;

    // A repository with no images reports an empty list, not a failure.
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/empty"}),
        "empty",
    )
    .await;
    let empty = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/empty"}),
        "empty",
    )
    .await;
    assert_eq!(
        empty["imageDetails"],
        json!([]),
        "expected an empty imageDetails for a repository with no images, got {empty}"
    );
}

#[tokio::test]
async fn both_paginated_readers_walk_the_whole_set_exactly_once_at_every_page_size() {
    let router = service();

    // Eleven repositories and eleven images, so every page size from 1 to one past the
    // set size is exercised — including the sizes that divide the set exactly and the
    // ones that leave a short final page.
    let repository_names: Vec<String> = (0..11).map(|i| format!("team/app{i:02}")).collect();
    for name in &repository_names {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            name,
        )
        .await;
    }
    let mut image_digests = BTreeSet::new();
    for i in 0..11 {
        let manifest = json!({"schemaVersion": 2, "n": i}).to_string();
        image_digests.insert(digest_of(&manifest));
        ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": "team/app00",
                "imageManifest": manifest,
                "imageTag": format!("v{i:02}"),
            }),
            "seed image",
        )
        .await;
    }

    let expected_repositories: BTreeSet<String> = repository_names.iter().cloned().collect();

    for page_size in 1..=12 {
        // DescribeRepositories.
        let case = format!("DescribeRepositories maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let reply = ok(&router, "DescribeRepositories", body, &case).await;
            let items = reply["repositories"].as_array().expect("repositories");
            assert!(
                items.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} items on a page, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|item| item["repositoryName"].as_str().unwrap().to_string()),
            );
            pages += 1;
            assert!(
                pages <= 20,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match reply["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: expected no repository twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, expected_repositories,
            "case {case:?}: expected every repository exactly once, got {seen:?}"
        );

        // DescribeImages.
        let case = format!("DescribeImages maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"repositoryName": "team/app00", "maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let reply = ok(&router, "DescribeImages", body, &case).await;
            let items = reply["imageDetails"].as_array().expect("imageDetails");
            assert!(
                items.len() <= page_size as usize,
                "case {case:?}: expected at most {page_size} items on a page, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|item| item["imageDigest"].as_str().unwrap().to_string()),
            );
            pages += 1;
            assert!(
                pages <= 20,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match reply["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: expected no image twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, image_digests,
            "case {case:?}: expected every image exactly once, got {seen:?}"
        );
    }

    // `maxResults` is modelled `range 1..1000`: both bounds are served, and one past
    // either is refused.
    for (case, max_results) in [("minimum", 1), ("maximum", 1000)] {
        ok(
            &router,
            "DescribeRepositories",
            json!({"maxResults": max_results}),
            case,
        )
        .await;
    }
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1001)] {
        refused(
            &router,
            "DescribeRepositories",
            json!({"maxResults": max_results}),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // A token this service did not mint is an invalid parameter, on both readers.
    for case in ["not-hex", "zz", "ff"] {
        refused(
            &router,
            "DescribeRepositories",
            json!({"nextToken": case}),
            "InvalidParameterException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn batch_delete_image_removes_what_it_reports_and_reports_what_it_cannot() {
    let router = service();
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/app"}),
        "seed",
    )
    .await;

    // Two manifests: one carries a single tag, the other carries two.
    let single = json!({"schemaVersion": 2, "name": "single"}).to_string();
    let double = json!({"schemaVersion": 2, "name": "double"}).to_string();
    let single_digest = digest_of(&single);
    let double_digest = digest_of(&double);
    for (manifest, tags) in [
        (&single, vec!["only"]),
        (&double, vec!["first", "second"]),
    ] {
        for tag in tags {
            ok(
                &router,
                "PutImage",
                json!({
                    "repositoryName": "team/app",
                    "imageManifest": manifest,
                    "imageTag": tag,
                }),
                tag,
            )
            .await;
        }
    }

    // Removing one of two tags leaves the image, with the remaining tag.
    let reply = ok(
        &router,
        "BatchDeleteImage",
        json!({"repositoryName": "team/app", "imageIds": [{"imageTag": "first"}]}),
        "one of two tags",
    )
    .await;
    assert_eq!(
        reply["imageIds"],
        json!([{"imageDigest": double_digest, "imageTag": "first"}]),
        "expected the removed identifier to be reported, got {reply}"
    );
    assert_eq!(
        reply["failures"],
        json!([]),
        "expected no failures, got {reply}"
    );
    let described = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app", "imageIds": [{"imageDigest": double_digest}]}),
        "one of two tags",
    )
    .await;
    assert_eq!(
        described["imageDetails"][0]["imageTags"],
        json!(["second"]),
        "expected the image to survive with its remaining tag, got {described}"
    );

    // Removing the last tag deletes the image.
    ok(
        &router,
        "BatchDeleteImage",
        json!({"repositoryName": "team/app", "imageIds": [{"imageTag": "only"}]}),
        "last tag",
    )
    .await;
    refused(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app", "imageIds": [{"imageDigest": single_digest}]}),
        "ImageNotFoundException",
        "last tag",
    )
    .await;

    // Deleting by digest removes the image and every tag on it.
    let by_digest = ok(
        &router,
        "BatchDeleteImage",
        json!({"repositoryName": "team/app", "imageIds": [{"imageDigest": double_digest}]}),
        "by digest",
    )
    .await;
    assert_eq!(
        by_digest["imageIds"],
        json!([{"imageDigest": double_digest, "imageTag": "second"}]),
        "expected the deleted digest and the tag it carried to be reported, got {by_digest}"
    );
    let left = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app"}),
        "by digest",
    )
    .await;
    assert_eq!(
        left["imageDetails"],
        json!([]),
        "expected the repository to hold no images, got {left}"
    );
    refused(
        &router,
        "DescribeImages",
        json!({"repositoryName": "team/app", "imageIds": [{"imageTag": "second"}]}),
        "ImageNotFoundException",
        "tags went with the digest",
    )
    .await;

    // Every unmatched identifier is reported with its own failure code, never silently
    // ignored — one row per `ImageFailureCode` this operation can report.
    let present = json!({"schemaVersion": 2, "name": "present"}).to_string();
    let present_digest = digest_of(&present);
    let keeper = json!({"schemaVersion": 2, "name": "keeper"}).to_string();
    ok(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": present, "imageTag": "here"}),
        "re-seed",
    )
    .await;
    ok(
        &router,
        "PutImage",
        json!({"repositoryName": "team/app", "imageManifest": keeper, "imageTag": "stay"}),
        "re-seed",
    )
    .await;
    let unknown_digest = format!("sha256:{}", "2".repeat(64));
    let batch = ok(
        &router,
        "BatchDeleteImage",
        json!({
            "repositoryName": "team/app",
            "imageIds": [
                {"imageTag": "here"},
                {"imageTag": "absent"},
                {"imageDigest": unknown_digest},
                {},
                {"imageDigest": unknown_digest, "imageTag": "stay"},
            ],
        }),
        "mixed batch",
    )
    .await;
    assert_eq!(
        batch["imageIds"],
        json!([{"imageDigest": present_digest, "imageTag": "here"}]),
        "expected only the one identifier that matched, got {batch}"
    );
    let codes: Vec<String> = batch["failures"]
        .as_array()
        .expect("failures")
        .iter()
        .map(|failure| failure["failureCode"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        codes,
        vec![
            "MissingDigestAndTag",
            "ImageNotFound",
            "ImageNotFound",
            "ImageTagDoesNotMatchDigest",
        ],
        "expected one failure per unmatched identifier, the entry naming neither a tag \
         nor a digest first and the lookups after it in request order, got {batch}"
    );
    for failure in batch["failures"].as_array().expect("failures") {
        assert!(
            failure["failureReason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "expected every failure to carry a reason, got {failure}"
        );
    }
}

#[tokio::test]
async fn delete_repository_only_drops_images_when_the_caller_forces_it() {
    let router = service();

    // An empty repository deletes without force.
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/empty"}),
        "empty",
    )
    .await;
    let deleted = ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "team/empty"}),
        "empty",
    )
    .await;
    assert_eq!(
        deleted["repository"]["repositoryName"], json!("team/empty"),
        "expected the deleted repository to be reported, got {deleted}"
    );
    refused(
        &router,
        "DescribeRepositories",
        json!({"repositoryNames": ["team/empty"]}),
        "RepositoryNotFoundException",
        "empty",
    )
    .await;

    // A repository that still holds images needs force; every image count behaves the
    // same way.
    for (case, image_count) in [("one image", 1), ("three images", 3)] {
        let name = format!("team/held{image_count}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        for i in 0..image_count {
            ok(
                &router,
                "PutImage",
                json!({
                    "repositoryName": name,
                    "imageManifest": json!({"schemaVersion": 2, "n": i}).to_string(),
                    "imageTag": format!("v{i}"),
                }),
                case,
            )
            .await;
        }

        refused(
            &router,
            "DeleteRepository",
            json!({"repositoryName": name}),
            "RepositoryNotEmptyException",
            case,
        )
        .await;
        refused(
            &router,
            "DeleteRepository",
            json!({"repositoryName": name, "force": false}),
            "RepositoryNotEmptyException",
            case,
        )
        .await;
        // The refusal left the repository, and its images, where they were.
        let still = ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": name}),
            case,
        )
        .await;
        assert_eq!(
            still["imageDetails"].as_array().map(Vec::len),
            Some(image_count as usize),
            "case {case:?}: expected the images to survive the refused delete, got {still}"
        );

        let forced = ok(
            &router,
            "DeleteRepository",
            json!({"repositoryName": name, "force": true}),
            case,
        )
        .await;
        assert_eq!(
            forced["repository"]["repositoryName"], json!(name),
            "case {case:?}: expected the deleted repository, got {forced}"
        );
        refused(
            &router,
            "DescribeRepositories",
            json!({"repositoryNames": [name]}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        refused(
            &router,
            "DescribeImages",
            json!({"repositoryName": name}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_declared_caps() {
    use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
        MAX_IMAGES_PER_REPOSITORY, MAX_REPOSITORIES,
    };

    let router = service();

    // Filling the registry to its cap succeeds; one past it is refused with the
    // `LimitExceededException` CreateRepository models, and stores nothing.
    for i in 0..MAX_REPOSITORIES {
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": format!("team/r{i:05}")}),
            "fill repositories",
        )
        .await;
    }
    refused(
        &router,
        "CreateRepository",
        json!({"repositoryName": "team/one-too-many"}),
        "LimitExceededException",
        "one repository past the cap",
    )
    .await;
    refused(
        &router,
        "DescribeRepositories",
        json!({"repositoryNames": ["team/one-too-many"]}),
        "RepositoryNotFoundException",
        "one repository past the cap",
    )
    .await;

    // The same for images within one repository.
    let repository = "team/r00000";
    for i in 0..MAX_IMAGES_PER_REPOSITORY {
        ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": repository,
                "imageManifest": json!({"schemaVersion": 2, "n": i}).to_string(),
            }),
            "fill images",
        )
        .await;
    }
    let one_too_many = json!({"schemaVersion": 2, "n": "over"}).to_string();
    refused(
        &router,
        "PutImage",
        json!({"repositoryName": repository, "imageManifest": one_too_many}),
        "LimitExceededException",
        "one image past the cap",
    )
    .await;
    refused(
        &router,
        "DescribeImages",
        json!({
            "repositoryName": repository,
            "imageIds": [{"imageDigest": digest_of(&one_too_many)}],
        }),
        "ImageNotFoundException",
        "one image past the cap",
    )
    .await;
}

#[tokio::test]
async fn the_operations_this_slice_does_not_cover_answer_from_state() {
    let router = service();

    // This surface carries no scaffolded operation any longer, so a call naming a
    // repository this registry does not hold is answered from state — the modelled client
    // error — rather than the 501 `NotImplemented` that means "nobody has written it yet".
    // (`DescribeImageTags` left the stub list when it was implemented —
    // `tests/all/image_tag_details.rs` drives it now; `TagResource` left it the same way
    // (`tests/all/tag_resource.rs`), and `UntagResource` last
    // (`tests/all/untag_resource.rs`).)
    for (operation, body) in [
        (
            "UntagResource",
            json!({"resourceArn": format!("{ARN_PREFIX}team/app"), "tagKeys": ["env"]}),
        ),
        ("DescribeImageTags", json!({"repositoryName": "team/app"})),
    ] {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected an unknown repository to be reported as a \
             modelled client error, got {status} with {parsed}"
        );
    }
}
