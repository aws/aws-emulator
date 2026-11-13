// SPDX-License-Identifier: CC0-1.0

//! The shared harness every suite in this crate drives the service through: the
//! kafka service mounted in a `MultiServiceRouter` and called over HTTP exactly as
//! the emulator serves it, plus the request builders a suite writes its table rows
//! in (`ClusterRequest` for `CreateCluster`, `ConnectionRequest` for
//! `CreateVpcConnection`).
//!
//! One harness rather than one per suite, so every suite dispatches through the
//! same path and a new suite adds only its rule.

// A suite reaches for the part of the harness its rule needs, so an item only one
// suite uses is not dead code — it is this harness's surface.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts;
use service_com_amazonaws_kafka_kafka::state::state_gate::{
    not_in_accepted_state_message, CLUSTER_ARN_PARAMETER,
    CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM,
};
use service_com_amazonaws_kafka_kafka::state::{
    BROKER_REBOOT_DELAY, CHANNEL_ACTIVATION_DELAY, CHANNEL_UPDATE_DELAY, CLUSTER_ACTIVATION_DELAY,
    CLUSTER_UPDATE_DELAY, CONFIGURATION_ARN_NOT_FOUND_MESSAGE, CONFIGURATION_DELETION_DELAY,
    CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM, VPC_CONNECTION_ACTIVATION_DELAY,
    VPC_CONNECTION_REJECTION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The signing region every request below is scoped to; it is the region the
/// service must mint its ARNs and endpoints in.
pub const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = "000000000000";

/// The access key id the calls below sign with unless a suite names another.
///
/// It names no session and no IAM user, so a request carrying it speaks for the
/// account's own published credential — the caller every suite but an
/// authorization one drives the service as.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The kafka service mounted in a router, driven over HTTP exactly as the emulator
/// serves it.
pub struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service was mounted on, so a suite whose rule is about state
    /// another service OWNS (the IAM policies of the calling principal, the STS
    /// session its credential names) can write that state through that service's own
    /// store, as the handlers read it.
    pub engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer, parsed.
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// The wire error code (`__type` of the JSON error envelope), or `""` on success.
    pub fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A member of the response body, or `Value::Null` when absent.
    pub fn at(&self, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or(&Value::Null)
    }

    /// The error envelope's message, or `""` when there is none.
    pub fn error_message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}

impl Emulator {
    pub fn new() -> Self {
        Self::mounting(false)
    }

    /// The same emulator with AWS SECRETS MANAGER mounted beside this service on the
    /// SAME engine, as the production router mounts them.
    ///
    /// A cluster's SASL/SCRAM credentials live in Secrets Manager secrets, and which of
    /// them this service can act on is resolved against Secrets Manager rather than
    /// held here — so a suite about that rule needs the service that OWNS the secrets to
    /// be there to be asked, and mints every secret it names through that service's own
    /// `CreateSecret` ([`Emulator::secrets_manager`]).
    pub fn with_secrets_manager() -> Self {
        Self::mounting(true)
    }

    fn mounting(secrets_manager: bool) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the core's business, not this suite's: the
        // requests below carry a SigV4 scope so the region and route key resolve,
        // but they are not signed.
        router.set_request_authentication(false);
        if secrets_manager {
            router.register_service(
                service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Call one AWS Secrets Manager operation — `target` is its name, as the
    /// `X-Amz-Target` header of that service's awsJson1.1 wire carries it — in `region`.
    pub async fn secrets_manager(&self, region: &str, target: &str, body: Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("secretsmanager.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("secretsmanager.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={DEFAULT_ACCESS_KEY_ID}/20240101/{region}\
                     /secretsmanager/aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    pub async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.call_in(REGION, method, uri, body).await
    }

    /// The same call, signed for `region` — which is where the service must mint
    /// the ARNs and endpoints of anything it creates.
    pub async fn call_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        self.call_as_in(DEFAULT_ACCESS_KEY_ID, region, method, uri, body)
            .await
    }

    /// The same call, signed for `region` with `access_key_id` — the credential
    /// whose principal the service authorizes the request as.
    pub async fn call_as_in(
        &self,
        access_key_id: &str,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("kafka.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{region}/kafka/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    pub async fn create(&self, request: &ClusterRequest) -> Reply {
        self.call("POST", "/v1/clusters", Some(request.to_json())).await
    }

    /// `CreateClusterV2`, which the model binds to `POST /api/v2/clusters`, with
    /// `body` as given — the one call behind both cluster kinds the v2 create
    /// accepts.
    pub async fn create_cluster_v2(&self, body: Value) -> Reply {
        self.call("POST", "/api/v2/clusters", Some(body)).await
    }

    /// Create a SERVERLESS cluster through `CreateClusterV2`, assert it succeeded,
    /// and hand back the minted ARN. `vpc_configs` is one subnet list per
    /// `VpcConfig` the create submits and `iam` turns SASL/IAM client
    /// authentication on when the row names it, which is the whole `Serverless`
    /// body a suite needs to stand a serverless cluster up; the full
    /// serverless-create matrix lives in the `create_cluster_v2` suite.
    pub async fn create_serverless_ok(
        &self,
        name: &str,
        vpc_configs: &[Vec<&str>],
        iam: Option<bool>,
    ) -> String {
        let mut serverless = json!({
            "vpcConfigs": vpc_configs
                .iter()
                .map(|subnets| json!({ "subnetIds": subnets }))
                .collect::<Vec<Value>>(),
        });
        if let Some(enabled) = iam {
            serverless["clientAuthentication"] =
                json!({ "sasl": { "iam": { "enabled": enabled } } });
        }
        let reply = self
            .create_cluster_v2(json!({ "clusterName": name, "serverless": serverless }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected a serverless CreateClusterV2 to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {name:?}: a serverless CreateClusterV2 returned no clusterArn")
            })
            .to_string()
    }

    /// `DescribeClusterV2`, which the model binds to `GET /api/v2/clusters/{ClusterArn}`
    /// — the reader that reports a cluster's `ClusterType`.
    pub async fn describe_v2(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/api/v2/clusters/{}", pct(arn)), None)
            .await
    }

    pub async fn describe(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/v1/clusters/{}", pct(arn)), None)
            .await
    }

    pub async fn bootstrap_brokers(&self, arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v1/clusters/{}/bootstrap-brokers", pct(arn)),
            None,
        )
        .await
    }

    /// `CreateChannel` under `cluster_arn`, signed for `region`. The channel
    /// operations are bound under the cluster's own ARN, so every channel suite
    /// addresses them through these four rather than rebuilding the URIs.
    pub async fn create_channel_in(&self, region: &str, cluster_arn: &str, body: Value) -> Reply {
        self.call_in(
            region,
            "POST",
            &format!("/v1/clusters/{}/channels", pct(cluster_arn)),
            Some(body),
        )
        .await
    }

    /// `CreateChannel` in the suite's region.
    pub async fn create_channel(&self, cluster_arn: &str, body: Value) -> Reply {
        self.create_channel_in(REGION, cluster_arn, body).await
    }

    /// `DescribeChannel`, signed for `region` — a channel is addressable only
    /// under the cluster that holds it and only from that cluster's region.
    pub async fn describe_channel_in(
        &self,
        region: &str,
        cluster_arn: &str,
        channel_arn: &str,
    ) -> Reply {
        self.call_in(
            region,
            "GET",
            &format!(
                "/v1/clusters/{}/channels/{}",
                pct(cluster_arn),
                pct(channel_arn)
            ),
            None,
        )
        .await
    }

    /// `DescribeChannel` in the suite's region.
    pub async fn describe_channel(&self, cluster_arn: &str, channel_arn: &str) -> Reply {
        self.describe_channel_in(REGION, cluster_arn, channel_arn)
            .await
    }

    /// `ListChannels` under `cluster_arn`, signed for `region`, with `query`
    /// appended as given (`""` for a request carrying no query member).
    pub async fn list_channels_in(&self, region: &str, cluster_arn: &str, query: &str) -> Reply {
        self.call_in(
            region,
            "GET",
            &format!("/v1/clusters/{}/channels{query}", pct(cluster_arn)),
            None,
        )
        .await
    }

    /// `ListChannels` in the suite's region.
    pub async fn list_channels(&self, cluster_arn: &str, query: &str) -> Reply {
        self.list_channels_in(REGION, cluster_arn, query).await
    }

    pub async fn list(&self, query: &str) -> Reply {
        self.call("GET", &format!("/v1/clusters{query}"), None).await
    }

    /// `ListClustersV2`, which the model binds to `GET /api/v2/clusters`.
    pub async fn list_v2(&self, query: &str) -> Reply {
        self.list_v2_in(REGION, query).await
    }

    /// The same listing, as an endpoint for `region` serves it.
    pub async fn list_v2_in(&self, region: &str, query: &str) -> Reply {
        self.call_in(region, "GET", &format!("/api/v2/clusters{query}"), None)
            .await
    }

    /// Create `request`, assert it succeeded, and hand back the minted ARN.
    pub async fn create_ok(&self, request: &ClusterRequest) -> String {
        self.create_ok_in(REGION, request).await
    }

    /// The same create, signed for `region`, which is the region the cluster and
    /// everything addressed under it then live in.
    pub async fn create_ok_in(&self, region: &str, request: &ClusterRequest) -> String {
        let reply = self
            .call_in(region, "POST", "/v1/clusters", Some(request.to_json()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateCluster in {region} to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: CreateCluster returned no clusterArn"))
            .to_string()
    }

    /// A cluster Amazon MSK would admit a VPC connection to, settled: the target
    /// [`ClusterRequest::vpc_connection_target`] builds, created and then waited
    /// past its `CREATING` span, which is what MSK requires before any
    /// VPC-connection operation on it is served. Hands back the minted ARN.
    pub async fn vpc_connection_target(&self, name: &str) -> String {
        self.vpc_connection_target_in(REGION, name).await
    }

    /// The same target, created under `region`'s endpoint.
    pub async fn vpc_connection_target_in(&self, region: &str, name: &str) -> String {
        let arn = self
            .create_ok_in(region, &ClusterRequest::vpc_connection_target(name))
            .await;
        sleep_past_activation();
        arn
    }

    /// `CreateConfiguration`, which the model binds to
    /// `POST /v1/configurations`. `description` is `None` where the request
    /// omits the member; a blob member travels base64-encoded in a restJson1
    /// payload.
    pub async fn create_configuration(
        &self,
        name: &str,
        description: Option<&str>,
        server_properties: &[u8],
    ) -> Reply {
        let mut body = json!({
            "name": name,
            "serverProperties": ::aws_smithy_types::base64::encode(server_properties),
        });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        self.call("POST", "/v1/configurations", Some(body)).await
    }

    /// Create a configuration, assert it succeeded, and hand back the minted
    /// ARN.
    pub async fn create_configuration_ok(
        &self,
        name: &str,
        description: Option<&str>,
        server_properties: &[u8],
    ) -> String {
        let reply = self
            .create_configuration(name, description, server_properties)
            .await;
        assert_eq!(
            reply.status, 200,
            "case name={name:?}: expected CreateConfiguration to succeed, got {} {}",
            reply.status, reply.body
        );
        reply.body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case name={name:?}: CreateConfiguration returned no arn"))
            .to_string()
    }

    /// Create one [`ConfigurationRequest`] row under `region`'s endpoint, apply
    /// the further revisions the row carries, assert each call succeeded, and
    /// hand back the minted ARN — the row-shaped counterpart of
    /// [`Self::create_configuration_ok`], so every configuration suite mints a
    /// table row through one path.
    pub async fn create_configuration_row_ok_in(
        &self,
        region: &str,
        request: &ConfigurationRequest,
    ) -> String {
        let reply = self
            .call_in(region, "POST", "/v1/configurations", Some(request.to_json()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateConfiguration in {region} to succeed, got {} {}",
            reply.status, reply.body
        );
        let arn = reply.body["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request:?}: CreateConfiguration returned no arn"))
            .to_string();
        for body in &request.extra_revisions {
            let updated = self
                .update_configuration_in(region, &arn, None, body)
                .await;
            assert_eq!(
                updated.status, 200,
                "case {request:?}: expected UpdateConfiguration to succeed, got {} {}",
                updated.status, updated.body
            );
        }
        arn
    }

    /// `UpdateConfiguration`, which the model binds to
    /// `PUT /v1/configurations/{Arn}`.
    pub async fn update_configuration(
        &self,
        arn: &str,
        description: Option<&str>,
        server_properties: &[u8],
    ) -> Reply {
        self.update_configuration_in(REGION, arn, description, server_properties)
            .await
    }

    /// The same update, as an endpoint for `region` serves it — only the region
    /// the configuration lives in holds it.
    pub async fn update_configuration_in(
        &self,
        region: &str,
        arn: &str,
        description: Option<&str>,
        server_properties: &[u8],
    ) -> Reply {
        let mut body = json!({
            "serverProperties": ::aws_smithy_types::base64::encode(server_properties),
        });
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        self.call_in(
            region,
            "PUT",
            &format!("/v1/configurations/{}", pct(arn)),
            Some(body),
        )
        .await
    }

    /// `DescribeConfiguration`, which the model binds to
    /// `GET /v1/configurations/{Arn}`.
    pub async fn describe_configuration(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/v1/configurations/{}", pct(arn)), None)
            .await
    }

    /// `DescribeConfigurationRevision`, which the model binds to
    /// `GET /v1/configurations/{Arn}/revisions/{Revision}`.
    pub async fn describe_configuration_revision(&self, arn: &str, revision: i64) -> Reply {
        self.describe_configuration_revision_in(REGION, arn, revision)
            .await
    }

    /// The same read, as an endpoint for `region` serves it.
    pub async fn describe_configuration_revision_in(
        &self,
        region: &str,
        arn: &str,
        revision: i64,
    ) -> Reply {
        self.call_in(
            region,
            "GET",
            &format!("/v1/configurations/{}/revisions/{revision}", pct(arn)),
            None,
        )
        .await
    }

    /// `CreateVpcConnection`, which the model binds to `POST /v1/vpc-connection`.
    pub async fn create_vpc_connection(&self, request: &ConnectionRequest) -> Reply {
        self.call("POST", "/v1/vpc-connection", Some(request.to_json()))
            .await
    }

    /// `DescribeVpcConnection`, whose `Arn` is an `@httpLabel`.
    pub async fn describe_vpc_connection(&self, arn: &str) -> Reply {
        self.call("GET", &vpc_connection_uri(arn), None).await
    }

    /// The same read, as the endpoint of `region` serves it.
    pub async fn describe_vpc_connection_in(&self, region: &str, arn: &str) -> Reply {
        self.call_in(region, "GET", &vpc_connection_uri(arn), None)
            .await
    }

    /// `DeleteVpcConnection`, on the same resource URI.
    pub async fn delete_vpc_connection(&self, arn: &str) -> Reply {
        self.call("DELETE", &vpc_connection_uri(arn), None).await
    }

    /// `RejectClientVpcConnection`, which the model binds to
    /// `PUT /v1/clusters/{ClusterArn}/client-vpc-connection` with the
    /// connection's ARN in the body (`vpcConnectionArn`).
    pub async fn reject_client_vpc_connection(
        &self,
        cluster_arn: &str,
        vpc_connection_arn: &str,
    ) -> Reply {
        self.reject_client_vpc_connection_in(REGION, cluster_arn, vpc_connection_arn)
            .await
    }

    /// The same reject, as the endpoint of `region` serves it.
    pub async fn reject_client_vpc_connection_in(
        &self,
        region: &str,
        cluster_arn: &str,
        vpc_connection_arn: &str,
    ) -> Reply {
        self.call_in(
            region,
            "PUT",
            &format!("/v1/clusters/{}/client-vpc-connection", pct(cluster_arn)),
            Some(json!({ "vpcConnectionArn": vpc_connection_arn })),
        )
        .await
    }

    /// Create `request`, assert it succeeded, and hand back the minted ARN.
    pub async fn create_vpc_connection_ok(&self, request: &ConnectionRequest) -> String {
        let reply = self.create_vpc_connection(request).await;
        assert_eq!(
            reply.status, 200,
            "case {request:?}: expected CreateVpcConnection to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {request:?}: CreateVpcConnection returned no vpcConnectionArn")
            })
            .to_string()
    }

    /// `ListVpcConnections`, which the model binds to `GET /v1/vpc-connections`
    /// with `MaxResults` and `NextToken` as query parameters; `query` is the
    /// whole query string including its `?`, or empty.
    pub async fn list_vpc_connections(&self, query: &str) -> Reply {
        self.list_vpc_connections_in(REGION, query).await
    }

    /// The same listing, as the endpoint of `region` serves it.
    pub async fn list_vpc_connections_in(&self, region: &str, query: &str) -> Reply {
        self.call_in(region, "GET", &format!("/v1/vpc-connections{query}"), None)
            .await
    }

    /// `ListClientVpcConnections`, which the model binds to
    /// `GET /v1/clusters/{ClusterArn}/client-vpc-connections` with `MaxResults`
    /// and `NextToken` as query parameters; `query` is the whole query string
    /// including its `?`, or empty.
    pub async fn list_client_vpc_connections(&self, cluster_arn: &str, query: &str) -> Reply {
        self.list_client_vpc_connections_in(REGION, cluster_arn, query)
            .await
    }

    /// The same listing, as the endpoint of `region` serves it.
    pub async fn list_client_vpc_connections_in(
        &self,
        region: &str,
        cluster_arn: &str,
        query: &str,
    ) -> Reply {
        self.call_in(
            region,
            "GET",
            &format!(
                "/v1/clusters/{}/client-vpc-connections{query}",
                pct(cluster_arn)
            ),
            None,
        )
        .await
    }

    /// Create `request` under `region`'s endpoint, assert it succeeded, and hand
    /// back the minted ARN — the regional counterpart of
    /// [`Self::create_vpc_connection_ok`].
    pub async fn create_vpc_connection_ok_in(
        &self,
        region: &str,
        request: &ConnectionRequest,
    ) -> String {
        let reply = self
            .call_in(
                region,
                "POST",
                "/v1/vpc-connection",
                Some(request.to_json()),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region} {request:?}: expected CreateVpcConnection to return 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["vpcConnectionArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {region} {request:?}: CreateVpcConnection returned no vpcConnectionArn")
            })
            .to_string()
    }
}

/// The `serverProperties` a configuration reply reports, decoded from its base64
/// wire form — the member is a blob, so a suite compares the raw bytes.
pub fn reported_server_properties(reply: &Reply, case: &str) -> Vec<u8> {
    let encoded = reply
        .at("/serverProperties")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected serverProperties, got {}", reply.body));
    ::aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case}: serverProperties is not base64: {error}"))
}

/// The instant a modelled timestamp member reports, in seconds since the epoch.
///
/// Every timestamp member of this service's model targets
/// `com.amazonaws.kafka#__timestampIso8601`, which carries
/// `@timestampFormat("date-time")`, so on this restJson1 wire the member is an RFC 3339
/// date-time STRING — never a JSON number. This reads that string and fails the case when
/// the member is absent, carries any other JSON type, or is not a date-time, so every
/// suite asserting an instant asserts the modelled TYPE along with it.
pub fn reported_instant_secs(value: &Value, case: &str) -> f64 {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case}: expected an ISO-8601 date-time string, got {value}")
    });
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case}: {text:?} is not an ISO-8601 date-time: {error}")
        })
        .as_secs_f64()
}

/// What Amazon MSK answers a create whose name another resource of its kind
/// already holds. Written out here rather than imported from the implementation,
/// so an assertion cannot agree with a constant that is wrong: it is the
/// service's one fixed sentence, naming neither the resource type, nor the name
/// submitted, nor the resource already holding it.
pub const NAME_ALREADY_EXISTS: &str = "A resource with this name already exists.";

/// Assert `reply` is the refusal every create of this service answers a name
/// already taken with: HTTP 409 `ConflictException` carrying
/// [`NAME_ALREADY_EXISTS`] and naming, in `invalidParameter`, the request member
/// that carried the name — `parameter`, by the wire name the request itself uses.
///
/// The message is asserted EQUAL to that fixed sentence, which is what proves the
/// submitted name reached none of it; `existing`, the ARN of the resource already
/// holding the name, is additionally asserted absent, since disclosing a resource
/// the caller cannot otherwise see is the worse half of composing the sentence.
///
/// Shared by every create suite, so the conflict cannot be worded per resource
/// type.
pub fn assert_name_already_exists(
    reply: &Reply,
    case: &str,
    parameter: &str,
    name: &str,
    existing: &str,
) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (409, "ConflictException"),
        "case {case}: expected 409 ConflictException for a name already in use ({name:?}), got \
         {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        NAME_ALREADY_EXISTS,
        "case {case}: expected the fixed taken-name sentence, got {:?}",
        reply.error_message()
    );
    assert!(
        !reply.error_message().contains(existing),
        "case {case}: the message must not disclose the existing ARN ({existing:?}), got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(parameter),
        "case {case}: the conflict must name {parameter} as the parameter at fault, got {}",
        reply.body
    );
}

/// What Amazon MSK answers a channel create whose `ChannelName` a channel of the
/// target cluster already holds. Written out here rather than imported from the
/// implementation, so an assertion cannot agree with a constant that is wrong:
/// the connectives are fixed and the submitted name is the template's one
/// variable part.
pub fn channel_name_already_exists_message(name: &str) -> String {
    format!("A channel with name '{name}' already exists for this cluster.")
}

/// Assert `reply` is the refusal a channel create answers a name already taken
/// in its cluster with: HTTP 400 `BadRequestException` carrying
/// [`channel_name_already_exists_message`] for the submitted `name` and naming
/// `channelName` in `invalidParameter`.
///
/// `existing`, the ARN of the channel already holding the name, is asserted
/// ABSENT from the message: a channel name the caller sent is its own, while the
/// identity of the resource holding it is not part of what the refusal says.
///
/// Shared by every suite that races or repeats a channel name, so none of them
/// can drift from another about what that refusal is.
pub fn assert_channel_name_already_exists(reply: &Reply, case: &str, name: &str, existing: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException for the channel name {name:?} already \
         taken, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        channel_name_already_exists_message(name),
        "case {case}: expected the already-exists sentence for {name:?}, got {:?}",
        reply.error_message()
    );
    assert!(
        !reply.error_message().contains(existing),
        "case {case}: the message must not disclose the existing ARN ({existing:?}), got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("channelName"),
        "case {case}: the refusal must name channelName as the parameter at fault, got {}",
        reply.body
    );
}

/// Assert `reply` is the refusal every operation of the MSK configuration family
/// answers an `Arn` it resolves to no configuration of `region` with: HTTP 400
/// `BadRequestException`, naming the `arn` parameter at fault by its WIRE name,
/// carrying the service's one fixed sentence for that condition — a sentence with
/// no variable parts, so neither `arn` nor `region` nor the account id may appear
/// in it.
///
/// Shared by every configuration suite, so none of them can drift from another
/// about what that refusal is.
pub fn assert_configuration_arn_resolves_to_nothing(
    reply: &Reply,
    case: &str,
    arn: &str,
    region: &str,
) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        CONFIGURATION_ARN_NOT_FOUND_MESSAGE,
        "case {case}: expected the unresolved-ARN wording, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("arn"),
        "case {case}: the refusal must name the arn parameter at fault, by the wire name the \
         request carries, got {}",
        reply.body
    );
    for (what, value) in [("the ARN", arn), ("the region", region), ("the account", ACCOUNT)] {
        assert!(
            !reply.error_message().contains(value),
            "case {case}: the message must not interpolate {what} ({value:?}), got {:?}",
            reply.error_message()
        );
    }
}

/// Assert `reply` is the refusal a call that CHANGES an MSK configuration
/// answers when the configuration is not in a state that call is accepted from:
/// HTTP 400 `BadRequestException`, naming `Arn` as the parameter at fault, and
/// carrying the one sentence that names the set of states it IS accepted from —
/// which is what tells this refusal apart from an `Arn` the endpoint resolves to
/// nothing (`assert_configuration_arn_resolves_to_nothing`).
///
/// The expected sentence is built from the service's own accepted-state set, so
/// a suite cannot assert a set the handlers do not gate on.
pub fn assert_configuration_state_refusal(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    // Built through the one sentence builder the service words this condition in,
    // so the harness holds no second copy of it.
    let expected =
        not_in_accepted_state_message(&CONFIGURATION_STATES_A_CHANGE_IS_ACCEPTED_FROM);
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected the state-machine wording {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("Arn"),
        "case {case}: the refusal must name Arn as the parameter at fault, got {}",
        reply.body
    );
}

/// Assert `reply` is the refusal a call that CHANGES an MSK cluster answers when
/// the cluster is not in a state that call is accepted from: HTTP 400
/// `BadRequestException`, naming the cluster's own ARN member as the parameter at
/// fault, and carrying the one sentence that names the set of states it IS
/// accepted from — the same sentence the configuration family answers for its own
/// resources (`assert_configuration_state_refusal`).
///
/// The expected sentence is built from the service's own accepted-state set, so a
/// suite cannot assert a set the handlers do not gate on.
pub fn assert_cluster_state_refusal(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    let expected =
        not_in_accepted_state_message(&CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM);
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected the state-machine wording {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_PARAMETER),
        "case {case}: the refusal must name {CLUSTER_ARN_PARAMETER} as the parameter at fault, \
         got {}",
        reply.body
    );
}

/// The resource URI of one MSK VPC connection, with the ARN percent-encoded into
/// the single `@httpLabel` path segment the model declares.
pub fn vpc_connection_uri(arn: &str) -> String {
    format!("/v1/vpc-connection/{}", pct(arn))
}

/// Percent-encode a URI component. An MSK ARN is an `@httpLabel`, so its `:` and
/// `/` must be encoded to keep it one path segment; a `NextToken` is base64,
/// whose `+` would otherwise decode to a space. Everything that is not an ASCII
/// alphanumeric is encoded — the conservative set, and over-encoding an
/// unreserved byte is still correct (RFC 3986 §2.1).
pub fn pct(value: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(*byte));
        } else {
            // Infallible: `{:02X}` raises no formatting error, and a `String` is
            // the one `fmt::Write` that cannot fail for any other reason.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// One row of a `CreateCluster` table: the required members plus whichever
/// optional ones the row exercises. `None` means "the request omits it", which is
/// how a row tests a documented default.
#[derive(Debug, Clone)]
pub struct ClusterRequest {
    pub name: String,
    pub kafka_version: &'static str,
    pub brokers: i64,
    pub instance_type: &'static str,
    pub subnets: Vec<&'static str>,
    pub client_broker: Option<&'static str>,
    pub sasl_scram: Option<bool>,
    pub sasl_iam: Option<bool>,
    pub storage_mode: Option<&'static str>,
    pub enhanced_monitoring: Option<&'static str>,
    pub public_access: Option<&'static str>,
    pub network_type: Option<&'static str>,
    pub vpc_tls: Option<bool>,
    pub vpc_scram: Option<bool>,
    pub vpc_iam: Option<bool>,
    pub tags: Vec<(&'static str, &'static str)>,
}

impl ClusterRequest {
    /// A minimal request: only the four required members, so every optional
    /// member's documented default applies.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            kafka_version: "3.6.0",
            brokers: 3,
            instance_type: "kafka.m5.large",
            subnets: vec!["subnet-a", "subnet-b", "subnet-c"],
            client_broker: None,
            sasl_scram: None,
            sasl_iam: None,
            storage_mode: None,
            enhanced_monitoring: None,
            public_access: None,
            network_type: None,
            vpc_tls: None,
            vpc_scram: None,
            vpc_iam: None,
            tags: Vec::new(),
        }
    }

    /// A cluster Amazon MSK admits a VPC connection to: every client
    /// authentication a connection may ask for
    /// (`VPC_CONNECTION_AUTHENTICATION_TYPES`) is turned on for the cluster's
    /// multi-VPC connectivity, and the two SASL schemes at the cluster level too.
    /// MSK serves `CreateVpcConnection` only for a scheme the target cluster turns
    /// on, so every suite whose rule is about the CONNECTION rather than about
    /// that precondition builds its target from this.
    pub fn vpc_connection_target(name: &str) -> Self {
        Self {
            sasl_scram: Some(true),
            sasl_iam: Some(true),
            vpc_tls: Some(true),
            vpc_scram: Some(true),
            vpc_iam: Some(true),
            ..Self::named(name)
        }
    }

    pub fn to_json(&self) -> Value {
        let mut broker_group = json!({
            "instanceType": self.instance_type,
            "clientSubnets": self.subnets,
        });

        let mut connectivity = Map::new();
        if let Some(access) = self.public_access {
            connectivity.insert("publicAccess".to_string(), json!({ "type": access }));
        }
        if let Some(network) = self.network_type {
            connectivity.insert("networkType".to_string(), json!(network));
        }
        let mut vpc_auth = Map::new();
        if let Some(enabled) = self.vpc_tls {
            vpc_auth.insert("tls".to_string(), json!({ "enabled": enabled }));
        }
        let mut vpc_sasl = Map::new();
        if let Some(enabled) = self.vpc_scram {
            vpc_sasl.insert("scram".to_string(), json!({ "enabled": enabled }));
        }
        if let Some(enabled) = self.vpc_iam {
            vpc_sasl.insert("iam".to_string(), json!({ "enabled": enabled }));
        }
        if !vpc_sasl.is_empty() {
            vpc_auth.insert("sasl".to_string(), Value::Object(vpc_sasl));
        }
        if !vpc_auth.is_empty() {
            connectivity.insert(
                "vpcConnectivity".to_string(),
                json!({ "clientAuthentication": Value::Object(vpc_auth) }),
            );
        }
        if !connectivity.is_empty() {
            broker_group["connectivityInfo"] = Value::Object(connectivity);
        }

        let mut body = json!({
            "clusterName": self.name,
            "kafkaVersion": self.kafka_version,
            "numberOfBrokerNodes": self.brokers,
            "brokerNodeGroupInfo": broker_group,
        });
        if let Some(client_broker) = self.client_broker {
            body["encryptionInfo"] =
                json!({ "encryptionInTransit": { "clientBroker": client_broker } });
        }
        let mut sasl = Map::new();
        if let Some(enabled) = self.sasl_scram {
            sasl.insert("scram".to_string(), json!({ "enabled": enabled }));
        }
        if let Some(enabled) = self.sasl_iam {
            sasl.insert("iam".to_string(), json!({ "enabled": enabled }));
        }
        if !sasl.is_empty() {
            body["clientAuthentication"] = json!({ "sasl": Value::Object(sasl) });
        }
        if let Some(mode) = self.storage_mode {
            body["storageMode"] = json!(mode);
        }
        if let Some(level) = self.enhanced_monitoring {
            body["enhancedMonitoring"] = json!(level);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        body
    }
}

/// Sleep past the emulator's own `CREATING` -> `ACTIVE` span, so a later read sees
/// a settled cluster. `std::thread::sleep` because the workspace's tokio carries no
/// timer feature; nothing else has to make progress here.
pub fn sleep_past_activation() {
    std::thread::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// How long [`poll_until_active`] waits for a cluster to report `ACTIVE` before
/// a suite calls it a failure: far longer than the span the service owes that
/// transition, so the deadline says nothing about how fast the host runs.
pub const CLUSTER_ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How long that poll waits between two reads of a cluster that has not settled
/// yet — a probing cadence, not a bound on the transition.
pub const CLUSTER_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Poll `DescribeCluster` until the cluster reports `ACTIVE`, and hand back the
/// version it settled at.
///
/// Waits for the CONDITION rather than for a span: the transition is one the
/// clock owes the cluster, so the loop reads again until it is reported or
/// [`CLUSTER_ACTIVE_DEADLINE`] passes. The shared counterpart of
/// [`sleep_past_activation`] for a suite that would rather look again than wait a
/// fixed time; `std::thread::sleep` between probes because the workspace's tokio
/// carries no timer feature, and nothing else has to make progress here.
pub async fn poll_until_active(emulator: &Emulator, arn: &str, case: &str) -> String {
    let deadline = Instant::now() + CLUSTER_ACTIVE_DEADLINE;
    loop {
        let described = emulator.describe(arn).await;
        if described.at("/clusterInfo/state").as_str() == Some("ACTIVE") {
            return described
                .at("/clusterInfo/currentVersion")
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: an ACTIVE cluster reported no currentVersion")
                })
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the cluster never reported ACTIVE, last read {}",
            described.body
        );
        std::thread::sleep(CLUSTER_POLL_INTERVAL);
    }
}

/// Sleep past the emulator's own `CREATING` -> `ACTIVE` span for a CHANNEL, which
/// is its own span, so a later read sees a settled channel. Shared by every
/// channel suite for the same reason [`sleep_past_activation`] is shared by the
/// cluster ones.
pub fn sleep_past_channel_activation() {
    std::thread::sleep(CHANNEL_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own `UPDATING` -> `ACTIVE` span for a CHANNEL, which
/// is the span an accepted `UpdateChannel` leaves the channel in.
pub fn sleep_past_channel_update() {
    std::thread::sleep(CHANNEL_UPDATE_DELAY + Duration::from_millis(250));
}

/// One row of a `CreateConfiguration` table: the member values one request
/// carries, as every configuration suite of this crate writes its rows.
///
/// `None` means "the request omits it", which is how a row tests a documented
/// default; `extra_revisions` are bodies a suite submits through
/// `UpdateConfiguration` after the create, so a row can exercise a
/// configuration that holds more than its first revision.
#[derive(Debug, Clone)]
pub struct ConfigurationRequest {
    pub name: String,
    pub description: Option<String>,
    pub kafka_versions: Vec<String>,
    /// The `server.properties` body, as raw bytes: the member is a blob, so it
    /// is base64 on the wire and may hold bytes that are not UTF-8 at all.
    pub server_properties: Vec<u8>,
    pub extra_revisions: Vec<Vec<u8>>,
}

impl ConfigurationRequest {
    /// A minimal request: only the two required members, so every optional
    /// member's documented default applies.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            kafka_versions: Vec::new(),
            server_properties: b"auto.create.topics.enable=true\n".to_vec(),
            extra_revisions: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            // A blob member travels base64-encoded in a restJson1 payload.
            "serverProperties": ::aws_smithy_types::base64::encode(&self.server_properties),
        });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.kafka_versions.is_empty() {
            body["kafkaVersions"] = json!(self.kafka_versions);
        }
        body
    }

    /// What `Configuration$Description` must report: the description as given,
    /// and the empty string when the request named none, because that member is
    /// `@required` on the `Configuration` shape the listing reports and is
    /// therefore always written.
    pub fn expected_description(&self) -> String {
        self.description.clone().unwrap_or_default()
    }

    /// What the OPTIONAL `Description` members report: the description as given,
    /// and NOTHING at all when the request named none or named an empty one — an
    /// empty description is no description, so the member is left out.
    pub fn expected_optional_description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .filter(|description| !description.is_empty())
    }
}

/// The configuration table every configuration suite of this crate walks. The
/// rows differ along what those rules generalise over: whether a description
/// was given (and whether it is empty or carries non-ASCII text), how many
/// Kafka versions were named, and the size and byte content of the
/// `server.properties` blob. One table, so a suite that adds a rule tests it
/// over the same inputs every other one does.
pub fn configuration_rows() -> Vec<ConfigurationRequest> {
    vec![
        ConfigurationRequest::named("no-description"),
        ConfigurationRequest {
            description: Some("a described configuration".to_string()),
            kafka_versions: vec!["2.8.1".to_string()],
            ..ConfigurationRequest::named("one-version")
        },
        ConfigurationRequest {
            description: Some(String::new()),
            kafka_versions: vec!["3.4.0".to_string(), "3.5.1".to_string(), "3.6.0".to_string()],
            server_properties: Vec::new(),
            ..ConfigurationRequest::named("three-versions-empty-properties")
        },
        ConfigurationRequest {
            description: Some("héllo — ünicode".to_string()),
            kafka_versions: vec!["3.6.0".to_string()],
            // A property whose VALUE carries bytes that are not valid UTF-8: a
            // blob is a byte string, so the round trip must not go through a
            // string conversion. (The body is still a properties document — the
            // line declares a property — because that is what this service
            // stores a configuration from.)
            server_properties: [
                b"compression.type=".as_slice(),
                &[0x00, 0xff, 0xfe, 0x10, 0x80],
            ]
            .concat(),
            ..ConfigurationRequest::named("binary_properties-1")
        },
        ConfigurationRequest {
            description: Some("a large properties file".to_string()),
            server_properties: b"num.partitions=1\n".repeat(512),
            ..ConfigurationRequest::named("large-properties")
        },
    ]
}

/// One row of a `CreateVpcConnection` table: every member the request carries.
#[derive(Debug, Clone)]
pub struct ConnectionRequest {
    pub target_cluster_arn: String,
    pub authentication: &'static str,
    pub vpc_id: &'static str,
    pub client_subnets: Vec<&'static str>,
    pub security_groups: Vec<&'static str>,
    pub tags: Vec<(&'static str, &'static str)>,
}

impl ConnectionRequest {
    /// A minimal, valid request against the cluster `cluster` names. The EC2 ids
    /// are in their 8-hex-digit form; a row that exercises the 17-digit one says
    /// so.
    pub fn to(cluster: &str) -> Self {
        Self {
            target_cluster_arn: cluster.to_string(),
            authentication: "SASL_IAM",
            vpc_id: "vpc-0a1b2c3d",
            client_subnets: vec!["subnet-0a1b2c3d"],
            security_groups: vec!["sg-0a1b2c3d"],
            tags: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut body = json!({
            "targetClusterArn": self.target_cluster_arn,
            "authentication": self.authentication,
            "vpcId": self.vpc_id,
            "clientSubnets": self.client_subnets,
            "securityGroups": self.security_groups,
        });
        if !self.tags.is_empty() {
            body["tags"] = self.tag_json();
        }
        body
    }

    /// The tags as the JSON map a reader must report.
    pub fn tag_json(&self) -> Value {
        Value::Object(
            self.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        )
    }
}

/// Sleep past the emulator's own `CREATING` -> `AVAILABLE` span, so a later read
/// sees a settled VPC connection.
pub fn sleep_past_vpc_connection_activation() {
    std::thread::sleep(VPC_CONNECTION_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own `REJECTING` -> `REJECTED` span, so a later read
/// sees the rejection the clock now owes a connection the cluster's owner
/// rejected — what a caller polling that rejection to completion waits for.
pub fn sleep_past_vpc_connection_rejection() {
    std::thread::sleep(VPC_CONNECTION_REJECTION_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own `UPDATING` -> `ACTIVE` span, so a later read
/// sees the cluster update the clock now owes the cluster. Beside
/// [`sleep_past_activation`] so every suite that drives an update operation
/// waits through one helper.
pub fn sleep_past_update() {
    std::thread::sleep(CLUSTER_UPDATE_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own `REBOOTING_BROKER` -> `ACTIVE` span, which a
/// broker reboot has for its own (it is observable for several client polls, so it
/// is longer than an update's). Beside [`sleep_past_update`] so every suite that
/// drives a reboot waits through one helper rather than naming the span itself.
pub fn sleep_past_broker_reboot() {
    std::thread::sleep(BROKER_REBOOT_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own configuration-deletion span, so a later call
/// sees the removal the clock now owes a `DELETING` configuration — what a
/// caller polling a configuration deletion to completion waits for.
pub fn sleep_past_configuration_deletion() {
    std::thread::sleep(CONFIGURATION_DELETION_DELAY + Duration::from_millis(250));
}

/// The name of the `.kraft` build line, as an Apache Kafka version MSK publishes
/// spells it: the versions whose cluster metadata a KRaft controller quorum
/// holds.
pub const KRAFT_LINE: &str = "kraft";

/// What [`kafka_version_line`] answers for a build line that is neither
/// ZooKeeper mode nor KRaft — the `.link` spelling, say: MSK publishes no newer
/// member of such a line and offers no upgrade off it.
pub const OTHER_LINE: &str = "other";

/// The build line an Apache Kafka version is on, read from the trailing
/// non-numeric component MSK spells the line with: [`KRAFT_LINE`] for the KRaft
/// metadata-mode versions, `""` for the ZooKeeper-mode ones — which the
/// `.tiered` spelling of a release is also on, tiered storage being a storage
/// option of that same build — and [`OTHER_LINE`] for any other build.
///
/// `x` is not a line: it is MSK's name for "the newest patch of this minor", so
/// it belongs to the release ([`kafka_version_release`] reads it). Shared by
/// every suite that places one published version against another, so a suite
/// cannot disagree with its neighbour about what a version spelling means.
pub fn kafka_version_line(version: &str) -> &str {
    match version
        .rsplit('.')
        .next()
        .filter(|last| *last != "x" && last.parse::<u32>().is_err())
        .unwrap_or_default()
    {
        "" | "tiered" => "",
        KRAFT_LINE => KRAFT_LINE,
        _ => OTHER_LINE,
    }
}

/// The release an Apache Kafka version names, as the components that order one
/// release against another: a numeric component as itself, `x` ("the newest
/// patch of this minor") above every numbered patch, and the trailing line word
/// dropped, because it names a build OF that release rather than a release.
pub fn kafka_version_release(version: &str) -> Vec<u32> {
    version
        .split('.')
        .filter_map(|part| match part.parse::<u32>() {
            Ok(number) => Some(number),
            Err(_) if part == "x" => Some(u32::MAX),
            Err(_) => None,
        })
        .collect()
}

/// How long an assumed-role session this harness registers lasts: long enough that
/// none expires while a suite runs.
const SESSION_SECONDS: i32 = 3_600;

/// Record `role` and the assumed-role session `access_key_id` speaks for, exactly
/// as `iam:CreateRole` and `sts:AssumeRole` record them, and hand back the session
/// principal's ARN — the identity `sts:GetCallerIdentity` reports for that
/// credential, and the one an authorization refusal names.
///
/// The IAM and STS stores are those services' own, reached through their state
/// accessors on the SAME engine this service is mounted on, which is where the
/// authorization gate reads them.
pub fn given_assumed_role_session(
    engine: &Arc<engine::InMemoryEngine>,
    role: &str,
    session: &str,
    access_key_id: &str,
) -> String {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role}");
    let session_arn = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role}/{session}");
    let iam_store =
        iam::service_state(&**engine).expect("the in-memory engine carries IAM's store");
    iam_store.lock().roles.insert(
        iam::name_key(role),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: "/".to_string(),
            role_name: role.to_string(),
            role_id: ::service_common::caller_principal::aws_unique_id("AROA", &role_arn),
            arn: role_arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let vend = sts::NewSession {
        role_arn: Some(role_arn.clone()),
        ..sts::NewSession::new(
            sts::Identity {
                user_id: sts::role_id(&role_arn),
                account: ACCOUNT.to_string(),
                arn: session_arn.clone(),
            },
            sts::SessionKind::AssumedRole,
            SESSION_SECONDS,
        )
    };
    let sts_store =
        sts::service_state(&**engine).expect("the in-memory engine carries STS's store");
    sts_store
        .lock()
        .insert(vend.into_session(access_key_id, SystemTime::now()))
        .expect("the session registry has room for one session");
    session_arn
}

/// Attach `document` to `role` as its inline policy, replacing whatever it carried
/// — what `iam:PutRolePolicy` does. `None` takes every policy off it, as
/// `iam:DeleteRolePolicy` does.
pub fn given_role_inline_policy(
    engine: &Arc<engine::InMemoryEngine>,
    role: &str,
    document: Option<&Value>,
) {
    /// The name the inline policy is held under, which no rule depends on.
    const POLICY_NAME: &str = "kafka-suite-inline-policy";
    let store = iam::service_state(&**engine).expect("the in-memory engine carries IAM's store");
    let mut store = store.lock();
    match document {
        None => {
            store.role_inline_policies.remove(&iam::name_key(role));
        }
        Some(document) => {
            store.role_inline_policies.insert(
                iam::name_key(role),
                BTreeMap::from([(
                    iam::name_key(POLICY_NAME),
                    iam::InlinePolicy {
                        policy_name: POLICY_NAME.to_string(),
                        policy_document: document.to_string(),
                    },
                )]),
            );
        }
    }
}
