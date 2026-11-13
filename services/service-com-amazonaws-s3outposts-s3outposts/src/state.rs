// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3outposts-s3outposts service.
//!
//! This API has two resources. An **Outpost** carries the S3 on Outposts
//! capacity and is reported by `ListOutpostsWithS3`; an **endpoint** is what a
//! VPC reaches that capacity through, minted by `CreateEndpoint`, reported by
//! `ListEndpoints` / `ListSharedEndpoints` and torn down by `DeleteEndpoint`.
//! [`OutpostRecord`] and [`EndpointRecord`] below are the one copy of each that
//! every handler of this service reads and writes.
//!
//! ## An endpoint an accepted call is still working on
//!
//! Neither `CreateEndpoint` nor `DeleteEndpoint` is finished when it answers —
//! each documents that "it can take up to 5 minutes for this action to finish" —
//! so an accepted call leaves the endpoint in the transient `EndpointStatus`
//! value for that action (`Pending`, `Deleting`) together with the
//! [`PendingTransition`] the clock owes it: the status the create ends in, or the
//! removal the teardown ends in. Until that instant the record is a record like
//! any other: the listings report it with its transient status, it still holds
//! the subnet, security group and CIDR it committed, and a second delete of it
//! resolves. [`ComAmazonawsS3outpostsS3outpostsState::settle_all`] is what
//! materialises everything already owed, so nothing sleeps and nothing depends
//! on being called.
//!
//! ## Where the Outposts come from
//!
//! An Outpost is hardware Amazon installs at a customer site, and this service
//! models NO operation that creates one — `ListOutpostsWithS3` only reports the
//! Outposts "that have S3 provisioned ... to which the calling Amazon Web
//! Services account has access". The service that DOES own that resource is AWS
//! Outposts, which this emulator co-hosts, so the Outposts reported here are the
//! ones that control plane holds: this service keeps no copy of them and resolves
//! the inventory from it on every call ([`outpost_inventory`]), so an Outpost
//! created, ordered for or decommissioned through the `outposts` API is one this
//! service answers for at the same instant. An Outpost that LEAVES that inventory
//! takes the endpoints stored on it with it — they are named by it and reached
//! only through it — which is what [`settled_inventory`] materialises on every
//! call, beside what the clock owes.
//!
//! Which of them belong in the inventory is GATED, not assumed: S3 capacity is
//! installed on an Outpost by ordering hardware that carries it, so an Outpost
//! enters this inventory only once the orders placed on it buy S3 storage
//! capacity, and only while it is the "Active (not pending any provisioning nor
//! decommissioned)" Outpost its own control plane reports — otherwise this
//! service would report capacity, and endpoints, for hardware nobody ordered.
//! [`OutpostRecord::capacity_in_bytes`] is that ordered capacity.
//!
//! An Outpost of ANOTHER account — one shared with the caller through Resource
//! Access Manager, which the emulated control plane holds none of, since it serves
//! one account — is declared by the operator running this emulator instead,
//! through [`PROVISIONED_OUTPOSTS_ENV`]. Unset — the default — means no shared
//! capacity, and an account whose control plane holds no S3-capable Outpost
//! either has none at all, which is what a new AWS account has. The gate above is
//! what it declares AGAINST, never around: an id that control plane holds an
//! Outpost under is that control plane's to answer for, so the declaration is the
//! answer only for an id it holds none under ([`outpost_inventory`]).
//!
//! ## Which Region a resource belongs to
//!
//! An Outpost is installed at a site anchored to ONE Region, and the S3 on
//! Outposts endpoints on it are reached through that Region's endpoint, so each
//! resource here belongs to one Region and is reachable only through it:
//! [`OutpostRecord::region`] records the Outpost's own (home) Region, an endpoint
//! takes the Region of the Outpost it is on ([`outpost_region`]),
//! and every lookup and listing resolves inside the Region the request was signed
//! for alone ([`request_region`]) — so a resource of another Region is reported by
//! no listing and resolves to the modelled 404, exactly as if the store held one
//! set of resources per Region. That holds of the EC2 resources an endpoint's
//! members are derived from too, since a subnet and the VPC it belongs to are a
//! Region's own: the subnet a client names in one Region is a different subnet
//! from the one it names under the same id in another
//! ([`ComAmazonawsS3outpostsS3outpostsState::subnet_vpcs`]).
//!
//! Every ARN this service reports (`Outpost$OutpostArn`, `Outpost$S3OutpostArn`,
//! `Endpoint$EndpointArn`, `CreateEndpointResult$EndpointArn`) is therefore built
//! from the record's ids and owner account plus the Region the RECORD belongs to —
//! and that Region's own partition ([`arn_partition`]) — never from the Region of
//! whoever is reading: an ARN identifies its resource, so the same record is
//! reported under one ARN to every caller, and an endpoint's ARN and its parent
//! Outpost's ARN always name the same Region.
//!
//! ## Why an ARN member can be ABSENT
//!
//! A value has to be one an ARN member's own pattern admits before that member
//! can name it, and that holds for EVERY segment the pattern closes, not just
//! one of them. Two of these segments are closed alternations:
//!
//! * the Outpost segment. `OutpostId` admits three branches
//!   (`op-[a-f0-9]{17}`, `\d{12}` and `ec2`) and the ARN shapes admit DIFFERENT
//!   subsets of them — [`OUTPOST_ARN_BRANCHES`], [`S3_OUTPOST_ARN_BRANCHES`] and
//!   [`ENDPOINT_ARN_BRANCHES`];
//! * the partition segment. Every one of these patterns spells its partition
//!   field out as a fixed alternation ([`ARN_PARTITIONS`]), which does not list
//!   every partition a Region can belong to.
//!
//! So for some Outposts, and for a Region in a partition outside that
//! alternation, there is no legal value for one of the ARNs at all. Each ARN
//! member is optional, so the projection leaves out the member it cannot form
//! instead of reporting a string the member's own pattern rejects; everything
//! else about the resource is reported as usual.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_outposts_outpostsolafservice::state as outposts_state;

use crate::ops::types::{
    Endpoint, NetworkInterface, Outpost, ResourceNotFoundException, ValidationException,
};

/// `EndpointStatus`'s transient value: "Pending".
pub const ENDPOINT_STATUS_PENDING: &str = "Pending";

/// `EndpointStatus`'s ready value: "Available".
pub const ENDPOINT_STATUS_AVAILABLE: &str = "Available";

/// `EndpointStatus`'s teardown value: "Deleting" — what an endpoint whose
/// deletion has been accepted reports while the teardown is still running.
pub const ENDPOINT_STATUS_DELETING: &str = "Deleting";

/// `EndpointAccessType`'s "Private", which `CreateEndpointRequest$AccessType`
/// documents as "the default access type value".
pub const ACCESS_TYPE_PRIVATE: &str = "Private";

/// `EndpointAccessType`'s "CustomerOwnedIp".
pub const ACCESS_TYPE_CUSTOMER_OWNED_IP: &str = "CustomerOwnedIp";

/// The largest page the three list operations serve, and the page they serve
/// when the request names no `MaxResults`.
///
/// `MaxResults` is modelled `@range(min: 0, max: 100)` with `@default(0)`, and
/// `ListOutpostsWithS3Request$MaxResults` documents "The limit is 100".
pub const PAGE_LIMIT: i32 = 100;

/// How long an endpoint stays in the transient status an accepted call put it
/// in before the clock owes it the next one: `Pending` before `Available` after
/// a create, `Deleting` before the record is gone after a delete.
///
/// `CreateEndpoint` and `DeleteEndpoint` each document that "it can take up to 5
/// minutes for this action to finish"; the emulator chooses a span instead, and
/// it is deliberately NOT zero, so a caller listing endpoints straight after
/// either call still sees the transient status. The due instant is stored on the
/// record and materialised by whichever later call reports the endpoint —
/// nothing sleeps.
pub const ENDPOINT_TRANSITION: Duration = Duration::from_millis(500);

/// The Region a request is served in when it carries no SigV4 credential scope to
/// read one from, and the home Region an operator who names none provisions an
/// Outpost in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The environment variable an operator uses to tell this emulated service which
/// FURTHER Outposts have S3 on Outposts capacity the caller reaches — the ones
/// its own AWS Outposts control plane cannot hold, because that control plane
/// serves one account: comma-separated
/// `<OutpostId>:<CapacityInBytes>[:<OwnerAccountId>[:<Region>]]` entries, read
/// once when this service's store is created
/// (`S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248"`).
///
/// Unset means the inventory is exactly what that control plane has S3 capacity
/// on ([`outpost_inventory`]) — none, for an account that never ordered any,
/// where `ListOutpostsWithS3` reports an empty inventory and every reference to
/// an Outpost is the modelled 404. An entry whose owner is
/// another account is one shared with the caller through Resource Access
/// Manager, which is what `Outpost$OwnerId` ("useful for comparing owned versus
/// shared outposts") distinguishes and what a caller can create endpoints on and
/// list with `ListSharedEndpoints`. An entry's Region is the home Region of the
/// Outpost it declares — the only Region that Outpost, and every endpoint on it,
/// is reachable through — and defaults to [`DEFAULT_REGION`].
pub const PROVISIONED_OUTPOSTS_ENV: &str = "S3_OUTPOSTS_PROVISIONED_OUTPOSTS";

/// The Outposts `setting` declares: comma-separated
/// `<OutpostId>:<CapacityInBytes>[:<OwnerAccountId>[:<Region>]]` entries,
/// surrounding space trimmed. The owner defaults to the account this emulator
/// serves, so an operator naming only an id and a capacity gets an Outpost the
/// caller owns, in [`DEFAULT_REGION`].
///
/// An entry that is not of that form, or whose values are not ones this API
/// could report (see [`OutpostRecord::provisioned`]), is SKIPPED rather than
/// guessed at: a typo provisions nothing instead of provisioning something
/// unintended.
pub fn parse_provisioned_outposts(setting: &str) -> Vec<OutpostRecord> {
    setting
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| {
            let mut fields = entry.split(':').map(str::trim);
            let outpost_id = fields.next()?;
            let capacity_in_bytes: i64 = fields.next()?.parse().ok()?;
            let owner_id = match fields.next() {
                Some(owner) => owner.to_string(),
                None => caller_account(),
            };
            let region = fields.next().unwrap_or(DEFAULT_REGION);
            // A fifth field means the entry is not the form documented above,
            // so it says something this parser does not understand.
            if fields.next().is_some() {
                return None;
            }
            OutpostRecord::provisioned(outpost_id, &owner_id, capacity_in_bytes, region)
        })
        .collect()
}

/// How many `/26` CIDRs fit in the `/16` the emulator reserves for endpoint
/// commitments — and so the largest number of endpoints the store holds at once.
const ENDPOINT_CIDR_BLOCK_COUNT: u32 = 1024;

/// What the clock owes an endpoint once the due instant has passed.
///
/// Both of this service's asynchronous actions are the same shape — the call is
/// accepted, the endpoint reports a transient status, and the work completes
/// later — but they differ in what completion IS: a create ends with the
/// endpoint in another status, a delete ends with no record at all.
#[derive(Debug, Clone)]
pub enum PendingEffect {
    /// The endpoint carries this `EndpointStatus` value from the due instant on.
    Status(String),
    /// The endpoint is gone from the due instant on: the teardown an accepted
    /// delete started has finished, so the record and everything it held are
    /// released.
    Removal,
}

/// A change the clock owes an endpoint: the instant it falls due, and what
/// becomes of the endpoint from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub effect: PendingEffect,
}

impl PendingTransition {
    /// The status change `due` owes, as an accepted create owes `Available`.
    pub fn to_status(due: SystemTime, status: &str) -> Self {
        Self {
            due,
            effect: PendingEffect::Status(status.to_string()),
        }
    }

    /// The removal `due` owes, as an accepted delete owes the end of the
    /// endpoint's teardown.
    pub fn to_removal(due: SystemTime) -> Self {
        Self {
            due,
            effect: PendingEffect::Removal,
        }
    }

    /// Whether `now` has reached the instant this change falls due.
    pub fn is_due(&self, now: SystemTime) -> bool {
        now >= self.due
    }
}

/// One Outpost with S3 on Outposts capacity, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct OutpostRecord {
    /// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`.
    pub outpost_id: String,
    /// The 12-digit account that owns the Outpost — the caller's own account for
    /// an owned Outpost, another account for one shared with the caller.
    pub owner_id: String,
    pub capacity_in_bytes: i64,
    /// The Outpost's home Region: the one Region it, and every endpoint on it,
    /// is reachable through, and the Region every ARN naming it carries.
    pub region: String,
}

impl OutpostRecord {
    /// The record for an Outpost provisioned with S3 capacity, or `None` when the
    /// values are not ones this API could ever report: `OutpostId`'s
    /// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` for the id, `AwsAccountId`'s `^\d{12}$`
    /// for the owner, a capacity above zero — an Outpost with no S3 capacity
    /// is not one `ListOutpostsWithS3` reports at all — and a Region code the
    /// ARN members' own Region field admits ([`is_region`]).
    pub fn provisioned(
        outpost_id: &str,
        owner_id: &str,
        capacity_in_bytes: i64,
        region: &str,
    ) -> Option<Self> {
        (is_outpost_id(outpost_id)
            && is_account_outpost_id(owner_id)
            && capacity_in_bytes > 0
            && is_region(region))
        .then(|| Self {
            outpost_id: outpost_id.to_string(),
            owner_id: owner_id.to_string(),
            capacity_in_bytes,
            region: region.to_string(),
        })
    }

    /// `arn:<partition>:outposts:<region>:<owner>:outpost/<id>`, as
    /// `OutpostArn`'s pattern spells it, naming the Outpost's own home Region and
    /// the partition that Region belongs to — or `None` when that pattern admits
    /// no such value: for an Outpost whose id branch it does not list
    /// ([`OUTPOST_ARN_BRANCHES`]), or for a Region whose partition it does not
    /// list ([`admitted_arn_partition`]).
    pub fn outpost_arn(&self) -> Option<String> {
        let partition = admitted_arn_partition(&self.region)?;
        arn_admits_outpost(OUTPOST_ARN_BRANCHES, &self.outpost_id).then(|| {
            format!(
                "arn:{partition}:outposts:{}:{}:outpost/{}",
                self.region, self.owner_id, self.outpost_id
            )
        })
    }

    /// `arn:<partition>:s3-outposts:<region>:<owner>:outpost/<id>/s3`, as
    /// `S3OutpostArn`'s pattern spells it: the name RAM shares, naming the
    /// Outpost's own home Region — or `None` when that pattern admits no such
    /// value: for an Outpost whose id branch it does not list
    /// ([`S3_OUTPOST_ARN_BRANCHES`]), or for a Region whose partition it does not
    /// list ([`admitted_arn_partition`]).
    pub fn s3_outpost_arn(&self) -> Option<String> {
        let partition = admitted_arn_partition(&self.region)?;
        arn_admits_outpost(S3_OUTPOST_ARN_BRANCHES, &self.outpost_id).then(|| {
            format!(
                "arn:{partition}:s3-outposts:{}:{}:outpost/{}/s3",
                self.region, self.owner_id, self.outpost_id
            )
        })
    }

    /// The `Outpost` projection `ListOutpostsWithS3` returns.
    pub fn to_outpost(&self) -> Outpost {
        Outpost {
            outpost_arn: self.outpost_arn(),
            s3_outpost_arn: self.s3_outpost_arn(),
            outpost_id: Some(self.outpost_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            capacity_in_bytes: self.capacity_in_bytes,
        }
    }
}

/// One S3 on Outposts endpoint, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    /// `EndpointId`'s `^[a-zA-Z0-9]{19}$`, minted by
    /// [`ComAmazonawsS3outpostsS3outpostsState::mint_endpoint_id`].
    pub endpoint_id: String,
    /// The Outpost this endpoint is on.
    pub outpost_id: String,
    /// "The VPC CIDR committed by this endpoint", allocated by
    /// [`ComAmazonawsS3outpostsS3outpostsState::allocate_cidr_block`].
    pub cidr_block: String,
    /// One of `EndpointStatus`'s values.
    pub status: String,
    /// The status change the clock owes this endpoint, if any.
    pub pending: Option<PendingTransition>,
    pub creation_time: SystemTime,
    /// The `NetworkInterfaceId`s of the interfaces provisioned for the endpoint.
    pub network_interfaces: Vec<String>,
    pub vpc_id: String,
    pub subnet_id: String,
    pub security_group_id: String,
    /// One of `EndpointAccessType`'s values.
    pub access_type: String,
    pub customer_owned_ipv4_pool: Option<String>,
    /// The 12-digit account that created, and so owns, this endpoint.
    pub owner_account: String,
}

impl EndpointRecord {
    /// `arn:<partition>:s3-outposts:<region>:<owner>:outpost/<outpost>/endpoint/<id>`,
    /// as `EndpointArn`'s pattern spells it, in the partition `home_region`
    /// belongs to — or `None` when that pattern admits no such value: for an
    /// endpoint whose Outpost's id branch it does not list
    /// ([`ENDPOINT_ARN_BRANCHES`]), or for a Region whose partition it does not
    /// list ([`admitted_arn_partition`]).
    ///
    /// `home_region` is the Region of the Outpost this endpoint is on
    /// ([`outpost_region`]), which is the
    /// endpoint's own Region: an endpoint has no Region apart from its parent
    /// Outpost's, so the two ARNs always agree on it.
    pub fn endpoint_arn(&self, home_region: &str) -> Option<String> {
        let partition = admitted_arn_partition(home_region)?;
        arn_admits_outpost(ENDPOINT_ARN_BRANCHES, &self.outpost_id).then(|| {
            format!(
                "arn:{partition}:s3-outposts:{home_region}:{}:outpost/{}/endpoint/{}",
                self.owner_account, self.outpost_id, self.endpoint_id
            )
        })
    }

    /// Materialise the status change the clock owes this endpoint, if `now` is
    /// past its due instant.
    ///
    /// Only `status` moves: `CreationTime` is "the time the endpoint was
    /// created" and never changes. A removal the clock owes is NOT materialised
    /// here — the record does not own whether it is stored, so
    /// [`ComAmazonawsS3outpostsS3outpostsState::settle_all`] completes that one,
    /// off [`Self::removal_due`].
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if !pending.is_due(now) {
            return;
        }
        if let PendingEffect::Status(status) = &pending.effect {
            self.status = status.clone();
            self.pending = None;
        }
    }

    /// True when the clock owes this endpoint's REMOVAL by `now`: an accepted
    /// delete started its teardown and the span that teardown takes has passed.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            matches!(pending.effect, PendingEffect::Removal) && pending.is_due(now)
        })
    }

    /// Accept this endpoint's deletion: it reports `Deleting` from `now` on, and
    /// the clock owes its removal once [`ENDPOINT_TRANSITION`] has passed.
    ///
    /// Whatever the clock owed before is replaced — a create still `Pending`
    /// when the delete arrives is torn down rather than completed — but a
    /// teardown already running is left exactly as it was: a repeated delete
    /// does not push the completion of the first one further out.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.status == ENDPOINT_STATUS_DELETING {
            return;
        }
        self.status = ENDPOINT_STATUS_DELETING.to_string();
        self.pending = Some(PendingTransition::to_removal(now + ENDPOINT_TRANSITION));
    }

    /// The `Endpoint` structure every list operation reports.
    ///
    /// `home_region` is the Region of the Outpost this endpoint is on — see
    /// [`Self::endpoint_arn`].
    pub fn to_endpoint(&self, home_region: &str) -> Endpoint {
        Endpoint {
            endpoint_arn: self.endpoint_arn(home_region),
            outposts_id: Some(self.outpost_id.clone()),
            cidr_block: Some(self.cidr_block.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            network_interfaces: Some(
                self.network_interfaces
                    .iter()
                    .map(|id| NetworkInterface {
                        network_interface_id: Some(id.clone()),
                    })
                    .collect(),
            ),
            vpc_id: Some(self.vpc_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            security_group_id: Some(self.security_group_id.clone()),
            access_type: Some(self.access_type.clone()),
            customer_owned_ipv4_pool: self.customer_owned_ipv4_pool.clone(),
            // `FailedReason` is "the failure reason, if any, for a create or
            // delete endpoint operation", so it belongs to an endpoint in
            // `Create_Failed` or `Delete_Failed`. No path in this emulator
            // reaches either status — a create either stores the endpoint or is
            // refused up front, and a teardown this emulator accepted always
            // completes — so there is never a failure reason to report.
            failed_reason: None,
        }
    }
}

/// Per-service state for com-amazonaws-s3outposts-s3outposts. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsS3outpostsS3outpostsState {
    /// The Outposts of ANOTHER account that are shared with this one, keyed by
    /// id — which names ONE Outpost, installed in the one home Region its record
    /// carries ([`OutpostRecord::region`]), so the key needs no Region beside it.
    ///
    /// These are the entries the AWS Outposts control plane cannot hold, because
    /// it serves one account: an Outpost shared through Resource Access Manager
    /// belongs to its owner's account. They join the control plane's own Outposts
    /// into the one inventory every call resolves ([`outpost_inventory`]), which
    /// is what the listings page over and every `OutpostId` resolves against.
    ///
    /// BOUNDED: nothing a client can send adds an entry — the API models no
    /// operation that shares or provisions an Outpost, so the only writer is
    /// [`Self::provision_outpost`], reached from [`PROVISIONED_OUTPOSTS_ENV`]
    /// once when the store is created.
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// Every endpoint, keyed by `(OutpostId, EndpointId)` — the pair
    /// `DeleteEndpoint` names — which is also the stable order the endpoint
    /// listings page over. The `OutpostId` half is what puts the endpoint in a
    /// Region: it is the Outpost's, and so the endpoint's
    /// ([`outpost_region`]).
    ///
    /// BOUNDED: an endpoint is only stored once [`Self::allocate_cidr_block`]
    /// has committed one of the `ENDPOINT_CIDR_BLOCK_COUNT` ranges the emulator
    /// reserves, so the map never holds more than that many records and a create
    /// past the bound is refused rather than stored.
    pub endpoints: BTreeMap<(String, String), EndpointRecord>,
    /// The VPC the emulator has associated with a subnet, keyed by `(Region,
    /// SubnetId)`. `Endpoint$VpcId` is "the ID of the VPC used for the endpoint"
    /// and a subnet belongs to exactly one VPC; this emulator models no EC2
    /// subnets, so it mints one VPC id the first time a subnet is used and
    /// reports that same id for every later endpoint in the same subnet.
    ///
    /// A subnet, like the VPC it belongs to, is a resource of ONE Region, so the
    /// Region is half of the key here exactly as it qualifies every other lookup
    /// in this store: the same `SubnetId` named in two Regions names two
    /// different subnets, which carry their own VPC ids, and the endpoints of one
    /// Region never decide what the other's report. The Region of an entry is the
    /// endpoint's own — the home Region of the Outpost it is on
    /// ([`outpost_region`]).
    ///
    /// BOUNDED by [`Self::endpoints`]: an entry exists only for a subnet a
    /// stored endpoint of that Region sits in. [`Self::remove_endpoint`] drops
    /// the entry with the last endpoint of that Region in that subnet, so a
    /// create/delete sequence cannot grow this map — and a subnet that no
    /// endpoint is left in is simply one this emulator no longer knows a VPC for,
    /// which the next endpoint there mints afresh.
    pub subnet_vpcs: BTreeMap<(String, String), String>,
    /// The counter endpoint ids are minted from.
    pub endpoint_id_seq: u64,
    /// The counter VPC ids are minted from.
    pub vpc_id_seq: u64,
    /// The counter network-interface ids are minted from.
    pub network_interface_id_seq: u64,
    /// The next `/26` index [`Self::allocate_cidr_block`] offers.
    pub cidr_block_seq: u32,
}

impl Default for ComAmazonawsS3outpostsS3outpostsState {
    /// An account with NO resources: no endpoints, and no shared Outposts beyond
    /// the ones [`PROVISIONED_OUTPOSTS_ENV`] says were shared with it (none,
    /// unless it is set).
    ///
    /// Nothing is invented here. The Outposts of the account itself are the ones
    /// its AWS Outposts control plane holds, resolved per call by
    /// [`outpost_inventory`] rather than copied in here, so a store this fresh
    /// still answers for every Outpost that control plane has S3 capacity on.
    fn default() -> Self {
        let mut state = Self {
            outposts: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            subnet_vpcs: BTreeMap::new(),
            endpoint_id_seq: 0,
            vpc_id_seq: 0,
            network_interface_id_seq: 0,
            cidr_block_seq: 0,
        };

        for record in ::std::env::var(PROVISIONED_OUTPOSTS_ENV)
            .map(|setting| parse_provisioned_outposts(&setting))
            .unwrap_or_default()
        {
            state.provision_outpost(record);
        }

        state
    }
}

impl ComAmazonawsS3outpostsS3outpostsState {
    /// Record that the Outpost `record` describes is shared with this account,
    /// replacing the record of an Outpost already declared.
    ///
    /// This is the ONLY way an Outpost of ANOTHER account enters the store,
    /// because the API models no operation that shares one: it is reached from
    /// [`PROVISIONED_OUTPOSTS_ENV`] at init, and from a test standing up an
    /// account that has one shared with it. The account's OWN Outposts come from
    /// its AWS Outposts control plane instead ([`outpost_inventory`]).
    pub fn provision_outpost(&mut self, record: OutpostRecord) {
        self.outposts.insert(record.outpost_id.clone(), record);
    }

    /// Mint an unused endpoint id matching `EndpointId`'s `^[a-zA-Z0-9]{19}$`.
    ///
    /// A `u64` is at most 16 hex digits, so the width-19 padding always yields
    /// exactly 19 of them; the loop skips an id already taken, which is what
    /// makes a wrapped counter safe.
    pub fn mint_endpoint_id(&mut self) -> String {
        loop {
            self.endpoint_id_seq = self.endpoint_id_seq.wrapping_add(1);
            let id = format!("{:019x}", self.endpoint_id_seq);
            if !self.endpoints.keys().any(|(_, existing)| existing == &id) {
                return id;
            }
        }
    }

    /// The VPC id this emulator reports for the subnet `subnet_id` of `region`,
    /// minting one the first time that subnet is used there.
    ///
    /// `region` is the Region the subnet — and so the endpoint committing a range
    /// of it — belongs to: the home Region of the Outpost the endpoint is on
    /// ([`outpost_region`]). A subnet is a Region's own resource
    /// ([`Self::subnet_vpcs`]), so what one Region reports for an id is resolved
    /// from that Region's entries alone and an id named in another Region mints
    /// its own.
    pub fn vpc_for_subnet(&mut self, region: &str, subnet_id: &str) -> String {
        let key = (region.to_string(), subnet_id.to_string());
        if let Some(vpc_id) = self.subnet_vpcs.get(&key) {
            return vpc_id.clone();
        }
        self.vpc_id_seq = self.vpc_id_seq.wrapping_add(1);
        let vpc_id = format!("vpc-{:017x}", self.vpc_id_seq);
        self.subnet_vpcs.insert(key, vpc_id.clone());
        vpc_id
    }

    /// Mint a network-interface id for an endpoint's interface.
    pub fn mint_network_interface_id(&mut self) -> String {
        self.network_interface_id_seq = self.network_interface_id_seq.wrapping_add(1);
        format!("eni-{:017x}", self.network_interface_id_seq)
    }

    /// Commit the next free `/26` to an endpoint, or `None` when every block in
    /// the reserved range is already committed.
    ///
    /// `Endpoint$CidrBlock` is "the VPC CIDR committed by this endpoint". This
    /// emulator models no EC2 subnet to carve that range out of, so it reserves
    /// the RFC 1918 block `10.0.0.0/16` for endpoint commitments and hands each
    /// endpoint a distinct, aligned `/26` out of it: no two live endpoints are
    /// ever committed the same range, and a deleted endpoint's range is offered
    /// again.
    pub fn allocate_cidr_block(&mut self) -> Option<String> {
        for _ in 0..ENDPOINT_CIDR_BLOCK_COUNT {
            let index = self.cidr_block_seq % ENDPOINT_CIDR_BLOCK_COUNT;
            self.cidr_block_seq = self.cidr_block_seq.wrapping_add(1);
            let candidate = format!("10.0.{}.{}/26", index / 4, (index % 4) * 64);
            if !self
                .endpoints
                .values()
                .any(|endpoint| endpoint.cidr_block == candidate)
            {
                return Some(candidate);
            }
        }
        None
    }

    /// The endpoint already committing `subnet_id` and `security_group_id` on
    /// `outpost_id`, whichever account owns that record.
    ///
    /// What an endpoint holds is a range of a subnet, reached through a security
    /// group, committed to an Outpost — EC2 and Outpost resources, not the
    /// caller's own: a record another account owns holds them exactly as one of
    /// the caller's does, so it answers here too. Which account owns an endpoint
    /// decides who may REPORT or REMOVE it (`ListEndpoints`, `DeleteEndpoint`),
    /// never whether the resources it already holds are free for another
    /// endpoint.
    ///
    /// An endpoint whose teardown is still running holds them too: what it
    /// committed is released when that teardown completes, not when the delete
    /// was accepted, so a create inside that window conflicts with it.
    pub fn endpoint_committing(
        &self,
        outpost_id: &str,
        subnet_id: &str,
        security_group_id: &str,
    ) -> Option<&EndpointRecord> {
        self.endpoints.values().find(|endpoint| {
            endpoint.outpost_id == outpost_id
                && endpoint.subnet_id == subnet_id
                && endpoint.security_group_id == security_group_id
        })
    }

    /// Materialise every change the clock owes, for a caller that is about to
    /// report on or resolve against all of them: an endpoint whose teardown has
    /// finished is removed (releasing what it held), and every other owed status
    /// change is applied.
    ///
    /// Nothing here depends on being called: each change was already owed at
    /// `now` whether or not anybody asked, so two callers reading at the same
    /// instant see the same endpoints.
    ///
    /// `inventory` is what an endpoint's Region is read from ([`outpost_region`]),
    /// which is what releasing the Region-scoped VPC mapping of a removed record
    /// resolves in ([`Self::remove_endpoint`]).
    pub fn settle_all(&mut self, inventory: &OutpostInventory, now: SystemTime) {
        let completed: Vec<(String, String)> = self
            .endpoints
            .iter()
            .filter(|(_, endpoint)| endpoint.removal_due(now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &completed {
            self.remove_endpoint(inventory, key);
        }
        for endpoint in self.endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// Remove the endpoint `key` names and release what its record held: the
    /// `/26` it committed is offered to a later endpoint again, and the VPC
    /// mapping for its subnet is dropped once no endpoint is left in that
    /// subnet.
    ///
    /// This is never the START of a teardown: `DeleteEndpoint` only accepts the
    /// deletion ([`EndpointRecord::begin_deletion`]), and [`Self::settle_all`]
    /// calls this for the endpoint whose teardown the clock has since completed.
    /// The other thing that ends an endpoint is its Outpost leaving this
    /// service's inventory, which
    /// [`Self::release_endpoints_of_absent_outposts`] releases through here too.
    ///
    /// Releasing the mapping is what keeps [`Self::subnet_vpcs`] bounded by
    /// [`Self::endpoints`]: no sequence of creates and deletes can leave an
    /// entry behind for a subnet nothing is stored in.
    pub fn remove_endpoint(
        &mut self,
        inventory: &OutpostInventory,
        key: &(String, String),
    ) -> Option<EndpointRecord> {
        let removed = self.endpoints.remove(key)?;
        self.release_subnet_vpcs(inventory, &removed.subnet_id);
        Some(removed)
    }

    /// Drop the VPC mapping of `subnet_id` in every Region that has no reachable
    /// endpoint left in it.
    ///
    /// The mapping belongs to one Region ([`Self::subnet_vpcs`]), so it is
    /// released per Region: an entry survives exactly while an endpoint of that
    /// same Region sits in the subnet, and the endpoints of another Region keep
    /// no entry of this one alive. An endpoint whose Outpost `inventory` no
    /// longer holds is in no Region at all ([`outpost_region`]) — no listing
    /// reports it and no reference resolves it — so it keeps no entry alive
    /// either.
    fn release_subnet_vpcs(&mut self, inventory: &OutpostInventory, subnet_id: &str) {
        let endpoints = &self.endpoints;
        self.subnet_vpcs.retain(|(region, subnet), _| {
            subnet != subnet_id
                || endpoints.values().any(|endpoint| {
                    &endpoint.subnet_id == subnet
                        && outpost_region(inventory, &endpoint.outpost_id) == Some(region.as_str())
                })
        });
    }

    /// Release every endpoint whose parent Outpost `inventory` no longer holds,
    /// exactly as [`Self::remove_endpoint`] releases one whose teardown has
    /// completed, and answer the keys released.
    ///
    /// An endpoint exists only ON an Outpost and is reached only THROUGH it: it
    /// is named by the `(OutpostId, EndpointId)` pair, takes that Outpost's
    /// Region ([`outpost_region`]) and is reported by no listing and resolved by
    /// no reference once that Outpost is gone. The Outposts are another control
    /// plane's resource, resolved afresh on every call ([`outpost_inventory`]),
    /// so an Outpost leaves this inventory the instant that control plane stops
    /// holding it with S3 capacity — and the endpoints it carried have to go with
    /// it, or the store keeps records no operation can report, resolve or remove,
    /// holding a `/26` [`Self::allocate_cidr_block`] would never offer again and
    /// a [`Self::subnet_vpcs`] entry for a subnet no reachable endpoint sits in.
    ///
    /// Nothing here depends on being called: the release was owed at the instant
    /// the Outpost left the inventory, whether or not anybody asked, the same way
    /// a completed teardown is — so two callers reading at the same instant see
    /// the same endpoints.
    pub fn release_endpoints_of_absent_outposts(
        &mut self,
        inventory: &OutpostInventory,
    ) -> Vec<(String, String)> {
        let orphaned: Vec<(String, String)> = self
            .endpoints
            .keys()
            .filter(|(outpost_id, _)| !inventory.contains_key(outpost_id))
            .cloned()
            .collect();
        for key in &orphaned {
            self.remove_endpoint(inventory, key);
        }
        orphaned
    }
}

/// Every Outpost with S3 on Outposts capacity the caller reaches, keyed by
/// `OutpostId`.
///
/// This is what the inventory listing pages over and what every `OutpostId` a
/// request names resolves against. The `BTreeMap` is what gives
/// `ListOutpostsWithS3` the one stable total order its pagination cursor indexes
/// into: the ids of a store are the same set in the same order on every call, so a
/// cursor minted by one page names the same place in the next.
pub type OutpostInventory = BTreeMap<String, OutpostRecord>;

/// The inventory this request is served: the Outposts the AWS Outposts control
/// plane has S3 capacity provisioned on, plus the ones of other accounts the
/// operator declared as shared with this one ([`PROVISIONED_OUTPOSTS_ENV`]).
///
/// Resolved per call rather than copied into this service's store: AWS Outposts
/// owns the Outpost, so what this service reports has to be what that control
/// plane holds NOW — an Outpost created, ordered for or decommissioned there is one
/// this service answers for from the next call on. A declared entry for an id the
/// control plane also holds is overridden by the control plane's own record, which
/// is the authority on that resource — in BOTH directions: that control plane
/// holding the id with S3 capacity puts its own record in, and holding the id
/// WITHOUT it (still provisioning, being decommissioned, or with no order that
/// bought S3 storage capacity) takes the declared entry out, because an
/// `OutpostId` names one Outpost and a declaration cannot keep the authority's
/// own "no" out of the answer. Only an id that control plane holds no Outpost
/// under at all is the declaration's to answer for, which is the shared Outpost
/// of another account it serves none of.
///
/// `store` is this service's own store, already locked by the caller; the control
/// plane's store is locked for the length of the projection and released before the
/// answer is built. The lock is only ever taken in that order — this service reads
/// AWS Outposts and never the other way round.
pub fn outpost_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ComAmazonawsS3outpostsS3outpostsState,
) -> OutpostInventory {
    let mut inventory = store.outposts.clone();
    for (outpost_id, held) in control_plane_outposts(engine) {
        match held {
            Some(record) => inventory.insert(outpost_id, record),
            None => inventory.remove(&outpost_id),
        };
    }
    inventory
}

/// The inventory this request is served, with everything already owed to this
/// service's own records materialised against it: every handler starts here.
///
/// Two things are owed at the instant a request is served, and neither depends on
/// being asked for: what the clock owes an endpoint an accepted call is still
/// working on ([`ComAmazonawsS3outpostsS3outpostsState::settle_all`]), and the
/// release of every endpoint whose Outpost has left the inventory
/// ([`ComAmazonawsS3outpostsS3outpostsState::release_endpoints_of_absent_outposts`]).
/// Resolving the inventory is where this service learns the second, because the
/// Outposts belong to another control plane and are resolved afresh on every call
/// ([`outpost_inventory`]) — so both are materialised here, in ONE place, and no
/// operation answers from records a change was already owed on.
pub fn settled_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &mut ComAmazonawsS3outpostsS3outpostsState,
    now: SystemTime,
) -> OutpostInventory {
    let inventory = outpost_inventory(engine, store);
    store.settle_all(&inventory, now);
    store.release_endpoints_of_absent_outposts(&inventory);
    inventory
}

/// The verdict of the co-hosted AWS Outposts control plane on every `OutpostId` it
/// holds an Outpost under: the record this service reports that Outpost as, or
/// `None` when it holds the id but the Outpost under it is not one with S3 on
/// Outposts capacity for this account to reach ([`s3_provisioned_outpost`]).
///
/// A negative verdict is an answer about that id, not the absence of one, which is
/// what lets [`outpost_inventory`] honour the authority in both directions; an id
/// absent from this map is one that control plane holds no Outpost under at all.
type ControlPlaneVerdicts = BTreeMap<String, Option<OutpostRecord>>;

/// What the co-hosted AWS Outposts control plane holds, keyed by the `OutpostId` it
/// holds each Outpost under, or nothing at all when this process hosts no such
/// control plane: an emulator without it holds no Outposts of the account's own,
/// exactly as an account with no Outposts does.
fn control_plane_outposts<Eng: Send + Sync + 'static>(engine: &Eng) -> ControlPlaneVerdicts {
    let Some(control_plane) = outposts_state::service_state(engine) else {
        return ControlPlaneVerdicts::new();
    };
    let mut control_plane = control_plane.lock();
    // Whether an Outpost has finished provisioning is a fact about the instant it is
    // asked for, so the lifecycle transitions the clock ALREADY owed at this instant
    // are materialised before the statuses are read — the same thing every read of
    // that control plane's own does, and nothing a later caller would not have seen
    // anyway.
    control_plane.settle_outposts();
    let control_plane = &*control_plane;
    // The capacity of every Outpost at once, totalled in one pass over the orders, so
    // resolving the whole inventory costs the orders the control plane holds rather
    // than their product with its Outposts.
    let provisioned = control_plane.provisioned_s3_capacity();
    control_plane
        .outposts
        .iter()
        .map(|(outpost_id, record)| {
            (
                outpost_id.clone(),
                s3_provisioned_outpost(&provisioned, record),
            )
        })
        .collect()
}

/// The Outpost `record` describes as this service reports it, or `None` when it is
/// not one with S3 on Outposts capacity for this account to reach.
///
/// `ListOutpostsWithS3` reports the Outposts "that have S3 provisioned", are
/// "Active (not pending any provisioning nor decommissioned)" and are ones "to
/// which the the calling Amazon Web Services account has access", so all three hold
/// of every record here: one still provisioning or already being decommissioned is
/// left out, and so is one no order has bought S3 storage capacity for, whose
/// capacity would be zero — an Outpost with no S3 capacity is not one this listing
/// reports at all ([`OutpostRecord::provisioned`]).
///
/// `provisioned` is the capacity each Outpost's orders bought, by region and id.
fn s3_provisioned_outpost(
    provisioned: &BTreeMap<String, BTreeMap<String, i64>>,
    record: &outposts_state::OutpostRecord,
) -> Option<OutpostRecord> {
    if record.outpost.life_cycle_status.as_deref() != Some(outposts_state::LIFE_CYCLE_STATUS_ACTIVE)
    {
        return None;
    }
    if record.decommission_requested_at.is_some() {
        return None;
    }
    let outpost_id = record.outpost.outpost_id.as_deref()?;
    let owner_id = record.outpost.owner_id.as_deref()?;
    let capacity_in_bytes = provisioned
        .get(&record.region)
        .and_then(|in_region| in_region.get(outpost_id))
        .copied()
        .unwrap_or_default();
    OutpostRecord::provisioned(outpost_id, owner_id, capacity_in_bytes, &record.region)
}

/// The Outpost `outpost_id` as it is reached from `region`, or the modelled
/// refusal when the inventory holds no such Outpost in that Region.
///
/// An `OutpostId` names one Outpost, which sits in one home Region, so an
/// Outpost of another Region resolves to nothing here: every reference a
/// request makes to an Outpost — creating an endpoint on it, listing its
/// endpoints, deleting one — goes through this ONE resolution, so a resource of
/// another Region is the modelled 404 rather than something this Region's
/// endpoint serves.
///
/// The refusal itself lives here rather than once per handler, exactly as
/// [`list_cursor`]'s does: resolving the Outpost is the whole of what the
/// handlers taking an `OutpostId` share, so each caller only lifts the payload
/// ([`not_found`]) into its own error enum and reads what it needs — the home
/// Region, say — off the record this hands back.
pub fn resolve_outpost<'a>(
    inventory: &'a OutpostInventory,
    region: &str,
    outpost_id: &str,
) -> Result<&'a OutpostRecord, ResourceNotFoundException> {
    inventory
        .get(outpost_id)
        .filter(|outpost| outpost.region == region)
        .ok_or_else(not_found)
}

/// The Outpost `outpost_id` as an operation scoped to the Outposts ANOTHER account
/// has shared with this one reaches it, or the modelled refusal when this Region's
/// inventory holds no such Outpost or holds one the calling account owns itself.
///
/// An operation whose documented subject is a resource reached through a
/// cross-account share serves only resources another account owns: the caller's
/// own Outpost was never shared with it, so it is outside that operation's scope
/// and is refused exactly like an Outpost the inventory does not hold — the same
/// [`not_found`] payload every unresolvable reference under an `OutpostId`
/// answers with, since the two cases are one fact from the caller's side: no
/// Outpost this operation serves.
///
/// Which account owns the Outpost is already on the record [`resolve_outpost`]
/// hands back ([`OutpostRecord::owner_id`], "useful for comparing owned versus
/// shared outposts"), so the share is read off the inventory rather than tracked
/// beside it: the Region resolution stays the one every door shares, and this
/// adds only the qualifier the scoped operation's own subject states.
pub fn resolve_shared_outpost<'a>(
    inventory: &'a OutpostInventory,
    region: &str,
    outpost_id: &str,
) -> Result<&'a OutpostRecord, ResourceNotFoundException> {
    let outpost = resolve_outpost(inventory, region, outpost_id)?;
    if outpost.owner_id == caller_account() {
        return Err(not_found());
    }
    Ok(outpost)
}

/// The home Region of the Outpost `outpost_id`, or `None` when the inventory holds
/// no such Outpost.
///
/// This is the Region of every endpoint on that Outpost, and the Region their
/// ARNs name: an endpoint is reached through the Outpost it is on, so it has
/// no Region of its own to disagree with.
pub fn outpost_region<'a>(inventory: &'a OutpostInventory, outpost_id: &str) -> Option<&'a str> {
    inventory
        .get(outpost_id)
        .map(|outpost| outpost.region.as_str())
}

/// The account this emulator serves every request for: the one canonical
/// emulator account.
pub fn caller_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The Region this request was signed for — the Region whose resources it is
/// served, and so the Region every ARN in the answer names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `value` is a Region code this service's ARNs can name: non-empty and
/// inside the `[a-z\-0-9]*` charset the Region field of `OutpostArn`,
/// `S3OutpostArn` and `EndpointArn` publishes.
///
/// A Region a stored record carries ends up in those members, so one outside the
/// charset would make an ARN its own pattern rejects — which is why
/// [`OutpostRecord::provisioned`] refuses it instead of storing it. A REQUEST may
/// still be signed for anything: such a request simply resolves no resource,
/// since no record belongs to that Region.
pub fn is_region(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The partition field of every ARN this service builds for `region`: the
/// partition that Region belongs to, read off its region code.
///
/// An ARN names its resource in full, so its partition field and its Region
/// field describe the same place: a Region of the China partition can only be
/// named by an `aws-cn` ARN, a GovCloud Region only by an `aws-us-gov` one, and
/// so on. `OutpostArn`, `S3OutpostArn` and `EndpointArn` each spell the
/// alternation out (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`) beside a
/// Region field their pattern leaves open, so the partition is a function of the
/// Region and never a constant.
///
/// The prefixes and partition names are AWS's own, and the table carries EVERY
/// partition AWS publishes a Region prefix for — the Regions AWS added after this
/// service's patterns were published (the European Sovereign Cloud's `eusc-de-`
/// and the later isolated prefixes) get their own partition too. The Region
/// decides, and naming the commercial partition for a Region outside it is the
/// very disagreement this function exists to prevent; it is also what would make
/// this service contradict the AWS Outposts control plane its Outposts come out
/// of ([`outpost_inventory`]), which reads the same published table for the ARNs
/// it reports those very Outposts under. The longer `us-isob-` / `us-isof-`
/// prefixes are matched before `us-iso-` so each lands in its own partition.
///
/// This is the partition the Region belongs to, which is not the same question
/// as whether an ARN member can name it: [`admitted_arn_partition`] answers
/// that one, off [`ARN_PARTITIONS`].
#[must_use]
pub fn arn_partition(region: &str) -> &'static str {
    const PARTITION_PREFIXES: &[(&str, &str)] = &[
        ("cn-", "aws-cn"),
        ("us-gov-", "aws-us-gov"),
        ("eusc-de-", "aws-eusc"),
        ("us-isob-", "aws-iso-b"),
        ("us-isof-", "aws-iso-f"),
        ("eu-isoe-", "aws-iso-e"),
        ("us-iso-", "aws-iso"),
    ];
    PARTITION_PREFIXES
        .iter()
        .find(|(prefix, _)| region.starts_with(prefix))
        .map_or("aws", |(_, partition)| *partition)
}

/// The partitions the ARN members of this service admit, spelled out by the
/// alternation every one of their patterns opens with
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso|aws-iso-b):…`, identical in
/// `Outpost$OutpostArn`, `Outpost$S3OutpostArn`, `Endpoint$EndpointArn` and
/// `CreateEndpointResult$EndpointArn`).
///
/// It is a CLOSED list: a Region whose partition is not on it has no ARN any of
/// these members can carry, the same way an Outpost id outside
/// [`OUTPOST_ARN_BRANCHES`] has none.
pub const ARN_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov", "aws-iso", "aws-iso-b"];

/// True when an ARN member of this service can name a resource in `partition`
/// at all — that is, when its own alternation lists it ([`ARN_PARTITIONS`]).
fn arn_admits_partition(partition: &str) -> bool {
    ARN_PARTITIONS.contains(&partition)
}

/// The partition field an ARN of `region` carries, or `None` when the ARN
/// members' own alternation does not admit that Region's partition and so no
/// legal ARN exists for a resource there.
#[must_use]
pub fn admitted_arn_partition(region: &str) -> Option<&'static str> {
    let partition = arn_partition(region);
    arn_admits_partition(partition).then_some(partition)
}

/// The page a list operation serves for `max_results`.
///
/// `MaxResults` is modelled `@range(min: 0, max: 100)` with `@default(0)`, so a
/// request that names no page size reaches a handler as `0`: that is "no
/// preference", and the page served is then the limit the model publishes.
/// Anything outside the range was refused by the boundary validator before the
/// handler ran, which the debug assertion surfaces rather than silently
/// repairing.
pub fn page_size(max_results: i32) -> usize {
    debug_assert!(
        (0..=PAGE_LIMIT).contains(&max_results),
        "MaxResults {max_results} should have been refused at the boundary"
    );
    let requested = if max_results <= 0 {
        PAGE_LIMIT
    } else {
        max_results
    };
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).expect("a page size in 1..=100 fits in usize")
}

/// Which branch of `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` a value lies
/// in.
///
/// The branch is more than "is this an Outpost id": the ARN shapes this service
/// reports admit DIFFERENT subsets of the three branches
/// ([`OUTPOST_ARN_BRANCHES`], [`S3_OUTPOST_ARN_BRANCHES`],
/// [`ENDPOINT_ARN_BRANCHES`]), so the branch an Outpost's id lies in is what
/// decides which ARN members can name it at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutpostIdBranch {
    /// `op-[a-f0-9]{17}`: the Outpost's own id.
    Op,
    /// `\d{12}`: the account-id form.
    AccountId,
    /// The literal `ec2`.
    Ec2,
}

/// The branch of `OutpostId` `value` lies in, or `None` when `value` is no
/// Outpost id at all.
pub fn outpost_id_branch(value: &str) -> Option<OutpostIdBranch> {
    if value == "ec2" {
        return Some(OutpostIdBranch::Ec2);
    }
    if let Some(hex) = value.strip_prefix("op-") {
        return (hex.len() == 17 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then_some(OutpostIdBranch::Op);
    }
    is_account_outpost_id(value).then_some(OutpostIdBranch::AccountId)
}

/// The branches of `OutpostId` that `OutpostArn`'s
/// `…:outposts:<region>:<account>:outpost/(op-[a-f0-9]{17}|ec2)$` admits: the
/// account-id form is not one of them, so no `OutpostArn` can name an Outpost
/// identified that way.
pub const OUTPOST_ARN_BRANCHES: &[OutpostIdBranch] = &[OutpostIdBranch::Op, OutpostIdBranch::Ec2];

/// The branches of `OutpostId` that `S3OutpostArn`'s
/// `…:s3-outposts:<region>:<account>:outpost/(op-[a-f0-9]{17}|\d{12})/s3$`
/// admits: `ec2` is not one of them.
pub const S3_OUTPOST_ARN_BRANCHES: &[OutpostIdBranch] =
    &[OutpostIdBranch::Op, OutpostIdBranch::AccountId];

/// The branches of `OutpostId` that `EndpointArn`'s
/// `…:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/[a-zA-Z0-9]{19}$` admits: as with
/// `OutpostArn`, the account-id form is not one of them. Spelled out rather than
/// aliased, because this is its own pattern and nothing ties the two together.
pub const ENDPOINT_ARN_BRANCHES: &[OutpostIdBranch] = &[OutpostIdBranch::Op, OutpostIdBranch::Ec2];

/// True when an ARN member whose pattern admits the `admitted` branches can
/// name the Outpost `outpost_id` — that is, when there is a legal value of that
/// member for this Outpost at all.
pub fn arn_admits_outpost(admitted: &[OutpostIdBranch], outpost_id: &str) -> bool {
    outpost_id_branch(outpost_id).is_some_and(|branch| admitted.contains(&branch))
}

/// True when `value` satisfies `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`.
///
/// The boundary validator already refuses a modelled `OutpostId` member, so this
/// is for a value the model does not constrain: a `NextToken`, which this
/// service's pagination carries an Outpost id in.
pub fn is_outpost_id(value: &str) -> bool {
    outpost_id_branch(value).is_some()
}

/// True when `value` satisfies `EndpointId`'s `^[a-zA-Z0-9]{19}$`.
pub fn is_endpoint_id(value: &str) -> bool {
    value.len() == 19 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// One page of any listing of this service: the records it reports, projected
/// into the shape the listing answers with, and the cursor the caller resumes at
/// when the walk stopped before the end of the listing.
#[derive(Debug)]
pub struct ListPage<Item> {
    pub items: Vec<Item>,
    pub next_token: Option<String>,
}

/// The key a listing resumes its walk at for `next_token`, as `decode` reads one
/// out of a cursor: `None` for a request that carries no cursor, and the refusal
/// for a token this service never issued — which is answered rather than served
/// as an arbitrary page.
///
/// `operation` is the listing's own name and `decode` its cursor's own form;
/// those are all that differ between the listings, so the refusal itself lives
/// here. The caller lifts the payload into its own error enum.
pub fn list_cursor<Key>(
    next_token: Option<&str>,
    operation: &str,
    decode: impl FnOnce(&str) -> Option<Key>,
) -> Result<Option<Key>, ValidationException> {
    match next_token {
        None => Ok(None),
        Some(token) => match decode(token) {
            Some(key) => Ok(Some(key)),
            None => Err(ValidationException {
                message: Some(format!(
                    "NextToken {token:?} is not a token returned by {operation}"
                )),
            }),
        },
    }
}

/// The page every listing of this service serves: the `records` that `include`
/// accepts, in the store map's own key order, starting at `resume` and at most
/// [`page_size`] of them, each projected by `project`, plus the cursor `encode`
/// mints for the first key the page did not serve.
///
/// Each listing differs only in the store map it walks, the form of its cursor
/// (`encode`/`decode`, see [`list_cursor`]), which records it reports
/// (`include`) and the shape it projects them into (`project`) — so the walk
/// itself lives here: every listing then pages over one order and mints the same
/// cursor for the same key.
pub fn list_page<'a, Key, Record, Item>(
    records: impl IntoIterator<Item = (&'a Key, &'a Record)>,
    max_results: i32,
    resume: Option<Key>,
    mut include: impl FnMut(&Record) -> bool,
    encode: impl Fn(&Key) -> String,
    mut project: impl FnMut(&Record) -> Item,
) -> ListPage<Item>
where
    Key: Ord + 'a,
    Record: 'a,
{
    let page_size = page_size(max_results);
    let mut items = Vec::with_capacity(page_size);
    let mut next_token = None;
    for (key, record) in records {
        if !include(record) {
            continue;
        }
        // Resume at the first key at or after the cursor, so a page is still
        // served when the record the cursor named was deleted meanwhile.
        if resume.as_ref().is_some_and(|from| key < from) {
            continue;
        }
        if items.len() == page_size {
            next_token = Some(encode(key));
            break;
        }
        items.push(project(record));
    }

    ListPage { items, next_token }
}

/// The Outpost id the inventory listing's cursor FORM names, or `None` when the
/// token is not of the form this listing mints.
///
/// The inventory is keyed by `OutpostId`, so the cursor IS that id — which
/// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` keeps inside `NextToken`'s own
/// `^[A-Za-z0-9\+\:\/\=\?\#-_]+$`, so the cursor a response carries is a value
/// the next request may legally send back.
///
/// This is the form alone; which of those ids the listing being resumed could
/// actually have minted a cursor for is [`outpost_cursor`]'s question, because it
/// depends on the Region the walk is being resumed in.
pub fn decode_outpost_cursor(token: &str) -> Option<String> {
    is_outpost_id(token).then(|| token.to_string())
}

/// The key the Outpost inventory resumes at for `next_token`, or the refusal for
/// a token this listing never issued — which a token naming an Outpost installed
/// in another Region is.
///
/// A cursor is a position in ONE walk, so it is read in the whole scope that walk
/// serves and not just checked for its form: this walk reports the Outposts the
/// `inventory` holds in `region` alone ([`outpost_page`]), and an Outpost is
/// installed at a site anchored to exactly one Region
/// ([`OutpostRecord::region`]), so an id the inventory holds in ANOTHER Region
/// names no place in this walk's order. Resuming there would answer a page of
/// whatever happens to sort after it — an empty one, reporting that a Region
/// whose untokened listing reports several S3-capable Outposts reaches none, when
/// the foreign id sorts last — or ignore the cursor altogether and re-serve the
/// first page. It is refused instead, exactly as a token of the wrong form is.
///
/// An id the inventory holds NOWHERE is deliberately admitted, for the same
/// reason [`list_page`] resumes at the first key at or after the cursor: the
/// inventory is the Outposts control plane's own and an Outpost can leave it
/// between two pages, and that departed record is exactly the one this listing's
/// cursor names.
pub fn outpost_cursor(
    next_token: Option<&str>,
    operation: &str,
    inventory: &OutpostInventory,
    region: &str,
) -> Result<Option<String>, ValidationException> {
    list_cursor(next_token, operation, |token| {
        decode_outpost_cursor(token).filter(|outpost_id| {
            outpost_region(inventory, outpost_id).is_none_or(|home| home == region)
        })
    })
}

/// The page the Outpost inventory serves: the Outposts the inventory holds IN
/// `region`, in its own `OutpostId` order, starting at `resume` and at most
/// [`page_size`] of them.
///
/// Every Outpost in the inventory has S3 capacity provisioned and is either owned by
/// this caller or shared with it, which is what the inventory reports — so the
/// one thing this listing filters on is the Region: an Outpost is installed at a
/// site in one Region and reported only through that Region's endpoint.
pub fn outpost_page(
    inventory: &OutpostInventory,
    region: &str,
    max_results: i32,
    resume: Option<String>,
) -> ListPage<Outpost> {
    list_page(
        inventory,
        max_results,
        resume,
        |outpost| outpost.region == region,
        String::clone,
        OutpostRecord::to_outpost,
    )
}

/// The cursor an endpoint listing returns to resume at `key`: the
/// `(OutpostId, EndpointId)` pair the next page starts at.
///
/// Both ids and the separator are inside `NextToken`'s own
/// `^[A-Za-z0-9\+\:\/\=\?\#-_]+$`, so the cursor a response carries is a value
/// the next request may legally send back.
pub fn encode_endpoint_cursor(key: &(String, String)) -> String {
    format!("{}/{}", key.0, key.1)
}

/// The `(OutpostId, EndpointId)` pair an endpoint listing's cursor FORM names,
/// or `None` when the token is not of the form these listings mint.
///
/// This is the form alone; which of those pairs a given listing could actually
/// have minted is [`endpoint_cursor`]'s question, because it depends on the
/// Region the walk is being resumed in.
pub fn decode_endpoint_cursor(token: &str) -> Option<(String, String)> {
    let (outpost_id, endpoint_id) = token.split_once('/')?;
    (is_outpost_id(outpost_id) && is_endpoint_id(endpoint_id))
        .then(|| (outpost_id.to_string(), endpoint_id.to_string()))
}

/// The key an endpoint listing resumes its walk at for `next_token`, or the
/// refusal for a token this listing never issued — which a token naming a place
/// outside the set this walk orders at all is.
///
/// A cursor is a position in ONE walk, so it is read in the whole scope that walk
/// serves and not just checked for its form: the only keys a listing can mint a
/// cursor for are the ones its own walk passes over. The key order these listings
/// share is dominated by the `OutpostId` half
/// ([`ComAmazonawsS3outpostsS3outpostsState::endpoints`]), so that half is what
/// the scope is read off, and it has to name an Outpost this walk orders on both
/// counts the scope has:
///
/// * the Region — the walk reports only endpoints whose parent Outpost the
///   `inventory` holds in `region` ([`endpoint_page`]), so an Outpost of another
///   Region, or none the inventory holds at all, names no place in it;
/// * `outpost_scope`, the Outpost a listing scoped to ONE named Outpost walks the
///   endpoints of — its walk passes over that Outpost's keys alone, so no other
///   Outpost's key is a position in it. `None` for a listing scoped to no single
///   Outpost, whose walk spans every Outpost of the Region.
///
/// Resuming at a key outside that scope would answer a page of whatever happens
/// to sort after it (often an empty one, reporting that a scope which has
/// endpoints has none) or ignore the cursor altogether and re-serve the first
/// page. It is refused instead, exactly as a token of the wrong form is.
///
/// The ENDPOINT half is deliberately not resolved: the walk resumes at the first
/// key at or after the cursor, so a page is still served when the endpoint the
/// cursor named was deleted between pages.
///
/// That tolerance is why an Outpost half the inventory holds NOWHERE is refused
/// here while [`outpost_cursor`] admits such an id: there the cursor IS the
/// record's own key, so an id the inventory no longer holds is the record
/// deleted between pages, whereas here the Outpost half is the SCOPE the walk
/// runs in, which an Outpost the inventory holds nothing of is no part of.
pub fn endpoint_cursor(
    next_token: Option<&str>,
    operation: &str,
    inventory: &OutpostInventory,
    region: &str,
    outpost_scope: Option<&str>,
) -> Result<Option<(String, String)>, ValidationException> {
    list_cursor(next_token, operation, |token| {
        decode_endpoint_cursor(token).filter(|(outpost_id, _)| {
            outpost_region(inventory, outpost_id) == Some(region)
                && outpost_scope.is_none_or(|scoped| outpost_id == scoped)
        })
    })
}

/// Whether `access_type` carries the `customer_owned_ipv4_pool` it requires, or
/// the refusal the one stated requirement earns.
///
/// `CreateEndpointRequest$AccessType` states the requirement in exactly ONE
/// direction — "if you choose `CustomerOwnedIp`, you must also provide the
/// customer-owned IP address pool (CoIP pool)" — and adds that "`Private` is the
/// default access type value". Nothing in the model or the documentation ties
/// the pool the other way round, so that is the only pairing this service
/// enforces: a pool named alongside the VPC access type (explicitly, or by the
/// default an absent member takes) is admitted like any other optional member
/// and stored as given, and the request goes on to resolve the resources it
/// names.
pub fn check_coip_pool(
    access_type: &str,
    customer_owned_ipv4_pool: Option<&str>,
) -> Result<(), ValidationException> {
    if access_type == ACCESS_TYPE_CUSTOMER_OWNED_IP && customer_owned_ipv4_pool.is_none() {
        return Err(ValidationException {
            message: Some(format!(
                "CustomerOwnedIpv4Pool is required when AccessType is {ACCESS_TYPE_CUSTOMER_OWNED_IP}"
            )),
        });
    }
    Ok(())
}

/// The page every endpoint listing serves: the endpoints of `region` that
/// `include` accepts, in [`ComAmazonawsS3outpostsS3outpostsState::endpoints`]'s own
/// key order, starting at `resume` and at most [`page_size`] of them, plus the
/// cursor naming the first key the page did not serve.
///
/// An endpoint belongs to the Region of the Outpost it is on, so the Region is
/// filtered on here rather than per listing: both listings report only what
/// `region`'s own endpoint reaches, and each endpoint's ARN names that same home
/// Region. What is left to differ per listing is `include`, which endpoints it
/// reports; the walk itself is [`list_page`]'s, so both page over one order,
/// settle what the clock owes at the same point, and mint the same cursor for the
/// same key.
pub fn endpoint_page(
    store: &mut ComAmazonawsS3outpostsS3outpostsState,
    inventory: &OutpostInventory,
    now: SystemTime,
    region: &str,
    max_results: i32,
    resume: Option<(String, String)>,
    mut include: impl FnMut(&EndpointRecord) -> bool,
) -> ListPage<Endpoint> {
    // A listing reports every endpoint it serves, so every status change the clock
    // already owes is materialised before the page is taken.
    store.settle_all(inventory, now);

    // The endpoint records the walk pages over are read beside the inventory the
    // Outposts they hang off live in, which is where their Region comes from.
    let home_region = |endpoint: &EndpointRecord| outpost_region(inventory, &endpoint.outpost_id);

    list_page(
        &store.endpoints,
        max_results,
        resume,
        |endpoint| home_region(endpoint) == Some(region) && include(endpoint),
        encode_endpoint_cursor,
        // The filter above admits only endpoints whose Outpost is in `region`, so
        // the Region each `Endpoint` is projected for is that Outpost's own.
        |endpoint| endpoint.to_endpoint(home_region(endpoint).unwrap_or(region)),
    )
}

/// The one `ResourceNotFoundException$Message` this service answers an
/// unresolvable reference under an `OutpostId` with, whichever branch of
/// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` the request used.
///
/// The text is a service constant, never built from the request: it names no id
/// and does not say which resource under the Outpost failed to resolve, so one
/// text covers every operation and every reference. Recorded behaviour — the
/// model carries no message text for `ResourceNotFoundException` — and it
/// reports what actually went wrong: an Outpost that does not exist or has no S3
/// capacity provisioned on it. An authorization failure is this service's
/// separate `AccessDeniedException` (403), not a 404 message.
pub const OUTPOST_NOT_FOUND_MESSAGE: &str =
    "The specified outpost does not exist, or does not have S3 capacity provisioned on it.";

/// True when `value` is the 12-digit (`\d{12}`) branch of `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` — the account-id form.
pub fn is_account_outpost_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// The `ResourceNotFoundException` payload for a reference under an `OutpostId`
/// that resolves to nothing this caller has access to — the Outpost itself, or
/// an endpoint on it.
///
/// It takes no argument on purpose: the message is one fixed constant, not a
/// function of the request's ids, of the form of the `OutpostId` or of which
/// resource failed to resolve. Every reference below an Outpost is resolved by
/// first resolving that Outpost, so the Outpost-level text is the only one a
/// caller ever sees.
pub fn not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(OUTPOST_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3outpostsS3outpostsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3outpostsS3outpostsState>)
}
