// SPDX-License-Identifier: CC0-1.0

//! The interface VPC endpoints a stack REFERENCES, driven end to end with the service that
//! owns them mounted beside this one.
//!
//! RULE: an `AccessEndpoints` entry does not describe an endpoint, it references one by the
//! `VpceId` of an interface VPC endpoint in the caller's account — so every write of that
//! reference (`CreateStack` and the `UpdateStack` that sets it) resolves the id against the
//! account's VPC endpoints and refuses, HTTP 400 `InvalidParameterValueException`, an id
//! the account has no endpoint for or one whose endpoint service is not AppStream's
//! streaming service. A refused write stores nothing: the name stays unknown to
//! `DescribeStacks` after a create, and a stack keeps the endpoints it already references
//! after an update.
//!
//! The rows differ along what the rule generalises over: WHICH endpoint service the id
//! points at (AppStream's streaming service, AppStream's own API service, a service that is
//! not AppStream at all) and whether the id resolves to an endpoint of the account at all
//! (one just minted, one minted and then deleted, a well-formed id nothing minted, a value
//! that is no endpoint id, and no id at all) — asserted through BOTH write paths.
//!
//! A VPC endpoint is an Amazon EC2 resource, so EC2 is mounted beside this service on ONE
//! engine and every id a row references was MINTED by it through its own front door: no
//! identifier written here can pass itself off as one the account holds. Nothing reaches
//! into either store.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::refusal_messages::{error_message, not_found_sentence};
use crate::stack_fleet_lifecycle::error_code;

/// The Region every request below is signed for, and the one the endpoint service names
/// these rows mint endpoints to carry.
const REGION: &str = "us-west-2";

/// The resource word AWS's refusal sentences use for a stack.
const STACK_KIND: &str = "stack";

/// The wire status this service's `ResourceNotFoundException` leaves with — the status
/// AppStream serves that refusal over, which the serializer answers for the class
/// (`crate::state::modelled_error_status` in the service crate) rather than taking the
/// shape's own `@httpError`. This suite asserts the ABSENCE of a stack, not the status of
/// the refusal that reports it.
const NOT_FOUND_STATUS: u16 = 400;

/// The `__type` AppStream answers a member value it cannot act on with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The CIDR block of the VPC these endpoints are created in, and of the subnet their
/// endpoint network interfaces sit in.
const VPC_BLOCK: &str = "10.42.0.0/16";
const SUBNET_BLOCK: &str = "10.42.1.0/24";

/// The endpoint type an access endpoint references: a VPC endpoint reached through endpoint
/// network interfaces, as `VpcEndpointType`'s `Interface` member spells it.
fn interface() -> ::aws_sdk_ec2::types::VpcEndpointType {
    ::aws_sdk_ec2::types::VpcEndpointType::Interface
}

/// An endpoint id of the shape EC2 mints (`vpce-` and 17 hex digits) that nothing in this
/// account ever minted, so it resolves to no endpoint.
const NEVER_MINTED: &str = "vpce-00000000000000000";

/// A `VpceId` that is not an endpoint id at all. The member is a plain string with
/// `@length min 1` (`smithy_query(query="shape", shape="AccessEndpoint")`), so it reaches
/// the resolution rather than being refused at the boundary — and resolves to nothing.
const NOT_AN_ENDPOINT_ID: &str = "the-endpoint-i-meant";

/// AWS's sentence for a `VpceId` the account has no VPC endpoint for. Written out here,
/// independently of the service, so the assertion pins the text a client reads.
fn not_valid_sentence(vpce_id: &str) -> String {
    format!(
        "The VPC endpoint {vpce_id} is not valid. Specify a valid VPC endpoint that exists \
         in your AWS account, and try again."
    )
}

/// AWS's sentence for a `VpceId` that names an endpoint to some other endpoint service.
/// Entirely fixed text; the two spaces after the first sentence are AWS's own.
const NOT_FOR_APPSTREAM_SENTENCE: &str =
    "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the \
     endpoint you specify corresponds to the AppStream 2.0 service, and try again.";

/// One emulator: a shared engine with this service and the owner of VPC endpoints mounted
/// on it, and no signature checking (these cases assert service behaviour, not SigV4).
///
/// Its own harness rather than the single-service one the sibling suites drive
/// (`crate::stack_fleet_lifecycle::Emulator`): what this suite exercises is the hop into
/// the service that owns the referenced resource, which needs that service mounted.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_appstream_photonadminproxyservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// An EC2 client over the in-process callback — the very seam this service's own
    /// resolution reaches EC2 through, so an endpoint minted here is minted exactly where a
    /// create will look for it.
    fn ec2(&self) -> ::aws_sdk_ec2::Client {
        let callback = EmbeddedEmulator::from_router(self.router.clone());
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// Send one AppStream operation through the awsJson1_1 front door and return
    /// `(status, body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("PhotonAdminProxyService.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/appstream/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Send an AppStream operation that must succeed, returning its body.
    async fn ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} {payload}"
        );
        payload
    }
}

/// The endpoint service name of `label` in [`REGION`], the way EC2 publishes one:
/// `com.amazonaws.<region>.<label>`.
fn endpoint_service(label: &str) -> String {
    format!("com.amazonaws.{REGION}.{label}")
}

/// A VPC and a subnet of the account, minted through EC2's own front door.
async fn network(ec2: &::aws_sdk_ec2::Client) -> String {
    let vpc = ec2
        .create_vpc()
        .cidr_block(VPC_BLOCK)
        .send()
        .await
        .expect("EC2 must create the VPC the endpoints sit in")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();
    ec2.create_subnet()
        .vpc_id(&vpc)
        .cidr_block(SUBNET_BLOCK)
        .send()
        .await
        .expect("EC2 must create the subnet the endpoint interfaces sit in")
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .expect("CreateSubnet must report the subnet it minted")
        .to_string();
    vpc
}

/// Mint one interface VPC endpoint to the endpoint service `label` and return its id.
async fn endpoint(ec2: &::aws_sdk_ec2::Client, vpc: &str, label: &str) -> String {
    let service_name = endpoint_service(label);
    ec2.create_vpc_endpoint()
        .vpc_id(vpc)
        .vpc_endpoint_type(interface())
        .service_name(&service_name)
        // A VPC you create has DNS hostnames off, which a private hosted zone needs; these
        // endpoints are only ids for the stack's access endpoints to resolve, so they ask for
        // no private DNS.
        .private_dns_enabled(false)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must create an endpoint to {service_name}: {error:?}"))
        .vpc_endpoint()
        .and_then(::aws_sdk_ec2::types::VpcEndpoint::vpc_endpoint_id)
        .unwrap_or_else(|| panic!("CreateVpcEndpoint must report the endpoint it minted"))
        .to_string()
}

/// One row of the class: the `AccessEndpoints` list a write carries, and what the service
/// must answer — `None` for a write it must accept, or the sentence it must refuse with.
struct Row {
    case: String,
    access_endpoints: Value,
    refusal: Option<String>,
}

/// The rows, over a network whose endpoints EC2 minted: an id that resolves to an AppStream
/// streaming endpoint of the account, ids that resolve to an endpoint of another endpoint
/// service (AppStream's own API service, and a service that is not AppStream at all) and
/// ids that resolve to no endpoint (one deleted, one never minted, one that is no endpoint
/// id).
///
/// Every row's entry CARRIES an id, because that is what this suite resolves: an entry that
/// carries none references nothing to resolve and is refused on the request's own shape,
/// which `tests/all/access_endpoint_request_shape.rs` pins.
///
/// `EndpointType` is `STREAMING` throughout: it is the only member `AccessEndpointType`
/// declares (`smithy_query(query="shape", shape="AccessEndpointType")`), and each list holds
/// ONE entry, since a second entry would repeat that type.
async fn rows(emulator: &Emulator) -> Vec<Row> {
    let ec2 = emulator.ec2();
    let vpc = network(&ec2).await;
    let streaming = endpoint(&ec2, &vpc, "appstream.streaming").await;
    let api = endpoint(&ec2, &vpc, "appstream.api").await;
    let other_service = endpoint(&ec2, &vpc, "sts").await;
    let deleted = endpoint(&ec2, &vpc, "appstream.streaming").await;
    ec2.delete_vpc_endpoints()
        .vpc_endpoint_ids(&deleted)
        .send()
        .await
        .expect("EC2 must delete the endpoint this row references");

    let streaming_entry = |vpce_id: &str| json!([{ "EndpointType": "STREAMING", "VpceId": vpce_id }]);
    vec![
        Row {
            case: "an endpoint to the AppStream streaming service".to_string(),
            access_endpoints: streaming_entry(&streaming),
            refusal: None,
        },
        Row {
            case: "an endpoint to AppStream's own API service".to_string(),
            access_endpoints: streaming_entry(&api),
            refusal: Some(NOT_FOR_APPSTREAM_SENTENCE.to_string()),
        },
        Row {
            case: "an endpoint to a service that is not AppStream".to_string(),
            access_endpoints: streaming_entry(&other_service),
            refusal: Some(NOT_FOR_APPSTREAM_SENTENCE.to_string()),
        },
        Row {
            case: "a streaming endpoint the account deleted".to_string(),
            access_endpoints: streaming_entry(&deleted),
            refusal: Some(not_valid_sentence(&deleted)),
        },
        Row {
            case: "a well-formed endpoint id nothing minted".to_string(),
            access_endpoints: streaming_entry(NEVER_MINTED),
            refusal: Some(not_valid_sentence(NEVER_MINTED)),
        },
        Row {
            case: "a value that is no endpoint id".to_string(),
            access_endpoints: streaming_entry(NOT_AN_ENDPOINT_ID),
            refusal: Some(not_valid_sentence(NOT_AN_ENDPOINT_ID)),
        },
    ]
}

/// Assert `operation` refused `body` with HTTP 400 `InvalidParameterValueException` and
/// `sentence`.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: Value,
    sentence: &str,
) {
    let (status, payload) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must refuse the endpoint with HTTP 400, got {status} \
         {payload}"
    );
    assert_eq!(
        error_code(&payload),
        INVALID_PARAMETER_VALUE,
        "case {case:?}: {operation} must answer {INVALID_PARAMETER_VALUE}, got {payload}"
    );
    assert_eq!(
        error_message(&payload),
        sentence,
        "case {case:?}: {operation} must state AWS's sentence for that refusal, got {payload}"
    );
    // And under the key a refusal of a shape this service's model does not declare carries
    // its sentence under — lower case, unlike the modelled error shapes' `Message`.
    assert_eq!(
        payload.get("message").and_then(Value::as_str),
        Some(sentence),
        "case {case:?}: {operation} must carry that sentence under `message`, got {payload}"
    );
}

/// The `AccessEndpoints` the stack `name` reports, read back through `DescribeStacks`.
async fn reported_endpoints(emulator: &Emulator, case: &str, name: &str) -> Value {
    let described = emulator
        .ok(case, "DescribeStacks", json!({ "Names": [name] }))
        .await;
    described["Stacks"][0]
        .get("AccessEndpoints")
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn create_stack_stores_only_a_reference_to_an_appstream_streaming_endpoint_of_the_account(
) {
    let emulator = Emulator::new();
    for (index, row) in rows(&emulator).await.into_iter().enumerate() {
        let name = format!("created-{index}");
        let request = json!({ "Name": &name, "AccessEndpoints": row.access_endpoints });

        match &row.refusal {
            None => {
                let created = emulator.ok(&row.case, "CreateStack", request).await;
                assert_eq!(
                    created["Stack"]["AccessEndpoints"], row.access_endpoints,
                    "case {:?}: the create must report the endpoints it accepted, got {created}",
                    row.case
                );
                assert_eq!(
                    reported_endpoints(&emulator, &row.case, &name).await,
                    row.access_endpoints,
                    "case {:?}: the stored stack must report the endpoints the create \
                     accepted",
                    row.case
                );
            }
            Some(sentence) => {
                refused(&emulator, &row.case, "CreateStack", request, sentence).await;

                // A refused create stores nothing: the name it asked for is addressable by
                // nobody.
                let (status, payload) = emulator
                    .call("DescribeStacks", json!({ "Names": [&name] }))
                    .await;
                assert_eq!(
                    status, NOT_FOUND_STATUS,
                    "case {:?}: a refused create must leave {name:?} unknown, got {status} \
                     {payload}",
                    row.case
                );
                assert_eq!(
                    error_message(&payload),
                    not_found_sentence(STACK_KIND, &name),
                    "case {:?}: the read of {name:?} must report it as missing, got {payload}",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn update_stack_resolves_the_endpoints_it_would_attach_and_keeps_the_stored_ones_when_it_refuses(
) {
    let emulator = Emulator::new();
    let rows = rows(&emulator).await;
    // The endpoints the stack starts out referencing: the row the service accepts, which is
    // what a refused update must leave in place.
    let attached = rows
        .iter()
        .find(|row| row.refusal.is_none())
        .map(|row| row.access_endpoints.clone())
        .expect("one row references an endpoint the account has");

    for (index, row) in rows.iter().enumerate() {
        let name = format!("updated-{index}");
        emulator
            .ok(
                &row.case,
                "CreateStack",
                json!({ "Name": &name, "AccessEndpoints": attached }),
            )
            .await;
        let request = json!({ "Name": &name, "AccessEndpoints": row.access_endpoints });

        match &row.refusal {
            None => {
                let updated = emulator.ok(&row.case, "UpdateStack", request).await;
                assert_eq!(
                    updated["Stack"]["AccessEndpoints"], row.access_endpoints,
                    "case {:?}: the update must report the endpoints it accepted, got \
                     {updated}",
                    row.case
                );
                assert_eq!(
                    reported_endpoints(&emulator, &row.case, &name).await,
                    row.access_endpoints,
                    "case {:?}: the stored stack must report the endpoints the update \
                     accepted",
                    row.case
                );
            }
            Some(sentence) => {
                refused(&emulator, &row.case, "UpdateStack", request, sentence).await;
                assert_eq!(
                    reported_endpoints(&emulator, &row.case, &name).await,
                    attached,
                    "case {:?}: a refused update must leave the endpoints the stack already \
                     references in place",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_write_is_served_where_the_owner_of_vpc_endpoints_cannot_be_asked() {
    // The one case this rule does NOT refuse: with no owner of VPC endpoints mounted
    // beside this service, "the account has no such endpoint" cannot be told apart from
    // "the owner of endpoints could not be asked", and only the first is a refusal — so the
    // single-service harness the sibling suites drive serves the very create the
    // multi-service one above refuses, which is also what keeps the refusals above a
    // property of the ENDPOINT rather than of the id's text.
    let single_service = crate::stack_fleet_lifecycle::Emulator::new();
    let case = "an unresolvable endpoint id, with no owner of VPC endpoints mounted";
    let access_endpoints = json!([{ "EndpointType": "STREAMING", "VpceId": NEVER_MINTED }]);
    let created = single_service
        .ok(
            case,
            "CreateStack",
            json!({ "Name": "served-without-ec2", "AccessEndpoints": &access_endpoints }),
        )
        .await;
    assert_eq!(
        created["Stack"]["AccessEndpoints"], access_endpoints,
        "case {case:?}: the create must store the reference it could not resolve, got \
         {created}"
    );
}

#[tokio::test]
async fn a_stack_may_reference_an_endpoint_that_is_still_coming_up() {
    // The rule resolves the REFERENCE, not the endpoint's lifecycle: an endpoint the
    // account has is one a stack may reference from the instant it is minted, so this case
    // writes the reference at the earliest instant it can — nothing is waited for.
    let emulator = Emulator::new();
    let ec2 = emulator.ec2();
    let vpc = network(&ec2).await;
    let just_minted = endpoint(&ec2, &vpc, "appstream.streaming").await;

    let case = "an endpoint minted in the same breath as the stack";
    let access_endpoints = json!([{ "EndpointType": "STREAMING", "VpceId": &just_minted }]);
    let created = emulator
        .ok(
            case,
            "CreateStack",
            json!({ "Name": "references-a-new-endpoint", "AccessEndpoints": &access_endpoints }),
        )
        .await;
    assert_eq!(
        created["Stack"]["AccessEndpoints"], access_endpoints,
        "case {case:?}: the create must report the endpoint it accepted, got {created}"
    );
}
