// SPDX-License-Identifier: CC0-1.0

//! The egress-only internet gateway store: the id form, the bounds, the byte charge, the one
//! writer, the removal, the `ClientToken` map that makes the create idempotent and the error
//! channel `CreateEgressOnlyInternetGateway`, `DescribeEgressOnlyInternetGateways` and
//! `DeleteEgressOnlyInternetGateway` share.
//!
//! The gateways themselves live on the service's one state struct
//! ([`ComAmazonawsEc2Amazonec2State::egress_only_internet_gateways`]) as the MODELLED
//! [`EgressOnlyInternetGateway`] shape, for the reason
//! [`ComAmazonawsEc2Amazonec2State::internet_gateways`] holds an internet gateway that way:
//! the create and the describe answer the same shape, so storing it is what stops them
//! reporting one gateway differently. Only the primitives over that store live here.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use super::{
    id_not_found, tag_list_bytes, ComAmazonawsEc2Amazonec2State, ATTACHMENT_STATUS_ATTACHED,
    INVALID_GATEWAY_ID_NOT_FOUND, MAX_STORED_RECORDS_PER_STORE,
    STORED_RECORD_OVERHEAD_BYTES,
};
use crate::ops::types::{EgressOnlyInternetGateway, InternetGatewayAttachment, Tag};
use crate::scaffolded::Scaffolded;

/// The id prefix an egress-only internet gateway is minted under.
///
/// EC2 names the resource's ids `eigw-<hex>` — the spelling
/// `CreateRouteRequest$EgressOnlyInternetGatewayId` documents — which is the same
/// `<prefix>-<hex>` form every other EC2 resource id takes, so
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const EGRESS_ONLY_INTERNET_GATEWAY_ID_PREFIX: &str = "eigw";

/// The one `ResourceType` a `CreateEgressOnlyInternetGateway` tag specification may name: "The
/// tags to assign to the egress-only internet gateway"
/// (`CreateEgressOnlyInternetGatewayRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const EGRESS_ONLY_INTERNET_GATEWAY_TAGGABLE_RESOURCE: &str = "egress-only-internet-gateway";

/// The largest page `DescribeEgressOnlyInternetGateways` serves, and the page it serves when
/// the request names no size.
///
/// The top of the modelled `MaxResults` range,
/// `DescribeEgressOnlyInternetGatewaysMaxResults`'s `range 5..255`
/// (`smithy_query(query="operation",operation="DescribeEgressOnlyInternetGateways")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_EGRESS_ONLY_INTERNET_GATEWAYS_PAGE_SIZE: usize = 255;

/// The caller-chosen bytes ONE stored egress-only internet gateway holds for the process's
/// lifetime: its tags, beside the per-record overhead every other store of this module
/// charges.
///
/// Everything else on the record — its id and the attachment its create wrote — is derived by
/// this crate out of ids it minted or resolved, so it is bounded already and not counted here.
/// The ONE definition of that charge:
/// [`ComAmazonawsEc2Amazonec2State::insert_egress_only_internet_gateway`] charges exactly it,
/// so a writer cannot store bytes the store's budget never saw.
pub fn egress_only_internet_gateway_payload_bytes(tags: &[Tag]) -> usize {
    STORED_RECORD_OVERHEAD_BYTES.saturating_add(tag_list_bytes(tags))
}

/// Refuse a request that named an egress-only internet gateway id this account holds no
/// gateway for: HTTP 400 [`INVALID_GATEWAY_ID_NOT_FOUND`] — the code EC2 answers a gateway id
/// that names no gateway of the account with — in the
/// `The <resource> ID '<id>' does not exist` wording EC2 answers an unresolvable id with,
/// built from the id the REQUEST carried.
///
/// The resource is named `eigw`, as the recorded refusal of this resource's id spells it —
/// the id's own prefix rather than the shape's name, which is this one resource's wording
/// and why it is written here rather than taken from a shared sentence.
///
/// `EgressOnlyInternetGatewayId` publishes no pattern
/// (`smithy_query(query="shape",shape="EgressOnlyInternetGatewayId")`), so there is no
/// spelling to call an id malformed by: one code covers an id of any shape, the same reading
/// [`super::INVALID_INTERNET_GATEWAY_ID_NOT_FOUND`] has for an internet gateway.
pub fn invalid_egress_only_internet_gateway_id_not_found<E>(
    egress_only_internet_gateway_id: &str,
) -> Scaffolded<E> {
    id_not_found(
        INVALID_GATEWAY_ID_NOT_FOUND,
        "eigw ID",
        egress_only_internet_gateway_id,
    )
}

/// Refuse a create the store has no room for: HTTP 400 `GatewayLimitExceeded`, the answer for
/// an [`ComAmazonawsEc2Amazonec2State::insert_egress_only_internet_gateway`] that stored
/// nothing.
pub fn egress_only_internet_gateway_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(EGRESS_ONLY_INTERNET_GATEWAY_LIMIT_EXCEEDED, 400, message)
}

/// EC2's quota code for one gateway too many (EC2 API "Error codes":
/// `GatewayLimitExceeded` — "you've reached the limit on the number of gateways you can
/// create"), which is the code this emulator refuses a create its store has no room for with.
pub const EGRESS_ONLY_INTERNET_GATEWAY_LIMIT_EXCEEDED: &str = "GatewayLimitExceeded";

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another egress-only internet gateway fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_internet_gateway_capacity`]: a create asks first so it can refuse with
    /// [`egress_only_internet_gateway_limit_exceeded`] instead of overshooting.
    pub fn has_egress_only_internet_gateway_capacity(&self) -> bool {
        self.egress_only_internet_gateways.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an egress-only internet gateway of the VPC `vpc_id` carrying `tags`, and answer
    /// the `EgressOnlyInternetGatewayId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller —
    /// the id and the attachment — so two writers cannot report the same gateway differently,
    /// exactly as [`Self::insert_internet_gateway`] does for an internet gateway. The gateway
    /// is attached to the VPC the create named, in the `attached` state: "Creates an
    /// egress-only internet gateway for your VPC" (`CreateEgressOnlyInternetGateway`), so the
    /// create itself is the attachment — there is no `Attach…` call for this kind of gateway,
    /// and [`ATTACHMENT_STATUS_ATTACHED`] is how the `AttachmentStatus` of
    /// `InternetGatewayAttachment$State` spells a live attachment.
    ///
    /// The caller-chosen bytes the record retains
    /// ([`egress_only_internet_gateway_payload_bytes`]) are charged against the store's byte
    /// budget here, so no path can store them uncharged.
    ///
    /// `None`, with NOTHING stored, when the store is at [`MAX_STORED_RECORDS_PER_STORE`]
    /// gateways or the bytes this one would hold would cross [`super::MAX_STORED_BYTES`]:
    /// both bounds are enforced here, so a writer that forgets to ask cannot grow the store
    /// without bound.
    pub fn insert_egress_only_internet_gateway(
        &mut self,
        vpc_id: &str,
        tags: Vec<Tag>,
    ) -> Option<String> {
        let charged_bytes = egress_only_internet_gateway_payload_bytes(&tags);
        if !self.has_egress_only_internet_gateway_capacity()
            || self.would_exceed_stored_bytes(charged_bytes)
        {
            return None;
        }
        let egress_only_internet_gateway_id = self.next_id(EGRESS_ONLY_INTERNET_GATEWAY_ID_PREFIX);
        let egress_only_internet_gateway = EgressOnlyInternetGateway {
            attachments: Some(vec![InternetGatewayAttachment {
                state: Some(ATTACHMENT_STATUS_ATTACHED.to_string()),
                vpc_id: Some(vpc_id.to_string()),
            }]),
            egress_only_internet_gateway_id: Some(egress_only_internet_gateway_id.clone()),
            tags: Some(tags),
        };
        self.add_stored_bytes(charged_bytes);
        self.egress_only_internet_gateways.insert(
            egress_only_internet_gateway_id.clone(),
            egress_only_internet_gateway,
        );
        Some(egress_only_internet_gateway_id)
    }

    /// Remove the egress-only internet gateway `egress_only_internet_gateway_id` names and
    /// answer it as it was last reported, or `None` when the store holds no such gateway.
    ///
    /// The answer is the record the removal took OUT of the store, on the contract
    /// [`Self::remove_internet_gateway`] has: what a delete reports is what the readers
    /// reported, never a second copy built beside it. The bytes the record holds go back to
    /// the store's budget on exactly the terms
    /// [`Self::insert_egress_only_internet_gateway`] took them on, read off the tags the
    /// record holds NOW — `CreateTags` and `DeleteTags` reach this gateway's tags too, so the
    /// charge the store is carrying for this record is the one its current tags define, not
    /// the one its create happened to take. The `ClientToken` entries that name it go with it,
    /// so a replayed token can never resolve to an id the store no longer holds, and a route
    /// still pointing at the gateway READS [`super::ROUTE_STATE_BLACKHOLE`]
    /// ([`Self::reported_route_table`] derives it): that is what `Route$State`
    /// means by `blackhole` — "the specified gateway isn't attached to the VPC" — and the
    /// gateway this delete took away attaches nothing at all now.
    pub fn remove_egress_only_internet_gateway(
        &mut self,
        egress_only_internet_gateway_id: &str,
    ) -> Option<EgressOnlyInternetGateway> {
        let egress_only_internet_gateway = self
            .egress_only_internet_gateways
            .remove(egress_only_internet_gateway_id)?;
        self.release_stored_bytes(egress_only_internet_gateway_payload_bytes(
            egress_only_internet_gateway
                .tags
                .as_deref()
                .unwrap_or_default(),
        ));
        self.forget_egress_only_internet_gateway_client_tokens_of(egress_only_internet_gateway_id);
        Some(egress_only_internet_gateway)
    }

    /// The VPC the egress-only internet gateway `egress_only_internet_gateway_id` is attached
    /// to, or `None` when the store holds no such gateway.
    ///
    /// Read off the gateway's own attachment set, the one place the attachment is recorded, so
    /// the `DeleteVpc` dependency check and the route target resolution cannot disagree about
    /// which VPC a gateway belongs to.
    pub fn egress_only_internet_gateway_attached_vpc(
        &self,
        egress_only_internet_gateway_id: &str,
    ) -> Option<String> {
        self.egress_only_internet_gateways
            .get(egress_only_internet_gateway_id)?
            .attachments
            .iter()
            .flatten()
            .find_map(|attachment| attachment.vpc_id.clone())
    }

    /// The egress-only internet gateway the `CreateEgressOnlyInternetGateway` `ClientToken`
    /// `client_token` already minted, if one did.
    ///
    /// A READ: a token that has minted nothing is not recorded here. The map is kept in step
    /// by [`Self::forget_egress_only_internet_gateway_client_tokens_of`], which the removal
    /// calls, so a token never names a gateway the store no longer holds — the same contract
    /// [`Self::route_table_minted_by_client_token`] has for a route table.
    pub fn egress_only_internet_gateway_minted_by_client_token(
        &self,
        client_token: &str,
    ) -> Option<String> {
        self.egress_only_internet_gateway_client_tokens
            .get(client_token)
            .cloned()
    }

    /// Record that the `ClientToken` `client_token` minted the egress-only internet gateway
    /// `egress_only_internet_gateway_id`, so a replay of that request reports that gateway
    /// instead of creating a second one.
    ///
    /// The one writer of [`Self::egress_only_internet_gateway_client_tokens`]. The token's own
    /// bytes are bounded by [`super::MAX_CLIENT_TOKEN_CHARS`], which the create holds it to,
    /// and the map holds at most one entry per stored gateway.
    pub fn record_egress_only_internet_gateway_client_token(
        &mut self,
        client_token: String,
        egress_only_internet_gateway_id: &str,
    ) {
        self.egress_only_internet_gateway_client_tokens
            .insert(client_token, egress_only_internet_gateway_id.to_string());
    }

    /// Drop every `ClientToken` entry that names the egress-only internet gateway
    /// `egress_only_internet_gateway_id` — what the removal of a gateway owes, since a token
    /// that names a gateway the store no longer holds could otherwise replay into a dangling
    /// id.
    pub fn forget_egress_only_internet_gateway_client_tokens_of(
        &mut self,
        egress_only_internet_gateway_id: &str,
    ) {
        self.egress_only_internet_gateway_client_tokens
            .retain(|_, minted| minted.as_str() != egress_only_internet_gateway_id);
    }
}
