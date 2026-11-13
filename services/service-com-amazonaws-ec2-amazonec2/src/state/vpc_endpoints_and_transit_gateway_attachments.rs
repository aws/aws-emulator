// SPDX-License-Identifier: CC0-1.0

//! The VPC endpoint, endpoint-service configuration and connection stores, and the
//! transit-gateway attachment stores every attachment kind is reported through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` VPC endpoint connections fit inside the store's ceiling
    /// ([`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`]).
    ///
    /// A writer asks this BEFORE storing anything and refuses the request with EC2's own
    /// quota code when it answers `false`, the way [`Self::has_dedicated_host_capacity`]
    /// gates `AllocateHosts`. [`Self::insert_vpc_endpoint_connection`] asks it again for
    /// itself, so no path can get past the ceiling by forgetting to.
    pub fn has_vpc_endpoint_connection_capacity(&self, count: usize) -> bool {
        self.vpc_endpoint_connections
            .len()
            .saturating_add(count)
            <= MAX_STORED_VPC_ENDPOINT_CONNECTIONS
    }

    /// Store a connection to one of the account's own VPC endpoint services, and answer
    /// the `VpcEndpointConnectionId` it was given — or `None` when the store is full.
    ///
    /// The connection id is minted here rather than taken from `spec`, the way
    /// [`Self::insert_subnet`] mints a `SubnetId`, and the record is keyed by its
    /// `VpcEndpointId` because one endpoint connects to exactly one service. The bytes it
    /// holds are charged against the store's byte budget here, so no path can store them
    /// uncharged.
    ///
    /// `None` means the store is at [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] and NOTHING
    /// was stored: the ceiling is enforced here as well as by
    /// [`Self::has_vpc_endpoint_connection_capacity`], so a writer that forgets the gate
    /// cannot grow the store without bound either. A writer that sees `None` refuses the
    /// request with EC2's own quota code rather than reporting a connection it does not
    /// hold. A connection REPLACING the one already stored under the same `VpcEndpointId`
    /// does not grow the store, so it is admitted whatever the count.
    pub fn insert_vpc_endpoint_connection(
        &mut self,
        spec: VpcEndpointConnectionSpec,
    ) -> Option<String> {
        // A write that REPLACES the connection of an endpoint already stored adds no
        // record, so only a new key has to fit inside the ceiling.
        let replaces_existing = self
            .vpc_endpoint_connections
            .contains_key(&spec.vpc_endpoint_id);
        if !replaces_existing && !self.has_vpc_endpoint_connection_capacity(1) {
            return None;
        }
        let vpc_endpoint_connection_id = self.next_id(VPC_ENDPOINT_CONNECTION_ID_PREFIX);
        let connection = VpcEndpointConnection {
            service_id: Some(spec.service_id),
            vpc_endpoint_id: Some(spec.vpc_endpoint_id),
            vpc_endpoint_owner: Some(spec.vpc_endpoint_owner),
            vpc_endpoint_state: Some(spec.vpc_endpoint_state),
            creation_timestamp: Some(system_time_to_date_time(spec.creation_timestamp)),
            dns_entries: Some(spec.dns_entries),
            network_load_balancer_arns: Some(spec.network_load_balancer_arns),
            gateway_load_balancer_arns: Some(spec.gateway_load_balancer_arns),
            ip_address_type: spec.ip_address_type,
            vpc_endpoint_connection_id: Some(vpc_endpoint_connection_id.clone()),
            tags: Some(spec.tags),
            vpc_endpoint_region: Some(spec.vpc_endpoint_region),
            payer_responsibilities: Some(spec.payer_responsibilities),
        };
        let vpc_endpoint_id = connection.vpc_endpoint_id.clone().unwrap_or_default();
        // A replacement gives the displaced record's bytes back before the new record is
        // charged, so rewriting one endpoint's connection cannot inflate the byte budget —
        // "a removal path must give back the same number"
        // ([`vpc_endpoint_connection_payload_bytes`]).
        if let Some(displaced) = self.vpc_endpoint_connections.get(&vpc_endpoint_id) {
            let displaced_bytes = vpc_endpoint_connection_payload_bytes(displaced);
            self.release_stored_bytes(displaced_bytes);
        }
        self.add_stored_bytes(vpc_endpoint_connection_payload_bytes(&connection));
        self.vpc_endpoint_connections
            .insert(vpc_endpoint_id, connection);
        Some(vpc_endpoint_connection_id)
    }
}

/// The bytes one stored VPC endpoint connection holds: the fixed overhead of the record
/// and the key it is stored under, plus the caller-chosen parts — its tags, the load
/// balancer ARNs of the service it connects to, and its DNS entries. The ids, the state,
/// the timestamp and the Region are derived by this module or bounded by the model, so they
/// are fixed-size, which is what the overhead constant stands for.
///
/// The overhead is charged so that a connection created with no tags, no ARNs and no DNS
/// entries is NOT free against the byte budget: every stored record costs something, so
/// repeated writes advance the budget whatever they carry.
///
/// The ONE definition of that charge, the way [`address_payload_bytes`] is for an address:
/// a writer charges exactly it, and a removal path must give back the same number.
pub fn vpc_endpoint_connection_payload_bytes(connection: &VpcEndpointConnection) -> usize {
    let arn_bytes = |arns: Option<&Vec<String>>| -> usize {
        arns.into_iter().flatten().map(String::len).sum()
    };
    let text_bytes = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    let dns_bytes = dns_entry_list_bytes(connection.dns_entries.as_deref().unwrap_or_default());
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(
            connection
                .vpc_endpoint_id
                .as_deref()
                .unwrap_or_default()
                .len(),
        )
        .saturating_add(tag_list_bytes(
            connection.tags.as_deref().unwrap_or_default(),
        ))
        .saturating_add(arn_bytes(connection.network_load_balancer_arns.as_ref()))
        .saturating_add(arn_bytes(connection.gateway_load_balancer_arns.as_ref()))
        .saturating_add(dns_bytes)
        .saturating_add(text_bytes(connection.vpc_endpoint_owner.as_ref()))
        .saturating_add(text_bytes(connection.vpc_endpoint_region.as_ref()))
}

/// The bytes a stored list of DNS entries holds: each entry's name and the hosted zone it
/// is served by.
///
/// The ONE definition of that charge, the way [`tag_list_bytes`] is for a tag list, so every
/// writer of a record that carries DNS entries — a VPC endpoint connection
/// ([`vpc_endpoint_connection_payload_bytes`]) and the endpoint itself — charges exactly it
/// and a removal path gives back the same number. The names are built from the service name
/// the caller supplied, so they are caller-sized rather than fixed.
pub fn dns_entry_list_bytes(entries: &[DnsEntry]) -> usize {
    entries
        .iter()
        .map(|entry| {
            entry.dns_name.as_deref().unwrap_or_default().len()
                + entry.hosted_zone_id.as_deref().unwrap_or_default().len()
        })
        .sum()
}

// =============================================================================
// VPC endpoint associations
// =============================================================================

/// The id prefix a stored VPC endpoint association is minted with.
///
/// `vpce-assoc-` rather than `vpce-`: an association and the endpoint it belongs to are
/// different resources, the way a service id and an endpoint id are
/// ([`VPC_ENDPOINT_SERVICE_ID_PREFIX`]). The model publishes no pattern for
/// `VpcEndpointAssociation$Id` (it is a plain `String` —
/// `smithy_query(query="shape",shape="VpcEndpointAssociation")`), so the id takes EC2's own
/// `<prefix>-<hex>` form through [`ComAmazonawsEc2Amazonec2State::next_id`] and is minted by
/// the store rather than taken from a request.
pub const VPC_ENDPOINT_ASSOCIATION_ID_PREFIX: &str = "vpce-assoc";

/// An association whose associated resource configuration is in service: "The resource is
/// accessible if the associated resource configuration is `AVAILABLE`"
/// (`VpcEndpointAssociation$AssociatedResourceAccessibility`).
pub const ASSOCIATED_RESOURCE_AVAILABLE: &str = "AVAILABLE";

/// An association whose associated resource configuration is not in service yet, so the
/// resource is not reachable through the endpoint: "When the state is `inaccessible`, it
/// returns `PENDING` or `FAILED`" (`DescribeVpcEndpointAssociationsRequest$Filters`).
pub const ASSOCIATED_RESOURCE_PENDING: &str = "PENDING";

/// An association whose associated resource configuration could not be reached at all — the
/// other value the `inaccessible` state of the filter above returns, and the one an
/// association's `FailureCode`/`FailureReason` explain.
pub const ASSOCIATED_RESOURCE_FAILED: &str = "FAILED";

/// Every accessibility an association can report, as the `associated-resource-accessibility`
/// FILTER spells it paired with the value
/// `VpcEndpointAssociation$AssociatedResourceAccessibility` carries for it.
///
/// Two sources spell one state two ways and both are part of the contract: the filter
/// documents "`associated-resource-accessibility` - The association state. When the state is
/// `accessible`, it returns `AVAILABLE`. When the state is `inaccessible`, it returns
/// `PENDING` or `FAILED`" (`DescribeVpcEndpointAssociationsRequest$Filters`), while the
/// reported member carries the accessibility of the resource configuration itself. A stored
/// association therefore holds the REPORTED value, and this table is what lets the
/// documented filter value select it — `inaccessible` naming two reported values, which is
/// why the pairs are a list rather than a map.
pub const ASSOCIATED_RESOURCE_ACCESSIBILITY_FILTER_VALUES: [(&str, &str); 3] = [
    ("accessible", ASSOCIATED_RESOURCE_AVAILABLE),
    ("inaccessible", ASSOCIATED_RESOURCE_PENDING),
    ("inaccessible", ASSOCIATED_RESOURCE_FAILED),
];

/// What one stored VPC endpoint association is written with, beside the association id the
/// store mints itself.
///
/// The spec of the resource, in the same shape [`VpcEndpointConnectionSpec`] has for a
/// connection: the writer that associates a resource with an endpoint fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_association`] turns it into the
/// modelled [`VpcEndpointAssociation`] the describe reports, so the id, the stored key and
/// the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct VpcEndpointAssociationSpec {
    /// "The ID of the VPC endpoint" the association belongs to
    /// (`VpcEndpointAssociation$VpcEndpointId`).
    pub vpc_endpoint_id: String,
    /// "The Amazon Resource Name (ARN) of the service network"
    /// (`VpcEndpointAssociation$ServiceNetworkArn`), for an endpoint of type service
    /// network; `None` for one that points at a resource configuration instead.
    pub service_network_arn: Option<String>,
    /// "The name of the service network" (`VpcEndpointAssociation$ServiceNetworkName`) —
    /// the name of the resource [`Self::service_network_arn`] names, which the ARN does not
    /// carry, so it comes from the writer that resolved it.
    pub service_network_name: Option<String>,
    /// "The Amazon Resource Name (ARN) of the associated resource"
    /// (`VpcEndpointAssociation$AssociatedResourceArn`).
    pub associated_resource_arn: Option<String>,
    /// "The Amazon Resource Name (ARN) of the resource configuration group"
    /// (`VpcEndpointAssociation$ResourceConfigurationGroupArn`), for a resource
    /// configuration that belongs to one of type `GROUP`.
    pub resource_configuration_group_arn: Option<String>,
    /// "The connectivity status of the resources associated to a VPC endpoint"
    /// (`VpcEndpointAssociation$AssociatedResourceAccessibility`), one of the reported
    /// values of [`ASSOCIATED_RESOURCE_ACCESSIBILITY_FILTER_VALUES`].
    pub associated_resource_accessibility: String,
    /// "An error code related to why an VPC endpoint association failed"
    /// (`VpcEndpointAssociation$FailureCode`), for an association that did.
    pub failure_code: Option<String>,
    /// "A message related to why an VPC endpoint association failed"
    /// (`VpcEndpointAssociation$FailureReason`).
    pub failure_reason: Option<String>,
    /// "The DNS entry of the VPC endpoint association"
    /// (`VpcEndpointAssociation$DnsEntry`).
    pub dns_entry: Option<DnsEntry>,
    /// "The private DNS entry of the VPC endpoint association"
    /// (`VpcEndpointAssociation$PrivateDnsEntry`).
    pub private_dns_entry: Option<DnsEntry>,
    /// "The tags to apply to the VPC endpoint association"
    /// (`VpcEndpointAssociation$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another VPC endpoint association fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_vpc_endpoint_connection_capacity`]: a writer asks this BEFORE storing
    /// anything and refuses the request with EC2's own quota code when it answers `false`.
    /// [`Self::insert_vpc_endpoint_association`] asks it again for itself, so no path can
    /// get past the ceiling by forgetting to.
    pub fn has_vpc_endpoint_association_capacity(&self) -> bool {
        self.vpc_endpoint_associations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store one association between a VPC endpoint and the resource it reaches, and answer
    /// the `Id` it was given — or `None` when the store is at its ceiling and NOTHING was
    /// stored.
    ///
    /// The association id is minted here rather than taken from `spec`, the way
    /// [`Self::insert_vpc_endpoint_connection`] mints a connection id, and the bytes the
    /// record holds are charged here ([`vpc_endpoint_association_payload_bytes`]) so no path
    /// can store them uncharged. A writer that sees `None` refuses the request with EC2's
    /// own quota code rather than reporting an association this store does not hold.
    pub fn insert_vpc_endpoint_association(
        &mut self,
        spec: VpcEndpointAssociationSpec,
    ) -> Option<String> {
        if !self.has_vpc_endpoint_association_capacity() {
            return None;
        }
        let association_id = self.next_id(VPC_ENDPOINT_ASSOCIATION_ID_PREFIX);
        let association = VpcEndpointAssociation {
            id: Some(association_id.clone()),
            vpc_endpoint_id: Some(spec.vpc_endpoint_id),
            service_network_arn: spec.service_network_arn,
            service_network_name: spec.service_network_name,
            associated_resource_accessibility: Some(spec.associated_resource_accessibility),
            failure_reason: spec.failure_reason,
            failure_code: spec.failure_code,
            dns_entry: spec.dns_entry,
            private_dns_entry: spec.private_dns_entry,
            associated_resource_arn: spec.associated_resource_arn,
            resource_configuration_group_arn: spec.resource_configuration_group_arn,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(vpc_endpoint_association_payload_bytes(&association));
        self.vpc_endpoint_associations
            .insert(association_id.clone(), association);
        Some(association_id)
    }
}

/// The bytes one stored VPC endpoint association holds: the fixed overhead of the record
/// and the key it is stored under, plus the caller-chosen parts — the ARNs of the service
/// network, the associated resource and its configuration group, the name of the service
/// network, the failure code and reason, the two DNS entries and the tags. The association
/// id and the accessibility are derived by this module or bounded by the model, so they are
/// fixed-size, which is what the overhead constant stands for.
///
/// The overhead is charged so that an association carrying no ARN, no DNS entry and no tag
/// is NOT free against the byte budget: every stored record costs something, so repeated
/// writes advance the budget whatever they carry.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a removal path must give back the same
/// number.
pub fn vpc_endpoint_association_payload_bytes(association: &VpcEndpointAssociation) -> usize {
    let text_bytes = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    let entry_bytes = |entry: Option<&DnsEntry>| -> usize {
        entry.map_or(0, |entry| dns_entry_list_bytes(::std::slice::from_ref(entry)))
    };
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(text_bytes(association.vpc_endpoint_id.as_ref()))
        .saturating_add(text_bytes(association.service_network_arn.as_ref()))
        .saturating_add(text_bytes(association.service_network_name.as_ref()))
        .saturating_add(text_bytes(association.associated_resource_arn.as_ref()))
        .saturating_add(text_bytes(
            association.resource_configuration_group_arn.as_ref(),
        ))
        .saturating_add(text_bytes(association.failure_code.as_ref()))
        .saturating_add(text_bytes(association.failure_reason.as_ref()))
        .saturating_add(entry_bytes(association.dns_entry.as_ref()))
        .saturating_add(entry_bytes(association.private_dns_entry.as_ref()))
        .saturating_add(tag_list_bytes(
            association.tags.as_deref().unwrap_or_default(),
        ))
}

// =============================================================================
// VPC endpoint connection notifications
// =============================================================================

/// The id prefix a stored connection notification is minted with.
///
/// The model publishes no pattern for `ConnectionNotificationId` (it is a plain `String` —
/// `smithy_query(query="shape",shape="ConnectionNotificationId")`), so the id takes EC2's
/// own `<prefix>-<hex>` form through [`ComAmazonawsEc2Amazonec2State::next_id`], with the
/// prefix AWS names a connection notification with. It is minted by the store and never
/// taken from a request.
pub const CONNECTION_NOTIFICATION_ID_PREFIX: &str = "vpce-nfn";

/// `InvalidConnectionNotificationId.NotFound` (HTTP 400): the request named a connection
/// notification id this account holds no notification for — EC2's `Invalid<Member>.NotFound`
/// for the member that named it, as [`INVALID_VPC_ENDPOINT_ID_NOT_FOUND`] is for
/// `VpcEndpointId`.
pub const INVALID_CONNECTION_NOTIFICATION_ID_NOT_FOUND: &str =
    "InvalidConnectionNotificationId.NotFound";

/// `InvalidConnectionNotificationId.Malformed` (HTTP 400): the request named something that
/// is not a connection notification id at all, so there is nothing to look up.
pub const INVALID_CONNECTION_NOTIFICATION_ID_MALFORMED: &str =
    "InvalidConnectionNotificationId.Malformed";

/// How [`id_not_found`] names this resource in the sentence it refuses with.
pub const CONNECTION_NOTIFICATION_KIND: &str = "connection notification ID";

/// Refuse a connection notification id this account holds no notification for, or one that
/// is not such an id at all — the malformed question first, so "not an id of this kind" is
/// never reported as "no such resource" ([`require_well_formed_id`]).
pub fn require_known_connection_notification_id<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    connection_notification_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_well_formed_id(
        CONNECTION_NOTIFICATION_ID_PREFIX,
        connection_notification_id,
        INVALID_CONNECTION_NOTIFICATION_ID_MALFORMED,
    )?;
    if store
        .vpc_endpoint_connection_notifications
        .contains_key(connection_notification_id)
    {
        return Ok(());
    }
    Err(id_not_found(
        INVALID_CONNECTION_NOTIFICATION_ID_NOT_FOUND,
        CONNECTION_NOTIFICATION_KIND,
        connection_notification_id,
    ))
}

/// What one stored connection notification is created with, beside the id the store mints
/// itself — the spec of the resource, in the same shape [`VpcEndpointConnectionSpec`] has
/// for a connection, so the id, the stored key and the byte charge are decided in exactly
/// one place.
#[derive(Debug, Clone)]
pub struct ConnectionNotificationSpec {
    /// "The ID of the endpoint service" the notification is for
    /// (`ConnectionNotification$ServiceId`), or `None` for a notification of an endpoint.
    pub service_id: Option<String>,
    /// "The ID of the VPC endpoint" the notification is for
    /// (`ConnectionNotification$VpcEndpointId`), or `None` for one of an endpoint service.
    pub vpc_endpoint_id: Option<String>,
    /// "The type of notification" (`ConnectionNotification$ConnectionNotificationType`), a
    /// member of the `ConnectionNotificationType` enum
    /// ([`crate::ops::types::CONNECTION_NOTIFICATION_TYPE_VALUES`]).
    pub connection_notification_type: String,
    /// "The ARN of the SNS topic for the notification"
    /// (`ConnectionNotification$ConnectionNotificationArn`).
    pub connection_notification_arn: String,
    /// "The events for the notification"
    /// (`ConnectionNotification$ConnectionEvents`).
    pub connection_events: Vec<String>,
    /// "The state of the notification"
    /// (`ConnectionNotification$ConnectionNotificationState`), a member of the
    /// `ConnectionNotificationState` enum
    /// ([`crate::ops::types::CONNECTION_NOTIFICATION_STATE_VALUES`]).
    pub connection_notification_state: String,
    /// "The Region for the endpoint service" (`ConnectionNotification$ServiceRegion`).
    pub service_region: String,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another connection notification fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_vpc_endpoint_connection_notification_capacity(&self) -> bool {
        self.vpc_endpoint_connection_notifications.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a connection notification and answer the `ConnectionNotificationId` it was
    /// given — or `None` when the store is at its ceiling and nothing was stored.
    ///
    /// The id is minted here rather than taken from `spec`, the way
    /// [`Self::insert_vpc_endpoint_connection`] mints a connection id, and the
    /// caller-chosen bytes the record holds are charged here
    /// ([`connection_notification_payload_bytes`]), so no path can store them uncharged.
    pub fn insert_vpc_endpoint_connection_notification(
        &mut self,
        spec: ConnectionNotificationSpec,
    ) -> Option<String> {
        if !self.has_vpc_endpoint_connection_notification_capacity() {
            return None;
        }
        let connection_notification_id = self.next_id(CONNECTION_NOTIFICATION_ID_PREFIX);
        let notification = ConnectionNotification {
            connection_notification_id: Some(connection_notification_id.clone()),
            service_id: spec.service_id,
            vpc_endpoint_id: spec.vpc_endpoint_id,
            connection_notification_type: Some(spec.connection_notification_type),
            connection_notification_arn: Some(spec.connection_notification_arn),
            connection_events: Some(spec.connection_events),
            connection_notification_state: Some(spec.connection_notification_state),
            service_region: Some(spec.service_region),
        };
        self.add_stored_bytes(connection_notification_payload_bytes(&notification));
        self.vpc_endpoint_connection_notifications
            .insert(connection_notification_id.clone(), notification);
        Some(connection_notification_id)
    }
}

/// The bytes one stored connection notification holds: the fixed overhead of the record and
/// the key it is stored under, plus the caller-chosen parts — the SNS topic ARN and the
/// event names. The ids, the state, the type and the Region are derived by this module or
/// bounded by the model, so they are fixed-size, which is what the overhead constant stands
/// for.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a removal path must give back the same
/// number.
pub fn connection_notification_payload_bytes(notification: &ConnectionNotification) -> usize {
    let text_bytes = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    let event_bytes: usize = notification
        .connection_events
        .iter()
        .flatten()
        .map(String::len)
        .sum();
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(text_bytes(notification.connection_notification_id.as_ref()))
        .saturating_add(text_bytes(notification.connection_notification_arn.as_ref()))
        .saturating_add(text_bytes(notification.service_id.as_ref()))
        .saturating_add(text_bytes(notification.vpc_endpoint_id.as_ref()))
        .saturating_add(text_bytes(notification.service_region.as_ref()))
        .saturating_add(event_bytes)
}

// =============================================================================
// A state move the clock owes, for the records that carry their own
// =============================================================================

/// A state a record is due to move to, and the instant the clock owes it.
///
/// The generic form of [`PendingTransition`], which is an instance's: a resource an
/// operation hands back in a NON-TERMINAL state ([`TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING`]
/// after an acceptance, say) still owes the move to its terminal one, and that move was due
/// at its instant whether or not anybody called again. The instant is RECORDED rather than
/// slept for, and [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it
/// once the clock has passed it. A resource an operation hands back ready carries none.
#[derive(Debug, Clone)]
pub struct PendingStateMove {
    /// The state the record moves to once `due_at` has passed.
    pub to: String,
    /// When the move falls due, on the same monotonic clock and at the same
    /// [`TRANSITION_DELAY`] span as every other transition this store owes.
    pub due_at: Instant,
}

impl PendingStateMove {
    /// The move to `to`, falling due [`TRANSITION_DELAY`] from now.
    pub fn starting_now(to: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            due_at: Instant::now() + TRANSITION_DELAY,
        }
    }

    /// The move, if it has come due by `now`.
    pub(super) fn due_by(&self, now: Instant) -> Option<&str> {
        (now >= self.due_at).then_some(self.to.as_str())
    }
}

/// Whether `id` is a well-formed EC2 resource id of the kind `prefix` names.
///
/// An EC2 resource id is `<prefix>-<hex>`, with 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form, as the model's own id patterns read
/// (`^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`). The generic form of
/// [`volume_id_is_well_formed`], for the resources whose own `@pattern` the model does not
/// publish (`smithy_query(query="shape",shape="TransitGatewayAttachmentId")` is a plain
/// `String`): EC2 answers that resource's `Invalid<Kind>.Malformed` for anything else,
/// BEFORE it looks the id up, so a caller can tell "not an id of this kind" from "no such
/// resource".
pub fn prefixed_id_is_well_formed(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(
        suffix.len(),
        SHORT_RESOURCE_ID_DIGITS | LONG_RESOURCE_ID_DIGITS
    ) && suffix
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The most characters of a refused resource id that [`invalid_id_message`] repeats back.
///
/// The message names the value the REQUEST sent, and a resource id member is a bare string
/// the model puts no `@length` on (`smithy_query(query="shape",shape="VolumeId")`), so the
/// echo is bounded here the way [`MAX_ECHOED_COIP_POOL_ID_CHARS`] bounds a refused pool
/// id's. Far above the longest id EC2 issues (a prefix plus 17 digits), so a real id is
/// always named in full.
pub const MAX_ECHOED_RESOURCE_ID_CHARS: usize = 128;

/// The sentence EC2 refuses a value it cannot read as a resource id of the kind the
/// parameter names with: the fixed prefix `Invalid id: ` and the offending value from the
/// REQUEST, in DOUBLE quotes.
///
/// ONE wording for every `Invalid<Kind>.Malformed` this crate answers, whatever the id kind
/// and whichever operation carries it. That is what the recorded refusals show: an instance
/// id, an AMI id, a security group id, a network interface attachment id and a Spot Fleet
/// request id are each quoted `Invalid id: "<value>"`, so the quoting is a fact of the
/// service's template rather than of one id kind, and reaching this function is how a
/// refusal cannot word it a second way. Only the first
/// [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters of the value are named.
pub fn invalid_id_message(id: &str) -> String {
    format!("Invalid id: \"{}\"", echoed_resource_id(id))
}

/// A resource id from the REQUEST, as a refusal may name it: its first
/// [`MAX_ECHOED_RESOURCE_ID_CHARS`] characters.
///
/// The ONE place a refused id is cut down to length, so every refusal that quotes a value the
/// caller chose bounds that echo the same way, whatever sentence it is quoted in —
/// [`invalid_id_message`]'s or one a single operation's recorded refusal words differently.
pub fn echoed_resource_id(id: &str) -> String {
    id.chars().take(MAX_ECHOED_RESOURCE_ID_CHARS).collect()
}

/// Whether `id` is NOT the null id of the kind `prefix` names: `<prefix>-` followed by
/// nothing but `0`.
///
/// The all-zero id is the value every client reaches for as a placeholder, and the mint
/// counts from one ([`ComAmazonawsEc2Amazonec2State::next_id`]), so no resource can ever
/// hold it. The ONE definition of that clause, for the id readers whose kind AWS answers
/// `Invalid<Kind>.Malformed` for it ([`issued_long_id_is_well_formed`],
/// [`snapshot_id_is_well_formed`] and [`fpga_image_id_is_well_formed`], each on the evidence
/// recorded with it) — it is not a rule about every EC2 id, so
/// [`prefixed_id_is_well_formed`] does not read it.
pub(super) fn id_is_not_the_null_id(prefix: &str, id: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| suffix.bytes().any(|digit| digit != b'0'))
}

/// Whether `id` is a well-formed LONG-form EC2 resource id of the kind `prefix` names that
/// EC2 could have ISSUED.
///
/// The stricter reading of [`prefixed_id_is_well_formed`], for the resource kinds AWS answers
/// `Invalid<Kind>.Malformed` for an id that merely LOOKS hexadecimal: the suffix carries
/// exactly [`LONG_RESOURCE_ID_DIGITS`] lowercase hexadecimal digits, its leading digit is
/// `0` (the long form carries 68 bits for a 64-bit internal identifier, so an issued id
/// always reads `<prefix>-0…`, which is also what [`ComAmazonawsEc2Amazonec2State::next_id`]
/// mints), and they are not all `0` (the placeholder every client reaches for, which the
/// mint counting from one can never produce).
///
/// Which resource kinds read ids this way is a fact about each kind, not about every EC2 id:
/// [`capacity_reservation_fleet_id_is_well_formed`] and the launch path's
/// [`issued_image_id_is_well_formed`] do, each on the evidence recorded with it, while an
/// instance id is read in the short form too ([`issued_id_is_well_formed`]) and a network
/// interface id answers `NotFound` for the same all-zero suffix and so reads ids through
/// [`prefixed_id_is_well_formed`] instead.
pub fn issued_long_id_is_well_formed(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix.len() == LONG_RESOURCE_ID_DIGITS
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && suffix.starts_with('0')
        && id_is_not_the_null_id(prefix, id)
}

/// Whether `id` is a well-formed EC2 resource id of the kind `prefix` names that EC2 could
/// have ISSUED, in EITHER of the two forms it has issued ids in.
///
/// [`prefixed_id_is_well_formed`]'s two forms with [`issued_long_id_is_well_formed`]'s
/// clauses on the LONG one: the id reads in one of the two published forms, it is not the
/// all-zero placeholder the mint counting from one can never produce
/// ([`id_is_not_the_null_id`]), and where it carries the long form its leading digit is `0`
/// (that form carries 68 bits for a 64-bit identifier, so an issued long id always reads
/// `<prefix>-0…`, which is what [`ComAmazonawsEc2Amazonec2State::next_id`] mints). The two
/// published forms and the hexadecimal alphabet are read through
/// [`prefixed_id_is_well_formed`] and the placeholder through [`id_is_not_the_null_id`], so
/// those keep one definition; only the long form's leading digit is read here, because
/// [`issued_long_id_is_well_formed`] cannot be reused for it without also refusing the
/// short form this reader accepts.
///
/// The reading of the kinds recorded answering `NotFound` for a SHORT-form id that names
/// nothing while answering `Invalid<Kind>.Malformed` for a long-form id outside the issued
/// form ([`instance_id_is_well_formed`], where that evidence is cited). Which reading a
/// kind takes is a fact about that kind: a kind recorded refusing anything but the long
/// form reads [`issued_long_id_is_well_formed`], and one that puts no clause on the digits
/// at all reads [`prefixed_id_is_well_formed`].
pub fn issued_id_is_well_formed(prefix: &str, id: &str) -> bool {
    prefixed_id_is_well_formed(prefix, id)
        && id_is_not_the_null_id(prefix, id)
        && id
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|suffix| {
                suffix.len() != LONG_RESOURCE_ID_DIGITS || suffix.starts_with('0')
            })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether `id` is an id of the kind `prefix` names that this service has ISSUED.
    ///
    /// The strictest of this module's id readings, for the id kinds AWS answers
    /// `Invalid<Kind>.Malformed` for a value that is merely IN the printed id form: an EC2 id
    /// reaches the service encoded rather than as the identifier it stands for — "Unable to
    /// unobfuscate ID."
    /// ([`crate::scaffolded::Scaffolded::instance_id_could_not_be_unobfuscated`]) is the
    /// sentence EC2 words for one it cannot decode — so a value no issue of that kind ever
    /// produced is refused BEFORE any lookup rather than reported as a resource that does not
    /// exist. Only an id the service could have handed out is looked up, which is what keeps
    /// the not-found half of the pair reachable: the resource an issued id named may since
    /// have gone.
    ///
    /// This store hands out ids from one monotonic counter ([`Self::next_id`], which writes
    /// `<prefix>-{counter:017x}`), so the ids it has issued are exactly the long-form values
    /// ([`issued_long_id_is_well_formed`]) at or below that counter — a hand-written
    /// documentation-shaped id sits astronomically above it, while every id this service ever
    /// answered with is at or below it.
    ///
    /// Which kinds read their ids this way is a fact about each kind, recorded with it:
    /// [`Self::network_interface_attachment_id_is_well_formed`] does, on the recorded refusal
    /// cited there, while a kind recorded answering `NotFound` for a hand-written long-form id
    /// reads the wider [`prefixed_id_is_well_formed`] or [`issued_long_id_is_well_formed`]
    /// instead.
    ///
    /// The counter is the store's ONE id counter, shared by every resource kind, so this says
    /// "a value the mint has reached" rather than "a value reached for THIS kind" — an id at
    /// or below it is looked up even when the mint spent that value on another kind, which is
    /// the conservative direction: it reports a value the service may have issued as
    /// not-found rather than reporting one it issued as unreadable.
    pub fn id_was_issued(&self, prefix: &str, id: &str) -> bool {
        issued_long_id_is_well_formed(prefix, id)
            && id
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('-'))
                // A long-form suffix under a leading `0` is 64 bits of hexadecimal, so the
                // value a well-formed one names always fits the counter's own width. A suffix
                // the parse REFUSES is one that does not fit 64 bits, which is above every
                // value the mint can reach rather than unreadable — so failing closed here is
                // the same answer as the comparison below would give.
                .and_then(|suffix| u64::from_str_radix(suffix, 16).ok())
                .is_some_and(|named| named <= self.id_counter)
    }
}

/// Whether `id` is a well-formed VPC ENDPOINT id, as opposed to an id of one of the two
/// other resources whose ids also begin `vpce-`.
///
/// [`VPC_ENDPOINT_SERVICE_ID_PREFIX`] and [`VPC_ENDPOINT_CONNECTION_ID_PREFIX`] extend the
/// endpoint's own prefix, so the prefix alone cannot separate them; it is the hex suffix
/// that does, an endpoint id carrying nothing but hex digits where those two carry `svc-`
/// or `conn-` first.
pub fn vpc_endpoint_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(VPC_ENDPOINT_ID_PREFIX, id)
}

// =============================================================================
// Transit gateway attachments
// =============================================================================

/// The id prefix every transit gateway attachment is minted with, whatever resource it
/// attaches: one `TransitGatewayAttachmentId` space serves them all, which is why
/// [`ComAmazonawsEc2Amazonec2State::transit_gateway_attachments`] is one map.
pub const TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX: &str = "tgw-attach";

/// The id prefix a transit gateway multicast domain is minted with.
pub const TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX: &str = "tgw-mcast-domain";

/// The id prefix a transit gateway route table announcement is minted with.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX: &str = "tgw-rtb-announce";

/// An attachment waiting for the owner of the attached resource to accept it: the
/// `TransitGatewayAttachmentState` enum's `pendingAcceptance` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE: &str = "pendingAcceptance";

/// An attachment whose acceptance is being carried out: the
/// `TransitGatewayAttachmentState` enum's `pending` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING: &str = "pending";

/// An attachment in service: the `TransitGatewayAttachmentState` enum's `available` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE: &str = "available";

/// An attachment whose rejection is being carried out: the
/// `TransitGatewayAttachmentState` enum's `rejecting` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING: &str = "rejecting";

/// An attachment the owner of the attached resource refused: the
/// `TransitGatewayAttachmentState` enum's `rejected` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED: &str = "rejected";

/// An attachment whose removal is being carried out: the `TransitGatewayAttachmentState`
/// enum's `deleting` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETING: &str = "deleting";

/// An attachment that is gone: the `TransitGatewayAttachmentState` enum's `deleted`
/// member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_DELETED: &str = "deleted";

/// An attachment on its way to having failed: the `TransitGatewayAttachmentState` enum's
/// `failing` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILING: &str = "failing";

/// An attachment that could not be made: the `TransitGatewayAttachmentState` enum's
/// `failed` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_FAILED: &str = "failed";

/// `pendingAcceptance` as the OTHER state enum spells it:
/// `TransitGatewayClientVpnAttachment$State` targets `TransitGatewayAttachmentStatusType`,
/// whose members are hyphenated (`pending-acceptance`, `pending`, `rejected`, `available`,
/// `deleting`, `deleted`) rather than camel-cased. One state, two spellings, both part of
/// the contract — see [`TransitGatewayAttachmentView::states`].
pub const CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE: &str = "pending-acceptance";

/// The `TransitGatewayAttachmentResourceType` of a VPC attachment: `vpc`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC: &str = "vpc";

/// The `TransitGatewayAttachmentResourceType` of a transit gateway peering attachment:
/// `tgw-peering`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING: &str = "tgw-peering";

/// The `TransitGatewayAttachmentResourceType` of a Client VPN attachment: `client-vpn`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN: &str = "client-vpn";

/// The `TransitGatewayAttachmentResourceType` of a Connect attachment: `connect`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT: &str = "connect";

/// The sentence EC2 refuses something that is not a transit gateway attachment id AT ALL
/// with, whichever code the operation at hand carries it under.
///
/// ENTIRELY fixed: this refusal interpolates nothing a caller sent, unlike the family's
/// not-found sentence ([`transit_gateway_attachment_deleted_or_absent`]), which names the id.
/// One definition because the operations that take an attachment id answer the same sentence
/// under different codes — [`require_well_formed_transit_gateway_attachment_id`] carries it
/// as that resource's own `Invalid<Kind>.Malformed`, while an operation that takes one as a
/// parameter OF a create can carry it as [`INVALID_PARAMETER_VALUE`].
pub const INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MESSAGE: &str =
    "Invalid Transit Gateway Attachment id.";

/// How one kind of attachment spells the states an acceptance or a rejection walks through.
///
/// The two state enums the model gives attachments name the same states with different
/// spellings, so the spellings are a property of the KIND rather than literals at a call
/// site: see [`TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE`] and
/// [`CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE`].
#[derive(Debug, Clone, Copy)]
pub struct TransitGatewayAttachmentStates {
    /// Waiting for the owner of the attached resource to accept.
    pub pending_acceptance: &'static str,
    /// The non-terminal state an accepted attachment passes through.
    pub accepting: &'static str,
    /// In service: what an accepted attachment reaches.
    pub available: &'static str,
    /// The non-terminal state a rejected attachment passes through.
    pub rejecting: &'static str,
    /// Refused: what a rejected attachment reaches.
    pub rejected: &'static str,
}

/// One transit gateway attachment, as the modelled shape its own describe reports.
///
/// Every kind shares the `TransitGatewayAttachmentId` space, so they share one store; each
/// kind keeps the shape the operations that report it answer with, so a member a writer
/// sets round-trips without a field-by-field copy.
#[derive(Debug, Clone)]
pub enum TransitGatewayAttachmentView {
    /// A VPC attachment (`AcceptTransitGatewayVpcAttachment`,
    /// `RejectTransitGatewayVpcAttachment`).
    Vpc(TransitGatewayVpcAttachment),
    /// A transit gateway peering attachment (`AcceptTransitGatewayPeeringAttachment`).
    Peering(TransitGatewayPeeringAttachment),
    /// A Client VPN attachment (`AcceptTransitGatewayClientVpnAttachment`).
    ClientVpn(TransitGatewayClientVpnAttachment),
    /// A Connect attachment — the GRE-based attachment a Connect peer peers over
    /// (`CreateTransitGatewayConnectPeer`, "Creates a Connect peer for a specified transit
    /// gateway Connect attachment").
    Connect(TransitGatewayConnect),
}

impl TransitGatewayAttachmentView {
    /// How this kind spells the states of an acceptance and a rejection.
    ///
    /// `TransitGatewayAttachmentStatusType` — the Client VPN attachment's state enum — has
    /// no member for the transient state of a rejection, so for that kind `rejecting` is
    /// the terminal `rejected` itself.
    pub fn states(&self) -> TransitGatewayAttachmentStates {
        match self {
            Self::Vpc(_) | Self::Peering(_) | Self::Connect(_) => TransitGatewayAttachmentStates {
                pending_acceptance: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
                accepting: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
                available: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
                rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING,
                rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            },
            Self::ClientVpn(_) => TransitGatewayAttachmentStates {
                pending_acceptance: CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
                accepting: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
                available: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
                rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
                rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            },
        }
    }

    /// The transit gateway this attachment is on, which a report of the attachment names as
    /// its `TransitGatewayId`.
    ///
    /// A peering attachment is on TWO gateways and carries neither as a plain member: the one
    /// it belongs to is the requester's ("information about the requester transit gateway",
    /// `TransitGatewayPeeringAttachment$RequesterTgwInfo`), the accepter's being the peer
    /// [`Self::resource_id`] reports as the resource this attachment attaches.
    pub fn transit_gateway_id(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.transit_gateway_id.as_deref(),
            Self::Peering(attachment) => attachment
                .requester_tgw_info
                .as_ref()
                .and_then(|info| info.transit_gateway_id.as_deref()),
            Self::ClientVpn(attachment) => attachment.transit_gateway_id.as_deref(),
            Self::Connect(attachment) => attachment.transit_gateway_id.as_deref(),
        }
    }

    /// The account that owns that gateway as the ATTACHMENT records it, or `None` for a kind
    /// whose shape carries no such member — for which the gateway's own record
    /// (`TransitGateway$OwnerId`) is what names its owner.
    ///
    /// Only a peering attachment records it, because a peering spans two accounts and the
    /// requester's gateway need not be one this store holds ("the ID of the Amazon Web
    /// Services account that owns the transit gateway", `PeeringTgwInfo$OwnerId`).
    pub fn transit_gateway_owner_id(&self) -> Option<&str> {
        match self {
            Self::Peering(attachment) => attachment
                .requester_tgw_info
                .as_ref()
                .and_then(|info| info.owner_id.as_deref()),
            Self::Vpc(_) | Self::ClientVpn(_) | Self::Connect(_) => None,
        }
    }

    /// When the attachment was created, as the modelled timestamp a report of it carries.
    ///
    /// `TransitGatewayClientVpnAttachment$CreationTime` is a STRING where every other kind's
    /// is a timestamp, so that one rendering is read back through the one
    /// [`parse_iso8601_timestamp`] this crate reads a stored timestamp string with — and an
    /// attachment whose stored rendering does not read as an instant carries none, rather
    /// than a made-up one.
    pub fn creation_time(&self) -> Option<::aws_smithy_types::DateTime> {
        match self {
            Self::Vpc(attachment) => attachment.creation_time,
            Self::Peering(attachment) => attachment.creation_time,
            Self::Connect(attachment) => attachment.creation_time,
            Self::ClientVpn(attachment) => {
                parse_iso8601_timestamp(attachment.creation_time.as_deref())
            }
        }
    }

    /// The attachment's tags, or `None` for a kind whose shape models none — reported absent
    /// rather than as an empty set, there being no tag set on the record to report.
    pub fn tags(&self) -> Option<&Vec<Tag>> {
        match self {
            Self::Vpc(attachment) => attachment.tags.as_ref(),
            Self::Peering(attachment) => attachment.tags.as_ref(),
            Self::Connect(attachment) => attachment.tags.as_ref(),
            // `TransitGatewayClientVpnAttachment` models no tags at all.
            Self::ClientVpn(_) => None,
        }
    }

    /// The attachment's `TransitGatewayAttachmentResourceType`.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::Vpc(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
            Self::Peering(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING,
            Self::ClientVpn(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
            Self::Connect(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CONNECT,
        }
    }

    /// The id of the resource this attachment attaches, which is what a report of the
    /// attachment names as its `ResourceId`.
    pub fn resource_id(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.vpc_id.as_deref(),
            Self::Peering(attachment) => attachment
                .accepter_tgw_info
                .as_ref()
                .and_then(|info| info.transit_gateway_id.as_deref()),
            Self::ClientVpn(attachment) => attachment.client_vpn_endpoint_id.as_deref(),
            // "The ID of the attachment from which the Connect attachment was created"
            // (`TransitGatewayConnect$TransportTransitGatewayAttachmentId`): the resource a
            // Connect attachment attaches is that transport attachment.
            Self::Connect(attachment) => attachment
                .transport_transit_gateway_attachment_id
                .as_deref(),
        }
    }

    /// The account that owns the resource this attachment attaches.
    pub fn resource_owner_id(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.vpc_owner_id.as_deref(),
            Self::Peering(attachment) => attachment
                .accepter_tgw_info
                .as_ref()
                .and_then(|info| info.owner_id.as_deref()),
            Self::ClientVpn(attachment) => attachment.client_vpn_owner_id.as_deref(),
            // `TransitGatewayConnect` carries no owner member: a Connect attachment is made
            // from an attachment of the same account, so there is no second owner to report.
            Self::Connect(_) => None,
        }
    }

    /// The attachment's current state.
    pub fn state(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.state.as_deref(),
            Self::Peering(attachment) => attachment.state.as_deref(),
            Self::ClientVpn(attachment) => attachment.state.as_deref(),
            Self::Connect(attachment) => attachment.state.as_deref(),
        }
    }

    /// Move the attachment to `state`.
    ///
    /// A peering attachment also carries a `Status`, which is the status OF that state
    /// (`TransitGatewayPeeringAttachment$Status` — "the status of the transit gateway
    /// peering attachment"), so it is moved with it rather than left naming the previous
    /// one.
    pub fn set_state(&mut self, state: &str) {
        match self {
            Self::Vpc(attachment) => attachment.state = Some(state.to_string()),
            Self::Peering(attachment) => {
                attachment.state = Some(state.to_string());
                attachment.status = Some(PeeringAttachmentStatus {
                    code: Some(state.to_string()),
                    message: None,
                });
            }
            Self::ClientVpn(attachment) => attachment.state = Some(state.to_string()),
            Self::Connect(attachment) => attachment.state = Some(state.to_string()),
        }
    }

    /// Stamp `attachment_id` onto the attachment, which the store mints rather than taking
    /// from a request.
    pub(super) fn set_attachment_id(&mut self, attachment_id: &str) {
        match self {
            Self::Vpc(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
            Self::Peering(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
            Self::ClientVpn(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
            Self::Connect(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
        }
    }

    /// The caller-chosen bytes this attachment holds: its tags, and — for an attachment
    /// shape that carries none — the ids of what it attaches, which a writer supplies.
    ///
    /// NOTHING here answers zero for a whole variant: the fixed per-record overhead
    /// [`STORED_RECORD_OVERHEAD_BYTES`] is charged beside it by
    /// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_attachment`], so no stored
    /// attachment is free against the byte budget whatever it carries.
    ///
    /// A handler that has to ask the store's byte budget whether the attachment it is about to
    /// write FITS measures it through the per-kind function this delegates to
    /// ([`transit_gateway_connect_payload_bytes`]) rather than through a view of its own, so
    /// the gate and this charge cannot drift.
    pub(super) fn payload_bytes(&self) -> usize {
        match self {
            Self::Vpc(attachment) => tag_list_bytes(attachment.tags.as_deref().unwrap_or_default()),
            Self::Peering(attachment) => {
                tag_list_bytes(attachment.tags.as_deref().unwrap_or_default())
            }
            // `TransitGatewayClientVpnAttachment` models no tags, so what a writer sizes
            // here is the Client VPN endpoint it names and the account that owns it.
            Self::ClientVpn(attachment) => attachment
                .client_vpn_endpoint_id
                .as_deref()
                .unwrap_or_default()
                .len()
                .saturating_add(
                    attachment
                        .client_vpn_owner_id
                        .as_deref()
                        .unwrap_or_default()
                        .len(),
                ),
            Self::Connect(attachment) => transit_gateway_connect_payload_bytes(attachment),
        }
    }
}

/// The caller-chosen bytes one stored Connect attachment holds: its tags, which are the only
/// member of `TransitGatewayConnect` a caller chooses the size of — the two attachment ids and
/// the gateway id are minted or resolved, and the tunnel protocol is one enum member.
///
/// Beside the view's own [`TransitGatewayAttachmentView::payload_bytes`], which delegates the
/// Connect arm to it, so the create that asks the byte budget whether its attachment FITS
/// weighs it with the very function the insert then charges by, without a view to wrap it in.
pub fn transit_gateway_connect_payload_bytes(attachment: &TransitGatewayConnect) -> usize {
    tag_list_bytes(attachment.tags.as_deref().unwrap_or_default())
}

/// One stored transit gateway attachment: the modelled view its describe reports, plus the
/// state move the clock still owes it.
#[derive(Debug, Clone)]
pub struct TransitGatewayAttachmentRecord {
    /// The attachment itself.
    pub view: TransitGatewayAttachmentView,
    /// The move to the terminal state of an acceptance or a rejection, for an attachment
    /// one of those left in a transient state; `None` for an attachment that owes none.
    pub pending: Option<PendingStateMove>,
}

impl TransitGatewayAttachmentRecord {
    /// Put the attachment into `transient` now and owe it `terminal`
    /// [`TRANSITION_DELAY`] later.
    ///
    /// The span is never zero, so a caller acting straight after the accepting or rejecting
    /// call still sees the transient state and the state errors that go with it.
    pub fn begin_move(&mut self, transient: &str, terminal: &str) {
        self.view.set_state(transient);
        self.pending = Some(PendingStateMove::starting_now(terminal));
    }
}

/// One subnet's association with a transit gateway multicast domain.
///
/// Flattened rather than holding the modelled
/// [`TransitGatewayMulticastDomainAssociation`], which carries exactly one subnet: the
/// association IS per subnet, so the record is per subnet and [`Self::view`] builds the
/// modelled shape when one is reported.
#[derive(Debug, Clone)]
pub struct MulticastDomainAssociationRecord {
    /// The attachment whose resource is associated with the domain.
    pub transit_gateway_attachment_id: String,
    /// The resource the attachment attaches.
    pub resource_id: String,
    /// That resource's `TransitGatewayAttachmentResourceType`.
    pub resource_type: String,
    /// The account that owns the resource.
    pub resource_owner_id: String,
    /// The associated subnet.
    pub subnet_id: String,
    /// The association's state, a member of
    /// `TransitGatewayMulitcastDomainAssociationState`.
    pub state: String,
    /// The move to `associated` an acceptance owes this association.
    pub pending: Option<PendingStateMove>,
}

impl MulticastDomainAssociationRecord {
    /// The modelled association a report of this record answers with.
    pub fn view(&self) -> TransitGatewayMulticastDomainAssociation {
        TransitGatewayMulticastDomainAssociation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_type: Some(self.resource_type.clone()),
            resource_owner_id: Some(self.resource_owner_id.clone()),
            subnet: Some(self.subnet_view()),
        }
    }

    /// The modelled subnet association this record is.
    pub fn subnet_view(&self) -> SubnetAssociation {
        SubnetAssociation {
            subnet_id: Some(self.subnet_id.clone()),
            state: Some(self.state.clone()),
        }
    }
}

/// An association waiting for the domain owner to accept it: the
/// `TransitGatewayMulitcastDomainAssociationState` enum's `pendingAcceptance` member.
pub const MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE: &str = "pendingAcceptance";

/// An association being carried out: that enum's `associating` member.
pub const MULTICAST_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association in force: that enum's `associated` member.
pub const MULTICAST_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// One transit gateway multicast domain and the resources associated with it.
#[derive(Debug, Clone)]
pub struct TransitGatewayMulticastDomainRecord {
    /// The domain's own id, minted by the store.
    pub transit_gateway_multicast_domain_id: String,
    /// The transit gateway the domain belongs to.
    pub transit_gateway_id: String,
    /// The account that owns the domain.
    pub owner_id: String,
    /// The domain's state, a member of `TransitGatewayMulticastDomainState`.
    pub state: String,
    /// The subnet associations of the domain, one per (attachment, subnet).
    pub associations: Vec<MulticastDomainAssociationRecord>,
}

/// Refuse a request that named a transit gateway multicast domain id no domain of the
/// account has: HTTP 400 [`INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND`].
///
/// Worded the way the transit gateway family words a resource of it that is not there —
/// [`invalid_transit_gateway_id_not_found`] for the gateway itself,
/// [`invalid_transit_gateway_route_table_id_not_found`] for its route tables — and built
/// from the id the REQUEST named. The ONE place this refusal is built, so every operation
/// that resolves a `TransitGatewayMulticastDomainId` refuses an unknown one identically.
pub fn invalid_transit_gateway_multicast_domain_id_not_found<E>(domain_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND,
        400,
        format!("Transit Gateway Multicast Domain {domain_id} was deleted or does not exist."),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The transit gateway multicast domain `domain_id` names, refusing a malformed id
    /// first and an unknown one second — the order [`require_well_formed_id`] documents, so
    /// "not a multicast domain id at all" is never reported as "no such multicast domain".
    ///
    /// The ONE place a `TransitGatewayMulticastDomainId` is resolved, on the contract of
    /// [`Self::require_transit_gateway_policy_table`], so every operation that takes one
    /// refuses the same id the same way.
    pub fn require_transit_gateway_multicast_domain<E>(
        &self,
        domain_id: &str,
    ) -> Result<&TransitGatewayMulticastDomainRecord, Scaffolded<E>> {
        require_well_formed_id(
            TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX,
            domain_id,
            INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED,
        )?;
        self.transit_gateway_multicast_domains
            .get(domain_id)
            .ok_or_else(|| invalid_transit_gateway_multicast_domain_id_not_found(domain_id))
    }
}

/// One stored transit gateway route table announcement.
#[derive(Debug, Clone)]
pub struct TransitGatewayRouteTableAnnouncementRecord {
    /// The announcement itself, as the modelled shape its operations answer with.
    pub view: TransitGatewayRouteTableAnnouncement,
    /// The move to `deleted` a delete owes this announcement.
    pub pending: Option<PendingStateMove>,
}

/// An announcement in force: the `TransitGatewayRouteTableAnnouncementState` enum's
/// `available` member.
pub const ANNOUNCEMENT_STATE_AVAILABLE: &str = "available";

/// An announcement a delete has started on: that enum's `deleting` member.
pub const ANNOUNCEMENT_STATE_DELETING: &str = "deleting";

/// An announcement a delete has finished with: that enum's `deleted` member.
pub const ANNOUNCEMENT_STATE_DELETED: &str = "deleted";

// =============================================================================
// Transit gateway Connect peers
// =============================================================================

/// The id prefix a transit gateway Connect peer is minted with.
pub const TRANSIT_GATEWAY_CONNECT_PEER_ID_PREFIX: &str = "tgw-connect-peer";

/// The `ResourceType` a `CreateTransitGatewayConnectPeer` tag specification may name: "The
/// tags to apply to the Connect peer" is the only resource that action tags.
pub const RESOURCE_TYPE_TRANSIT_GATEWAY_CONNECT_PEER: &str = "transit-gateway-connect-peer";

/// A Connect peer that is being created: the `TransitGatewayConnectPeerState` enum's
/// `pending` member.
pub const CONNECT_PEER_STATE_PENDING: &str = "pending";

/// A Connect peer in service: that enum's `available` member.
pub const CONNECT_PEER_STATE_AVAILABLE: &str = "available";

/// The tunnel protocol of a Connect attachment and of every peer on it: `ProtocolValue` has
/// exactly one member, `gre` ("A Connect attachment is a GRE-based tunnel attachment",
/// `CreateTransitGatewayConnect`).
pub const PROTOCOL_VALUE_GRE: &str = "gre";

/// A BGP session that is not established: the `BgpStatus` enum's `down` member.
///
/// What a Connect peer this emulator mints reports, and keeps reporting: a BGP session is
/// established by the appliance on the other side of the GRE tunnel, and nothing in this
/// emulator is that appliance, so reporting `up` would claim a session nobody opened.
pub const BGP_STATUS_DOWN: &str = "down";

/// The index, into a CIDR block, of its first HOST address: the address after the block's
/// own base (network) address.
///
/// The address a tunnel endpoint can be given out of a block, which is what "the first
/// available IP address from the transit gateway CIDR block"
/// (`CreateTransitGatewayConnectPeerRequest$TransitGatewayAddress`) and "the first address
/// from the range" (`$InsideCidrBlocks`, whose range the AWS CLI reference shows peering from
/// `169.254.6.1` for an inside block of `169.254.6.0/29`) both name. ONE definition, so the
/// inside range and the gateway's blocks are counted the same way.
pub const FIRST_HOST_ADDRESS_INDEX: u32 = 1;

/// `InvalidTransitGatewayConnectPeerID.NotFound` (HTTP 400): the request named a Connect
/// peer id this account holds no peer for.
pub const INVALID_TRANSIT_GATEWAY_CONNECT_PEER_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayConnectPeerID.NotFound";

/// `InvalidTransitGatewayConnectPeerID.Malformed` (HTTP 400): the request named something
/// that is not a Connect peer id at all.
pub const INVALID_TRANSIT_GATEWAY_CONNECT_PEER_ID_MALFORMED: &str =
    "InvalidTransitGatewayConnectPeerID.Malformed";

/// One stored transit gateway Connect peer: the modelled shape its own describe reports,
/// plus the state move the clock still owes it.
///
/// The same shape as [`TransitGatewayAttachmentRecord`] and for the same reason: every member
/// a writer sets round-trips through `DescribeTransitGatewayConnectPeers` without a
/// field-by-field copy, and the non-terminal state a create leaves the peer in carries the
/// [`PendingStateMove`] to its terminal one.
#[derive(Debug, Clone)]
pub struct TransitGatewayConnectPeerRecord {
    /// The Connect peer itself.
    pub view: TransitGatewayConnectPeer,
    /// The move to `available` a create owes a `pending` peer; `None` once it has been
    /// applied.
    pub pending: Option<PendingStateMove>,
}

/// The caller-chosen bytes one stored Connect peer holds: its tags and the addresses and
/// inside CIDR blocks the request asked for.
///
/// The ONE definition of that charge, the way [`transit_gateway_payload_bytes`] is a transit
/// gateway's: the create asks it before the write and
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_connect_peer`] charges exactly it.
pub fn transit_gateway_connect_peer_payload_bytes(peer: &TransitGatewayConnectPeer) -> usize {
    let tags = tag_list_bytes(peer.tags.as_deref().unwrap_or_default());
    let configuration = peer.connect_peer_configuration.as_ref();
    let addresses = configuration
        .and_then(|configuration| configuration.peer_address.as_deref())
        .map_or(0, str::len)
        .saturating_add(
            configuration
                .and_then(|configuration| configuration.transit_gateway_address.as_deref())
                .map_or(0, str::len),
        );
    let inside_blocks: usize = configuration
        .and_then(|configuration| configuration.inside_cidr_blocks.as_deref())
        .unwrap_or_default()
        .iter()
        .map(String::len)
        .sum();
    tags.saturating_add(addresses).saturating_add(inside_blocks)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Connect peer fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_connect_peer_capacity(&self) -> bool {
        self.transit_gateway_connect_peers.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a Connect peer in the non-terminal state a create leaves it in, with the move to
    /// `available` the clock owes it, and answer the `TransitGatewayConnectPeerId` it was
    /// given — or `None` when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and nothing
    /// was stored.
    ///
    /// The id, the state and the owed move are minted HERE rather than taken from `view`, the
    /// way [`Self::insert_transit_gateway_attachment`] mints an attachment id, so no path can
    /// report a peer in a state the store does not hold or store one uncharged.
    pub fn insert_transit_gateway_connect_peer(
        &mut self,
        mut view: TransitGatewayConnectPeer,
    ) -> Option<String> {
        if !self.has_transit_gateway_connect_peer_capacity() {
            return None;
        }
        let peer_id = self.next_id(TRANSIT_GATEWAY_CONNECT_PEER_ID_PREFIX);
        view.transit_gateway_connect_peer_id = Some(peer_id.clone());
        view.state = Some(CONNECT_PEER_STATE_PENDING.to_string());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + transit_gateway_connect_peer_payload_bytes(&view),
        );
        self.transit_gateway_connect_peers.insert(
            peer_id.clone(),
            TransitGatewayConnectPeerRecord {
                view,
                pending: Some(PendingStateMove::starting_now(CONNECT_PEER_STATE_AVAILABLE)),
            },
        );
        Some(peer_id)
    }

    /// The transit gateway the Connect attachment `attachment_id` names belongs to, or `None`
    /// when that id names no Connect attachment of this account.
    pub fn connect_attachment_transit_gateway_id(&self, attachment_id: &str) -> Option<&str> {
        match self.transit_gateway_attachments.get(attachment_id) {
            Some(TransitGatewayAttachmentRecord {
                view: TransitGatewayAttachmentView::Connect(attachment),
                ..
            }) => attachment.transit_gateway_id.as_deref(),
            _ => None,
        }
    }

    /// Every transit gateway address a Connect peer of the transit gateway
    /// `transit_gateway_id` already holds.
    ///
    /// A peer names its Connect attachment, and the attachment names its transit gateway, so
    /// the set is reached through the attachment store rather than kept as a second copy on
    /// the peer.
    pub fn transit_gateway_connect_addresses_in_use(
        &self,
        transit_gateway_id: &str,
    ) -> BTreeSet<&str> {
        self.transit_gateway_connect_peers
            .values()
            .filter(|record| {
                record
                    .view
                    .transit_gateway_attachment_id
                    .as_deref()
                    .and_then(|attachment_id| {
                        self.connect_attachment_transit_gateway_id(attachment_id)
                    })
                    == Some(transit_gateway_id)
            })
            .filter_map(|record| {
                record
                    .view
                    .connect_peer_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.transit_gateway_address.as_deref())
            })
            .collect()
    }

    /// Every interior BGP range a Connect peer of the transit gateway `transit_gateway_id`
    /// already holds, as those peers report them.
    ///
    /// Reached through the attachment store for the same reason as
    /// [`Self::transit_gateway_connect_addresses_in_use`]: a peer names its attachment and
    /// the attachment names its gateway, so no second copy is kept on the peer.
    pub fn transit_gateway_connect_inside_blocks_in_use(
        &self,
        transit_gateway_id: &str,
    ) -> BTreeSet<&str> {
        self.transit_gateway_connect_peers
            .values()
            .filter(|record| {
                record
                    .view
                    .transit_gateway_attachment_id
                    .as_deref()
                    .and_then(|attachment_id| {
                        self.connect_attachment_transit_gateway_id(attachment_id)
                    })
                    == Some(transit_gateway_id)
            })
            .flat_map(|record| {
                record
                    .view
                    .connect_peer_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.inside_cidr_blocks.as_deref())
                    .unwrap_or_default()
                    .iter()
                    .map(String::as_str)
            })
            .collect()
    }

    /// The address a Connect peer of `transit_gateway_id` gets when the request named none:
    /// "If not specified, Amazon automatically assigns the first available IP address from
    /// the transit gateway CIDR block"
    /// (`CreateTransitGatewayConnectPeerRequest$TransitGatewayAddress`).
    ///
    /// The gateway's CIDR blocks are searched in the order the gateway holds them, from
    /// [`FIRST_HOST_ADDRESS_INDEX`] — a block's base address is the block itself, not an
    /// address a tunnel endpoint can be given — and only the blocks of the family `want_ipv6`
    /// names are considered, because the peer's two addresses "must be the same IP address
    /// family". `None` means the gateway has no block of that family with an address no other
    /// peer of it already holds.
    ///
    /// The search is bounded: the first free address of a block is within its first
    /// `addresses_in_use + 1` host addresses, so no block is walked further than that.
    pub fn first_available_transit_gateway_connect_address(
        &self,
        transit_gateway_id: &str,
        want_ipv6: bool,
    ) -> Option<String> {
        let in_use = self.transit_gateway_connect_addresses_in_use(transit_gateway_id);
        let limit = u32::try_from(in_use.len()).unwrap_or(u32::MAX);
        let blocks = self
            .transit_gateways
            .get(transit_gateway_id)?
            .options
            .as_ref()?
            .transit_gateway_cidr_blocks
            .as_deref()
            .unwrap_or_default();
        for block in blocks {
            if cidr_block_prefix_length(block).is_none()
                || nth_address_in_cidr_block(block, 0)
                    .as_deref()
                    .and_then(ip_address_is_ipv6)
                    != Some(want_ipv6)
            {
                continue;
            }
            for offset in 0..=limit {
                let index = FIRST_HOST_ADDRESS_INDEX.saturating_add(offset);
                match nth_address_in_cidr_block(block, index) {
                    None => break,
                    Some(address) if !in_use.contains(address.as_str()) => return Some(address),
                    Some(_) => continue,
                }
            }
        }
        None
    }
}

// =============================================================================
// Transit gateway metering policies
// =============================================================================

/// The id prefix a transit gateway metering policy is minted with.
///
/// The model publishes no `@pattern` for `TransitGatewayMeteringPolicyId`
/// (`smithy_query(query="shape",shape="TransitGatewayMeteringPolicyId")` is a plain
/// `String`), so the prefix follows the transit gateway family's own abbreviation style, the
/// way [`TRANSIT_GATEWAY_POLICY_TABLE_ID_PREFIX`] abbreviates a policy table's. Every reader
/// and writer of the kind takes it from HERE, so the form an id is minted in and the form one
/// is checked against cannot drift apart.
pub const TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX: &str = "tgw-mp";

/// A metering policy whose creation is still being carried out: the
/// `TransitGatewayMeteringPolicyState` enum's `pending` member
/// ([`crate::ops::types::TRANSIT_GATEWAY_METERING_POLICY_STATE_VALUES`]), which is what
/// `CreateTransitGatewayMeteringPolicy` answers with until [`TRANSITION_DELAY`] has passed.
pub const TRANSIT_GATEWAY_METERING_POLICY_STATE_PENDING: &str = "pending";

/// A metering policy that is in force: that enum's `available` member, the terminal state the
/// clock owes a freshly created policy.
pub const TRANSIT_GATEWAY_METERING_POLICY_STATE_AVAILABLE: &str = "available";

/// The `ResourceType` a `CreateTransitGatewayMeteringPolicy` tag specification may name: the
/// metering policy it creates ("The tags to assign to the metering policy",
/// `CreateTransitGatewayMeteringPolicyRequest$TagSpecifications`), which is the `ResourceType`
/// enum's `transit-gateway-metering-policy` member
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY: &str = "transit-gateway-metering-policy";

/// `InvalidTransitGatewayMeteringPolicyId.NotFound` (HTTP 400): the request named a transit
/// gateway metering policy id this account holds no policy for.
pub const INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayMeteringPolicyId.NotFound";

/// `InvalidTransitGatewayMeteringPolicyIdMalformedException` (HTTP 400): the request named
/// something that is not a transit gateway metering policy id at all.
///
/// The code AWS answers for this resource, in the exception-suffixed spelling the newer EC2
/// actions use (the same spelling `InvalidLaunchTemplateName.NotFoundException` carries), as
/// the recorded refusal of a `CreateTransitGatewayMeteringPolicyEntry` naming an id outside
/// the id form shows. Every operation that resolves a `TransitGatewayMeteringPolicyId` takes
/// it from HERE, so one resource answers one code.
pub const INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED: &str =
    "InvalidTransitGatewayMeteringPolicyIdMalformedException";

/// Refuse a request that named a transit gateway metering policy id no policy of the account
/// has: HTTP 400 `InvalidTransitGatewayMeteringPolicyId.NotFound`, worded the way
/// [`invalid_transit_gateway_id_not_found`] words the transit gateway's own.
pub fn invalid_transit_gateway_metering_policy_id_not_found<E>(
    transit_gateway_metering_policy_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_NOT_FOUND,
        400,
        format!(
            "Transit Gateway Metering Policy {transit_gateway_metering_policy_id} was deleted or \
             does not exist."
        ),
    )
}

/// Whether `id` is a well-formed transit gateway metering policy id:
/// `<prefix>-<8 or 17 lowercase hex digits>` under this resource's own prefix
/// ([`TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX`]), read by the one generic id reader
/// ([`prefixed_id_is_well_formed`]).
pub fn transit_gateway_metering_policy_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX, id)
}

/// Refuse something that is not a transit gateway metering policy id at all: HTTP 400
/// [`INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED`], answered BEFORE the id is looked
/// up, so "not an id of this kind" is never reported as "no such policy".
///
/// The ONE place this resource's malformed refusal is worded, for every operation that
/// resolves a `TransitGatewayMeteringPolicyId`. The sentence is AWS's own for this
/// resource — `The <resource type> ID '<id>' is malformed`, the template
/// [`invalid_image_usage_report_id_malformed`] carries for its own kind, with the resource
/// spelled as the modelled `ResourceType` spells it
/// ([`RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY`]) — built from the id the REQUEST sent,
/// bounded by the one echo every refusal of this crate quotes a caller's value through.
pub fn invalid_transit_gateway_metering_policy_id_malformed<E>(
    transit_gateway_metering_policy_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_METERING_POLICY_ID_MALFORMED,
        400,
        format!(
            "The {RESOURCE_TYPE_TRANSIT_GATEWAY_METERING_POLICY} ID '{echoed}' is malformed",
            echoed = echoed_resource_id(transit_gateway_metering_policy_id),
        ),
    )
}

/// Hold a `TransitGatewayMeteringPolicyId` to the id form before anything is looked up:
/// [`invalid_transit_gateway_metering_policy_id_malformed`] for a value outside it.
pub fn require_well_formed_transit_gateway_metering_policy_id<E>(
    transit_gateway_metering_policy_id: &str,
) -> Result<(), Scaffolded<E>> {
    if transit_gateway_metering_policy_id_is_well_formed(transit_gateway_metering_policy_id) {
        return Ok(());
    }
    Err(invalid_transit_gateway_metering_policy_id_malformed(
        transit_gateway_metering_policy_id,
    ))
}

/// The caller-chosen bytes one stored metering policy holds: its tags and the middlebox
/// attachment ids the request named.
///
/// The ONE definition of that charge, the way [`transit_gateway_payload_bytes`] is a transit
/// gateway's, so a writer cannot store one uncharged and
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway_metering_policy`] charges exactly
/// this.
pub fn transit_gateway_metering_policy_payload_bytes(policy: &TransitGatewayMeteringPolicy) -> usize {
    let tags = tag_list_bytes(policy.tags.as_deref().unwrap_or_default());
    let middlebox_attachments: usize = policy
        .middlebox_attachment_ids
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(String::len)
        .sum();
    tags.saturating_add(middlebox_attachments)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the store can hold another transit gateway metering policy.
    ///
    /// The same contract as [`Self::has_transit_gateway_route_table_capacity`]: a create asks
    /// first and refuses with [`resource_limit_exceeded`], and the writer below asks the same
    /// predicate, so the two cannot drift apart.
    pub fn has_transit_gateway_metering_policy_capacity(&self) -> bool {
        self.transit_gateway_metering_policies.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a transit gateway metering policy and answer the
    /// `TransitGatewayMeteringPolicyId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    ///
    /// The id is minted here rather than taken from `view`, exactly as
    /// [`Self::insert_transit_gateway_policy_table`] mints a policy table's, and the
    /// caller-chosen bytes the policy holds are charged on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, so no path can store one
    /// uncharged.
    ///
    /// The state in `view` is stored as it stands and nothing is owed on the policy: a
    /// transient one comes from [`Self::insert_pending_transit_gateway_metering_policy`],
    /// which is what a CREATE goes through.
    pub fn insert_transit_gateway_metering_policy(
        &mut self,
        mut view: TransitGatewayMeteringPolicy,
    ) -> Option<String> {
        if !self.has_transit_gateway_metering_policy_capacity() {
            return None;
        }
        let metering_policy_id = self.next_id(TRANSIT_GATEWAY_METERING_POLICY_ID_PREFIX);
        view.transit_gateway_metering_policy_id = Some(metering_policy_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + transit_gateway_metering_policy_payload_bytes(&view),
        );
        self.transit_gateway_metering_policies
            .insert(metering_policy_id.clone(), view);
        Some(metering_policy_id)
    }

    /// Store the metering policy a create minted and answer it as it was stored, or `None`
    /// when the store is at [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    ///
    /// Everything the request does not decide is decided HERE rather than by the handler, so
    /// a later writer of the kind cannot record a fresh policy differently: the id comes from
    /// [`Self::insert_transit_gateway_metering_policy`], the state is the TRANSIENT
    /// [`TRANSIT_GATEWAY_METERING_POLICY_STATE_PENDING`] with
    /// [`TRANSIT_GATEWAY_METERING_POLICY_STATE_AVAILABLE`] owed one [`TRANSITION_DELAY`]
    /// later — the span is never zero, so a caller acting straight after the create still
    /// sees the policy pending — and "the date and time when the metering policy update
    /// becomes effective" (`TransitGatewayMeteringPolicy$UpdateEffectiveAt`) is that same
    /// instant, read off the wall clock, so the member and the owed move cannot disagree
    /// about when this policy starts metering.
    pub fn insert_pending_transit_gateway_metering_policy(
        &mut self,
        mut view: TransitGatewayMeteringPolicy,
    ) -> Option<TransitGatewayMeteringPolicy> {
        view.state = Some(TRANSIT_GATEWAY_METERING_POLICY_STATE_PENDING.to_string());
        view.update_effective_at = Some(system_time_to_date_time(
            SystemTime::now() + TRANSITION_DELAY,
        ));
        let metering_policy_id = self.insert_transit_gateway_metering_policy(view)?;
        self.transit_gateway_metering_policy_moves.insert(
            metering_policy_id.clone(),
            PendingStateMove::starting_now(TRANSIT_GATEWAY_METERING_POLICY_STATE_AVAILABLE),
        );
        self.transit_gateway_metering_policies
            .get(&metering_policy_id)
            .cloned()
    }
}

// =============================================================================
// Transit gateway policy tables, and the attachments associated with them
// =============================================================================

/// The id prefix a transit gateway policy table is minted with.
pub const TRANSIT_GATEWAY_POLICY_TABLE_ID_PREFIX: &str = "tgw-ptb";

/// A policy table association being carried out: the `TransitGatewayAssociationState`
/// enum's `associating` member
/// (`smithy_query(query="shape",shape="TransitGatewayAssociationState")`, whose members are
/// `associating`, `associated`, `disassociating`, `disassociated`).
pub const TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association in force: that enum's `associated` member.
pub const TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// A policy table in service: the `TransitGatewayPolicyTableState` enum's `available`
/// member (that enum being `pending`, `available`, `deleting`, `deleted`).
pub const TRANSIT_GATEWAY_POLICY_TABLE_STATE_AVAILABLE: &str = "available";

/// One attachment's association with a transit gateway policy table: the modelled
/// [`TransitGatewayPolicyTableAssociation`] every operation that reports one answers with,
/// plus the move the clock owes it.
///
/// The modelled shape itself rather than a parallel record, for the reason every other
/// store here holds one: a member a writer sets round-trips through
/// `GetTransitGatewayPolicyTableAssociations` without a field-by-field copy that can
/// silently drop one.
#[derive(Debug, Clone)]
pub struct TransitGatewayPolicyTableAssociationRecord {
    /// The association as the model shapes it, carrying the state reached so far.
    pub view: TransitGatewayPolicyTableAssociation,
    /// The move to `associated` an association owes while it is still `associating`;
    /// `None` for one that owes none.
    pub pending: Option<PendingStateMove>,
}

/// One transit gateway policy table and the attachments associated with it.
///
/// The associations are keyed by `TransitGatewayAttachmentId`, because an association IS
/// per (table, attachment): one attachment cannot be associated with one table twice, and
/// the key order is the stable order a report of them pages over.
#[derive(Debug, Clone)]
pub struct TransitGatewayPolicyTableRecord {
    /// The policy table itself, as the modelled shape its own operations report.
    pub view: TransitGatewayPolicyTable,
    /// The attachments associated with this table, by `TransitGatewayAttachmentId`.
    pub associations: BTreeMap<String, TransitGatewayPolicyTableAssociationRecord>,
    /// The entries of this table, by the rule number that identifies one within it, as the
    /// modelled [`TransitGatewayPolicyTableEntry`] shape itself.
    ///
    /// Keyed by the rule number because that is what identifies an entry WITHIN its table —
    /// `DeleteTransitGatewayPolicyTableEntry` and `ModifyTransitGatewayPolicyTableEntry` each
    /// name an entry by its table and that number alone — so a table cannot hold two entries
    /// at one number, and the key order is "lower rule numbers are evaluated first and take
    /// precedence" (`CreateTransitGatewayPolicyTableEntryRequest$PolicyRuleNumber`), which is
    /// the stable order `GetTransitGatewayPolicyTableEntries` reports and pages over. The
    /// entries live INSIDE the table's record, so a table taken out of the store takes its
    /// entries with it; what one is stored as and the store's one writer of them are
    /// [`crate::state::transit_gateway_policy_table_entries`].
    pub entries: BTreeMap<i32, TransitGatewayPolicyTableEntry>,
}

/// `InvalidTransitGatewayPolicyTableId.NotFound` (HTTP 400): the request named a transit
/// gateway policy table id this account holds no policy table for.
pub const INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayPolicyTableId.NotFound";

/// `InvalidTransitGatewayPolicyTableId.Malformed` (HTTP 400): the request named something
/// that is not a transit gateway policy table id at all.
pub const INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_MALFORMED: &str =
    "InvalidTransitGatewayPolicyTableId.Malformed";

/// Refuse a request that named a transit gateway policy table id no policy table of the
/// account has: HTTP 400 `InvalidTransitGatewayPolicyTableId.NotFound`.
///
/// The sentence is the one the policy table's own operations and the report of its entries
/// answer this condition with, and it names the table THE REQUEST named. The operations that
/// write an ENTRY of a table word the same refusal differently — see
/// [`invalid_transit_gateway_policy_table_id_not_found_for_entry`] — so each is worded in one
/// place rather than per handler.
pub fn invalid_transit_gateway_policy_table_id_not_found<E>(
    transit_gateway_policy_table_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_NOT_FOUND,
        400,
        format!(
            "Transit Gateway Policy Table {transit_gateway_policy_table_id} was deleted or does \
             not exist."
        ),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The transit gateway policy table `transit_gateway_policy_table_id` names, refusing a
    /// malformed id first and an unknown one second — the order
    /// [`require_well_formed_id`] documents, so "not an id of this kind" is never reported
    /// as "no such policy table".
    ///
    /// The ONE place a `TransitGatewayPolicyTableId` is resolved, so every operation that
    /// takes one refuses the same id the same way.
    pub fn require_transit_gateway_policy_table<E>(
        &self,
        transit_gateway_policy_table_id: &str,
    ) -> Result<&TransitGatewayPolicyTableRecord, Scaffolded<E>> {
        require_well_formed_id(
            TRANSIT_GATEWAY_POLICY_TABLE_ID_PREFIX,
            transit_gateway_policy_table_id,
            INVALID_TRANSIT_GATEWAY_POLICY_TABLE_ID_MALFORMED,
        )?;
        self.transit_gateway_policy_tables
            .get(transit_gateway_policy_table_id)
            .ok_or_else(|| {
                invalid_transit_gateway_policy_table_id_not_found(transit_gateway_policy_table_id)
            })
    }

    /// The policy table `transit_gateway_attachment_id` is already associated with, if any.
    ///
    /// An attachment is associated with at most one policy table, so this answers at most
    /// one id; it is a scan of the tables the account holds, which is bounded by
    /// [`MAX_STORED_RECORDS_PER_STORE`] like every other store here.
    pub fn policy_table_of_attachment(&self, transit_gateway_attachment_id: &str) -> Option<&str> {
        self.transit_gateway_policy_tables
            .iter()
            .find(|(_, table)| {
                table
                    .associations
                    .contains_key(transit_gateway_attachment_id)
            })
            .map(|(policy_table_id, _)| policy_table_id.as_str())
    }

    /// Store a transit gateway policy table and answer the `TransitGatewayPolicyTableId` it
    /// was given — or `None` when the store is full.
    ///
    /// The id is minted here rather than taken from `view`, exactly as
    /// [`Self::insert_transit_gateway_attachment`] mints an attachment id, and the
    /// caller-chosen bytes the table holds (its tags) are charged here on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, so no path can store one
    /// uncharged. A table is stored with no associations and no entries: every association is
    /// made by a request, and so is every entry.
    pub fn insert_transit_gateway_policy_table(
        &mut self,
        mut view: TransitGatewayPolicyTable,
    ) -> Option<String> {
        if self.transit_gateway_policy_tables.len() >= MAX_STORED_RECORDS_PER_STORE {
            return None;
        }
        let policy_table_id = self.next_id(TRANSIT_GATEWAY_POLICY_TABLE_ID_PREFIX);
        view.transit_gateway_policy_table_id = Some(policy_table_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.transit_gateway_policy_tables.insert(
            policy_table_id.clone(),
            TransitGatewayPolicyTableRecord {
                view,
                associations: BTreeMap::new(),
                entries: BTreeMap::new(),
            },
        );
        Some(policy_table_id)
    }
}

/// Associate the transit gateway attachment `transit_gateway_attachment_id` with the
/// transit gateway policy table `transit_gateway_policy_table_id`, and answer the
/// association as it now stands.
///
/// The rule `AssociateTransitGatewayPolicyTable` follows, in one place so a later
/// disassociation and report cannot disagree with it: both resources must exist, an
/// attachment already associated with a policy table cannot be associated with another, and
/// a new association starts in the TRANSIENT `associating` state with the clock owing it
/// `associated` — the span is never zero, so a caller acting straight after this call still
/// sees `associating`.
///
/// The association's `ResourceId` and `ResourceType` are read off the stored attachment
/// ("The resource ID of the transit gateway attachment" /  "The resource type for the
/// transit gateway policy table association",
/// `TransitGatewayPolicyTableAssociation$ResourceId` / `$ResourceType`), never off the
/// request, which names neither. The CALLER refuses a malformed id of either kind first, so
/// nothing here depends on the request's form.
pub fn associate_transit_gateway_policy_table<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    transit_gateway_policy_table_id: &str,
    transit_gateway_attachment_id: &str,
) -> Result<TransitGatewayPolicyTableAssociation, Scaffolded<E>> {
    // The clock first: an association whose move has fallen due is already `associated`, so
    // what this call is asked against is the state the store is ACTUALLY in.
    store.materialize_due_transitions();

    // The policy table the request names, before the attachment: the operation's members
    // name the table first, and a request naming two unknown resources is told about the
    // first one.
    store.require_transit_gateway_policy_table(transit_gateway_policy_table_id)?;

    // "The ID of the transit gateway attachment to associate with the policy table"
    // (`AssociateTransitGatewayPolicyTableRequest$TransitGatewayAttachmentId`): an id
    // naming no attachment of the account is refused with the attachment's own NotFound
    // code rather than silently associating nothing.
    let attachment = store
        .transit_gateway_attachments
        .get(transit_gateway_attachment_id)
        .ok_or_else(|| {
            invalid_transit_gateway_attachment_id_not_found(transit_gateway_attachment_id)
        })?;
    let resource_id = attachment.view.resource_id().map(str::to_string);
    let resource_type = attachment.view.resource_type().to_string();

    // One attachment, at most one policy table — so a second association is refused with
    // EC2's own `Resource.AlreadyAssociated` and NOTHING is written, whether the table it
    // already belongs to is this one or another.
    if let Some(holder) = store.policy_table_of_attachment(transit_gateway_attachment_id) {
        return Err(resource_already_associated(format!(
            "The transit gateway attachment '{transit_gateway_attachment_id}' is already \
             associated with the transit gateway policy table '{holder}'"
        )));
    }

    let view = TransitGatewayPolicyTableAssociation {
        transit_gateway_policy_table_id: Some(transit_gateway_policy_table_id.to_string()),
        transit_gateway_attachment_id: Some(transit_gateway_attachment_id.to_string()),
        resource_id,
        resource_type: Some(resource_type),
        state: Some(TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATING.to_string()),
    };
    // An association retains no caller-chosen text at all — every string on it is either a
    // word this module chose or an id read back out of another store, and an attachment is
    // associated at most once, so the table cannot hold more associations than the
    // attachment store holds attachments ([`MAX_STORED_RECORDS_PER_STORE`]). No byte charge
    // is made for one, for the reason a Windows fast launch configuration makes none.
    let table = store
        .transit_gateway_policy_tables
        .get_mut(transit_gateway_policy_table_id)
        .ok_or_else(|| {
            internal_error(format!(
                "the transit gateway policy table '{transit_gateway_policy_table_id}' resolved a \
                 moment ago is not in the store any more"
            ))
        })?;
    table.associations.insert(
        transit_gateway_attachment_id.to_string(),
        TransitGatewayPolicyTableAssociationRecord {
            view: view.clone(),
            pending: Some(PendingStateMove::starting_now(
                TRANSIT_GATEWAY_ASSOCIATION_STATE_ASSOCIATED,
            )),
        },
    );
    Ok(view)
}

// =============================================================================
// IPAM pools, and the one BYOIP state the release path has no word for
// =============================================================================

/// The id prefix an IPAM pool is minted with.
pub const IPAM_POOL_ID_PREFIX: &str = "ipam-pool";

/// A range whose advertisement has been asked for and is not in force yet: the
/// `ByoipCidrState` enum's `pending-advertising` member.
///
/// The rest of that catalog, the store of ranges ([`ByoipCidrRecord`]) and the CIDR
/// canonicalisation ([`canonical_byoip_cidr`]) are the BYOIP section above — this is the one
/// member the provision/release path has no use for, and an advertisement is the only
/// operation that puts a range into it.
pub const BYOIP_CIDR_STATE_PENDING_ADVERTISING: &str = "pending-advertising";

/// The `IpamScopeType` of a pool in the public scope, which is the only scope a BYOIP CIDR
/// can be moved into: "Only used for provisioning IP address CIDRs to pools in the public
/// scope" (`IpamPool$PublicIpSource`).
pub const IPAM_SCOPE_TYPE_PUBLIC: &str = "public";

/// The `AddressFamily` of an IPv4 pool or range.
pub const ADDRESS_FAMILY_IPV4: &str = "ipv4";

/// The `AddressFamily` of an IPv6 pool or range.
pub const ADDRESS_FAMILY_IPV6: &str = "ipv6";

/// The id prefix an IPAM scope is minted with.
///
/// The prefix AWS's own refusal spells for a scope that does not exist ("The IPAM scope
/// (ipam-scope-…) does not exist", recorded in the wire corpus for a `CreateIpamPool` naming
/// an unknown scope), read through the one [`prefixed_id_is_well_formed`] every resource of
/// this module asks.
pub const IPAM_SCOPE_ID_PREFIX: &str = "ipam-scope";

/// The `IpamScopeType` of a scope holding the account's own internal space: the other member
/// of that catalog beside [`IPAM_SCOPE_TYPE_PUBLIC`]
/// (`smithy_query(query="shape",shape="IpamScopeType")` is `public | private`).
pub const IPAM_SCOPE_TYPE_PRIVATE: &str = "private";

/// A pool whose creation has been asked for and has not settled yet: the `IpamPoolState`
/// enum's `create-in-progress` member
/// (`smithy_query(query="shape",shape="IpamPoolState")`), which is what `CreateIpamPool`
/// answers with.
pub const IPAM_POOL_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// A pool whose creation has settled: the `IpamPoolState` enum's `create-complete` member,
/// which the clock owes an [`IPAM_POOL_STATE_CREATE_IN_PROGRESS`] pool one
/// [`TRANSITION_DELAY`] after the call that created it.
pub const IPAM_POOL_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A pool whose deletion has been asked for and has not settled yet: the `IpamPoolState`
/// enum's `delete-in-progress` member, the TRANSIENT state a delete answers with.
pub const IPAM_POOL_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A pool whose deletion has settled: that same enum's `delete-complete` member, which the
/// clock owes every pool a delete has started on, one [`TRANSITION_DELAY`] later.
pub const IPAM_POOL_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// Whether a pool in `state` has already had its deletion asked for: it is either still
/// [`IPAM_POOL_STATE_DELETE_IN_PROGRESS`] or already [`IPAM_POOL_STATE_DELETE_COMPLETE`],
/// and in neither is there a pool left to delete.
///
/// The ONE definition of "is this pool on its way out", so the delete path, the pools-within
/// walk and the scope's pool count all read a pool's way out the same way.
pub fn ipam_pool_is_deleting(pool: &IpamPool) -> bool {
    let state = pool.state.as_deref().unwrap_or_default();
    state == IPAM_POOL_STATE_DELETE_IN_PROGRESS || state == IPAM_POOL_STATE_DELETE_COMPLETE
}

/// The `IpamPool$Locale` a pool created without one reports: the pool's default locale,
/// spelled out.
///
/// "Default is none and means any locale" (`CreateIpamPoolRequest$Locale`) — a pool whose
/// create named no locale allocates in any Region of its IPAM, and the Region catalog this
/// emulator serves holds no Region spelled this way ([`region_name_is_well_formed`] refuses
/// it as a locale), so the spelling cannot collide with a locale a pool actually names.
pub const IPAM_POOL_LOCALE_NONE: &str = "None";

/// `pool` as a response reports it.
///
/// "The locale of the IPAM pool" (`IpamPool$Locale`) is a member of the pool structure EVERY
/// read of a pool carries: a pool created without a locale reports
/// [`IPAM_POOL_LOCALE_NONE`] rather than leaving the element out, so a client that reads the
/// member to decide where a pool allocates finds it on every pool instead of having to tell
/// "no locale" from "no such member".
///
/// The record itself keeps no locale for such a pool — the default is what the pool REPORTS,
/// not something the create stored — so this is applied where a pool is reported, the way
/// [`reported_network_acl_entry`] is the one projection of a stored ACL entry; every
/// operation that reports an [`IpamPool`] goes through it, so no two readers report one pool
/// differently.
pub fn reported_ipam_pool(mut pool: IpamPool) -> IpamPool {
    pool.locale
        .get_or_insert_with(|| IPAM_POOL_LOCALE_NONE.to_string());
    pool
}

/// The `IpamPoolPublicIpSource` a public-scope pool gets when the request names none:
/// "Default is byoip" (`CreateIpamPoolRequest$PublicIpSource`).
pub const IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP: &str = "byoip";

/// The `ResourceType` an IPAM pool is tagged under, from the modelled catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES` carries `ipam-pool`), and the type segment of
/// its ARN.
pub const RESOURCE_TYPE_IPAM_POOL: &str = "ipam-pool";

/// The `ResourceType` an IPAM scope is tagged under, from the same catalog, and the type
/// segment of its ARN.
pub const RESOURCE_TYPE_IPAM_SCOPE: &str = "ipam-scope";

/// How deep a tree of IPAM pools may go: "The pool depth quota is 10"
/// (`IpamPool$PoolDepth`). A top-level pool sits at depth 1 and a pool created inside a
/// source pool one deeper, so a create that would pass this is refused rather than stored.
pub const MAX_IPAM_POOL_DEPTH: i32 = 10;

/// The depth of a pool created with no source pool: "the depth of pools in your IPAM pool"
/// (`IpamPool$PoolDepth`) counts the pool itself, so a top-level pool is at 1.
pub const TOP_LEVEL_IPAM_POOL_DEPTH: i32 = 1;

/// The longest netmask an address of `family` can carry: "Possible netmask lengths for IPv4
/// addresses are 0 - 32. Possible netmask lengths for IPv6 addresses are 0 - 128"
/// (`CreateIpamPoolRequest$AllocationMinNetmaskLength`).
///
/// The modelled `IpamNetmaskLength` carries `range 0..128`, which the boundary validator has
/// already applied, so this is the per-family bound INSIDE it that a pool's own allocation
/// rules are held to. The family has been through the modelled `AddressFamily` enum
/// (`ipv4 | ipv6`) by the time this is asked, so anything that is not IPv6 is IPv4.
pub fn max_netmask_length_for_family(family: &str) -> i32 {
    if family == ADDRESS_FAMILY_IPV6 {
        128
    } else {
        32
    }
}

/// The address family a canonical BYOIP range belongs to, as `AddressFamily` spells it on
/// the wire.
///
/// One definition of "which family is this range": the decision is
/// [`AddressFamily::of_cidr`]'s and this only names the result the way the model does
/// ([`AddressFamily::wire_name`]), so the two cannot drift.
pub fn byoip_cidr_address_family(cidr: &str) -> &'static str {
    AddressFamily::of_cidr(cidr).wire_name()
}

// =============================================================================
// Error codes of the resources above
// =============================================================================

/// `InvalidTransitGatewayAttachmentID.NotFound` (HTTP 400): the request named a transit
/// gateway attachment id this account holds no attachment for.
pub const INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayAttachmentID.NotFound";

/// `InvalidTransitGatewayAttachmentID.Malformed` (HTTP 400): the request named something
/// that is not a transit gateway attachment id at all.
pub const INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED: &str =
    "InvalidTransitGatewayAttachmentID.Malformed";

/// `InvalidTransitGatewayMulticastDomainId.NotFound` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayMulticastDomainId.NotFound";

/// `InvalidTransitGatewayMulticastDomainId.Malformed` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED: &str =
    "InvalidTransitGatewayMulticastDomainId.Malformed";

/// `InvalidTransitGatewayRouteTableAnnouncementId.NotFound` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayRouteTableAnnouncementId.NotFound";

/// `InvalidTransitGatewayRouteTableAnnouncementId.Malformed` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED: &str =
    "InvalidTransitGatewayRouteTableAnnouncementId.Malformed";

/// `InvalidVpcEndpointId.NotFound` (HTTP 400): the request named a VPC endpoint id this
/// account holds no endpoint for.
pub const INVALID_VPC_ENDPOINT_ID_NOT_FOUND: &str = "InvalidVpcEndpointId.NotFound";

/// `InvalidVpcEndpointId.Malformed` (HTTP 400).
pub const INVALID_VPC_ENDPOINT_ID_MALFORMED: &str = "InvalidVpcEndpointId.Malformed";

/// `InvalidVpcEndpointServiceId.NotFound` (HTTP 400): the request named a VPC endpoint
/// service id this account holds no endpoint service for.
pub const INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND: &str =
    "InvalidVpcEndpointServiceId.NotFound";

/// `InvalidVpcEndpointServiceId.Malformed` (HTTP 400).
pub const INVALID_VPC_ENDPOINT_SERVICE_ID_MALFORMED: &str =
    "InvalidVpcEndpointServiceId.Malformed";

/// Refuse a well-formed VPC endpoint service id this account holds no endpoint service for:
/// HTTP 400 [`INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND`].
///
/// The resource is named the way AWS names it in this refusal — `The VpcEndpointService Id
/// '<id>' does not exist`, the shape's own name with `Id` spelt as one word — rather than in
/// the prose an operation might reach for, with the id from the REQUEST. The ONE place this
/// refusal is built, so every operation that resolves a `ServiceId` answers an unknown one
/// identically.
pub fn invalid_vpc_endpoint_service_id_not_found<E>(service_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND,
        "VpcEndpointService Id",
        service_id,
    )
}

/// `InvalidIpamPoolId.NotFound` (HTTP 400): the request named an IPAM pool id this account
/// holds no pool for.
pub const INVALID_IPAM_POOL_ID_NOT_FOUND: &str = "InvalidIpamPoolId.NotFound";

/// `InvalidIpamPoolId.Malformed` (HTTP 400).
pub const INVALID_IPAM_POOL_ID_MALFORMED: &str = "InvalidIpamPoolId.Malformed";

/// `InvalidIpamScopeId.NotFound` (HTTP 400): the request named an IPAM scope id this account
/// holds no scope for — the code AWS answers a `CreateIpamPool` naming an unknown scope
/// with, as the wire corpus records.
pub const INVALID_IPAM_SCOPE_ID_NOT_FOUND: &str = "InvalidIpamScopeId.NotFound";

/// `InvalidIpamScopeId.Malformed` (HTTP 400): the request named something that is not an
/// IPAM scope id at all, so there is nothing to look up.
pub const INVALID_IPAM_SCOPE_ID_MALFORMED: &str = "InvalidIpamScopeId.Malformed";

/// Refuse a well-formed IPAM scope id this account holds no scope for: HTTP 400
/// [`INVALID_IPAM_SCOPE_ID_NOT_FOUND`].
///
/// The wording is this resource's own — "The IPAM scope (…) does not exist.", the sentence
/// AWS answered a `CreateIpamPool` naming a scope that does not exist with — built from the
/// id the REQUEST named, never from the recording. The parenthesised id is why this resource
/// does not go through [`id_not_found`], whose sentence quotes the id instead.
pub fn invalid_ipam_scope_id_not_found<E>(scope_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPAM_SCOPE_ID_NOT_FOUND,
        400,
        format!("The IPAM scope ({scope_id}) does not exist."),
    )
}

/// Refuse an id of the kind `prefix` names that this service cannot resolve: a malformed
/// one with `malformed`, an unknown one with `not_found`, in that order — the order the
/// service asks the two questions in, so "not an id of this kind" is never reported as "no
/// such resource".
///
/// The ONE place that ordering lives, so every resource of this module answers it the same
/// way; [`require_known_volume`] is the same contract written out for volumes.
pub fn require_well_formed_id<E>(
    prefix: &str,
    id: &str,
    malformed: &'static str,
) -> Result<(), Scaffolded<E>> {
    if prefixed_id_is_well_formed(prefix, id) {
        return Ok(());
    }
    Err(Scaffolded::service_error(
        malformed,
        400,
        invalid_id_message(id),
    ))
}

/// Refuse a request that named a resource id this account holds no resource for: HTTP 400
/// with `code`, which is that resource's own `Invalid<Kind>.NotFound`.
pub fn id_not_found<E>(code: &'static str, kind: &str, id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(code, 400, format!("The {kind} '{id}' does not exist"))
}

/// What the owner of an attached resource decided about a transit gateway attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentDecision {
    /// Accept it: "Accepts a request to attach a VPC to a transit gateway"
    /// (`AcceptTransitGatewayVpcAttachment`).
    Accept,
    /// Reject it: "Rejects a request to attach a VPC to a transit gateway"
    /// (`RejectTransitGatewayVpcAttachment`).
    Reject,
}

impl AttachmentDecision {
    /// The decision as the refusal below names it.
    pub(super) fn verb(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Reject => "rejected",
        }
    }
}

/// Act on the transit gateway attachment `attachment_id` names, and answer it as it now
/// stands.
///
/// The ONE place the rule every accepting and rejecting operation of the attachment family
/// follows is written: "the attachment must be in the `pendingAcceptance` state"
/// (`AcceptTransitGatewayVpcAttachment`), so an attachment in any other state is refused
/// with `IncorrectState` and nothing is written; an accepted one moves to the transient
/// state of an acceptance with the clock owing it `available`, a rejected one to the
/// transient state of a rejection with `rejected` owed.
///
/// `resource_type` is the
/// `TransitGatewayAttachmentResourceType` the calling operation acts on — every kind shares
/// one id space, so an id naming an attachment of ANOTHER kind names no attachment this
/// operation can act on and is refused as unknown. The CALLER refuses a malformed
/// `attachment_id` first, before its own `DryRun` arm, so that nothing here depends on the
/// request's form.
pub fn decide_transit_gateway_attachment<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    attachment_id: &str,
    resource_type: &'static str,
    decision: AttachmentDecision,
) -> Result<TransitGatewayAttachmentView, Scaffolded<E>> {
    // The clock first: an attachment whose move has fallen due is already in its terminal
    // state, so whether this call is admissible is asked of the state the attachment is
    // ACTUALLY in rather than of the one it was left in.
    store.materialize_due_transitions();
    // Which sentence an unresolvable id is refused with is a fact of the attachment KIND this
    // operation acts on: a VPC attachment and a transit gateway peering attachment are the
    // transit gateway's OWN resources, so they carry that family's prose
    // ([`transit_gateway_attachment_deleted_or_absent`]); a client VPN attachment is the Client
    // VPN endpoint's resource and keeps the service's shared `transit-gateway-attachment ID`
    // template, the one every operation that takes an attachment id as a parameter of another
    // action answers.
    let owned_by_the_transit_gateway = matches!(
        resource_type,
        TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC
            | TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING
    );
    let record = store
        .transit_gateway_attachments
        .get_mut(attachment_id)
        .filter(|record| record.view.resource_type() == resource_type)
        .ok_or_else(|| {
            if owned_by_the_transit_gateway {
                transit_gateway_attachment_deleted_or_absent(attachment_id)
            } else {
                invalid_transit_gateway_attachment_id_not_found(attachment_id)
            }
        })?;
    let states = record.view.states();
    if record.view.state() != Some(states.pending_acceptance) {
        return Err(incorrect_state(format!(
            "The transit gateway attachment '{attachment_id}' is in state '{state}'; only an \
             attachment in state '{pending_acceptance}' can be {verb}",
            state = record.view.state().unwrap_or_default(),
            pending_acceptance = states.pending_acceptance,
            verb = decision.verb(),
        )));
    }
    match decision {
        AttachmentDecision::Accept => record.begin_move(states.accepting, states.available),
        AttachmentDecision::Reject => record.begin_move(states.rejecting, states.rejected),
    }
    Ok(record.view.clone())
}

/// The server error a handler answers when the attachment this module just resolved for it
/// turns out to be of another kind — which the resolution above has already ruled out, so
/// reaching it means this module is inconsistent with itself rather than that the request
/// was wrong.
pub fn unexpected_attachment_kind<E>(attachment_id: &str) -> Scaffolded<E> {
    internal_error(format!(
        "the attachment '{attachment_id}' resolved a moment ago is not of the kind this \
         operation acts on"
    ))
}

/// The `ResourceType` an attachment is tagged and ARN'd under — the `transit-gateway-attachment`
/// member of that enum (`smithy_query(query="shape",shape="ResourceType")`) — which is also how
/// EC2 spells this resource kind in the SHARED refusal below, exactly as
/// [`RESOURCE_TYPE_IPAM_INTERNET_REGISTRY_ASSOCIATION`] does for its own resource.
pub const RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT: &str = "transit-gateway-attachment";

/// Refuse a well-formed transit gateway attachment id this account holds no attachment for:
/// HTTP 400 [`INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND`], worded by the one writer of
/// that refusal ([`id_not_found`]).
///
/// The wording EC2 answers an operation that takes an attachment id as a PARAMETER of some
/// other action with — an association, an announcement, a describe's id selection, and the
/// acceptance of a CLIENT VPN attachment, whose resource is the Client VPN endpoint's rather
/// than the transit gateway's. The two attachment kinds the transit gateway itself owns as
/// resources are refused by [`transit_gateway_attachment_deleted_or_absent`] instead; which of
/// the two an acceptance or rejection answers is decided in ONE place
/// ([`decide_transit_gateway_attachment`]).
pub fn invalid_transit_gateway_attachment_id_not_found<E>(attachment_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        &format!("{RESOURCE_TYPE_TRANSIT_GATEWAY_ATTACHMENT} ID"),
        attachment_id,
    )
}

/// Refuse a well-formed id of an attachment the TRANSIT GATEWAY owns as a resource — a VPC
/// attachment or a transit gateway peering attachment — that this account holds none of: HTTP
/// 400 [`INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND`].
///
/// Worded the way the rest of the transit gateway family words the same refusal about its own
/// resources — `<Resource> <id> was deleted or does not exist.`, the sentence
/// [`invalid_transit_gateway_id_not_found`], [`invalid_transit_gateway_route_table_id_not_found`]
/// and [`invalid_transit_gateway_multicast_domain_id_not_found`] already carry, with the id
/// UNQUOTED and taken from the request.
pub fn transit_gateway_attachment_deleted_or_absent<E>(attachment_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        400,
        format!("Transit Gateway Attachment {attachment_id} was deleted or does not exist."),
    )
}

/// Refuse something that is not a transit gateway attachment id at all: HTTP 400
/// [`INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED`], BEFORE the id is looked up, so "not an
/// id of this kind" is never reported as "no such attachment".
///
/// The ONE place this resource's malformed refusal is worded, for every operation that resolves
/// a `TransitGatewayAttachmentId`: like
/// [`require_well_formed_transit_gateway_route_table_id`] beside it, the message NAMES the
/// resource rather than using the resource-agnostic wording of [`require_well_formed_id`]. The
/// sentence is ENTIRELY fixed — this refusal interpolates no value, unlike the route table's —
/// so nothing a caller sent reaches it.
pub fn require_well_formed_transit_gateway_attachment_id<E>(
    attachment_id: &str,
) -> Result<(), Scaffolded<E>> {
    if prefixed_id_is_well_formed(TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX, attachment_id) {
        return Ok(());
    }
    Err(Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        400,
        INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MESSAGE.to_string(),
    ))
}

// =============================================================================
// Writers for the resources above
// =============================================================================

/// How many records each of the stores the writers below fill holds before a writer is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] and [`MAX_STORED_DEDICATED_HOSTS`]
/// and for the same reason: nothing in this emulator reclaims one of these records, and
/// every write mints a FRESH id, so without a count ceiling repeated writes would grow
/// memory without bound. ONE number for all of them because they are bounded for the same
/// reason and at the same scale, and it is well above any published quota a test would
/// exercise. A count ceiling is no bound on the BYTES each record holds, so every write
/// also charges [`STORED_RECORD_OVERHEAD_BYTES`] on top of the caller-chosen text the
/// record carries.
pub const MAX_STORED_RECORDS_PER_STORE: usize = 4096;

/// The fixed overhead one of the records the writers below store costs whatever it
/// carries: the modelled record itself and the map entry holding it, as a CHOSEN constant
/// in the same spirit as the count ceilings above — the point is that a
/// record created with no tags and no optional text is NOT free against
/// [`MAX_STORED_BYTES`], so repeated writes advance the budget whatever they carry, not
/// that it is byte-exact.
pub const STORED_RECORD_OVERHEAD_BYTES: usize = 512;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another transit gateway attachment fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_vpc_endpoint_connection_capacity`]: a writer asks
    /// this BEFORE storing anything and refuses the request with EC2's own quota code when
    /// it answers `false`, and [`Self::insert_transit_gateway_attachment`] asks it again
    /// for itself, so no path can get past the ceiling by forgetting to.
    pub fn has_transit_gateway_attachment_capacity(&self) -> bool {
        self.transit_gateway_attachments.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another transit gateway multicast domain fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_multicast_domain_capacity(&self) -> bool {
        self.transit_gateway_multicast_domains.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another transit gateway route table announcement fits inside the store's
    /// ceiling ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_route_table_announcement_capacity(&self) -> bool {
        self.transit_gateway_route_table_announcements.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another VPC endpoint service configuration fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_vpc_endpoint_service_capacity(&self) -> bool {
        self.vpc_endpoint_services.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another VPC endpoint fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_vpc_endpoint_capacity(&self) -> bool {
        self.vpc_endpoints.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another IPAM pool fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_ipam_pool_capacity(&self) -> bool {
        self.ipam_pools.len() < MAX_STORED_RECORDS_PER_STORE
    }
}
