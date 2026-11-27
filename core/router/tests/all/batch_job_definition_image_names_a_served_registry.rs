// SPDX-License-Identifier: CC0-1.0

//! The registry address a read of an AWS Batch job definition publishes, driven end to end
//! across the two crates that have to agree on it: Amazon ECR, which ASSIGNS the address a
//! repository is reached at, and AWS Batch, which HOLDS that address as the image of a
//! registered revision and reports it back.
//!
//! The rule under test: **an address this emulator publishes for a caller to reach is an
//! address this emulator SERVES, and a service reports an address a caller handed it
//! unchanged — so the image a job definition is read back with is the URI Amazon ECR
//! published for the repository it was registered against, and a client that splits that
//! value on the first `/`, dials the authority and asks for the path reaches the very
//! repository in this emulator.** `Repository$repositoryUri` is "The URI for the repository.
//! You can use this URI for container image `push` and `pull` operations", so the authority
//! has to lead back to this process: it names the registry that holds the repository — the
//! owning account, the fixed `dkr` and `ecr` labels and that registry's Region — over the zone
//! this emulator announces its own names in, which is the zone of the address it is reached
//! at, and never a zone AWS publishes its own endpoints in, which resolves at genuine AWS
//! infrastructure or nowhere near here. A consumer of
//! `ContainerProperties$image` ("The image used to start a container") PULLS what it reads, so
//! an address published there that addresses no repository of this registry takes the pull
//! nowhere while every other member of the definition looks healthy. The image itself is a
//! value the REGISTRATION supplied, so AWS Batch owes it back verbatim; what has to name this
//! registry's repository is the address Amazon ECR authored for it. The URL a `docker login`
//! is pointed at is the other half — an address this process is REACHED at — and each row
//! reads that it names no zone AWS publishes its own endpoints in.
//!
//! Beside the address, each row reads the member set the same response reports for the
//! container it describes: AWS Batch answers a `networkConfiguration` with its
//! interface-configuration list — present and EMPTY where the registration supplied no
//! entry — and a `containerProperties` carrying the two deprecated resource reservations
//! (`vcpus`, `memory`, both `@deprecated` "use `resourceRequirements` instead" and both
//! documented as unsupported on Fargate), so a client that diffs a revision against desired
//! state never sees one of those members move between a value and absent. The owning crate's
//! `tests/all/job_definition_container_member_set.rs` spans that member set on its own; what
//! is read here is the document a deployment really reads back — the one whose image names a
//! repository another crate assigned the address of.
//!
//! The rows differ along what the rule generalises over: the address this emulator is SERVED
//! at (an IPv4 socket, the loopback name with a port, and a host an operator chose with and
//! without a port), which the published registry address must not move with, the endpoint the
//! Amazon ECR call ARRIVED on
//! (this service's own endpoint host, and the registry
//! host AWS publishes), the Region the calls
//! are made in — whose registry the published authority names — the repository name (plain,
//! namespaced, and the punctuation
//! `RepositoryName`'s pattern allows), how the registration ADDRESSES the image (the bare URI,
//! which is the untagged form, a tag, a digest) and the `assignPublicIp` the Fargate
//! registration names. Every row DIALS the published authority, and the last test follows the
//! described image the way a container runtime does — it pulls the manifest at the address the
//! read handed it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

/// The account id this emulator mints every identity and every registry id from.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The `X-Amz-Target` prefix Amazon ECR's model fixes (service shape
/// `AmazonEC2ContainerRegistry_V20150921`, `sigv4Name: ecr`), on the awsJson1.1 protocol it
/// speaks.
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The DNS suffixes AWS publishes its OWN endpoints under: a name in one of them resolves at
/// genuine AWS infrastructure or nowhere near this process, so no address this emulator
/// publishes for a caller to dial carries one. Spelled out here rather than imported, so an
/// address slipping into one of them fails these rows.
const AWS_ENDPOINT_DNS_SUFFIXES: [&str; 4] =
    [".amazonaws.com", ".amazonaws.com.cn", ".api.aws", ".on.aws"];

/// The two members `ContainerProperties` reports whatever the registration supplied. Source:
/// `smithy_query(query="shape",shape="ContainerProperties",model_path="services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json")`
/// — the two `@deprecated` integer members, each documented as deprecated in favour of
/// `resourceRequirements` and as not supported for jobs running on Fargate.
const DEPRECATED_RESERVATIONS: [&str; 2] = ["vcpus", "memory"];

/// The media type a Docker v2 manifest is pushed and served under.
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of the config blob a Docker v2 manifest references.
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The media type of a layer blob a Docker v2 manifest references.
const DOCKER_LAYER_MEDIA_TYPE: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";

/// How a registration addresses the image inside the repository whose URI it was handed.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// The bare URI, carrying no tag at all — the untagged form.
    Bare,
    /// The URI followed by `:<tag>`.
    Tag(&'static str),
    /// The URI followed by `@<digest>`, the content-addressed form.
    Digest,
}

impl Addressed {
    /// What this addressing appends to the repository's URI.
    fn suffix(self, digest: &str) -> String {
        match self {
            Self::Bare => String::new(),
            Self::Tag(tag) => format!(":{tag}"),
            Self::Digest => format!("@{digest}"),
        }
    }

    /// The reference a registration writes for the repository published at `uri`.
    fn reference(self, uri: &str, digest: &str) -> String {
        format!("{uri}{}", self.suffix(digest))
    }
}

/// One deployment: where this emulator is served, where the Amazon ECR call arrives, the
/// Region the calls are made in, the repository the image names, how the registration
/// addresses it, and the public-IP assignment the Fargate registration states.
struct Row {
    case: &'static str,
    /// The address this emulator is SERVED at, which is where every address these services
    /// announce for a caller to dial comes from.
    served_base: &'static str,
    /// The `Host` the Amazon ECR calls of this row are addressed to — an endpoint this
    /// emulator does not answer on, so the address it publishes cannot be the one the call
    /// came in on.
    ecr_host: EcrHost,
    region: &'static str,
    repository: &'static str,
    addressed: Addressed,
    /// The `assignPublicIp` the registration's `networkConfiguration` names, or `None` where
    /// it names the structure with no member at all.
    assign_public_ip: Option<&'static str>,
}

/// The endpoint an Amazon ECR call is addressed to.
#[derive(Debug, Clone, Copy)]
enum EcrHost {
    /// The service's own endpoint host, `api.ecr.<region>.amazonaws.com`.
    ServiceEndpoint,
    /// The registry host AWS publishes for this account's registry in that Region.
    AwsRegistryHost,
}

impl EcrHost {
    /// The `Host` header such a call carries, in `region`.
    fn host(self, region: &str) -> String {
        match self {
            Self::ServiceEndpoint => format!("api.ecr.{region}.amazonaws.com"),
            Self::AwsRegistryHost => format!("{ACCOUNT}.dkr.ecr.{region}.amazonaws.com"),
        }
    }
}

/// The deployments the table drives.
const ROWS: &[Row] = &[
    Row {
        case: "an untagged image of a plain repository, served on an IPv4 socket",
        served_base: "https://127.0.0.1:36063",
        ecr_host: EcrHost::ServiceEndpoint,
        region: "us-west-2",
        repository: "batch-worker",
        addressed: Addressed::Bare,
        assign_public_ip: Some("ENABLED"),
    },
    Row {
        case: "a tagged image of a namespaced repository, served at a loopback name, created \
               through the registry host AWS publishes",
        served_base: "https://localhost:39355",
        ecr_host: EcrHost::AwsRegistryHost,
        region: "eu-central-1",
        repository: "team/batch-worker",
        addressed: Addressed::Tag("2026-01"),
        assign_public_ip: Some("DISABLED"),
    },
    Row {
        case: "a digest-addressed image, served at a host an operator chose",
        served_base: "https://registry.emulator.test:8443",
        ecr_host: EcrHost::ServiceEndpoint,
        region: "us-east-1",
        repository: "org/team/batch-worker",
        addressed: Addressed::Digest,
        assign_public_ip: None,
    },
    Row {
        case: "an image of a repository whose name carries the punctuation the pattern \
               allows, served at a host an operator chose carrying no port",
        served_base: "https://registry.emulator.test",
        ecr_host: EcrHost::AwsRegistryHost,
        region: "cn-north-1",
        repository: "a.b_c--d",
        addressed: Addressed::Tag("latest"),
        assign_public_ip: Some("ENABLED"),
    },
];

/// The emulator a row drives: Amazon ECR and AWS Batch on ONE engine, exactly as
/// `core/router/src/lib.rs` mounts them, SERVED at the row's own base — which is what
/// `OperationContext::endpoint` reports and so the address these services announce for a
/// caller to dial. Request authentication is off, as in every other suite here.
struct Emulator {
    router: MultiServiceRouter,
    region: &'static str,
    ecr_host: String,
}

impl Emulator {
    /// The emulator of `row`.
    fn new(row: &Row) -> Self {
        Self::at(row.served_base, row.region, row.ecr_host)
    }

    /// The emulator served at `served_base`, whose calls are made in `region` and whose
    /// Amazon ECR calls arrive on `ecr_host`.
    fn at(served_base: &str, region: &'static str, ecr_host: EcrHost) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.set_url_base(
            EndpointUrl::parse(served_base)
                .unwrap_or_else(|| panic!("the base URL {served_base} must parse")),
        );
        router.register_service(
            service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine),
        );
        Self {
            router,
            region,
            ecr_host: ecr_host.host(region),
        }
    }

    /// One Amazon ECR call on the awsJson1.1 wire, which has to be served.
    async fn ecr(&self, case: &str, operation: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", self.ecr_host.clone())
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{ECR_TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization("ecr", self.region))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {case:?}: expected ecr {operation} to answer 200, got {status} with \
             {answered}"
        );
        answered
    }

    /// One AWS Batch call on the restJson1 wire, which has to be served.
    async fn batch(&self, case: &str, path: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("host", format!("batch.{}.amazonaws.com", self.region))
            .header("content-type", "application/json")
            .header("authorization", authorization("batch", self.region))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {case:?}: expected batch {path} to answer 200, got {status} with {answered}"
        );
        answered
    }

    /// `GET <path>` at the registry address `authority`, presenting `token` the way a
    /// container client presents a `docker login` credential (HTTP Basic, which is what an
    /// Amazon ECR authorization token decodes to): the status and body a registry client
    /// would see.
    async fn registry_get(
        &self,
        authority: &str,
        path: &str,
        token: &str,
        accept: Option<&str>,
    ) -> (u16, String) {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(path)
            .header("host", authority)
            .header("authorization", format!("Basic {token}"));
        if let Some(media_type) = accept {
            request = request.header("accept", media_type);
        }
        let request = request
            .body(Bytes::new())
            .expect("a well-formed registry request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, String::from_utf8_lossy(response.body()).to_string())
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and which Region it is made in. The signature is never checked:
/// this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The authority of the address this emulator is served at, host and port: what a caller
/// reaches this process at, read off the served base itself rather than imported, so a zone
/// taken from anything else fails these rows.
fn served_authority(served_base: &str) -> &str {
    served_base
        .split_once("://")
        .map_or(served_base, |(_scheme, authority)| authority)
        .trim_end_matches('/')
}

/// The string at `pointer`, or a panic naming the case and the body that carried none.
fn string_at(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a string at {pointer}, got {body}"))
        .to_string()
}

/// The `host[:port]` and the remainder of `value`, split on the FIRST `/` — exactly how a
/// container client splits a registry address: the authority is what it dials and the
/// remainder names the repository, which may carry `/` as a namespace path of its own.
fn authority_and_path<'value>(case: &str, value: &'value str) -> (&'value str, &'value str) {
    value.split_once('/').unwrap_or_else(|| {
        panic!("case {case:?}: expected <authority>/<repository>, got {value:?}")
    })
}

/// Assert that `address` is published on the authority Amazon ECR assigns a repository when
/// this emulator is served at `served_base` — that served address, host and port whole, which
/// is no name AWS publishes its own endpoints in — and answer that authority.
///
/// An address a service hands a client is one the client DIALS, so it has to lead back to this
/// process; the composition itself is Amazon ECR's own rule and its own suites' subject
/// (`src/state/registry_address.rs`, `tests/all/repository_uri_registry_layout.rs`). What is
/// read here is the half a consumer of the address depends on, which is that dialling it
/// arrives at this emulator — which every row then does. Taken off the row's own served
/// address rather than imported, so an address built on anything else fails these rows.
fn assert_dialable_authority(case: &str, served_base: &str, address: &str) -> String {
    let (authority, _path) = authority_and_path(case, address);
    assert_eq!(
        authority,
        served_authority(served_base),
        "case {case:?}: expected {address:?} to be published at the address this emulator is \
         reached at, got {authority:?}"
    );
    authority.to_string()
}

/// Assert that `lowered` — the lower-cased authority of `address` — names no zone AWS
/// publishes its OWN endpoints in. Such a name resolves at genuine AWS infrastructure or
/// nowhere near this process, so a client handed one leaves the emulated environment carrying
/// the credential it is holding.
fn assert_no_aws_endpoint_zone(case: &str, address: &str, lowered: &str) {
    for suffix in AWS_ENDPOINT_DNS_SUFFIXES {
        assert!(
            !lowered.ends_with(suffix),
            "case {case:?}: {address:?} is published on {lowered:?}, a name under {suffix:?} \
             — a zone AWS publishes its own endpoints in, which this emulator does not answer \
             in"
        );
    }
}

/// The content digest of `content`: the `sha256:<hex>` form a registry addresses a blob by,
/// computed here independently of the emulator's own helper.
fn content_digest(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(content)))
}

/// The job-definition name a row registers, derived from its repository so no two rows share
/// one: the name's own pattern admits letters, numbers, hyphens and underscores only.
fn definition_name(repository: &str) -> String {
    format!(
        "jd-{}",
        repository.replace(['/', '.'], "-").replace('_', "-")
    )
}

/// Register a Fargate container job definition whose container runs `image`, and answer the
/// one revision a read reports for it — read TWICE, because a member set that healed or
/// decayed between two reads is not the one a client can diff against.
async fn register_and_describe(
    emulator: &Emulator,
    case: &str,
    name: &str,
    image: &str,
    assign_public_ip: Option<&str>,
) -> Value {
    let mut network = json!({});
    if let Some(value) = assign_public_ip {
        network["assignPublicIp"] = json!(value);
    }
    emulator
        .batch(
            case,
            "/v1/registerjobdefinition",
            &json!({
                "jobDefinitionName": name,
                "type": "container",
                "platformCapabilities": ["FARGATE"],
                "containerProperties": {
                    "image": image,
                    // "For jobs that run on Fargate resources, you must provide an execution
                    // role" (`ContainerProperties$executionRoleArn`).
                    "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/batch-execution"),
                    "resourceRequirements": [
                        {"type": "VCPU", "value": "0.25"},
                        {"type": "MEMORY", "value": "512"},
                    ],
                    "networkConfiguration": network,
                },
            }),
        )
        .await;
    let first = describe(emulator, case, name).await;
    let again = describe(emulator, case, name).await;
    assert_eq!(
        first, again,
        "case {case:?}: a later read must report the same document, got {first} then {again}"
    );
    first
}

/// The one revision `DescribeJobDefinitions` answers for `name`.
async fn describe(emulator: &Emulator, case: &str, name: &str) -> Value {
    let answered = emulator
        .batch(
            case,
            "/v1/describejobdefinitions",
            &json!({"jobDefinitionName": name}),
        )
        .await;
    let definitions = answered["jobDefinitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        definitions.len(),
        1,
        "case {case:?}: expected exactly one described revision of {name:?}, got {answered}"
    );
    definitions[0].clone()
}

/// Put `content` into `repository` as a layer, through the documented push sequence —
/// `InitiateLayerUpload`, `UploadLayerPart`, `CompleteLayerUpload` — and answer the digest the
/// registry registered it under, which is the digest a manifest references it by.
async fn given_layer(
    emulator: &Emulator,
    case: &str,
    repository: &str,
    content: &[u8],
) -> String {
    let opened = emulator
        .ecr(
            case,
            "InitiateLayerUpload",
            &json!({"repositoryName": repository}),
        )
        .await;
    let upload_id = string_at(case, &opened, "/uploadId");
    let last = i64::try_from(content.len()).expect("a test layer fits in an i64") - 1;
    emulator
        .ecr(
            case,
            "UploadLayerPart",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "partFirstByte": 0,
                "partLastByte": last,
                // A blob travels base64-encoded on the awsJson1.1 wire, with the codec the
                // wire serializer itself uses.
                "layerPartBlob": aws_smithy_types::base64::encode(content),
            }),
        )
        .await;
    let completed = emulator
        .ecr(
            case,
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [content_digest(content)],
            }),
        )
        .await;
    let registered = string_at(case, &completed, "/layerDigest");
    assert_eq!(
        registered,
        content_digest(content),
        "case {case:?}: expected the layer to be registered under its own content digest"
    );
    registered
}

/// Push one image into `repository` through the documented sequence and answer its manifest
/// and the digest the registry minted for it.
async fn given_image(
    emulator: &Emulator,
    case: &str,
    repository: &str,
    tag: &str,
) -> (String, String) {
    let config_bytes = format!("config of {repository}").into_bytes();
    let config = given_layer(emulator, case, repository, &config_bytes).await;
    let layer_bytes = format!("layer of {repository}").into_bytes();
    let layer = given_layer(emulator, case, repository, &layer_bytes).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": DOCKER_V2_MANIFEST,
        "config": {
            "mediaType": DOCKER_CONFIG_MEDIA_TYPE,
            "digest": config,
            "size": config_bytes.len(),
        },
        "layers": [{
            "mediaType": DOCKER_LAYER_MEDIA_TYPE,
            "digest": layer,
            "size": layer_bytes.len(),
        }],
    })
    .to_string();
    let put = emulator
        .ecr(
            case,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
        )
        .await;
    let digest = string_at(case, &put, "/image/imageId/imageDigest");
    (manifest, digest)
}

#[tokio::test]
async fn a_read_back_job_definition_names_the_registry_address_amazon_ecr_published() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new(row);

        // The address Amazon ECR assigns the repository, which is what the registration
        // below is fed.
        let created = emulator
            .ecr(
                case,
                "CreateRepository",
                &json!({"repositoryName": row.repository}),
            )
            .await;
        let uri = string_at(case, &created, "/repository/repositoryUri");
        let published = assert_dialable_authority(case, row.served_base, &uri);
        let (_authority, path) = authority_and_path(case, &uri);
        assert_eq!(
            path, row.repository,
            "case {case:?}: expected the repository's own name as the path of {uri:?}, got \
             {path:?}"
        );

        // The reference the registration writes on that address, and the revision a read
        // reports for it.
        let digest = content_digest(row.repository.as_bytes());
        let reference = row.addressed.reference(&uri, &digest);
        let name = definition_name(row.repository);
        let described =
            register_and_describe(&emulator, case, &name, &reference, row.assign_public_ip).await;
        let container = &described["containerProperties"];

        // What the registration supplied comes back verbatim — and so the address the read
        // publishes is the one Amazon ECR authored, which this emulator answers on.
        let image = container["image"].as_str().unwrap_or_default();
        assert_eq!(
            image, reference,
            "case {case:?}: expected the read to report the image the registration supplied, \
             got {described}"
        );
        let image_authority = assert_dialable_authority(case, row.served_base, image);
        assert_eq!(
            image_authority, published,
            "case {case:?}: expected the read to publish the authority Amazon ECR assigned \
             the repository ({published:?}), got {image:?}"
        );
        let (_authority, repository_path) = authority_and_path(case, image);
        let expected_path = format!("{}{}", row.repository, row.addressed.suffix(&digest));
        assert_eq!(
            repository_path, expected_path,
            "case {case:?}: expected the image to address {expected_path:?} at \
             {published:?}, got {image:?}"
        );

        // The member set the same response reports for that container: the
        // interface-configuration list present and empty, the stated public-IP assignment,
        // and both deprecated reservations present.
        let network = &container["networkConfiguration"];
        assert_eq!(
            network.get("interfaceConfigurations"),
            Some(&json!([])),
            "case {case:?}: expected the interface-configuration list to be reported empty, \
             got {network}"
        );
        assert_eq!(
            network.get("assignPublicIp").and_then(Value::as_str),
            row.assign_public_ip,
            "case {case:?}: expected assignPublicIp {:?}, got {network}",
            row.assign_public_ip
        );
        for member in DEPRECATED_RESERVATIONS {
            assert_eq!(
                container.get(member),
                Some(&Value::Null),
                "case {case:?}: expected {member} to be reported for a Fargate container that \
                 reserved none, got {container}"
            );
        }

        // What an address is FOR: dialled as it was handed over, with the credential a
        // `docker login` of this caller is issued, it is this emulator's own registry that
        // answers at the address the described image names. The URL that `docker login` is
        // pointed at is Amazon ECR's own announcement, dialled by its own suites; what is
        // read of it here is only that it names no zone AWS serves.
        let issued = emulator.ecr(case, "GetAuthorizationToken", &json!({})).await;
        let token = string_at(case, &issued, "/authorizationData/0/authorizationToken");
        let proxy_endpoint = string_at(case, &issued, "/authorizationData/0/proxyEndpoint");
        let login_authority = proxy_endpoint
            .strip_prefix("https://")
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an https registry URL, got {proxy_endpoint:?}")
            })
            .to_ascii_lowercase();
        assert_no_aws_endpoint_zone(case, &proxy_endpoint, &login_authority);
        let (status, body) = emulator
            .registry_get(&image_authority, "/v2/", &token, None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the registry API must answer at the published address \
             {image_authority:?}, which the described image {image:?} names, got HTTP {status} \
             with {body:?}"
        );
    }
}

#[tokio::test]
async fn a_pull_at_the_image_a_job_definition_names_reaches_this_registry() {
    // What the rule is FOR: a consumer of the described image pulls it. So the manifest is
    // fetched at the very address the read handed over, split the way a container runtime
    // splits it, with the credential a `docker login` is issued — and the image this registry
    // holds is what answers.
    //
    // The rows differ along how the registration addresses the pushed image (by tag, by
    // digest) and along the repository name (plain, namespaced).
    let rows: [(&str, &str, Addressed); 3] = [
        ("by tag, a plain repository", "pulled-worker", Addressed::Tag("v1")),
        (
            "by tag, a namespaced repository",
            "team/pulled-worker",
            Addressed::Tag("v1"),
        ),
        ("by digest", "digest/pulled-worker", Addressed::Digest),
    ];
    let served_base = "https://127.0.0.1:36063";
    // The Region these calls are made in, stated once so the registration and the expectation
    // cannot drift apart.
    let region = "us-west-2";

    for (case, repository, addressed) in rows {
        let emulator = Emulator::at(served_base, region, EcrHost::ServiceEndpoint);
        let created = emulator
            .ecr(case, "CreateRepository", &json!({"repositoryName": repository}))
            .await;
        let uri = string_at(case, &created, "/repository/repositoryUri");
        let (manifest, digest) = given_image(&emulator, case, repository, "v1").await;

        let reference = addressed.reference(&uri, &digest);
        let name = definition_name(repository);
        let described =
            register_and_describe(&emulator, case, &name, &reference, Some("ENABLED")).await;
        let image = described["containerProperties"]["image"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            image, reference,
            "case {case:?}: expected the read to report the image the registration supplied, \
             got {described}"
        );

        // Split exactly as a runtime does: the authority it dials, then the repository and
        // the reference inside it.
        let authority = assert_dialable_authority(case, served_base, &image);
        let (_authority, addressed_repository) = authority_and_path(case, &image);
        let (repository_path, pulled) = match addressed {
            Addressed::Digest => addressed_repository
                .split_once('@')
                .unwrap_or_else(|| panic!("case {case:?}: {image:?} names no digest")),
            _ => addressed_repository
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("case {case:?}: {image:?} names no tag")),
        };
        assert_eq!(
            repository_path, repository,
            "case {case:?}: expected the image to name the repository it was registered \
             against, got {repository_path:?}"
        );

        let issued = emulator.ecr(case, "GetAuthorizationToken", &json!({})).await;
        let token = string_at(case, &issued, "/authorizationData/0/authorizationToken");
        let (status, body) = emulator
            .registry_get(
                &authority,
                &format!("/v2/{repository_path}/manifests/{pulled}"),
                &token,
                Some(DOCKER_V2_MANIFEST),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a pull at the described image {image:?} must reach this registry, \
             got HTTP {status} with {body:?}"
        );
        assert_eq!(
            body, manifest,
            "case {case:?}: expected the pushed manifest from {image:?}, got {body:?}"
        );
    }
}
