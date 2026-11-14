// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through.
//!
//! Each case goes over the protocol the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what a suite asserts is the HTTP
//! behaviour and not a handler called directly.
//!
//! It lives here, and not in one suite, so a second suite does not grow its own
//! copy of the dispatch, the error reading and the timestamp parsing.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328 as logs;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_firehose_firehose_20150804 as firehose;
use service_com_amazonaws_kinesis_kinesis_20131202 as kinesis;
use service_com_amazonaws_route53profiles_route53profiles as profiles;
use service_com_amazonaws_route53resolver_route53resolver::state::FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE;
use service_com_amazonaws_route53resolver_route53resolver::EmulatedService;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The region every request below is signed for; it is where the ARNs the
/// service mints must be rooted.
pub const REGION: &str = "us-west-2";
/// The canonical placeholder account id every emulator ARN carries.
pub const ACCOUNT: &str = "000000000000";

/// The IPv4 CIDR block every VPC [`Emulator::hold_vpcs`] registers carries. Two
/// VPCs of one account may carry the same block, so one is enough for every row
/// of every suite.
const HELD_VPC_CIDR_BLOCK: &str = "172.31.0.0/16";

/// The emulator a suite drives: the router every request goes through, and the
/// shared engine it is mounted on.
///
/// It dereferences to the router, so a suite that only makes requests uses it
/// exactly as it would the router itself. The engine is what gives a suite
/// access to the account's resources in ANOTHER service's store — the Amazon
/// EC2 subnets a Resolver endpoint's IP addresses are placed in
/// ([`Emulator::hold_subnets`]).
pub struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl std::ops::Deref for Emulator {
    type Target = MultiServiceRouter;

    fn deref(&self) -> &Self::Target {
        &self.router
    }
}

impl Emulator {
    /// Put one subnet per id into the account's own Amazon EC2 store, inside a
    /// VPC of that store.
    ///
    /// `CreateResolverEndpoint` and `AssociateResolverEndpointIpAddress` place
    /// an endpoint's IP addresses IN subnets of the caller's VPC and resolve
    /// every `SubnetId` against that store, so a suite whose write must be
    /// served registers the subnets it names here first — the way
    /// [`rack_outpost`] racks the Outpost a Resolver on an Outpost needs.
    ///
    /// Each subnet is stored as the modelled shape Amazon EC2 reports, in the
    /// one VPC this account's store holds for these suites, `available`, with a
    /// /24 block of that VPC and a zone of [`REGION`]. A suite that needs
    /// subnets in a SECOND VPC racks one with [`Emulator::hold_vpc`] and names
    /// it to [`Emulator::hold_subnets_in`].
    pub fn hold_subnets<I>(&self, subnet_ids: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let existing = self.ec2_store().lock().vpcs.keys().next().cloned();
        let vpc_id = existing.unwrap_or_else(|| self.hold_vpc());
        self.hold_subnets_in(&vpc_id, subnet_ids);
    }

    /// Put one VPC into the account's own Amazon EC2 store, with its own /16
    /// block, and answer its id.
    ///
    /// The VPC a Resolver endpoint reports as its `HostVPCId` is the VPC that
    /// owns the subnets its addresses sit in, so a suite that asserts the two
    /// cannot be told apart needs an account holding more than one VPC.
    pub fn hold_vpc(&self) -> String {
        let ec2_state = self.ec2_store();
        let mut store = ec2_state.lock();
        // Each VPC gets a distinct second octet, so no subnet of one VPC spans
        // the addresses of a subnet of another.
        let octet = store.vpcs.len() % 256;
        let vpc_id = store.next_id("vpc");
        store.vpcs.insert(
            vpc_id.clone(),
            ec2::ops::types::Vpc {
                vpc_id: Some(vpc_id.clone()),
                cidr_block: Some(format!("10.{octet}.0.0/16")),
                state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
        vpc_id
    }

    /// Put one subnet per id into the VPC `vpc_id`, each with a /24 block of
    /// that VPC's own block and a zone of [`REGION`].
    pub fn hold_subnets_in<I>(&self, vpc_id: &str, subnet_ids: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let ec2_state = self.ec2_store();
        let mut store = ec2_state.lock();
        let octet = store
            .vpcs
            .get(vpc_id)
            .and_then(|vpc| vpc.cidr_block.clone())
            .and_then(|block| {
                block
                    .split('.')
                    .nth(1)
                    .and_then(|octet| octet.parse::<usize>().ok())
            })
            .unwrap_or_default();
        for subnet_id in subnet_ids {
            let subnet_id = subnet_id.as_ref().to_string();
            if store.subnets.contains_key(&subnet_id) {
                continue;
            }
            let block = store.subnets.len() % 256;
            store.subnets.insert(
                subnet_id.clone(),
                ec2::ops::types::Subnet {
                    subnet_id: Some(subnet_id),
                    vpc_id: Some(vpc_id.to_string()),
                    cidr_block: Some(format!("10.{octet}.{block}.0/24")),
                    availability_zone: Some(format!("{REGION}a")),
                    state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                },
            );
        }
    }

    /// Put one VPC per id into the account's own Amazon EC2 store.
    ///
    /// A Resolver resource is associated WITH a VPC of the caller's account, and
    /// the association writes (`AssociateResolverRule`,
    /// `AssociateResolverQueryLogConfig`, `AssociateFirewallRuleGroup`) resolve
    /// the VPC id they are given against that store, so a suite whose write must
    /// be served registers the VPCs it names here first — the way
    /// [`Emulator::hold_subnets`] registers the subnets an endpoint's addresses
    /// sit in.
    ///
    /// Each VPC is stored as the modelled shape Amazon EC2 reports, `available`,
    /// with [`HELD_VPC_CIDR_BLOCK`]. An id the store already holds a VPC under
    /// is left exactly as it was.
    pub fn hold_vpcs<I>(&self, vpc_ids: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let ec2_state = self.ec2_store();
        let mut store = ec2_state.lock();
        for vpc_id in vpc_ids {
            let vpc_id = vpc_id.as_ref().to_string();
            store
                .vpcs
                .entry(vpc_id.clone())
                .or_insert_with(|| ec2::ops::types::Vpc {
                    vpc_id: Some(vpc_id),
                    cidr_block: Some(HELD_VPC_CIDR_BLOCK.to_string()),
                    state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
                    ..Default::default()
                });
        }
    }

    /// Every VPC the account's own Amazon EC2 store holds, in the order that
    /// store reports and pages over them (by `VpcId`) — what a client joining
    /// Resolver's per-VPC configurations to Amazon EC2 reads with
    /// `DescribeVpcs`. Read out of that store, never written here.
    ///
    /// An account holds the ambient default VPC of its Region from the moment it
    /// exists, so this is never empty.
    pub fn held_vpc_ids(&self) -> Vec<String> {
        let ec2_state = self.ec2_store();
        let store = ec2_state.lock();
        store.vpcs.keys().cloned().collect()
    }

    /// Take the VPC `vpc_id` names out of the account's Amazon EC2 store, so a
    /// Resolver write that names it afterwards resolves to nothing.
    pub fn release_vpc(&self, vpc_id: &str) {
        self.ec2_store().lock().vpcs.remove(vpc_id);
    }

    /// The VPC the account's Amazon EC2 store holds the subnet `subnet_id` in —
    /// what a client joining Resolver output to Amazon EC2 reads with
    /// `DescribeSubnets`. Read out of that store, never written here.
    pub fn vpc_of_subnet(&self, subnet_id: &str) -> Option<String> {
        let ec2_state = self.ec2_store();
        let store = ec2_state.lock();
        store
            .subnets
            .get(subnet_id)
            .and_then(|subnet| subnet.vpc_id.clone())
    }

    /// The IPv4 block the account's Amazon EC2 store holds the subnet
    /// `subnet_id` with.
    pub fn cidr_block_of_subnet(&self, subnet_id: &str) -> Option<String> {
        let ec2_state = self.ec2_store();
        let store = ec2_state.lock();
        store
            .subnets
            .get(subnet_id)
            .and_then(|subnet| subnet.cidr_block.clone())
    }

    /// Every elastic network interface the account's Amazon EC2 store holds, as
    /// `DescribeNetworkInterfaces` reports them.
    pub fn network_interfaces(&self) -> Vec<NetworkInterfaceView> {
        let ec2_state = self.ec2_store();
        let store = ec2_state.lock();
        store
            .network_interfaces
            .values()
            .map(|interface| NetworkInterfaceView {
                subnet_id: interface.subnet_id.clone().unwrap_or_default(),
                vpc_id: interface.vpc_id.clone().unwrap_or_default(),
                description: interface.description.clone().unwrap_or_default(),
                requester_id: interface.requester_id.clone(),
                requester_managed: interface.requester_managed.unwrap_or_default(),
                private_ips: interface
                    .private_ip_addresses
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|address| address.private_ip_address.clone())
                    .collect(),
            })
            .collect()
    }

    /// Take the subnet `subnet_id` names out of the account's Amazon EC2 store,
    /// so a Resolver write that names it afterwards resolves to nothing.
    pub fn release_subnet(&self, subnet_id: &str) {
        self.ec2_store().lock().subnets.remove(subnet_id);
    }

    /// The account's own Amazon EC2 store on the engine this emulator shares
    /// with it.
    fn ec2_store(&self) -> engine::ServiceState<ec2::state::ComAmazonawsEc2Amazonec2State> {
        ec2::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's Amazon EC2 store")
    }

    /// Put one Route 53 Profile into the account's own Route 53 Profiles store,
    /// under the id `profile_id`, and answer that id.
    ///
    /// `ResolverConfig$ResourceId` is "the ID of the Amazon Virtual Private
    /// Cloud VPC or a Route 53 Profile that you're configuring Resolver for", so
    /// the Resolver behavior configuration resolves the id it is given against
    /// that inventory too — and a suite whose call must be served for a Profile
    /// registers it here first, the way [`Emulator::hold_vpcs`] registers a VPC.
    pub fn hold_profile(&self, profile_id: &str) -> String {
        let state = profiles::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's Route 53 Profiles store");
        let mut store = state.lock();
        let now = std::time::SystemTime::now();
        store
            .profiles
            .entry(profile_id.to_string())
            .or_insert_with(|| profiles::state::Profile {
                id: profile_id.to_string(),
                arn: format!(
                    "arn:aws:route53profiles:{REGION}:{ACCOUNT}:profile/{profile_id}"
                ),
                name: format!("profile-{profile_id}"),
                owner_id: ACCOUNT.to_string(),
                status: profiles::state::STATUS_COMPLETE.to_string(),
                status_message: None,
                share_status: profiles::state::SHARE_STATUS_NOT_SHARED.to_string(),
                creation_time: now,
                modification_time: now,
                client_token: format!("token-{profile_id}"),
            });
        profile_id.to_string()
    }

    /// Put the resource each `DestinationArn` names into the store of the
    /// service that OWNS it, so a query logging configuration naming that
    /// destination can be created.
    ///
    /// `CreateResolverQueryLogConfig` delivers to a destination the account
    /// HOLDS — an Amazon S3 bucket, a CloudWatch Logs log group, a Kinesis data
    /// stream or a Kinesis Data Firehose delivery stream — and resolves the ARN
    /// it is given against that service's own store, so a suite whose create
    /// must be served registers its destinations here first, the way
    /// [`Emulator::hold_vpcs`] registers the VPCs an association names.
    ///
    /// A destination already held is left exactly as it was.
    pub fn hold_query_log_destinations<I>(&self, destination_arns: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        for destination_arn in destination_arns {
            let destination_arn = destination_arn.as_ref();
            let fields: Vec<&str> = destination_arn.splitn(6, ':').collect();
            let [_, _, service, region, _, resource] = fields.as_slice() else {
                panic!("a destination ARN carries six fields, got {destination_arn:?}");
            };
            match *service {
                // The destination is the BUCKET, whatever key prefix the ARN
                // carries under it.
                "s3" => self.hold_bucket(resource.split('/').next().unwrap_or_default()),
                "logs" => {
                    let group = resource
                        .strip_prefix("log-group:")
                        .map(|group| group.strip_suffix(":*").unwrap_or(group))
                        .unwrap_or_else(|| panic!("a log-group ARN, got {destination_arn:?}"));
                    self.hold_log_group(region, group);
                }
                "kinesis" => {
                    let stream = resource
                        .strip_prefix("stream/")
                        .unwrap_or_else(|| panic!("a data-stream ARN, got {destination_arn:?}"));
                    self.hold_data_stream(region, stream);
                }
                "firehose" => {
                    let stream = resource.strip_prefix("deliverystream/").unwrap_or_else(|| {
                        panic!("a delivery-stream ARN, got {destination_arn:?}")
                    });
                    self.hold_delivery_stream(destination_arn, stream);
                }
                other => panic!("{other:?} owns no query log destination: {destination_arn:?}"),
            }
        }
    }

    /// Put one bucket into the account's own Amazon S3 store.
    fn hold_bucket(&self, bucket: &str) {
        let state = s3::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's Amazon S3 store");
        let _creation = state.lock().create_bucket(bucket, REGION);
    }

    /// Put one log group into the account's own CloudWatch Logs store.
    fn hold_log_group(&self, region: &str, group: &str) {
        let state = logs::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's CloudWatch Logs store");
        let mut store = state.lock();
        if store.log_groups.contains_key(group) {
            return;
        }
        store.log_groups.insert(
            group.to_string(),
            logs::state::LogGroupRecord::new(region, group, 0),
        );
    }

    /// Put one Kinesis data stream into the account's own Amazon Kinesis store,
    /// held by the Region's endpoint and active from the moment it exists.
    fn hold_data_stream(&self, region: &str, stream: &str) {
        let state = kinesis::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's Amazon Kinesis store");
        let mut store = state.lock();
        let key = kinesis::state::RegionalName::new(region, stream);
        if store.streams.contains_key(&key) {
            return;
        }
        let now = std::time::SystemTime::now();
        store.streams.insert(
            key,
            kinesis::state::Stream {
                name: stream.to_string(),
                region: region.to_string(),
                arn: kinesis::state::stream_arn(region, stream),
                stream_mode: kinesis::state::STREAM_MODE_ON_DEMAND.to_string(),
                shards: Vec::new(),
                retention_period_hours: kinesis::state::DEFAULT_RETENTION_PERIOD_HOURS,
                created_at: now,
                active_at: now,
                encryption_type: kinesis::state::ENCRYPTION_TYPE_NONE.to_string(),
                key_id: None,
                shard_level_metrics: Vec::new(),
                warm_throughput_mi_bps: None,
                max_record_size_in_ki_b: kinesis::state::DEFAULT_MAX_RECORD_SIZE_KI_B,
                record_distribution_strategy: None,
                deleted_at: None,
                updating_until: None,
                reshards: Vec::new(),
                encryption_starts: Vec::new(),
                encryption_stops: Vec::new(),
                tags: Default::default(),
                consumers: Vec::new(),
                policy: None,
            },
        );
    }

    /// Put one Firehose delivery stream into the account's own Kinesis Data
    /// Firehose store, active from the moment it exists.
    fn hold_delivery_stream(&self, arn: &str, stream: &str) {
        let state = firehose::state::service_state(&*self.engine)
            .expect("the in-memory engine carries the account's Firehose store");
        let mut store = state.lock();
        if store.streams.contains_key(stream) {
            return;
        }
        let now = std::time::SystemTime::now();
        store.streams.insert(
            stream.to_string(),
            firehose::state::DeliveryStream {
                name: stream.to_string(),
                arn: arn.to_string(),
                status: firehose::state::StreamStatus::Active,
                pending: None,
                stream_type: "DirectPut".to_string(),
                version_id: 1,
                create_timestamp: now,
                last_update_timestamp: now,
                source: None,
                destinations: Vec::new(),
                encryption: None,
                pending_encryption: None,
                encryption_calls: Vec::new(),
                tags: Default::default(),
                records: Vec::new(),
                buffered_bytes: 0,
                next_destination_id: 1,
                delivery_retry_at: None,
                source_positions: Default::default(),
            },
        );
    }
}

/// One elastic network interface of the account, as a suite reads it out of the
/// Amazon EC2 store: what a caller joining Resolver's reported addresses to
/// Amazon EC2 compares them against.
#[derive(Debug, Clone)]
pub struct NetworkInterfaceView {
    pub subnet_id: String,
    pub vpc_id: String,
    pub description: String,
    /// "The alias or Amazon Web Services account ID of the principal or service
    /// that created the network interface", and whether the interface is one
    /// Amazon Web Services manages — both reported for an interface another
    /// service created to serve a resource of the account.
    pub requester_id: Option<String>,
    pub requester_managed: bool,
    pub private_ips: Vec<String>,
}

/// Whether the IPv4 address `address` lies inside the block `cidr_block`.
///
/// What a suite asserts an address Resolver CHOSE by: an address of a subnet is
/// one of the addresses that subnet spans, whatever the width of its block.
pub fn address_inside_block(cidr_block: &str, address: &str) -> bool {
    let Some((base, prefix)) = cidr_block.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u32>() else {
        return false;
    };
    let (Ok(base), Ok(address)) = (
        base.parse::<std::net::Ipv4Addr>(),
        address.parse::<std::net::Ipv4Addr>(),
    ) else {
        return false;
    };
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix.min(32))
    };
    u32::from(base) & mask == u32::from(address) & mask
}

/// A fresh emulator: one shared engine, this one service mounted on it.
pub fn emulator() -> Emulator {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same emulator, mounted on an engine the caller keeps a handle to — for a
/// suite that must observe the store itself and not only the wire (whether a
/// call wrote a record at all, say).
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> Emulator {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// A fresh emulator with this service AND the account's own Amazon EC2 service
/// on ONE shared engine.
///
/// The per-VPC singleton configurations this service maintains belong to the
/// VPCs the ACCOUNT holds, and which VPCs those are is a fact of the Amazon EC2
/// inventory, so a suite that asserts the two services agree creates its VPCs
/// through the Amazon EC2 API — over the wire, exactly as an account does — and
/// never by writing that store itself ([`Emulator::hold_vpcs`]).
pub fn emulator_with_ec2() -> Emulator {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// A fresh emulator with this service AND the account's own AWS Outposts
/// service on ONE shared engine.
///
/// A Resolver on an Outpost can only be created on an Outpost the account has,
/// so a suite that creates one racks it first — through the Outposts API, over
/// the wire, exactly as an account does ([`rack_outpost`]) — and the two
/// services read the one inventory because they share the engine.
pub fn emulator_with_outposts() -> Emulator {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_outposts_outpostsolafservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router, engine }
}

/// Rack one Outpost in `region` and answer its `OutpostArn`.
///
/// Driven over the AWS Outposts wire (restJson1, `POST /sites` then
/// `POST /outposts`), so the Outpost is one that service really holds and the
/// ARN is the one it minted — never a value written out here.
pub async fn rack_outpost(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let site = outposts_ok(
        router,
        region,
        "POST",
        "/sites",
        Some(json!({ "Name": format!("site for {name}") })),
    )
    .await;
    let site_id = string_member(
        site.get("Site").unwrap_or_else(|| {
            panic!("CreateSite must answer with the Site it created, got {site}")
        }),
        "SiteId",
    );

    let outpost = outposts_ok(
        router,
        region,
        "POST",
        "/outposts",
        Some(json!({ "Name": name, "SiteId": site_id })),
    )
    .await;
    let arn = string_member(
        outpost.get("Outpost").unwrap_or_else(|| {
            panic!("CreateOutpost must answer with the Outpost it created, got {outpost}")
        }),
        "OutpostArn",
    );
    assert!(
        !arn.is_empty(),
        "the racked Outpost must report an OutpostArn, got {arn:?}"
    );
    arn
}

/// Take the Outpost `outpost_arn` names out of the account's inventory, through
/// the AWS Outposts wire (`DELETE /outposts/{OutpostId}`).
///
/// The route binds the identifier to one path segment, so the Outpost is named
/// by the bare `op-…` id its ARN carries — the other form the modelled
/// identifier pattern admits.
pub async fn unrack_outpost(router: &MultiServiceRouter, region: &str, outpost_arn: &str) {
    let outpost_id = outpost_arn
        .rsplit_once('/')
        .map(|(_, id)| id)
        .unwrap_or(outpost_arn);
    outposts_ok(
        router,
        region,
        "DELETE",
        &format!("/outposts/{outpost_id}"),
        None,
    )
    .await;
}

/// Dispatch one restJson1 request to the AWS Outposts service, addressed to
/// `region`, and require HTTP 200.
async fn outposts_ok(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let payload = body
        .map(|body| serde_json::to_vec(&body).expect("the request body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://outposts.{region}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{region}/outposts/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{method} {path} in {region} must succeed, got {status}: {parsed}"
    );
    parsed
}

/// Call one operation and return `(http status, parsed body)`.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_tracing(router, operation, body, None).await
}

/// The same call, carrying the tracing header `tracing` when the caller sends
/// one — which is what a Resolver refusal reports the trace of the request
/// under, so a suite that asserts on that clause chooses the chain itself.
pub async fn call_tracing(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    tracing: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Route53Resolver.{operation}"))
        // The SigV4 credential scope is what routes the request to this service
        // and what names the region its ARNs are minted in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20261021/{REGION}/route53resolver/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=not-verified-in-this-test"
            ),
        );
    if let Some(tracing) = tracing {
        builder = builder.header("x-amzn-trace-id", tracing);
    }
    let request = builder
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A refusal `Message` split into the sentence it states and the trace id its
/// closing clause carries, when it closes with that clause and the id in it is
/// in the form X-Ray gives one (`1-<8 hex>-<24 hex>`, read through the one
/// reading of that form the services share).
///
/// `None` for a message that closes with no such clause, so a suite asserting
/// the clause reports what it got instead.
pub fn traced_message(message: &str) -> Option<(&str, &str)> {
    let (stated, clause) = message.rsplit_once(" Trace Id: ")?;
    let trace = clause.strip_prefix('"')?.strip_suffix('"')?;
    service_common::xray_trace::is_trace_id(trace).then_some((stated, trace))
}

/// The tracing header naming chain `index`, and the `Root` a refusal of a
/// request carrying it has to be reported under — a trace id in the form X-Ray
/// gives one (`1-<8 hex>-<24 hex>`), so a suite that asserts on the clause
/// chooses the chain itself instead of writing one out.
pub fn tracing_chain(index: u32) -> (String, String) {
    let root = format!("1-6b100000-{index:024x}");
    (format!("Root={root};Sampled=0"), root)
}

/// How long [`settled_association`] waits before it looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long [`settled_association`] keeps looking before it gives up: generous,
/// so how fast the host runs never decides the outcome.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// Look at the DNS Firewall rule-group association `id` again until the change
/// in flight has settled, so a case that needs a settled one reads a settled
/// one.
///
/// The CONDITION is what is waited for — the status the association REPORTS —
/// and never a span of the clock, so a loaded or a faster host changes nothing
/// about the outcome.
pub async fn settled_association(router: &MultiServiceRouter, id: &str, case: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = ok(
            router,
            "GetFirewallRuleGroupAssociation",
            serde_json::json!({ "FirewallRuleGroupAssociationId": id }),
        )
        .await;
        let association = read
            .get("FirewallRuleGroupAssociation")
            .unwrap_or_else(|| panic!("case {case:?}: the read answers an association, got {read}"));
        let status = string_member(association, "Status");
        if status == FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_COMPLETE {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the association must settle, it still reports {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Call an operation that must succeed, and return its body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {parsed}");
    parsed
}

/// The `__type` of an error response, which carries the modelled error's shape
/// name on the awsJson protocols.
pub fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert an operation failed with `expected` as its modelled error.
pub async fn expect_error(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
    expected: &str,
) {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains(expected),
        "case {case:?}: {operation} must answer {expected}, got {parsed}"
    );
}

pub fn string_member(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The same object with `Status` dropped, so two reads can be compared without
/// the member the clock is allowed to move.
pub fn without_status(value: &Value) -> Value {
    let mut cloned = value.clone();
    if let Some(object) = cloned.as_object_mut() {
        object.remove("Status");
    }
    cloned
}

/// Parse a modelled `Rfc3339TimeString` (length 20..40) into a point in time, so
/// two timestamps are compared numerically rather than as text.
pub fn parse_time(case: &str, member: &str, raw: &str) -> aws_smithy_types::DateTime {
    assert!(
        raw.len() >= 20 && raw.len() <= 40,
        "case {case:?}: {member} must satisfy Rfc3339TimeString's length 20..40, got {raw:?}"
    );
    aws_smithy_types::DateTime::from_str(raw, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} must be an RFC 3339 time, got {raw:?}: {error}")
        })
}
