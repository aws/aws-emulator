// SPDX-License-Identifier: CC0-1.0

//! The DNS server addresses a directory serves, driven end to end over HTTP with the owner
//! of subnets mounted beside this service.
//!
//! RULE: a directory whose servers this service runs places one server in each subnet its
//! create named, and each server holds an address out of THAT subnet's own CIDR block — so
//! `DirectoryDescription$DnsIpAddrs` ("the IP addresses of the DNS servers for the
//! directory") reports one address per chosen subnet, inside that subnet's block, and what
//! `DescribeDomainControllers` reports about the servers individually agrees with it. A
//! directory that serves no DNS of its own, an AD Connector, reports the self-managed
//! directory's servers its create named instead.
//!
//! The rows differ along what the rule generalises over: the size of the subnet's block
//! (prefix lengths `/16`, `/20`, `/24` and `/28`), its base address, the order the create
//! names the subnets in, whether the subnets are ones a client minted with
//! `ec2:CreateSubnet` or ones EC2 materialises for the account's default VPC (whose blocks
//! this suite never writes), how many servers the directory runs, and the directory type.
//!
//! A subnet is an Amazon EC2 resource, so EC2 is mounted beside this service on ONE engine
//! and every subnet a case names was MINTED by it: no identifier written here can pass
//! itself off as one the account holds, and the blocks the assertions check against are the
//! ones EC2 itself reports. Nothing reaches into either store.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ipnet::Ipv4Net;
use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::ad_connector::connect_directory;
use crate::directory_lifecycle::{
    create_directory, descriptions, ok, string, PASSWORD, REGION, STAGE_ACTIVE,
};
use crate::microsoft_ad::create_microsoft_ad;

/// How many addresses at the START of a subnet's block carry no server of the account.
///
/// AWS's own addressing rules for a VPC subnet: the first four addresses and the last
/// address of every subnet CIDR block are reserved, so an address a directory server holds
/// is neither of those.
const RESERVED_LEADING_ADDRESSES: u32 = 4;

/// The DNS servers of the self-managed directory an AD Connector forwards to, as
/// `DirectoryConnectSettings$CustomerDnsIps` names them. Addresses of a network this
/// emulator does not hold — that is the point: a connector serves DNS from the directory it
/// connects to.
const CUSTOMER_DNS_IPS: [&str; 2] = ["10.7.0.10", "10.7.1.10"];

/// A `SubnetId` of the shape the modelled pattern
/// `^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$` admits that EC2 mints no subnet for, so it
/// reaches the create's resolution and resolves to nothing. `pub(crate)` because the sibling
/// suite `directory_security_group` asserts what a directory placed in nothing reports.
pub(crate) const ABSENT_SUBNET: &str = "subnet-00000000deadbeef0";

/// How long a polled state may take to settle before a case fails: far longer than any span
/// this service publishes, and well inside what a client's waiter allows.
const SETTLE_DEADLINE: Duration = Duration::from_secs(20);

/// How often one of the waits below asks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Wait until `DescribeDirectories` reports `directory_id` in service, the way a client with
/// a waiter does: ask, and ask again until the stage chain the create owes has settled or a
/// generous deadline passes.
async fn in_service(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let started = Instant::now();
    loop {
        let described = describe_one(router, directory_id, case).await;
        let stage = string(&described, "Stage", case);
        if stage == STAGE_ACTIVE {
            return;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: expected {directory_id} to report itself in service within \
             {SETTLE_DEADLINE:?}, last observed stage {stage:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the directory reports `expected` DNS addresses — the servers a create placed or
/// a resize added come up on the clock, so a client reads the settled set by asking again —
/// and return them.
async fn dns_addresses_settling_at(
    router: &MultiServiceRouter,
    directory_id: &str,
    expected: usize,
    case: &str,
) -> Vec<String> {
    let started = Instant::now();
    loop {
        let described = describe_one(router, directory_id, case).await;
        let addresses = dns_ip_addrs(&described, case);
        if addresses.len() == expected {
            return addresses;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: expected {expected} DNS address(es) within {SETTLE_DEADLINE:?}, last \
             observed {addresses:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The status a domain controller that is in service reports: the `Active` value of the
/// modelled `DomainControllerStatus` enum.
const CONTROLLER_ACTIVE: &str = "Active";

/// Wait until a resize has RUN: `expected` controllers, every one of them in service. A
/// controller with a transition still owed is a resize still running — the operation's own
/// documentation says another update cannot be made during one — so this is the condition a
/// client waits for before asking for the next number.
async fn resize_settled(
    router: &MultiServiceRouter,
    directory_id: &str,
    expected: usize,
    case: &str,
) {
    let started = Instant::now();
    loop {
        let listed = controllers(router, directory_id, case).await;
        let settled = listed.len() == expected
            && listed
                .iter()
                .all(|controller| controller["Status"] == json!(CONTROLLER_ACTIVE));
        if settled {
            return;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: expected {expected} controller(s) in service within \
             {SETTLE_DEADLINE:?}, last observed {listed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One subnet the account HAS, as EC2 — the service that owns subnets — reports it.
///
/// Nothing here is written by this suite: the identifier, the block and the zone are all
/// read back from the owner, which is also where the service under test resolves them.
/// `pub(crate)` because the sibling suite `directory_security_group` places directories in
/// the same account network.
#[derive(Debug, Clone)]
pub(crate) struct AccountSubnet {
    pub(crate) id: String,
    cidr: String,
    zone: String,
}

/// The emulator one case runs against: this service and EC2 on one engine, plus the subnets
/// the account has.
pub(crate) struct Network {
    pub(crate) router: MultiServiceRouter,
    /// The VPC `ec2:CreateVpc` minted, the one the account really holds: a create that names
    /// another is naming a VPC the owner of VPCs reports nothing for.
    pub(crate) vpc: String,
    /// The subnets `ec2:CreateSubnet` minted in a VPC of the client's own, in the order
    /// [`CLIENT_SUBNET_BLOCKS`] names their blocks.
    pub(crate) client: Vec<AccountSubnet>,
    /// The subnets EC2 materialises for the account's default VPC in this Region, in zone
    /// order — blocks this suite never writes.
    pub(crate) default: Vec<AccountSubnet>,
}

/// The blocks the client's own subnets carry, which is what the rows differ along: four
/// prefix lengths and four bases, none overlapping inside [`CLIENT_VPC_BLOCK`].
const CLIENT_SUBNET_BLOCKS: [&str; 4] = [
    "10.168.0.0/24",
    "10.168.1.0/24",
    "10.168.2.0/28",
    "10.168.16.0/20",
];

/// The block the client's own VPC carries, which every subnet above is cut out of.
const CLIENT_VPC_BLOCK: &str = "10.168.0.0/16";

/// The Availability Zones the client's subnets sit in, one per subnet, written as
/// `<region><letter>` — the form AWS names a zone of a Region in. The fourth subnet shares
/// the first one's zone, since EC2 reports three zones for this Region: a VPC may hold
/// several subnets in one zone, and every pair a case names still spans two.
const ZONES: [&str; 4] = ["us-east-1a", "us-east-1b", "us-east-1c", "us-east-1a"];

/// Stand this service up beside EC2 on one engine and build the account's network through
/// EC2's own API.
///
/// Shared with the sibling suites that need the OWNER of VPCs and subnets mounted beside this
/// service (`crate::vpc_resolution`, `crate::directory_security_group`), rather than stood up
/// a second time there.
pub(crate) async fn network() -> Network {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise the services, not SigV4: requests carry a credential scope so the
    // router can name the target service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // The very seam this service's own subnet resolution reaches EC2 through, so a network
    // built here is minted exactly where a create will look for it.
    let ec2 = ec2_client(&EmbeddedEmulator::from_router(router.clone()));

    let vpc = ec2
        .create_vpc()
        .cidr_block(CLIENT_VPC_BLOCK)
        .send()
        .await
        .expect("EC2 must create the client's VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();

    let mut client = Vec::with_capacity(CLIENT_SUBNET_BLOCKS.len());
    for (block, zone) in CLIENT_SUBNET_BLOCKS.iter().zip(ZONES) {
        let id = ec2
            .create_subnet()
            .vpc_id(&vpc)
            .cidr_block(*block)
            .availability_zone(zone)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must create a subnet for {block}: {error:?}"))
            .subnet()
            .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
            .unwrap_or_else(|| panic!("CreateSubnet must report the subnet it minted for {block}"))
            .to_string();
        client.push(reported(&ec2, &id).await);
    }

    // ASKED, never assumed: the account's own default subnets, whose blocks this suite does
    // not write anywhere.
    let described = ec2
        .describe_subnets()
        .send()
        .await
        .expect("EC2 must report the account's subnets");
    let mut default: Vec<AccountSubnet> = described
        .subnets()
        .iter()
        .filter(|subnet| subnet.default_for_az().unwrap_or(false))
        .filter_map(|subnet| {
            Some(AccountSubnet {
                id: subnet.subnet_id()?.to_string(),
                cidr: subnet.cidr_block()?.to_string(),
                zone: subnet.availability_zone()?.to_string(),
            })
        })
        .filter(|subnet| subnet.zone.starts_with(REGION))
        .collect();
    default.sort_by(|left, right| left.zone.cmp(&right.zone));
    assert!(
        default.len() >= 2,
        "the account must have at least two default subnets in {REGION} for these cases, got \
         {default:?}"
    );

    Network {
        router,
        vpc,
        client,
        default,
    }
}

/// An EC2 client over `callback`, the in-process seam this emulator routes on: no socket is
/// opened and the endpoint URL is only the address that transport routes by.
pub(crate) fn ec2_client(callback: &EmbeddedEmulator) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
        .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// The subnet `subnet_id` as EC2 reports it: the block and zone a directory placed in it
/// must agree with.
async fn reported(ec2: &::aws_sdk_ec2::Client, subnet_id: &str) -> AccountSubnet {
    let described = ec2
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"));
    let subnet = described
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .unwrap_or_else(|| panic!("EC2 must hold the subnet {subnet_id}"));
    AccountSubnet {
        id: subnet_id.to_string(),
        cidr: subnet
            .cidr_block()
            .unwrap_or_else(|| panic!("EC2 must report a block for {subnet_id}"))
            .to_string(),
        zone: subnet
            .availability_zone()
            .unwrap_or_else(|| panic!("EC2 must report a zone for {subnet_id}"))
            .to_string(),
    }
}

/// Assert `address` is an address the subnet `cidr` can really hand one of its servers:
/// inside the block, and neither of the addresses AWS reserves at its two ends.
fn assert_address_in(address: &str, cidr: &str, case: &str) {
    let block: Ipv4Net = cidr
        .parse()
        .unwrap_or_else(|error| panic!("case {case:?}: EC2 reported the block {cidr:?}: {error}"));
    let parsed: Ipv4Addr = address.parse().unwrap_or_else(|error| {
        panic!("case {case:?}: expected an IPv4 address, got {address:?}: {error}")
    });
    assert!(
        block.contains(&parsed),
        "case {case:?}: expected an address inside {cidr}, got {address}"
    );
    let value = u32::from(parsed);
    assert!(
        value >= u32::from(block.network()) + RESERVED_LEADING_ADDRESSES
            && value < u32::from(block.broadcast()),
        "case {case:?}: expected an assignable address of {cidr} (past its first \
         {RESERVED_LEADING_ADDRESSES} and before its last), got {address}"
    );
}

/// The `DnsIpAddrs` a description reports, in the order it reports them.
fn dns_ip_addrs(described: &Value, case: &str) -> Vec<String> {
    described["DnsIpAddrs"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a DnsIpAddrs list, got {described}")
        })
        .iter()
        .map(|address| {
            address
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an address, got {described}"))
                .to_string()
        })
        .collect()
}

/// The one description of `directory_id`, read back through `DescribeDirectories`.
async fn describe_one(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Value {
    let answer = ok(
        router,
        "DescribeDirectories",
        json!({ "DirectoryIds": [directory_id] }),
        case,
    )
    .await;
    let found = descriptions(&answer, case);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one description of {directory_id}, got {found:?}"
    );
    found[0].clone()
}

/// The `DomainControllers` `DescribeDomainControllers` reports for `directory_id`, in the
/// order it reports them.
async fn controllers(router: &MultiServiceRouter, directory_id: &str, case: &str) -> Vec<Value> {
    let answer = ok(
        router,
        "DescribeDomainControllers",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["DomainControllers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected DomainControllers, got {answer}"))
        .clone()
}

/// A `CreateMicrosoftAD` body placing a directory in `subnets` of the VPC `vpc` — the one
/// the account really holds, which the create resolves against the owner of VPCs.
fn microsoft_ad_in(name: &str, vpc: &str, subnets: &[AccountSubnet]) -> Value {
    json!({
        "Name": name,
        "Password": PASSWORD,
        "VpcSettings": {
            "VpcId": vpc,
            "SubnetIds": subnets.iter().map(|subnet| subnet.id.clone()).collect::<Vec<_>>(),
        }
    })
}

/// RULE: a Managed Microsoft AD directory serves DNS at one address per subnet its create
/// named, each inside that subnet's own block, and its domain controllers report the same
/// addresses, subnets and zones.
#[tokio::test]
async fn a_microsoft_ad_serves_one_dns_address_per_chosen_subnet_inside_its_block() {
    let network = network().await;
    let router = &network.router;

    // The rows differ in the blocks the chosen subnets carry (two equal /24s, a /20 beside a
    // /28, and the account's own default subnets, whose blocks this suite never writes) and
    // in the order the create names them.
    let rows: [(&str, Vec<AccountSubnet>); 4] = [
        (
            "two /24 subnets",
            vec![network.client[0].clone(), network.client[1].clone()],
        ),
        (
            "a /20 and a /28",
            vec![network.client[3].clone(), network.client[2].clone()],
        ),
        (
            "the same pair named in the other order",
            vec![network.client[2].clone(), network.client[3].clone()],
        ),
        (
            "the account's default subnets",
            vec![network.default[0].clone(), network.default[1].clone()],
        ),
    ];

    for (index, (case, subnets)) in rows.iter().enumerate() {
        let name = format!("dns{index}.example.com");
        let directory_id =
            create_microsoft_ad(router, microsoft_ad_in(&name, &network.vpc, subnets), case).await;
        // The servers a create places come up on the clock, so the description is read once
        // the directory reports itself in service rather than after a fixed wait.
        in_service(router, &directory_id, case).await;

        let described = describe_one(router, &directory_id, case).await;
        assert_eq!(
            described["Stage"],
            json!(STAGE_ACTIVE),
            "case {case:?}: expected the directory in service before its DNS is read, got \
             {described}"
        );
        let addresses = dns_ip_addrs(&described, case);
        assert_eq!(
            addresses.len(),
            subnets.len(),
            "case {case:?}: expected one DNS address per chosen subnet, got {addresses:?}"
        );
        for (address, subnet) in addresses.iter().zip(subnets) {
            assert_address_in(address, &subnet.cidr, case);
        }
        assert_eq!(
            addresses.iter().collect::<BTreeSet<_>>().len(),
            addresses.len(),
            "case {case:?}: expected the directory's servers to hold distinct addresses, got \
             {addresses:?}"
        );

        // The Availability Zones come from the subnets themselves, as EC2 reports them.
        let mut expected_zones: Vec<String> =
            subnets.iter().map(|subnet| subnet.zone.clone()).collect();
        expected_zones.sort();
        expected_zones.dedup();
        assert_eq!(
            described["VpcSettings"]["AvailabilityZones"],
            json!(expected_zones),
            "case {case:?}: expected the zones of the chosen subnets, got {described}"
        );

        // What the servers report ONE BY ONE agrees with the directory's DNS: a client that
        // reaches a controller by address reaches a server the directory serves DNS at.
        let listed = controllers(router, &directory_id, case).await;
        assert_eq!(
            listed.len(),
            subnets.len(),
            "case {case:?}: expected one controller per chosen subnet, got {listed:?}"
        );
        for (controller, subnet) in listed.iter().zip(subnets) {
            assert_eq!(
                controller["SubnetId"],
                json!(subnet.id),
                "case {case:?}: expected the controller in {}, got {controller}",
                subnet.id
            );
            assert_eq!(
                controller["AvailabilityZone"],
                json!(subnet.zone),
                "case {case:?}: expected the zone EC2 reports for {}, got {controller}",
                subnet.id
            );
            let address = controller["DnsIpAddr"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a DnsIpAddr on the controller, got {controller}")
            });
            assert_address_in(address, &subnet.cidr, case);
            assert!(
                addresses.iter().any(|served| served == address),
                "case {case:?}: expected the controller address {address} among the \
                 directory's DNS addresses {addresses:?}"
            );
        }

        // Reading again reports the same addresses: a describe allocates nothing.
        let again = describe_one(router, &directory_id, case).await;
        assert_eq!(
            dns_ip_addrs(&again, case),
            addresses,
            "case {case:?}: expected a second read to report the same addresses, got {again}"
        );
    }
}

/// RULE: the same placement rule holds for a Simple AD directory, whose servers this service
/// runs but does not report one by one — `DnsIpAddrs` is one address per chosen subnet,
/// inside that subnet's block.
#[tokio::test]
async fn a_simple_ad_serves_one_dns_address_per_chosen_subnet_inside_its_block() {
    let network = network().await;
    let router = &network.router;

    // The rows differ in the blocks of the chosen subnets and in whether they are the
    // client's own or the account's default ones.
    let rows: [(&str, Vec<AccountSubnet>); 3] = [
        (
            "two /24 subnets",
            vec![network.client[0].clone(), network.client[1].clone()],
        ),
        (
            "a /28 and a /20",
            vec![network.client[2].clone(), network.client[3].clone()],
        ),
        (
            "the account's default subnets",
            vec![network.default[0].clone(), network.default[1].clone()],
        ),
    ];

    for (index, (case, subnets)) in rows.iter().enumerate() {
        let body = json!({
            "Name": format!("simple{index}.example.com"),
            "Password": PASSWORD,
            "Size": "Small",
            "VpcSettings": {
                "VpcId": &network.vpc,
                "SubnetIds": subnets.iter().map(|subnet| subnet.id.clone()).collect::<Vec<_>>(),
            }
        });
        let directory_id = create_directory(router, body, case).await;
        // The servers a create places come up on the clock, so the description is read once
        // the directory reports itself in service rather than after a fixed wait.
        in_service(router, &directory_id, case).await;

        let described = describe_one(router, &directory_id, case).await;
        let addresses = dns_ip_addrs(&described, case);
        assert_eq!(
            addresses.len(),
            subnets.len(),
            "case {case:?}: expected one DNS address per chosen subnet, got {addresses:?}"
        );
        for (address, subnet) in addresses.iter().zip(subnets) {
            assert_address_in(address, &subnet.cidr, case);
        }
    }
}

/// RULE: a directory that runs MORE servers than it has subnets serves one DNS address per
/// server, and the second server of a subnet takes another address of that same block rather
/// than the first one again.
#[tokio::test]
async fn every_domain_controller_of_a_directory_adds_its_own_dns_address() {
    let network = network().await;
    let router = &network.router;
    let case = "a resize past the subnet count";
    let subnets = vec![network.client[0].clone(), network.client[2].clone()];

    let directory_id = create_microsoft_ad(
        router,
        microsoft_ad_in("resize.example.com", &network.vpc, &subnets),
        case,
    )
    .await;
    // The servers a create places come up on the clock, so the addresses are read once the
    // directory reports itself in service rather than after a fixed wait.
    in_service(router, &directory_id, case).await;
    let created = dns_ip_addrs(&describe_one(router, &directory_id, case).await, case);

    // The rows walk the server count up past the number of subnets, so a subnet has to carry
    // a second and a third server.
    for desired in [3_i64, 4, 5] {
        let case = &format!("{case}: DesiredNumber {desired}");
        ok(
            router,
            "UpdateNumberOfDomainControllers",
            json!({ "DirectoryId": &directory_id, "DesiredNumber": desired }),
            case,
        )
        .await;

        let wanted = usize::try_from(desired).unwrap_or_default();
        // The resize has to RUN before the next number can be asked for, and before the
        // addresses it adds are the settled set.
        resize_settled(router, &directory_id, wanted, case).await;
        let addresses = dns_addresses_settling_at(router, &directory_id, wanted, case).await;
        let described = describe_one(router, &directory_id, case).await;
        assert_eq!(
            i64::try_from(addresses.len()).unwrap_or_default(),
            desired,
            "case {case:?}: expected one DNS address per server, got {addresses:?} ({described})"
        );
        assert_eq!(
            addresses.iter().collect::<BTreeSet<_>>().len(),
            addresses.len(),
            "case {case:?}: expected every server to hold its own address, got {addresses:?}"
        );
        // Each address still belongs to one of the subnets the create named, and the ones
        // the create allocated are unchanged.
        for address in &addresses {
            let blocks = subnets.iter().map(|subnet| subnet.cidr.clone()).collect::<Vec<_>>();
            assert!(
                subnets.iter().any(|subnet| {
                    subnet
                        .cidr
                        .parse::<Ipv4Net>()
                        .is_ok_and(|block| address.parse::<Ipv4Addr>().is_ok_and(|parsed| block.contains(&parsed)))
                }),
                "case {case:?}: expected {address} inside one of {blocks:?}"
            );
        }
        assert_eq!(
            &addresses[..created.len()],
            &created[..],
            "case {case:?}: expected the servers the create placed to keep their addresses, \
             got {addresses:?}"
        );
    }
}

/// RULE: an AD Connector serves no DNS of its own — `DnsIpAddrs` reports the self-managed
/// directory's servers its create named, while the connector's OWN servers are placed one
/// per chosen subnet and reported as `ConnectIps`.
#[tokio::test]
async fn an_ad_connector_serves_the_self_managed_directorys_dns_servers() {
    let network = network().await;
    let router = &network.router;
    let case = "a connector in two client subnets";
    let subnets = [network.client[1].clone(), network.client[3].clone()];

    let body = json!({
        "Name": "connected.example.com",
        "Password": "Sim2Real-Connect-Pw1!",
        "Size": "Small",
        "ConnectSettings": {
            "VpcId": &network.vpc,
            "SubnetIds": subnets.iter().map(|subnet| subnet.id.clone()).collect::<Vec<_>>(),
            "CustomerDnsIps": CUSTOMER_DNS_IPS,
            "CustomerUserName": "ad-connector-svc",
        }
    });
    let directory_id = connect_directory(router, body, case).await;
    // The connector's servers come up on the clock, so the description is read once the
    // connector reports itself in service rather than after a fixed wait.
    in_service(router, &directory_id, case).await;

    let described = describe_one(router, &directory_id, case).await;
    assert_eq!(
        dns_ip_addrs(&described, case),
        CUSTOMER_DNS_IPS.map(String::from).to_vec(),
        "case {case:?}: expected the self-managed directory's DNS servers, got {described}"
    );

    let connect_ips: Vec<String> = described["ConnectSettings"]["ConnectIps"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConnectIps, got {described}"))
        .iter()
        .filter_map(|address| address.as_str().map(str::to_string))
        .collect();
    assert_eq!(
        connect_ips.len(),
        subnets.len(),
        "case {case:?}: expected one connector server per chosen subnet, got {connect_ips:?}"
    );
    for (address, subnet) in connect_ips.iter().zip(&subnets) {
        assert_address_in(address, &subnet.cidr, case);
    }
    let mut expected_zones: Vec<String> = subnets.iter().map(|subnet| subnet.zone.clone()).collect();
    expected_zones.sort();
    assert_eq!(
        described["ConnectSettings"]["AvailabilityZones"],
        json!(expected_zones),
        "case {case:?}: expected the zones of the chosen subnets, got {described}"
    );
}

/// RULE: nothing is invented for a subnet the account does not have — a directory placed in
/// identifiers the owner of subnets reports nothing for serves no address at all, rather
/// than one made up here.
#[tokio::test]
async fn a_directory_placed_in_subnets_the_account_does_not_have_serves_no_address() {
    let network = network().await;
    let router = &network.router;

    // (case, the subnets the create names, how many of them the account has)
    let rows: [(&str, Vec<String>, usize); 3] = [
        (
            "two identifiers the account has no subnet for",
            vec![ABSENT_SUBNET.to_string(), "subnet-0000beef".to_string()],
            0,
        ),
        (
            "one held subnet and one absent",
            vec![network.client[0].id.clone(), ABSENT_SUBNET.to_string()],
            1,
        ),
        (
            "an absent subnet named first",
            vec![ABSENT_SUBNET.to_string(), network.client[1].id.clone()],
            1,
        ),
    ];

    for (index, (case, subnet_ids, held)) in rows.iter().enumerate() {
        let body = json!({
            "Name": format!("absent{index}.example.com"),
            "Password": PASSWORD,
            "VpcSettings": { "VpcId": &network.vpc, "SubnetIds": subnet_ids }
        });
        let answer = ok(router, "CreateMicrosoftAD", body, case).await;
        let directory_id = string(&answer, "DirectoryId", case);
        // The servers a create places come up on the clock, so the description is read once
        // the directory reports itself in service rather than after a fixed wait.
        in_service(router, &directory_id, case).await;

        let described = describe_one(router, &directory_id, case).await;
        // The placement the request named is still reported back as the request spelled it:
        // this service echoes the settings block whatever the owner of subnets holds.
        assert_eq!(
            described["VpcSettings"]["SubnetIds"],
            json!(subnet_ids),
            "case {case:?}: expected the requested SubnetIds, got {described}"
        );
        let served = described["DnsIpAddrs"]
            .as_array()
            .map(|addresses| addresses.len())
            .unwrap_or_default();
        assert_eq!(
            served, *held,
            "case {case:?}: expected an address only for the subnets the account has, got \
             {described}"
        );
    }
}
