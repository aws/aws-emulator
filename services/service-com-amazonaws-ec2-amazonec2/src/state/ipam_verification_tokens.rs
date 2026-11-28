// SPDX-License-Identifier: CC0-1.0

//! The IPAM external resource verification TOKEN store: the value a mint draws, how long that
//! value stays usable, the states a token takes and the removal a delete owes it.
//!
//! "A verification token is an Amazon Web Services-generated random value that you can use to
//! prove ownership of an external resource" (`IpamExternalResourceVerificationToken`): the
//! value is the service's own, so every member of a token but its tags is derived here rather
//! than taken from a request — the id, the ARN, the IPAM it was created by, that IPAM's Region,
//! the random value itself, the name, the instant the value stops being usable and the two
//! states a token passes through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// The id prefix a stored verification token is minted with.
///
/// The model publishes no pattern for `IpamExternalResourceVerificationTokenId`
/// (`smithy_query(query="shape",shape="IpamExternalResourceVerificationToken")` targets a
/// plain `String`), so the id takes EC2's own `<prefix>-<hex>` form through
/// [`ComAmazonawsEc2Amazonec2State::next_id`] with the prefix AWS gives the resource:
/// `ipam-ervt-<hex>`, the abbreviation beside the resource's full modelled type
/// ([`RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN`]) exactly as an IPAM resource
/// discovery's `ipam-res-disco-` id sits beside its `ipam-resource-discovery` ARN type.
pub const IPAM_VERIFICATION_TOKEN_ID_PREFIX: &str = "ipam-ervt";

/// The ARN resource type of a verification token, which is also the `ResourceType` a
/// `TagSpecification` names to tag one — from the modelled catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES` carries
/// `ipam-external-resource-verification-token`).
pub const RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN: &str =
    "ipam-external-resource-verification-token";

/// A token whose creation has settled: the `IpamExternalResourceVerificationTokenState` enum's
/// `create-complete` member
/// (`smithy_query(query="shape",shape="IpamExternalResourceVerificationTokenState")`).
///
/// The state a MINT leaves a token in. Nothing is provisioned for a token — its value is drawn
/// by the call that asks for it ([`ComAmazonawsEc2Amazonec2State::mint_token_value`]) and is
/// usable the moment that call answers — so the create has nothing left to finish and the
/// clock owes the record no further move.
pub const IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A token whose deletion has been asked for and has not settled yet: that same enum's
/// `delete-in-progress` member, which is what
/// `DeleteIpamExternalResourceVerificationToken` answers with.
pub const IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A token whose value may still be presented: the `TokenState` enum's `valid` member
/// (`smithy_query(query="shape",shape="TokenState")`), which is what
/// `IpamExternalResourceVerificationToken$Status` reports until the token's `NotAfter` passes.
pub const IPAM_VERIFICATION_TOKEN_STATUS_VALID: &str = "valid";

/// A token whose value may no longer be presented: that same enum's `expired` member, which
/// its `Status` reports from its `NotAfter` onwards.
pub const IPAM_VERIFICATION_TOKEN_STATUS_EXPIRED: &str = "expired";

/// "Token name" (`IpamExternalResourceVerificationToken$TokenName`).
///
/// The create carries no name member at all
/// (`smithy_query(query="operation",operation="CreateIpamExternalResourceVerificationToken")`
/// takes only the IPAM, the tags, the client token and the dry-run flag), so the name is the
/// SERVICE's and not a caller's: every token this emulator mints carries the one name the
/// resource is known by, which is what a `token-name` filter therefore selects on.
pub const IPAM_VERIFICATION_TOKEN_NAME: &str = "ipam-verification-token";

/// How long a minted token's value stays usable — the span between the mint and the
/// `NotAfter` it reports.
///
/// "Token expiration" is all the model says of `NotAfter`
/// (`smithy_query(query="doc",shape="IpamExternalResourceVerificationToken",member="NotAfter")`),
/// naming no span, so the span is this emulator's own CHOICE: long enough that a caller can
/// carry the value to the external authority it is proving ownership at within one session,
/// short enough that an expired token is reachable in a test by holding one past it. Nothing is
/// slept for it: the `Status` a read reports is derived from this instant
/// ([`ipam_verification_token_status_at`]).
pub const IPAM_VERIFICATION_TOKEN_VALIDITY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// `InvalidIpamExternalResourceVerificationTokenId.NotFound`: the request named a verification
/// token id this account holds no token for.
pub const INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND: &str =
    "InvalidIpamExternalResourceVerificationTokenId.NotFound";

/// `InvalidIpamExternalResourceVerificationTokenId.Malformed`: the request named something
/// that is not a verification token id at all, so there is nothing to look up — the
/// counterpart of [`INVALID_IPAM_ID_MALFORMED`] for this resource.
pub const INVALID_IPAM_VERIFICATION_TOKEN_ID_MALFORMED: &str =
    "InvalidIpamExternalResourceVerificationTokenId.Malformed";

/// How this resource is NAMED where a refusal quotes it: the spelling AWS itself uses for the
/// resource, which is its modelled type
/// ([`RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN`]) — as the recorded refusal of a
/// token that no longer exists shows.
pub const IPAM_VERIFICATION_TOKEN_KIND: &str = RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN;

/// How this resource's ID is named in the not-found sentence — the kind
/// [`id_not_found_message`] interpolates, which makes `The
/// ipam-external-resource-verification-token ID '<id>' does not exist`, the sentence AWS answers
/// a request naming a token that does not exist with.
pub const IPAM_VERIFICATION_TOKEN_ID_KIND: &str =
    "ipam-external-resource-verification-token ID";

/// Refuse a request that named a verification token the account holds none of: HTTP 400
/// [`INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND`].
///
/// The sentence is the QUOTED template every resource of this crate reports an absent id with
/// ([`id_not_found_message`]), with this resource's own kind in it and the id the REQUEST named:
/// `The ipam-external-resource-verification-token ID '<id>' does not exist`. The IPAM and its
/// scopes answer a parenthesised sentence of their own instead
/// ([`invalid_ipam_id_not_found`]), so WHICH an operation answers is a fact of the operation —
/// the same split [`ipam_id_not_found`] records for the BYOASN surface.
pub fn invalid_ipam_verification_token_id_not_found<E>(token_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND,
        IPAM_VERIFICATION_TOKEN_ID_KIND,
        token_id,
    )
}

/// Whether the token `token` has had its deletion asked for already — the one definition of
/// "this token is on its way out", so no path restarts the clock on one and no reader counts it
/// among the tokens its IPAM still has.
pub fn ipam_verification_token_is_deleting(token: &IpamExternalResourceVerificationToken) -> bool {
    token.state.as_deref() == Some(IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS)
}

/// The `Status` the token `token` carries at `now`: [`IPAM_VERIFICATION_TOKEN_STATUS_VALID`]
/// until the `NotAfter` it reports, [`IPAM_VERIFICATION_TOKEN_STATUS_EXPIRED`] from then on.
///
/// DERIVED from the instant the token itself carries rather than stored beside it, so the two
/// members cannot disagree: a token whose expiration has passed is one whose value the external
/// authority will no longer accept, whether or not anybody called in between. A token carrying
/// no expiration at all is not expired by one.
pub fn ipam_verification_token_status_at(
    token: &IpamExternalResourceVerificationToken,
    now: SystemTime,
) -> &'static str {
    let Some(not_after) = token.not_after else {
        return IPAM_VERIFICATION_TOKEN_STATUS_VALID;
    };
    if system_time_to_date_time(now).as_secs_f64() < not_after.as_secs_f64() {
        IPAM_VERIFICATION_TOKEN_STATUS_VALID
    } else {
        IPAM_VERIFICATION_TOKEN_STATUS_EXPIRED
    }
}

/// What a `CreateIpamExternalResourceVerificationToken` decides about the token it mints: the
/// IPAM that creates it and the tag payload the request named.
///
/// Everything else a token reports is the SERVICE's, so it is derived by the one writer
/// ([`ComAmazonawsEc2Amazonec2State::insert_ipam_verification_token`]) rather than carried
/// here — which is what keeps a create and a later read from building one member two ways.
#[derive(Debug, Clone)]
pub struct IpamVerificationTokenSpec {
    /// "The ID of the IPAM that will create the token"
    /// (`CreateIpamExternalResourceVerificationTokenRequest$IpamId`), as the stored IPAM
    /// resolved it.
    pub ipam_id: String,
    /// "ARN of the IPAM that created the token"
    /// (`IpamExternalResourceVerificationToken$IpamArn`), taken from that stored IPAM.
    pub ipam_arn: Option<String>,
    /// "Region of the IPAM that created the token"
    /// (`IpamExternalResourceVerificationToken$IpamRegion`), taken from that stored IPAM.
    pub ipam_region: Option<String>,
    /// "Token tags" (`CreateIpamExternalResourceVerificationTokenRequest$TagSpecifications`),
    /// already bounded by the one shared reader of a tag payload.
    pub tags: Vec<Tag>,
    /// The request's `ClientToken`, which names the token it mints from then on.
    pub client_token: Option<String>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// The random value one verification token carries: 16 bytes of the host's own randomness
    /// in the hyphenated UUID form AWS issues a token value in, through the one renderer this
    /// workspace shares.
    ///
    /// "An Amazon Web Services-generated random value"
    /// (`IpamExternalResourceVerificationToken`) — so the value is DRAWN rather than derived
    /// from the request, and two tokens of one IPAM carry different values. A host that will
    /// not hand out randomness leaves the draw with the store's own monotonic counter in its
    /// low bytes instead of failing the call: a token value has to be unique for the proof it
    /// stands for to name one token, which holds however the draw came out.
    fn mint_token_value(&mut self) -> String {
        let mut bytes = [0_u8; 16];
        if ::aws_lc_rs::rand::fill(&mut bytes).is_err() {
            self.id_counter = self.id_counter.saturating_add(1);
            bytes[8..].copy_from_slice(&self.id_counter.to_be_bytes());
        }
        ::service_common::uuid_from_random_bytes(bytes)
    }

    /// Whether another verification token fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]) — the same contract
    /// [`Self::has_ipam_scope_capacity`] has for a scope: a writer asks this BEFORE storing
    /// anything and refuses the request with EC2's own quota code when it answers `false`.
    pub fn has_ipam_verification_token_capacity(&self) -> bool {
        self.ipam_verification_tokens.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Mint one verification token for the IPAM `spec` names and answer the
    /// `IpamExternalResourceVerificationTokenId` it was given — or `None` when the store is at
    /// its ceiling and NOTHING was stored.
    ///
    /// The ONE way a token is stored, so the members the SERVICE owns cannot be built two ways:
    /// the id, the ARN, the random value, the name, the expiration and the state are all
    /// derived here, and the IPAM's own facts come from the stored IPAM the caller resolved.
    /// The token is stored in the terminal [`IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE`]
    /// state for the reason that constant records, so the clock owes a minted token nothing.
    ///
    /// `spec.client_token` names the token from here on, so a replay of the same request
    /// answers it instead of minting a second one, and every caller-chosen byte the record
    /// retains — the tags and that token — is charged to the store's byte budget beside the
    /// fixed [`STORED_RECORD_OVERHEAD_BYTES`] each record costs.
    pub fn insert_ipam_verification_token(
        &mut self,
        spec: IpamVerificationTokenSpec,
    ) -> Option<String> {
        if !self.has_ipam_verification_token_capacity() {
            return None;
        }
        let token_id = self.next_id(IPAM_VERIFICATION_TOKEN_ID_PREFIX);
        let token_value = self.mint_token_value();
        let view = IpamExternalResourceVerificationToken {
            ipam_external_resource_verification_token_id: Some(token_id.clone()),
            // "Token ARN" (`$IpamExternalResourceVerificationTokenArn`): an EC2 IPAM-family
            // ARN under this resource's modelled type, whose Region segment is empty for the
            // reason [`ipam_resource_arn`] records — the token reports the Region of the IPAM
            // that created it in `IpamRegion` instead.
            ipam_external_resource_verification_token_arn: Some(ipam_resource_arn(
                RESOURCE_TYPE_IPAM_EXTERNAL_RESOURCE_VERIFICATION_TOKEN,
                &token_id,
            )),
            ipam_id: Some(spec.ipam_id),
            ipam_arn: spec.ipam_arn,
            ipam_region: spec.ipam_region,
            token_value: Some(token_value),
            token_name: Some(IPAM_VERIFICATION_TOKEN_NAME.to_string()),
            // "Token expiration" (`$NotAfter`), one validity span from the mint.
            not_after: Some(system_time_to_date_time(
                SystemTime::now() + IPAM_VERIFICATION_TOKEN_VALIDITY,
            )),
            // "Token status" (`$Status`), which is `valid` until that instant and is kept in
            // step with it by [`Self::expire_due_ipam_verification_tokens`].
            status: Some(IPAM_VERIFICATION_TOKEN_STATUS_VALID.to_string()),
            tags: Some(spec.tags.clone()),
            state: Some(IPAM_VERIFICATION_TOKEN_STATE_CREATE_COMPLETE.to_string()),
        };
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(&spec.tags)
                + spec.client_token.as_deref().unwrap_or_default().len(),
        );
        if let Some(client_token) = spec.client_token {
            self.ipam_verification_token_client_tokens
                .insert(client_token, token_id.clone());
        }
        self.ipam_verification_tokens
            .insert(token_id.clone(), view);
        Some(token_id)
    }

    /// Start deleting the verification token `token_id` names, and answer it as the store then
    /// holds it — `None` when the account holds no such token.
    ///
    /// The token is left in the TRANSIENT
    /// [`IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS`] state the modelled catalog carries,
    /// with the clock owing its REMOVAL one [`TRANSITION_DELAY`] later
    /// ([`Self::ipam_verification_token_delete_at`], applied by
    /// [`Self::materialize_due_transitions`]) — the arrangement
    /// [`Self::begin_ipam_scope_deletion`] has for a scope, for the same reason: a deleted
    /// token is one `DescribeIpamExternalResourceVerificationTokens` answers
    /// [`INVALID_IPAM_VERIFICATION_TOKEN_ID_NOT_FOUND`] for, so the row cannot stay in the
    /// store for ever. The span is never zero, so a caller acting straight after this call
    /// still sees the delete under way, and nothing is slept for.
    pub fn begin_ipam_verification_token_deletion(
        &mut self,
        token_id: &str,
    ) -> Option<IpamExternalResourceVerificationToken> {
        let token = self.ipam_verification_tokens.get_mut(token_id)?;
        token.state = Some(IPAM_VERIFICATION_TOKEN_STATE_DELETE_IN_PROGRESS.to_string());
        let view = token.clone();
        self.ipam_verification_token_delete_at
            .insert(token_id.to_string(), Instant::now() + TRANSITION_DELAY);
        Some(view)
    }

    /// Remove one verification token, giving the bytes it held back to the store's budget.
    ///
    /// The exact reverse of [`Self::insert_ipam_verification_token`]: the record's tags and the
    /// `ClientToken`s that minted it are given back, and the move the clock still owed it goes
    /// with it. A token left in the store naming an IPAM nothing resolves would be a dangling
    /// row, which is why [`Self::remove_ipam_verification_tokens_of`] takes an IPAM's tokens
    /// with the IPAM.
    pub(super) fn remove_ipam_verification_token(&mut self, token_id: &str) {
        let Some(token) = self.ipam_verification_tokens.remove(token_id) else {
            return;
        };
        self.release_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                .saturating_add(tag_list_bytes(token.tags.as_deref().unwrap_or_default())),
        );
        self.ipam_verification_token_delete_at.remove(token_id);
        let spent: Vec<String> = self
            .ipam_verification_token_client_tokens
            .iter()
            .filter(|(_, minted)| minted.as_str() == token_id)
            .map(|(client_token, _)| client_token.clone())
            .collect();
        let mut released = 0_usize;
        for client_token in spent {
            self.ipam_verification_token_client_tokens
                .remove(&client_token);
            released = released.saturating_add(client_token.len());
        }
        self.release_stored_bytes(released);
    }

    /// Remove every verification token the IPAM `ipam_id` names created.
    ///
    /// A token reports "the ID of the IPAM that created the token"
    /// (`IpamExternalResourceVerificationToken$IpamId`), so a token of an IPAM the store no
    /// longer holds would name an IPAM nothing resolves: the tokens go when the IPAM goes,
    /// exactly as its scopes and their pools do ([`Self::remove_ipam`]).
    pub(super) fn remove_ipam_verification_tokens_of(&mut self, ipam_id: &str) {
        let doomed: Vec<String> = self
            .ipam_verification_tokens
            .iter()
            .filter(|(_, token)| token.ipam_id.as_deref() == Some(ipam_id))
            .map(|(token_id, _)| token_id.clone())
            .collect();
        for token_id in doomed {
            self.remove_ipam_verification_token(&token_id);
        }
    }

    /// Apply the removal the clock owes a deleted verification token, as of `now`.
    ///
    /// A token a `DeleteIpamExternalResourceVerificationToken` left `delete-in-progress` was
    /// GONE at the instant that call recorded for it, whether or not anybody called again — so
    /// the removal is applied from [`Self::materialize_due_transitions`], its one caller,
    /// before anything reads the store, exactly as
    /// [`Self::materialize_due_ipam_scope_deletions`] applies a scope's.
    pub(super) fn materialize_due_ipam_verification_token_deletions(&mut self, now: Instant) {
        let due: Vec<String> = self
            .ipam_verification_token_delete_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(token_id, _)| token_id.clone())
            .collect();
        for token_id in due {
            self.ipam_verification_token_delete_at.remove(&token_id);
            self.remove_ipam_verification_token(&token_id);
        }
    }

    /// Bring every stored token's `Status` in step with the wall clock, as of `now`.
    ///
    /// A token whose `NotAfter` has passed was expired at that instant whether or not anybody
    /// called ([`ipam_verification_token_status_at`] is the one reading of that), so applying it
    /// here changes nothing a later caller would not have seen anyway — the same contract every
    /// other clock-owed move in [`Self::materialize_due_transitions`] has. A token whose
    /// deletion is already under way is left alone: it is on its way out of the store, not
    /// changing what its value may still be used for.
    pub(super) fn expire_due_ipam_verification_tokens(&mut self, now: SystemTime) {
        for token in self.ipam_verification_tokens.values_mut() {
            if ipam_verification_token_is_deleting(token) {
                continue;
            }
            let status = ipam_verification_token_status_at(token, now);
            if token.status.as_deref() != Some(status) {
                token.status = Some(status.to_string());
            }
        }
    }
}
