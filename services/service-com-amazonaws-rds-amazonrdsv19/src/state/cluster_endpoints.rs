// SPDX-License-Identifier: CC0-1.0

//! The CUSTOM endpoints of an Aurora DB cluster: the record `CreateDBClusterEndpoint`
//! stores, the projection every one of the four cluster-endpoint operations answers with,
//! and the two endpoints a cluster has without anybody creating them.
//!
//! A custom endpoint is one DNS name in front of a chosen set of the cluster's member
//! instances ("Creates a new custom endpoint and associates it with an Amazon Aurora DB
//! cluster"), so it is a resource of its own — it carries an identifier, an immutable
//! server-minted id and an ARN — while the address it publishes and the state it reports
//! follow the CLUSTER it stands in front of. That is why the projection here is a function
//! of both records rather than of the endpoint alone, and why nothing about the cluster is
//! copied onto the endpoint record.

use super::*;

/// The key a DB cluster endpoint is stored and looked up under: its identifier, which
/// `CreateDBClusterEndpointMessage$DBClusterEndpointIdentifier` documents as "stored as a
/// lowercase string" — so the record keeps the folded value and the key is that same
/// value, and two requests differing only in case name ONE endpoint.
pub fn db_cluster_endpoint_key(identifier: &str) -> String {
    identifier.to_ascii_lowercase()
}

/// `arn:aws:rds:<region>:<account>:cluster-endpoint:<identifier>` — the ARN form RDS gives
/// a custom endpoint, with the `cluster-endpoint` resource type its own tagging
/// documentation names it by.
pub fn db_cluster_endpoint_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("cluster-endpoint:{identifier}"),
    )
}

/// The immutable id RDS reports as `DBClusterEndpointResourceIdentifier`: "a unique
/// system-generated identifier for an endpoint. It remains the same for the whole life of
/// the endpoint", written `cluster-endpoint-` followed by an opaque token of the width every
/// server-assigned id of a REGIONAL resource of this service carries
/// ([`RESOURCE_ID_TOKEN_LEN`] — the width a DB cluster's own `DbClusterResourceId` and a DB
/// instance's `DbiResourceId` are written in), minted once per endpoint and never reused
/// ([`minted_token`]).
///
/// One width for every resource id this service mints, rather than one per resource kind: a
/// custom endpoint's id is a server-assigned id of a regional resource exactly as those two
/// are, and a client that validates the form it holds for one holds it for all three.
pub fn mint_db_cluster_endpoint_resource_id(mint: u64) -> String {
    format!(
        "cluster-endpoint-{}",
        minted_token("db-cluster-endpoint", mint, RESOURCE_ID_TOKEN_LEN)
    )
}

/// How many custom endpoints one DB cluster may carry: the allowance Amazon Aurora
/// publishes ("Custom endpoints per DB cluster": 5). A create past it is the modelled
/// `DBClusterEndpointQuotaExceededFault`, whose own documentation words it as a statement
/// about the cluster ([`DB_CLUSTER_ENDPOINT_QUOTA_MESSAGE`]) — which is also what bounds
/// this store, since an endpoint exists only for a cluster the per-region cluster quota
/// already counts.
pub const DB_CLUSTER_ENDPOINT_QUOTA_PER_CLUSTER: usize = 5;

/// The longest `DBClusterEndpointIdentifier` this service accepts.
///
/// The model states no `@length` for the member and the API publishes no bound of its own,
/// so the endpoint's identifier is held to the identifier rule of the cluster it belongs to
/// — "must contain from 1 to 63 letters, numbers, or hyphens", the length every other
/// identifier of a DB cluster's own namespace carries ([`check_identifier`]).
pub const MAX_DB_CLUSTER_ENDPOINT_IDENTIFIER_LEN: usize = 63;

/// The status a custom endpoint reports from the create that was accepted for it until the
/// clock makes its transition to [`DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE`] due.
///
/// `DBClusterEndpoint$Status` publishes the value list this and the three constants below
/// are drawn from: "creating, available, deleting, inactive, modifying".
pub const DB_CLUSTER_ENDPOINT_STATUS_CREATING: &str = "creating";

/// The status a custom endpoint reports once it is usable, from
/// `DBClusterEndpoint$Status`'s published list.
pub const DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE: &str = "available";

/// The status a custom endpoint reports while a modification of its membership or its type
/// is being applied, from `DBClusterEndpoint$Status`'s published list.
///
/// A TRANSIENT status: the clock owes the endpoint [`DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE`]
/// again, and while it lasts the endpoint is in no state another modification or a delete
/// can be taken in — the modelled `InvalidDBClusterEndpointStateFault`.
pub const DB_CLUSTER_ENDPOINT_STATUS_MODIFYING: &str = "modifying";

/// The status a custom endpoint reports once a delete has been accepted for it, from
/// `DBClusterEndpoint$Status`'s published list.
///
/// A TRANSIENT status: the clock owes the record's removal from the store, after which
/// every read of the identifier is the modelled `DBClusterEndpointNotFoundFault`.
pub const DB_CLUSTER_ENDPOINT_STATUS_DELETING: &str = "deleting";

/// The status an endpoint reports that "can't be used for a certain kind of cluster", the
/// last value of `DBClusterEndpoint$Status`'s published list: what a cluster's own two
/// endpoints report while the cluster accepts no connections at all
/// ([`builtin_endpoint_status`]).
pub const DB_CLUSTER_ENDPOINT_STATUS_INACTIVE: &str = "inactive";

/// `EndpointType` as a CUSTOM endpoint reports it: "the type of the endpoint. One of:
/// READER, WRITER, CUSTOM" (`DBClusterEndpoint$EndpointType`) — every endpoint a caller
/// creates is of this type, whichever type the create asked for, and that requested type is
/// reported beside it as `CustomEndpointType`.
pub const DB_CLUSTER_ENDPOINT_TYPE_CUSTOM: &str = "CUSTOM";

/// `EndpointType` as the cluster's own WRITER endpoint reports it — "the connection
/// endpoint for the primary instance of the DB cluster", which no create makes.
pub const DB_CLUSTER_ENDPOINT_TYPE_WRITER: &str = "WRITER";

/// `EndpointType` as the cluster's own READER endpoint reports it, the one that
/// "load-balances connections across the Aurora Replicas".
pub const DB_CLUSTER_ENDPOINT_TYPE_READER: &str = "READER";

/// `CustomEndpointType` for an endpoint that admits EITHER kind of member, the third value
/// `CreateDBClusterEndpointMessage$EndpointType` publishes.
pub const DB_CLUSTER_ENDPOINT_TYPE_ANY: &str = "ANY";

/// The values `CreateDBClusterEndpointMessage$EndpointType` documents: "the type of the
/// endpoint, one of: READER, WRITER, ANY", which is what a custom endpoint reports as its
/// `CustomEndpointType`.
pub const CUSTOM_ENDPOINT_TYPES: [&str; 3] = [
    DB_CLUSTER_ENDPOINT_TYPE_READER,
    DB_CLUSTER_ENDPOINT_TYPE_WRITER,
    DB_CLUSTER_ENDPOINT_TYPE_ANY,
];

/// The same three values in the order the REFUSAL of a value outside them writes them — the
/// writer, the reader, then the type that admits either ([`check_custom_endpoint_type`]).
///
/// A sentence states its own list: this front door words that refusal with the writer first,
/// where the create's documentation lists the reader first ([`CUSTOM_ENDPOINT_TYPES`]), and
/// the domain itself is the same three values — so the order lives beside the sentence that
/// writes it rather than reordering the domain every other reader of it shares.
pub const CUSTOM_ENDPOINT_TYPES_AS_REFUSED: [&str; 3] = [
    DB_CLUSTER_ENDPOINT_TYPE_WRITER,
    DB_CLUSTER_ENDPOINT_TYPE_READER,
    DB_CLUSTER_ENDPOINT_TYPE_ANY,
];

/// The `EndpointType` a request asked for, as the endpoint reports it, or the message the
/// request is refused with.
///
/// The member targets a plain string, so the boundary validator enforces nothing for it;
/// the three values its own documentation publishes are the domain, compared without regard
/// to case and reported back in the upper case `CustomEndpointType`'s published values are
/// written in. A value outside the domain is what the Query API's `InvalidParameterValue`
/// reports, which is the error the caller answers it with.
///
/// The refusal is the sentence this front door states that domain with: that the type is
/// invalid, and then the types that are valid, comma-separated in the order it writes them
/// ([`CUSTOM_ENDPOINT_TYPES_AS_REFUSED`]). It echoes the REJECTED value nowhere — the whole
/// sentence is fixed text about the member's domain — so it is stated from the domain rather
/// than formatted from the request.
pub fn check_custom_endpoint_type(value: &str) -> Result<String, String> {
    match CUSTOM_ENDPOINT_TYPES
        .into_iter()
        .find(|published| published.eq_ignore_ascii_case(value))
    {
        Some(published) => Ok(published.to_string()),
        None => Err(format!(
            "Invalid endpoint type. The valid endpoint types are: {}",
            CUSTOM_ENDPOINT_TYPES_AS_REFUSED.join(",")
        )),
    }
}

/// The message a `DBClusterEndpointAlreadyExistsFault` carries: Amazon RDS reports WHICH
/// identifier was already taken and WHICH endpoint holds it, as
/// `An endpoint with the identifier <identifier> already exists: <endpoint address>`.
///
/// The error shape's own documentation ("The specified custom endpoint can't be created
/// because it already exists.") describes the error CLASS and is not what goes on the wire,
/// so the sentence is formatted here — the same reason
/// [`db_cluster_endpoint_not_found_message`] is formatted rather than taken from the model.
///
/// `named` is the value the REQUEST carried, so a caller matching the message against the
/// identifier it sent finds it there, and `endpoint` is the ADDRESS the stored endpoint
/// already holding that identifier publishes ([`DbClusterEndpointRecord::to_wire`]'s
/// `Endpoint`), read out of this store — which is what tells a client creating several
/// endpoints in one flow which one it collided with.
pub fn db_cluster_endpoint_already_exists_message(named: &str, endpoint: &str) -> String {
    format!("An endpoint with the identifier {named} already exists: {endpoint}")
}

/// The sentence a `DBClusterEndpointQuotaExceededFault` carries, as the model publishes it
/// for the error class: "The cluster already has the maximum number of custom endpoints."
/// Like the one above it names neither the cluster nor the allowance, so it too is a
/// constant.
pub const DB_CLUSTER_ENDPOINT_QUOTA_MESSAGE: &str =
    "The cluster already has the maximum number of custom endpoints.";

/// The message a `DBClusterEndpointNotFoundFault` carries: the
/// `<resource type> <identifier> not found` form Amazon RDS words this fault in, naming the
/// endpoint the request addressed. The error shape's documentation ("The specified custom
/// endpoint doesn't exist.") describes the error CLASS and is not what goes on the wire, so
/// the sentence is formatted from the request instead of taken from the model.
///
/// `named` is the value the REQUEST carried, not the key the store folded it to, so a
/// caller matching the message against the identifier it sent finds it there — the same
/// reason [`crate::state::db_cluster_not_found_message`] names the request's spelling. RDS
/// writes this one with no terminal period. One function for every call that ADDRESSES an
/// endpoint — the modification and the delete alike — so the two cannot drift apart.
pub fn db_cluster_endpoint_not_found_message(named: &str) -> String {
    format!("DBClusterEndpoint {named} not found")
}

/// The sentence an `InvalidDBClusterEndpointStateFault` carries, as the model publishes it
/// for the error class: "The requested operation can't be performed on the endpoint while
/// the endpoint is in this state."
///
/// The STATUS the endpoint is in is what a caller polls on, and it is what a read of the
/// endpoint reports rather than part of this sentence, so nothing is interpolated here
/// either.
pub const DB_CLUSTER_ENDPOINT_STATE_MESSAGE: &str =
    "The requested operation can't be performed on the endpoint while the endpoint is in this \
     state.";

/// The DB cluster statuses a new custom endpoint can be created on: the cluster is up, or
/// it is still being provisioned and the endpoint comes up with it.
///
/// A custom endpoint is added TO a cluster, so there has to be a cluster to add it to: a
/// cluster that is stopping, stopped, starting, rebooting, failing over, taking a
/// modification or on its way out is not one an endpoint can be created on, which is what
/// the modelled `InvalidDBClusterStateFault` reports
/// ([`cluster_state_message`]). The two members are the statuses that refusal names, in the
/// order it names them — the same pair a DB shard group is provisioned on
/// ([`SHARD_GROUP_CLUSTER_STATUSES`]), for the same reason.
pub const CLUSTER_ENDPOINT_CLUSTER_STATUSES: [&str; 2] =
    [DB_CLUSTER_STATUS_AVAILABLE, DB_CLUSTER_STATUS_CREATING];

/// How many entries one of a custom endpoint's two membership lists may carry.
///
/// Neither member publishes a bound, and every entry is resolved against the region's
/// stored DB instances and then STORED, so the lists are capped the way every other
/// unbounded request list of this service is ([`check_request_list_len`]). The cap is the
/// region's own DB instance allowance, because an entry that names no instance of the
/// region is refused anyway: a longer list cannot name more instances than exist.
pub const MAX_CLUSTER_ENDPOINT_MEMBERS: usize = DB_INSTANCE_QUOTA_PER_REGION;

/// Refuse membership lists longer than [`MAX_CLUSTER_ENDPOINT_MEMBERS`], naming the member
/// that broke the bound — one check for the create that stores a membership and the
/// modification that replaces it, so neither can drift from the other.
pub fn check_cluster_endpoint_members(
    static_members: &[String],
    excluded_members: &[String],
) -> Result<(), String> {
    check_request_list_len(
        "StaticMembers",
        static_members.len(),
        MAX_CLUSTER_ENDPOINT_MEMBERS,
    )?;
    check_request_list_len(
        "ExcludedMembers",
        excluded_members.len(),
        MAX_CLUSTER_ENDPOINT_MEMBERS,
    )
}

/// The sentence a request that names BOTH of a custom endpoint's membership members is
/// refused with.
///
/// A custom endpoint's membership is stated one way or the other: `StaticMembers` is the
/// "list of DB instance identifiers that are part of the custom endpoint group", while
/// `ExcludedMembers` lists the ones that are not and is "relevant only if the list of
/// static members is empty" — so a request naming both states the membership twice and
/// says two different things. It is a VALUE the members do not accept (the Query API's
/// `InvalidParameterValue`), not a membership of its own.
///
/// Amazon RDS words it as one sentence about the two kinds of membership, in prose rather
/// than by the modelled member names, with no variable part at all: neither the endpoint
/// the request addressed nor the instances either list carried are in it, so this is a
/// constant rather than a `format!` over the request. One wording for the create that
/// stores a membership and the modification that replaces it, so neither can drift from the
/// other.
pub const ONLY_ONE_MEMBERSHIP_LIST: &str = "You can't specify both static and excluded members";

/// Refuse a request that names both membership members, whichever operation carries the
/// pair ([`ONLY_ONE_MEMBERSHIP_LIST`]).
///
/// A member is NAMED when the request carries it at all: an empty list is a membership the
/// caller stated — the one that puts no instance in the group, or keeps none out of it —
/// and not a member it left out, which is why the check is on PRESENCE.
pub fn check_one_membership_list(
    static_members: Option<&Vec<String>>,
    excluded_members: Option<&Vec<String>>,
) -> Result<(), String> {
    if static_members.is_some() && excluded_members.is_some() {
        return Err(ONLY_ONE_MEMBERSHIP_LIST.to_string());
    }
    Ok(())
}

/// The sentence a modification that asks for NO change is refused with: the one Amazon RDS
/// words for a `Modify*` call that names nothing to modify.
///
/// It has no variable part — the service reports only that the request asked for no
/// modification, naming neither the resource it addressed nor the members it could have
/// carried — so it is a constant rather than a `format!` over the request.
pub const NO_MODIFICATIONS_REQUESTED_MESSAGE: &str = "No modifications were requested";

/// A stored custom endpoint of a DB cluster: everything the service persists about it.
///
/// The ARN, the DNS address and the endpoint TYPE are not stored as text: each is a
/// function of the stored region, identifier and requested type — and, for the address, of
/// the cluster record the endpoint names ([`Self::to_wire`]) — so none of them can be left
/// naming a cluster or an identifier the endpoint no longer has.
#[derive(Debug, Clone)]
pub struct DbClusterEndpointRecord {
    /// `DBClusterEndpointIdentifier`, stored lowercased: "this parameter is stored as a
    /// lowercase string".
    pub identifier: String,
    /// The region the endpoint was created in; the scope of its ARN and its address.
    pub region: String,
    /// `DBClusterIdentifier`: "the DB cluster identifier of the DB cluster associated with
    /// the endpoint", resolved in the store before the endpoint was stored.
    pub db_cluster_identifier: String,
    /// `DBClusterEndpointResourceIdentifier`, minted once.
    pub resource_id: String,
    /// `CustomEndpointType`: "the type associated with a custom endpoint. One of: READER,
    /// WRITER, ANY" — the type the create or the last modification asked for.
    pub custom_endpoint_type: String,
    /// `Status`.
    pub status: String,
    /// The transition the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `StaticMembers`: "list of DB instance identifiers that are part of the custom
    /// endpoint group", each resolved in the store before the endpoint was stored.
    pub static_members: Vec<String>,
    /// `ExcludedMembers`: "list of DB instance identifiers that aren't part of the custom
    /// endpoint group", resolved the same way.
    pub excluded_members: Vec<String>,
    /// The address of the LISTENER this endpoint's own sessions arrive on, for a cluster
    /// whose engine wire this emulator serves.
    ///
    /// A custom endpoint is one DNS name in front of a chosen set of the cluster's members,
    /// so it is an address a client CONNECTS to with its ordinary driver: it gets a listener
    /// of its own ([`data_plane::cluster_endpoint`]) and publishes that listener's
    /// address, because a name this machine does not resolve would be no endpoint at all,
    /// and an address shared with another of the cluster's endpoints could not route to the
    /// members this endpoint selects.
    ///
    /// `None` for a cluster this emulator serves no wire for, which keeps the DNS-form name
    /// ([`Self::to_wire`]) and gets no listener.
    pub served_endpoint: Option<ServedEndpoint>,
    /// The tags the create assigned to the endpoint.
    ///
    /// Stored because `Tags` is "the tags to be assigned to the Amazon RDS resource", so a
    /// create that carried them has really assigned them; the modelled `DBClusterEndpoint`
    /// shape carries no tag list, so no read of the endpoint itself reports them.
    pub tags: Vec<Tag>,
}

impl DbClusterEndpointRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store — the same contract as
    /// [`DbShardGroupRecord::settle`].
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// `arn:aws:rds:<region>:<account>:cluster-endpoint:<identifier>`, from the stored
    /// region and identifier.
    pub fn arn(&self) -> String {
        db_cluster_endpoint_arn(&self.region, &self.identifier)
    }

    /// Whether the endpoint is in a state a modification or a delete can be taken in: it is
    /// up, and owes no transition of its own.
    ///
    /// Every other state — still being created, already taking a modification, or on its
    /// way out — is the modelled `InvalidDBClusterEndpointStateFault`
    /// ([`DB_CLUSTER_ENDPOINT_STATE_MESSAGE`]), which is what makes two calls on one
    /// endpoint serialise.
    pub fn is_available(&self) -> bool {
        self.status == DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE && self.pending_status.is_none()
    }

    /// Whether the member `identifier` is in this endpoint's GROUP, by the two membership
    /// lists: `StaticMembers` is the "list of DB instance identifiers that are part of the
    /// custom endpoint group", and where it is empty `ExcludedMembers` lists the ones that
    /// are not while "all other eligible instances are reachable through the custom
    /// endpoint" — which is why the exclusions are read only for an endpoint with no static
    /// list, exactly as `ExcludedMembers` documents ("relevant only if the list of static
    /// members is empty").
    ///
    /// A DB instance identifier "isn't case-sensitive" wherever this front door resolves
    /// one, so the comparison disregards case.
    pub fn admits_member(&self, identifier: &str) -> bool {
        if !self.static_members.is_empty() {
            return self
                .static_members
                .iter()
                .any(|member| member.eq_ignore_ascii_case(identifier));
        }
        !self
            .excluded_members
            .iter()
            .any(|member| member.eq_ignore_ascii_case(identifier))
    }

    /// Whether a member in the cluster's WRITING role (`is_writer`) is one this endpoint's
    /// TYPE routes to: `CustomEndpointType` is "the type associated with a custom endpoint.
    /// One of: READER, WRITER, ANY", so a READER endpoint routes to the cluster's read-only
    /// members, a WRITER endpoint to the one it writes through, and an ANY endpoint to
    /// either.
    pub fn admits_writer_role(&self, is_writer: bool) -> bool {
        match self.custom_endpoint_type.to_ascii_uppercase().as_str() {
            DB_CLUSTER_ENDPOINT_TYPE_READER => !is_writer,
            DB_CLUSTER_ENDPOINT_TYPE_WRITER => is_writer,
            // "ANY" — the third value the member publishes, and the only one left.
            _ => true,
        }
    }

    /// Whether a session on this endpoint may WRITE at all: a READER endpoint never writes,
    /// whichever member it reaches, and the other two types write exactly while the member
    /// they reached is the one the cluster writes through — which is what the caller decides
    /// from the member ([`ComAmazonawsRdsAmazonrdsv19State::cluster_endpoint_member`]).
    pub fn serves_reads_only(&self) -> bool {
        self.custom_endpoint_type
            .eq_ignore_ascii_case(DB_CLUSTER_ENDPOINT_TYPE_READER)
    }

    /// Accept a modification: the endpoint reports [`DB_CLUSTER_ENDPOINT_STATUS_MODIFYING`]
    /// from now on and the clock owes it [`DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE`]
    /// [`STATUS_TRANSITION_DELAY`] from now. Nothing sleeps: the due instant is stored and
    /// materialised by [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`].
    pub fn begin_modification(&mut self) {
        self.status = DB_CLUSTER_ENDPOINT_STATUS_MODIFYING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(
            DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE,
        ));
    }

    /// Accept a delete: the endpoint reports [`DB_CLUSTER_ENDPOINT_STATUS_DELETING`] from
    /// now on, and the clock owes its removal from the store, after which every read of the
    /// identifier is the modelled `DBClusterEndpointNotFoundFault` — exactly as
    /// [`DbShardGroupRecord::begin_deletion`] does for a DB shard group.
    pub fn begin_deletion(&mut self) {
        self.status = DB_CLUSTER_ENDPOINT_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Project the stored endpoint onto the modelled `DBClusterEndpoint` wire shape — the
    /// output of all four custom-endpoint operations and one entry of the
    /// `DescribeDBClusterEndpoints` report, so none of them can disagree about an endpoint.
    ///
    /// `cluster` is the cluster record the endpoint names, which is where its ADDRESS comes
    /// from ([`Self::address_on`]). An endpoint whose cluster this store no longer holds
    /// reports no address rather than an invented one; a cluster's custom endpoints leave
    /// the store with the cluster, so that is a state no read reaches.
    pub fn to_wire(&self, cluster: Option<&DbClusterRecord>) -> CreateDBClusterEndpointOutput {
        CreateDBClusterEndpointOutput {
            db_cluster_endpoint_identifier: Some(self.identifier.clone()),
            db_cluster_identifier: Some(self.db_cluster_identifier.clone()),
            db_cluster_endpoint_resource_identifier: Some(self.resource_id.clone()),
            endpoint: Some(self.address(cluster)).filter(|address| !address.is_empty()),
            status: Some(self.status.clone()),
            // "One of: READER, WRITER, CUSTOM": an endpoint a caller created is a CUSTOM
            // one, and the type the request asked for is the `CustomEndpointType` beside it.
            endpoint_type: Some(DB_CLUSTER_ENDPOINT_TYPE_CUSTOM.to_string()),
            custom_endpoint_type: Some(self.custom_endpoint_type.clone()),
            static_members: Some(self.static_members.clone()),
            excluded_members: Some(self.excluded_members.clone()),
            db_cluster_endpoint_arn: Some(self.arn()),
        }
    }

    /// `Endpoint`: "the DNS address of the endpoint", whose published form is
    /// `<endpoint identifier>.cluster-custom-<server-assigned label>.<region>.<suffix>` —
    /// the endpoint's own identifier in front of the CLUSTER's label, region and endpoint
    /// suffix, so a custom endpoint sits beside the two endpoints the cluster itself
    /// publishes ([`DbClusterRecord::endpoint`]) and resolves where they do.
    ///
    /// The suffix is read off the cluster's own name rather than composed a second time
    /// ([`DbClusterRecord::custom_endpoint_host`]), so a custom endpoint and the cluster it
    /// fronts cannot disagree about the zone they are announced in: for a cluster whose data
    /// plane this emulator serves that is the zone its listener is reached in, and for every
    /// other cluster the endpoint domain of the control plane that owns its engine.
    ///
    /// A HOST, as the cluster's own two endpoint members are: no endpoint member of this
    /// control plane carries a port of its own, so a client dials what it resolves this name
    /// to with the cluster's `Port`.
    fn address_on(&self, cluster: &DbClusterRecord) -> String {
        cluster.custom_endpoint_host(&self.identifier)
    }

    /// `Endpoint` as this endpoint publishes it: the address of its OWN listener where this
    /// emulator opened one ([`Self::served_endpoint`]), and the DNS name above for a cluster
    /// it serves no wire for.
    ///
    /// The served form carries the PORT the core assigned that listener
    /// (`service_common::data_plane_addressing::host_port_authority`), because the modelled
    /// `DBClusterEndpoint` shape has no port member of its own and the core owns the port
    /// range — so a client that reads this string has the whole address it needs, exactly as
    /// a DB proxy's `Endpoint` carries one ([`DbProxyRecord::endpoint`]).
    ///
    /// An endpoint whose cluster this store no longer holds and that has no listener of its
    /// own has no address at all: the empty value is what the caller leaves the member out
    /// for, rather than an invented name. A cluster's custom endpoints leave the store with
    /// the cluster, so that is a state no read reaches.
    fn address(&self, cluster: Option<&DbClusterRecord>) -> String {
        if let Some(served) = &self.served_endpoint {
            return ::service_common::data_plane_addressing::host_port_authority(
                &served.host,
                u16::try_from(served.port).unwrap_or_default(),
            );
        }
        cluster.map(|cluster| self.address_on(cluster)).unwrap_or_default()
    }
}

/// The two endpoints a DB cluster has WITHOUT anybody creating them, writer first: "the
/// connection endpoint for the primary instance" and the reader endpoint that
/// "load-balances connections across the Aurora Replicas".
///
/// `DescribeDBClusterEndpoints` reports them AFTER the custom endpoints created for the
/// cluster — its `Filters` member publishes `reader`, `writer` and `custom` as the values of
/// `db-cluster-endpoint-type`, and only these two are of the first two types — each with
/// the four members a cluster's own endpoint has: the cluster it belongs to, the address it
/// publishes, its state and its type. It carries no identifier, no immutable id, no ARN and
/// no membership, because none of those exists for an endpoint no create made, so each is
/// LEFT OUT rather than invented.
pub fn builtin_cluster_endpoints(cluster: &DbClusterRecord) -> Vec<CreateDBClusterEndpointOutput> {
    let status = builtin_endpoint_status(&cluster.status);
    [
        (DB_CLUSTER_ENDPOINT_TYPE_WRITER, cluster.endpoint()),
        (DB_CLUSTER_ENDPOINT_TYPE_READER, cluster.reader_endpoint()),
    ]
    .into_iter()
    .map(|(endpoint_type, address)| CreateDBClusterEndpointOutput {
        db_cluster_identifier: Some(cluster.identifier.clone()),
        endpoint: Some(address),
        status: Some(status.to_string()),
        endpoint_type: Some(endpoint_type.to_string()),
        ..CreateDBClusterEndpointOutput::default()
    })
    .collect()
}

/// What a cluster's own endpoint reports as its `Status`, for a cluster in `cluster_status`.
///
/// The endpoint's status list is its own — "creating, available, deleting, inactive,
/// modifying" — and narrower than the cluster's, so the cluster's state is read onto it:
/// the endpoints of a cluster still being provisioned are being created with it, the
/// endpoints of a cluster whose delete was accepted are going with it, the endpoints of a
/// cluster that accepts no connections at all (one that is stopped, stopping or starting)
/// "can't be used" and are `inactive`, and in every other state — up, taking a
/// modification, backing up, rebooting, failing over, being promoted — the cluster answers
/// connections, so its endpoints are available.
pub fn builtin_endpoint_status(cluster_status: &str) -> &'static str {
    match cluster_status {
        DB_CLUSTER_STATUS_CREATING => DB_CLUSTER_ENDPOINT_STATUS_CREATING,
        DB_CLUSTER_STATUS_DELETING => DB_CLUSTER_ENDPOINT_STATUS_DELETING,
        DB_CLUSTER_STATUS_STOPPING | DB_CLUSTER_STATUS_STOPPED | DB_CLUSTER_STATUS_STARTING => {
            DB_CLUSTER_ENDPOINT_STATUS_INACTIVE
        }
        _ => DB_CLUSTER_ENDPOINT_STATUS_AVAILABLE,
    }
}

/// Why a custom endpoint's membership list names a DB instance this service cannot put in
/// the endpoint group.
///
/// The two cases are the two faults the operations that carry a membership declare for it,
/// which is why they are different variants: an identifier naming no instance at all, and
/// one naming an instance that is not in a state it can join a group in. Each carries the
/// message its fault reports.
#[derive(Debug, Clone)]
pub enum MembershipMiss {
    /// No DB instance of that identifier — the modelled `DBInstanceNotFoundFault`.
    NotFound(String),
    /// An instance whose delete has already been accepted, so it is on its way out — the
    /// modelled `InvalidDBInstanceStateFault`.
    InvalidState(String),
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The custom endpoints stored in `region`, in folded-identifier order.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by what
    /// one region holds however many regions the store holds.
    pub fn cluster_endpoints_in(&self, region: &str) -> Vec<&DbClusterEndpointRecord> {
        range_in(&self.db_cluster_endpoints, region)
    }

    /// The custom endpoints of ONE cluster, in the same order: what the cluster reports as
    /// its `CustomEndpoints` and what the per-cluster allowance
    /// ([`DB_CLUSTER_ENDPOINT_QUOTA_PER_CLUSTER`]) is counted against.
    pub fn cluster_endpoints_of(
        &self,
        region: &str,
        db_cluster_identifier: &str,
    ) -> Vec<&DbClusterEndpointRecord> {
        self.cluster_endpoints_in(region)
            .into_iter()
            .filter(|record| record.db_cluster_identifier == db_cluster_identifier)
            .collect()
    }

    /// The custom endpoint `named` resolves to in `region`, or `None` when nothing of that
    /// identifier exists — what a request that NAMES a custom endpoint is resolved against.
    ///
    /// `DBClusterEndpointIdentifier` is "stored as a lowercase string", so the lookup folds
    /// the way [`db_cluster_endpoint_key`] keys the store and a caller naming the endpoint
    /// in any case reaches the one endpoint.
    pub fn resolve_db_cluster_endpoint(
        &self,
        region: &str,
        named: &str,
    ) -> Option<&DbClusterEndpointRecord> {
        self.db_cluster_endpoints
            .get(&(region.to_string(), db_cluster_endpoint_key(named)))
    }

    /// The MEMBER DB instance a session arriving on the custom endpoint `record` is routed
    /// to: the first of the cluster's members, in the order the cluster reports them (the
    /// writer first), that this endpoint's TYPE and its membership lists both admit
    /// ([`DbClusterEndpointRecord::admits_writer_role`],
    /// [`DbClusterEndpointRecord::admits_member`]) and that is not on its way out.
    ///
    /// That is what a custom endpoint IS: "a custom endpoint for an Aurora cluster
    /// represents a set of DB instances that you choose", so a session on it reaches one of
    /// the chosen members and never one the endpoint leaves out — which is the difference
    /// between this address and the cluster's own two.
    ///
    /// `None` when the endpoint selects no member at all: a cluster this store no longer
    /// holds, one with no member instance yet, and a membership that admits none of the
    /// members the cluster has. Such a session is served against the CLUSTER itself, in the
    /// role this endpoint's type gives it, exactly as the cluster's own endpoints are.
    pub fn cluster_endpoint_member(
        &self,
        record: &DbClusterEndpointRecord,
    ) -> Option<&DbInstanceRecord> {
        let cluster = self.resolve_db_cluster(&record.region, &record.db_cluster_identifier)?;
        let members = self.cluster_members(cluster);
        let writes = members
            .first()
            .map(|member| member.identifier.clone())
            .unwrap_or_default();
        members.into_iter().find(|member| {
            !member.is_deleting()
                && record.admits_writer_role(writes.eq_ignore_ascii_case(&member.identifier))
                && record.admits_member(&member.identifier)
        })
    }

    /// Project a stored custom endpoint onto the modelled `DBClusterEndpoint` shape, with
    /// the address read from the cluster record it names — the one way a handler projects
    /// an endpoint, the cluster-endpoint counterpart of [`Self::cluster_to_wire`].
    pub fn cluster_endpoint_to_wire(
        &self,
        record: &DbClusterEndpointRecord,
    ) -> CreateDBClusterEndpointOutput {
        record.to_wire(self.resolve_db_cluster(&record.region, &record.db_cluster_identifier))
    }

    /// The DB instances a custom endpoint's membership list `named` resolves to in `region`,
    /// in the order the request listed them, or why the first entry that cannot be a member
    /// is refused ([`MembershipMiss`]).
    ///
    /// A membership entry is "a DB instance identifier", which "isn't case-sensitive"
    /// wherever this front door resolves one, and the resolved identifier — the one the
    /// instance record carries — is what is stored and reported back, so a membership a
    /// read reports names instances this store really holds. An entry naming no instance,
    /// or one whose delete has already been accepted, is one of the two faults the
    /// operations that carry a membership declare, which is why the miss rather than one of
    /// their error enums is returned here: one resolution for the create that stores a
    /// membership and the modification that replaces it.
    pub fn resolve_cluster_endpoint_members(
        &self,
        region: &str,
        named: &[String],
    ) -> Result<Vec<String>, MembershipMiss> {
        let mut resolved: Vec<String> = Vec::with_capacity(named.len());
        for name in named {
            let key = (
                region.to_string(),
                db_instance_identifier_of(name).to_ascii_lowercase(),
            );
            let Some(instance) = self.db_instances.get(&key) else {
                return Err(MembershipMiss::NotFound(format!(
                    "DBInstance {name} not found."
                )));
            };
            if instance.is_deleting() {
                return Err(MembershipMiss::InvalidState(
                    modify_db_instance_invalid_state_message(&instance.status),
                ));
            }
            if !resolved.contains(&instance.identifier) {
                resolved.push(instance.identifier.clone());
            }
        }
        Ok(resolved)
    }
}
