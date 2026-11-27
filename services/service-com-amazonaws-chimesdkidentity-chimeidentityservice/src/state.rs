// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkidentity-chimeidentityservice service.
//!
//! The `AppInstance` is this service's root resource: every other identity resource
//! (users, bots, admins, endpoints, retention settings) hangs off exactly one of them.
//! State is therefore a single `BTreeMap` of app instances keyed by ARN, with each
//! instance owning its users and its bots, plus four index maps:
//!
//! * `users_by_arn` — a user ARN to its owning instance ARN, because
//!   `DescribeAppInstanceUser` / `DeleteAppInstanceUser` are keyed only by the user ARN.
//! * `bots_by_arn` — the same index for the bots an instance owns, because the bot
//!   operations are keyed only by the bot ARN.
//! * `idempotency` — a create operation's `ClientRequestToken` to the ARN that token
//!   already minted, so repeating one token never mints a second resource.
//! * `deleted_app_instances` — the ARN of every app instance this store has deleted, so
//!   a request addressed to a dead instance (or to a resource the delete cascaded away
//!   with it) is refused as one this caller may no longer make rather than as an ARN
//!   that never named anything.
//!
//! An `AppInstanceUserEndpoint` is not addressed by an ARN of its own — every operation
//! on one names its owning user's ARN plus an `EndpointId` — so an endpoint is held in
//! its user's own `endpoints` map rather than in an index beside the two above, and the
//! deletion of a user (or of the instance that cascades to it) takes its endpoints with
//! it. For the same reason the register token that minted an endpoint lives ON the
//! endpoint record rather than in `idempotency`, which maps a token to an ARN.
//!
//! `BTreeMap` (not `HashMap`) everywhere: list operations page in key order, so the
//! iteration order has to be stable for a `NextToken` to be resumable.
//!
//! Two things this service reads are NOT in the struct below, because another Chime
//! service reads or writes them too and the engine keys state by type: the app-instance
//! ADMIN directory ([`app_instance_admins`]) and the family's ARN-keyed TAG directory
//! ([`chime_tags`]). A third, the family's identity NAME directory
//! ([`identity_names`]), is one this service WRITES for a sibling to read: the name an
//! `AppInstanceUser` or `AppInstanceBot` carries is reported by Chime SDK Messaging on
//! every channel participant, which cannot see the records below.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::BuildHasher as _;
use std::ops::Bound;
use std::time::{SystemTime, UNIX_EPOCH};

use ::service_common::page_tokens::PageTokenKey;

use crate::ops::types::{
    AppInstanceRetentionSettings, Configuration, EndpointAttributes, EndpointState,
    ExpirationSettings,
};

/// The one rule that decides whether a tagging call may address an ARN, which all three
/// generic tagging handlers resolve through rather than each stating it again.
pub mod tagging_arn;

pub use tagging_arn::{
    tagging_arn_refusal, too_many_tags_message, withdrawal_for, TaggingArnRefusal,
    RESOURCE_LIMIT_EXCEEDED_CODE,
};

/// The region used for a minted ARN when the request carries no SigV4 credential
/// scope to read one from (an unsigned or presigned request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The SigV4 signing name of the Chime SDK services, and so the service segment of
/// every ARN this emulator mints: the model's `aws.auth#sigv4` name is `chime`.
pub const ARN_SERVICE: &str = "chime";

/// The largest page any list operation in this service returns, from the model's
/// `MaxResults` range (`min 1, max 50`) on `ListAppInstances` / `ListAppInstanceUsers`.
/// Also the page size used when the caller supplies no `MaxResults`.
pub const MAX_PAGE_SIZE: i32 = 50;

/// The version marker a `NextToken` this service issues carries, so a token whose
/// cursor this store authenticates but whose layout an earlier version of this code
/// wrote is recognised as not one the running listing issued.
const TOKEN_VERSION: &str = "v1";

/// How many `AppInstance`s this emulator holds for one account, and how many
/// `AppInstanceUser`s one `AppInstance` holds.
///
/// These are the EMULATOR's capacity bounds, not a claim about any published AWS
/// quota: the store lives in process memory, so the resource count has to be bounded
/// somewhere or a create loop grows it without limit. A create past the bound is
/// refused with the modelled `ResourceLimitExceededException` ("The request exceeds
/// the resource limit"), which both create operations declare, so a client sees a
/// modelled quota refusal rather than an emulator that swells.
pub const MAX_APP_INSTANCES: usize = 100;

/// See [`MAX_APP_INSTANCES`].
pub const MAX_APP_INSTANCE_USERS: usize = 500;

/// How many `AppInstanceBot`s one `AppInstance` holds; see [`MAX_APP_INSTANCES`] for
/// why the store is bounded at all.
pub const MAX_APP_INSTANCE_BOTS: usize = 100;

/// How many `AppInstanceUserEndpoint`s one `AppInstanceUser` holds; see
/// [`MAX_APP_INSTANCES`] for why the store is bounded at all. A register past the bound
/// is refused with the modelled `ResourceLimitExceededException`, which
/// `RegisterAppInstanceUserEndpoint` declares.
pub const MAX_APP_INSTANCE_USER_ENDPOINTS: usize = 100;

/// The ceiling on
/// [`idempotency`](ComAmazonawsChimesdkidentityChimeidentityserviceState::idempotency).
///
/// That map's size is already one entry per LIVE resource, which the two caps above
/// bound: an entry is written only where a create actually minted a new resource
/// (`insert_user` refuses to overwrite a live ARN, and an instance ARN is a fresh
/// uuid), and it is released again when that resource is deleted. This constant is
/// the structural ceiling that keeps the map bounded even if a future create path
/// records a token without minting anything: past it, a token is not recorded.
pub const MAX_IDEMPOTENCY_TOKENS: usize =
    MAX_APP_INSTANCES * (1 + MAX_APP_INSTANCE_USERS + MAX_APP_INSTANCE_BOTS);

/// How many deleted `AppInstance` ARNs the store remembers, so a request addressed to a
/// deleted instance — or to a resource that hung off one — is told apart from a request
/// addressed to an ARN that never named anything.
///
/// One entry per instance the store can hold ([`MAX_APP_INSTANCES`]), which keeps the
/// set bounded: past it the oldest deletion is forgotten and its ARN reads as an unknown
/// one again.
pub const MAX_DELETED_APP_INSTANCES: usize = MAX_APP_INSTANCES;

/// The `Message` a refusal of a request this caller may not make carries.
///
/// One sentence, whichever resource the caller addressed and whatever left that ARN
/// unresolvable: a client branches on the `Code` (`Forbidden`), and the reason a
/// particular request was refused stays in the comment at the refusal rather than being
/// disclosed to a caller that may not read the resource.
pub const FORBIDDEN_REQUEST_MESSAGE: &str = "You are not allowed to make this request.";

/// The `Message` the modelled conflict carries when a create's `ClientRequestToken`
/// already named a resource and the request it is presented with is not the one that
/// token made.
pub const TOKEN_ALREADY_ASSOCIATED_MESSAGE: &str =
    "The provided ClientRequestToken is already associated with an existing resource.";

/// The `Message` the modelled conflict carries when the resource a create names already
/// exists.
pub const RESOURCE_ALREADY_EXISTS_MESSAGE: &str =
    "The resource that you are trying to create already exists.";

/// A persisted `AppInstanceUser`.
#[derive(Debug, Clone)]
pub struct AppInstanceUserRecord {
    pub arn: String,
    pub app_instance_arn: String,
    pub user_id: String,
    pub name: String,
    pub metadata: Option<String>,
    pub created_timestamp: SystemTime,
    /// When this user was last written. A user is stamped when it is CREATED (AWS
    /// reports a last-updated instant on a user that has never been updated, equal to
    /// its created instant), and re-stamped by every operation that changes it — unlike
    /// an [`AppInstanceRecord`], whose stamp appears only on a real update.
    pub last_updated_timestamp: SystemTime,
    pub expiration_settings: Option<ExpirationSettings>,
    /// This user's endpoints, keyed by `EndpointId`.
    pub endpoints: BTreeMap<String, AppInstanceUserEndpointRecord>,
}

/// A persisted `AppInstanceUserEndpoint`: a device registered under an
/// `AppInstanceUser` that receives that user's messages.
#[derive(Debug, Clone)]
pub struct AppInstanceUserEndpointRecord {
    /// The minted `EndpointId`, which is also this record's key in its user's map.
    pub endpoint_id: String,
    pub app_instance_user_arn: String,
    /// `Name` is optional on a register, so an endpoint may carry none.
    pub name: Option<String>,
    /// `AppInstanceUserEndpointType` — one of `APNS`, `APNS_SANDBOX`, `GCM`.
    pub r#type: String,
    /// The ARN of the resource the endpoint belongs to, which for every endpoint type
    /// is the Amazon Pinpoint application that delivers the push notification.
    pub resource_arn: String,
    pub endpoint_attributes: EndpointAttributes,
    pub created_timestamp: SystemTime,
    /// When this endpoint was last written: stamped when it is registered and
    /// re-stamped by every operation that changes it, like an
    /// [`AppInstanceUserRecord`]'s.
    pub last_updated_timestamp: SystemTime,
    /// `AllowMessages` as the caller set it, or `None` while no call has: the model
    /// declares no default for the member, so an endpoint nobody opted in or out
    /// reports none.
    pub allow_messages: Option<String>,
    /// The read-only state the service computed for this endpoint.
    pub endpoint_state: EndpointState,
    /// The `ClientRequestToken` whose register minted this endpoint, so a repeat of
    /// that token replays this endpoint instead of minting a second one. It is released
    /// with the record, so a deregistered endpoint's token is free again.
    pub client_request_token: String,
    /// The fingerprint of the register request that minted this endpoint, so a token
    /// presented again with DIFFERENT parameters is told apart from a retry. Written
    /// once at register and never re-stamped: a later update changes the endpoint, not
    /// the request this token stands for.
    pub request_fingerprint: String,
}

/// A persisted `AppInstanceBot`: an Amazon Lex V2 chat bot created under an
/// `AppInstance`.
#[derive(Debug, Clone)]
pub struct AppInstanceBotRecord {
    pub arn: String,
    pub app_instance_arn: String,
    /// `Name` is optional on `CreateAppInstanceBot`, so a bot may carry none.
    pub name: Option<String>,
    pub metadata: Option<String>,
    /// The bot's data-processing instructions, reported back verbatim by the
    /// operations that describe it.
    pub configuration: Configuration,
    pub created_timestamp: SystemTime,
    /// When this bot was last written. Like an [`AppInstanceUserRecord`] and unlike an
    /// [`AppInstanceRecord`], a bot carries a last-updated instant from the moment it
    /// is created, which every operation that changes it re-stamps.
    pub last_updated_timestamp: SystemTime,
}

/// A persisted `AppInstance` and everything it owns.
#[derive(Debug, Clone)]
pub struct AppInstanceRecord {
    pub arn: String,
    pub name: String,
    pub metadata: Option<String>,
    pub created_timestamp: SystemTime,
    /// When this instance was last CHANGED, or `None` while it still carries exactly
    /// what created it.
    ///
    /// On an `AppInstance` a last-updated instant reports an UPDATE, not a creation: an
    /// instance nobody has updated has none to report, so the optional
    /// `AppInstance$LastUpdatedTimestamp` member ("The time an AppInstance was last
    /// updated") is left out of the response until an operation changes the instance.
    pub last_updated_timestamp: Option<SystemTime>,
    /// This instance's users, keyed by `AppInstanceUserArn`.
    pub users: BTreeMap<String, AppInstanceUserRecord>,
    /// This instance's bots, keyed by `AppInstanceBotArn`.
    pub bots: BTreeMap<String, AppInstanceBotRecord>,
    /// The data-retention settings last written for this instance, or `None` while
    /// nobody has written any.
    ///
    /// Retention settings are a sub-resource of the instance addressed by its own route
    /// rather than a member of the `AppInstance` shape, so they are held beside the
    /// instance here and are reported only by the retention-settings reader.
    pub retention_settings: Option<AppInstanceRetentionSettings>,
    /// The instant from which the data this instance holds outside the retention window
    /// is deleted: the instant the settings now in effect were written.
    ///
    /// `None` while no retention period is in effect — settings that name no number of
    /// days are how a caller turns retention off, and nothing is then due for deletion,
    /// so there is no deletion instant to report.
    pub retention_deletion_initiated_at: Option<SystemTime>,
}

/// What [`ComAmazonawsChimesdkidentityChimeidentityserviceState::insert_user`] did, so
/// the handler maps each outcome to the error the model declares for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertUser {
    /// The user was stored under its parent.
    Stored,
    /// The named `AppInstanceArn` does not exist; nothing was written.
    NoSuchAppInstance,
    /// A user with this ARN already exists; nothing was written. A stored user is
    /// never overwritten, so one live resource can never collect a second record — nor
    /// a second idempotency entry.
    AlreadyExists,
    /// The parent already holds [`MAX_APP_INSTANCE_USERS`] users; nothing was written.
    AtCapacity,
}

/// What [`ComAmazonawsChimesdkidentityChimeidentityserviceState::insert_endpoint`] did,
/// so the handler maps each outcome to the error the model declares for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertEndpoint {
    /// The endpoint was stored under its owning user.
    Stored,
    /// The named `AppInstanceUserArn` does not exist; nothing was written.
    NoSuchAppInstanceUser,
    /// The user already holds [`MAX_APP_INSTANCE_USER_ENDPOINTS`] endpoints; nothing
    /// was written.
    AtCapacity,
}

/// What [`ComAmazonawsChimesdkidentityChimeidentityserviceState::insert_bot`] did, so
/// the handler maps each outcome to the error the model declares for it. The variants
/// mirror [`InsertUser`]'s, minus the already-exists case: a bot ARN carries a freshly
/// minted id rather than one the caller chose, so it cannot collide with a stored bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertBot {
    /// The bot was stored under its parent.
    Stored,
    /// The named `AppInstanceArn` does not exist; nothing was written.
    NoSuchAppInstance,
    /// The parent already holds [`MAX_APP_INSTANCE_BOTS`] bots; nothing was written.
    AtCapacity,
}

/// What a create's `ClientRequestToken` minted, and the request it minted it for.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The ARN that create minted.
    pub arn: String,
    /// The request the token carried, as [`request_fingerprint`] renders it.
    pub request_fingerprint: String,
}

/// What a create found for the `ClientRequestToken` it carries, so the handler answers
/// each case with what the model declares for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// This token has minted nothing that is still live, so the create mints a resource.
    Fresh,
    /// The same request again: answer with the ARN it already minted and create nothing.
    Replays(String),
    /// A token already associated with a live resource, presented with a DIFFERENT
    /// request: not a retry of anything.
    Mismatch,
}

/// Why an ARN this service holds no live resource for is unresolvable, which is what
/// decides WHICH modelled refusal a request addressed to it earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedArn {
    /// The ARN is a deleted `AppInstance`'s own ARN, or the ARN of a resource that hung
    /// off one: the instance that would have served the request is gone, so the request
    /// is one this caller may no longer make.
    DeletedAppInstance,
    /// No resource of this service ever carried this ARN.
    NeverExisted,
}

/// Per-service state for com-amazonaws-chimesdkidentity-chimeidentityservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkidentityChimeidentityserviceState {
    /// Every app instance, keyed by `AppInstanceArn`.
    pub app_instances: BTreeMap<String, AppInstanceRecord>,
    /// `AppInstanceUserArn` -> owning `AppInstanceArn`.
    pub users_by_arn: BTreeMap<String, String>,
    /// `AppInstanceBotArn` -> owning `AppInstanceArn`.
    pub bots_by_arn: BTreeMap<String, String>,
    /// `<operation>:<ClientRequestToken>` -> what that token minted, and for which
    /// request.
    pub idempotency: BTreeMap<String, IdempotentCreate>,
    /// The ARN of every `AppInstance` this store has deleted -> the sequence number of
    /// that deletion, so the oldest can be dropped once the set holds
    /// [`MAX_DELETED_APP_INSTANCES`].
    ///
    /// A deleted `AppInstance` is what told this emulator it could serve a request
    /// against its own ARN and against the ARNs of the resources that hung off it, so
    /// once it is gone such a request is refused differently from one naming an ARN that
    /// never existed — see [`unresolved_arn`](
    /// ComAmazonawsChimesdkidentityChimeidentityserviceState::unresolved_arn).
    pub deleted_app_instances: BTreeMap<String, u64>,
    /// The sequence number the next deletion is recorded under, so the set's eviction
    /// order is the deletion order rather than the ARN order.
    deletions: u64,
    /// The secret every `NextToken` this store's listings hand out is authenticated
    /// under — one key per store, drawn from the system's randomness when the store is
    /// created and never reported (see [`encode_page_token`]).
    page_token_key: PageTokenKey,
}

impl ComAmazonawsChimesdkidentityChimeidentityserviceState {
    /// The key this store's page tokens are minted and verified under.
    ///
    /// `Copy`, so a listing takes the key off the store and goes on to borrow the
    /// records it is about to page in the same call.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }

    /// The user with this ARN, resolved through the `users_by_arn` index in one hop.
    pub fn user(&self, user_arn: &str) -> Option<&AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.get(user_arn)?;
        self.app_instances.get(instance_arn)?.users.get(user_arn)
    }

    /// The user with this ARN, for a caller that is about to CHANGE it; resolved
    /// through the same one-hop index as [`user`](Self::user).
    pub fn user_mut(&mut self, user_arn: &str) -> Option<&mut AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.get(user_arn)?.clone();
        self.app_instances
            .get_mut(&instance_arn)?
            .users
            .get_mut(user_arn)
    }

    /// The endpoint `endpoint_id` names under the user `user_arn` names.
    pub fn endpoint(
        &self,
        user_arn: &str,
        endpoint_id: &str,
    ) -> Option<&AppInstanceUserEndpointRecord> {
        self.user(user_arn)?.endpoints.get(endpoint_id)
    }

    /// The endpoint a previous register under `user_arn` already minted for `token`.
    ///
    /// The token lives on the record it minted, so a token whose endpoint has since
    /// been deregistered resolves to nothing here and is free to mint another.
    pub fn endpoint_registered_with(
        &self,
        user_arn: &str,
        token: &str,
    ) -> Option<&AppInstanceUserEndpointRecord> {
        self.user(user_arn)?
            .endpoints
            .values()
            .find(|endpoint| endpoint.client_request_token == token)
    }

    /// The endpoint `endpoint_id` names under the user `user_arn` names, for a caller
    /// that is about to CHANGE it.
    pub fn endpoint_mut(
        &mut self,
        user_arn: &str,
        endpoint_id: &str,
    ) -> Option<&mut AppInstanceUserEndpointRecord> {
        self.user_mut(user_arn)?.endpoints.get_mut(endpoint_id)
    }

    /// Remove an endpoint from the user it belongs to.
    ///
    /// The register token that minted it lives on the record, so it goes with the
    /// endpoint and is free to mint another.
    pub fn remove_endpoint(
        &mut self,
        user_arn: &str,
        endpoint_id: &str,
    ) -> Option<AppInstanceUserEndpointRecord> {
        self.user_mut(user_arn)?.endpoints.remove(endpoint_id)
    }

    /// Store an endpoint under the user it belongs to.
    ///
    /// The user lookup here is the ONLY one a caller needs: an absent user is reported
    /// as [`InsertEndpoint::NoSuchAppInstanceUser`] and a full one as
    /// [`InsertEndpoint::AtCapacity`], and in neither case is anything written.
    pub fn insert_endpoint(&mut self, record: AppInstanceUserEndpointRecord) -> InsertEndpoint {
        let user_arn = record.app_instance_user_arn.clone();
        let Some(user) = self.user_mut(&user_arn) else {
            return InsertEndpoint::NoSuchAppInstanceUser;
        };
        if user.endpoints.len() >= MAX_APP_INSTANCE_USER_ENDPOINTS {
            return InsertEndpoint::AtCapacity;
        }
        user.endpoints.insert(record.endpoint_id.clone(), record);
        InsertEndpoint::Stored
    }

    /// The bot with this ARN, resolved through the `bots_by_arn` index in one hop.
    pub fn bot(&self, bot_arn: &str) -> Option<&AppInstanceBotRecord> {
        let instance_arn = self.bots_by_arn.get(bot_arn)?;
        self.app_instances.get(instance_arn)?.bots.get(bot_arn)
    }

    /// The bot with this ARN, for a caller that is about to CHANGE it; resolved through
    /// the same one-hop index as [`bot`](Self::bot).
    pub fn bot_mut(&mut self, bot_arn: &str) -> Option<&mut AppInstanceBotRecord> {
        let instance_arn = self.bots_by_arn.get(bot_arn)?.clone();
        self.app_instances
            .get_mut(&instance_arn)?
            .bots
            .get_mut(bot_arn)
    }

    /// The name the identity `identity_arn` names carries, for an operation that reports
    /// an `Identity` (an ARN and the name of whatever it names).
    ///
    /// An app instance's identity is either an [`AppInstanceUser`](AppInstanceUserRecord),
    /// which always carries a name, or an [`AppInstanceBot`](AppInstanceBotRecord), whose
    /// name is optional — so BOTH are resolved here and an identity this service does not
    /// hold (or a bot created without one) simply has no name to report.
    pub fn identity_name(&self, identity_arn: &str) -> Option<String> {
        if let Some(user) = self.user(identity_arn) {
            return Some(user.name.clone());
        }
        self.bot(identity_arn)?.name.clone()
    }

    /// Store a bot under its parent instance and index it.
    ///
    /// As with [`insert_user`](Self::insert_user) the parent lookup here is the ONLY one
    /// a caller needs, and nothing is written on either refusal.
    pub fn insert_bot(&mut self, record: AppInstanceBotRecord) -> InsertBot {
        let Some(instance) = self.app_instances.get_mut(&record.app_instance_arn) else {
            return InsertBot::NoSuchAppInstance;
        };
        if instance.bots.len() >= MAX_APP_INSTANCE_BOTS {
            return InsertBot::AtCapacity;
        }
        self.bots_by_arn
            .insert(record.arn.clone(), record.app_instance_arn.clone());
        instance.bots.insert(record.arn.clone(), record);
        InsertBot::Stored
    }

    /// Store a user under its parent instance and index it.
    ///
    /// The parent lookup here is the ONLY one a caller needs: an absent parent is
    /// reported as [`InsertUser::NoSuchAppInstance`] and a full one as
    /// [`InsertUser::AtCapacity`], and in neither case is anything written.
    pub fn insert_user(&mut self, record: AppInstanceUserRecord) -> InsertUser {
        let Some(instance) = self.app_instances.get_mut(&record.app_instance_arn) else {
            return InsertUser::NoSuchAppInstance;
        };
        if instance.users.contains_key(&record.arn) {
            return InsertUser::AlreadyExists;
        }
        if instance.users.len() >= MAX_APP_INSTANCE_USERS {
            return InsertUser::AtCapacity;
        }
        self.users_by_arn
            .insert(record.arn.clone(), record.app_instance_arn.clone());
        instance.users.insert(record.arn.clone(), record);
        InsertUser::Stored
    }

    /// Remove a bot from its parent instance and from the index.
    pub fn remove_bot(&mut self, bot_arn: &str) -> Option<AppInstanceBotRecord> {
        let instance_arn = self.bots_by_arn.remove(bot_arn)?;
        let removed = self
            .app_instances
            .get_mut(&instance_arn)
            .and_then(|instance| instance.bots.remove(bot_arn))?;
        self.forget_idempotency_for(&[bot_arn]);
        Some(removed)
    }

    /// Remove a user from its parent instance and from the index.
    pub fn remove_user(&mut self, user_arn: &str) -> Option<AppInstanceUserRecord> {
        let instance_arn = self.users_by_arn.remove(user_arn)?;
        let removed = self
            .app_instances
            .get_mut(&instance_arn)
            .and_then(|instance| instance.users.remove(user_arn))?;
        self.forget_idempotency_for(&[user_arn]);
        Some(removed)
    }

    /// Remove an app instance and cascade to everything it owns: its users and bots go
    /// with it, so a later `DescribeAppInstanceUser` or `ListAppInstanceBots` on one of
    /// them resolves to nothing. The idempotency tokens that minted the instance and
    /// its children are released too.
    pub fn remove_app_instance(&mut self, arn: &str) -> Option<AppInstanceRecord> {
        let instance = self.app_instances.remove(arn)?;
        self.record_app_instance_deletion(arn);
        self.users_by_arn
            .retain(|_, owning_arn| owning_arn.as_str() != arn);
        self.bots_by_arn
            .retain(|_, owning_arn| owning_arn.as_str() != arn);
        let mut gone: BTreeSet<&str> = instance.users.keys().map(String::as_str).collect();
        gone.extend(instance.bots.keys().map(String::as_str));
        gone.insert(arn);
        self.idempotency
            .retain(|_, minted| !gone.contains(minted.arn.as_str()));
        Some(instance)
    }

    /// Drop the idempotency entries that minted these now-deleted ARNs.
    ///
    /// A token is released with the resource it minted, so the map holds at most one
    /// entry per live resource rather than one per create ever made.
    fn forget_idempotency_for(&mut self, arns: &[&str]) {
        self.idempotency
            .retain(|_, minted| !arns.contains(&minted.arn.as_str()));
    }

    /// Remember that the `AppInstance` `arn` named has been deleted.
    ///
    /// Bounded at [`MAX_DELETED_APP_INSTANCES`] by forgetting the oldest deletion, so
    /// the set never outgrows the instances the store itself can hold.
    fn record_app_instance_deletion(&mut self, arn: &str) {
        self.deletions = self.deletions.wrapping_add(1);
        self.deleted_app_instances
            .insert(arn.to_string(), self.deletions);
        while self.deleted_app_instances.len() > MAX_DELETED_APP_INSTANCES {
            let Some(oldest) = self
                .deleted_app_instances
                .iter()
                .min_by_key(|(_, recorded)| **recorded)
                .map(|(deleted_arn, _)| deleted_arn.clone())
            else {
                break;
            };
            self.deleted_app_instances.remove(&oldest);
        }
    }

    /// Why `arn` resolves to no live resource of this service: because the `AppInstance`
    /// it belongs to has been deleted, or because nothing here ever named it.
    ///
    /// Every resource of this service hangs off exactly one `AppInstance`, and a user,
    /// bot or endpoint ARN carries its instance's ARN as its prefix, so one lookup over
    /// the deleted instances answers for the instance itself and for everything the
    /// delete cascaded away with it.
    pub fn unresolved_arn(&self, arn: &str) -> UnresolvedArn {
        if self
            .deleted_app_instances
            .keys()
            .any(|deleted| arn_is_at_or_under(arn, deleted))
        {
            return UnresolvedArn::DeletedAppInstance;
        }
        UnresolvedArn::NeverExisted
    }

    /// True when `arn` still names a live resource of this service.
    pub fn resource_exists(&self, arn: &str) -> bool {
        self.app_instances.contains_key(arn)
            || self.users_by_arn.contains_key(arn)
            || self.bots_by_arn.contains_key(arn)
    }

    /// Whether a tagging call may address `arn`.
    ///
    /// The family's one rule for that question lives in
    /// [`::service_common::chime_tags::addresses_taggable_resource`]; this store answers
    /// the half of it only this service can — whether an ARN that names a resource THIS
    /// service mints ([`names_identity_resource`]) is one it still holds.
    pub fn taggable(&self, arn: &str) -> bool {
        ::service_common::chime_tags::addresses_taggable_resource(
            arn,
            names_identity_resource(arn).then(|| self.resource_exists(arn)),
        )
    }

    /// What the `ClientRequestToken` `token` means for this `operation` presented with
    /// the request `request_fingerprint` stands for.
    ///
    /// A token whose resource has since been deleted is forgotten here, so the token
    /// can mint a new resource rather than replaying an ARN that resolves to nothing.
    ///
    /// The comparison is against the request the token MADE ([`IdempotentCreate`]'s
    /// fingerprint), not against what the resource holds now: an update or a tagging
    /// call that changed the resource afterwards must not turn an honest retry of the
    /// original create into a conflict.
    pub fn token_replay(
        &mut self,
        operation: &str,
        token: &str,
        request_fingerprint: &str,
    ) -> TokenReplay {
        let key = idempotency_key(operation, token);
        let Some(recorded) = self.idempotency.get(&key).cloned() else {
            return TokenReplay::Fresh;
        };
        if !self.resource_exists(&recorded.arn) {
            self.idempotency.remove(&key);
            return TokenReplay::Fresh;
        }
        if recorded.request_fingerprint == request_fingerprint {
            return TokenReplay::Replays(recorded.arn);
        }
        TokenReplay::Mismatch
    }

    /// Record that `operation` minted `arn` for `token`, for the request
    /// `request_fingerprint` stands for.
    ///
    /// Answers `false`, recording nothing, once the map holds
    /// [`MAX_IDEMPOTENCY_TOKENS`] entries: the resource caps bind first, so this is
    /// unreachable through the operations as written, and it is here so the map stays
    /// bounded whatever a caller does. The resource itself is created either way; only
    /// the token's replay is given up.
    pub fn record_idempotency(
        &mut self,
        operation: &str,
        token: &str,
        arn: &str,
        request_fingerprint: &str,
    ) -> bool {
        let key = idempotency_key(operation, token);
        if self.idempotency.len() >= MAX_IDEMPOTENCY_TOKENS && !self.idempotency.contains_key(&key) {
            return false;
        }
        self.idempotency.insert(
            key,
            IdempotentCreate {
                arn: arn.to_string(),
                request_fingerprint: request_fingerprint.to_string(),
            },
        );
        true
    }
}

/// The key an idempotency token is stored under. Prefixed with the operation name so
/// `CreateAppInstance` and `CreateAppInstanceUser` can be called with the same token
/// (they are different requests) without either replaying the other's ARN.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkidentityChimeidentityserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkidentityChimeidentityserviceState>)
}

/// The SHARED app-instance ADMIN directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// An `AppInstanceAdmin` is promoted here and the role it grants is read by Chime SDK
/// Messaging, which reserves an app instance's private-channel listing and its message
/// deletion for an administrator. The engine keys state by TYPE, so the promotions live
/// in ONE directory under one shared type
/// ([`::service_common::chime_app_instance_admins::AppInstanceAdminDirectory`]) rather
/// than in a copy inside [`ComAmazonawsChimesdkidentityChimeidentityserviceState`],
/// which the messaging service could not see.
///
/// A promotion names one identity and one app instance, both resources of THIS service, so
/// every operation that promotes, demotes or DELETES either side keeps the directory in
/// step with this store: a promotion of a deleted identity, or of a deleted app instance,
/// would report an administrator that no longer exists.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn app_instance_admins<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
            >,
        )
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the resource
/// records of this state struct, because the family's generic tagging routes are served
/// by one member for every member's resource: a copy inside
/// [`ComAmazonawsChimesdkidentityChimeidentityserviceState`] would be invisible to a
/// sibling serving a tagging call at its own endpoint.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}

/// The SHARED Chime IDENTITY NAME directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// The name an identity this service owns is registered under is PUBLISHED here, keyed by
/// that identity's ARN
/// ([`::service_common::chime_identity_names::ChimeIdentityNameDirectory`]), because the
/// sibling that has to REPORT it — Chime SDK Messaging, which puts every channel
/// participant on the wire as an ARN and a name — cannot read this service's own store:
/// the engine keys state by TYPE. This service's own record stays the authority, so every
/// operation that registers, renames or deletes an identity keeps the directory in step
/// with it; an entry that was never written is simply a name no reader reports.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn identity_names<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::chime_identity_names::ChimeIdentityNameDirectory>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::chime_identity_names::ChimeIdentityNameDirectory,
            >,
        )
}

/// Whether `arn` names a resource THIS service mints, so that this service's store is
/// what says whether it exists.
///
/// The ARNs this service mints are the app instance itself
/// (`arn:…:chime:…:app-instance/<id>`) and its identity children, which hang off it:
/// `…/user/<id>` ([`app_instance_user_arn`]) and `…/bot/<id>`. A sibling's resource hangs
/// off the same app instance under a segment of its own (Chime SDK Messaging's
/// `…/channel/<id>`), and every other family member's resources carry a different resource
/// type altogether, so neither reads as this service's.
pub fn names_identity_resource(arn: &str) -> bool {
    let Some(resource) = arn.splitn(6, ':').nth(5) else {
        return false;
    };
    let tokens: Vec<&str> = resource.split('/').collect();
    match tokens.as_slice() {
        ["app-instance", id] => !id.is_empty(),
        ["app-instance", id, child, child_id] => {
            !id.is_empty() && !child_id.is_empty() && matches!(*child, "user" | "bot")
        }
        _ => false,
    }
}

/// Whether `arn` names an IDENTITY of the `AppInstance` `app_instance_arn` names — an
/// `AppInstanceUser` (`…/user/<id>`) or an `AppInstanceBot` (`…/bot/<id>`) of that one
/// instance.
///
/// An identity of an app instance is a CHILD of it that this service mints, so the answer
/// is both halves: the ARN hangs under this instance, and it names one of this service's
/// identity resources ([`names_identity_resource`]). A child under any other segment hangs
/// off the same instance but belongs to a sibling service (Chime SDK Messaging's
/// `…/channel/<id>`) and names no identity; so does the instance's own ARN, and so does a
/// deeper path under an identity segment.
pub fn names_identity_of(app_instance_arn: &str, arn: &str) -> bool {
    arn != app_instance_arn
        && arn_is_at_or_under(arn, app_instance_arn)
        && names_identity_resource(arn)
}

/// Whether `arn` is `prefix` itself or names a resource UNDER it.
///
/// A child ARN of this family is its parent's ARN plus a `/`-separated segment pair
/// (`…/user/<id>`, `…/bot/<id>`), so a prefix match is only a containment when the next
/// character is the separator: without that check an instance whose uuid merely starts
/// with another's would read as its child.
fn arn_is_at_or_under(arn: &str, prefix: &str) -> bool {
    arn == prefix
        || (arn.len() > prefix.len()
            && arn.starts_with(prefix)
            && arn.as_bytes()[prefix.len()] == b'/')
}

/// The region to mint an ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`]. Read from the SigV4 credential scope, the same signal the
/// router routes on, so an ARN is in the region the client addressed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh `AppInstanceArn`: `arn:aws:chime:<region>:<account>:app-instance/<uuid>`.
///
/// The account segment comes from `engine::emulator_arn`, which is the single source
/// of the emulator's account id.
pub fn new_app_instance_arn(region: &str) -> String {
    let resource = format!("app-instance/{}", new_uuid_v4());
    ::engine::emulator_arn(ARN_SERVICE, region, &resource)
}

/// A fresh version-4 UUID in the lowercase hyphenated form an AWS ARN carries.
///
/// The 16 bytes are random: `sha2` over the std hasher's per-instance random keys
/// (`RandomState` is seeded from the OS) plus the monotonic clock, with the version
/// and variant nibbles then set as RFC 4122 section 4.4 specifies. Nothing here is
/// derived from the request, so two creates never collide and a minted id is not
/// guessable from one a caller has seen. Only the two nibbles and the hyphenation are
/// written by hand; the randomness and the hex-coding come from maintained crates this
/// workspace already carries.
fn new_uuid_v4() -> String {
    use sha2::Digest as _;

    let entropy = ::std::collections::hash_map::RandomState::new();
    let mut hasher = sha2::Sha256::new();
    hasher.update(entropy.hash_one(0u64).to_be_bytes());
    hasher.update(entropy.hash_one(u64::MAX).to_be_bytes());
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
            .to_be_bytes(),
    );

    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 4 in the high nibble of octet 6, variant 0b10 in the two high bits of
    // octet 8 — what makes the value a well-formed v4 UUID rather than 16 loose bytes.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An `AppInstanceUserArn` is its instance's ARN with `/user/<AppInstanceUserId>`
/// appended, so a user ARN carries its parent and the id the caller chose.
pub fn app_instance_user_arn(app_instance_arn: &str, app_instance_user_id: &str) -> String {
    format!("{app_instance_arn}/user/{app_instance_user_id}")
}

/// A fresh `AppInstanceBotArn`: its instance's ARN with `/bot/<uuid>` appended.
///
/// `CreateAppInstanceBot` takes no caller-chosen id, so the bot segment is a freshly
/// minted uuid (the same source as an `AppInstanceArn`'s) rather than anything derived
/// from the request.
pub fn new_app_instance_bot_arn(app_instance_arn: &str) -> String {
    format!("{app_instance_arn}/bot/{}", new_uuid_v4())
}

/// A fresh `EndpointId` for an `AppInstanceUserEndpoint`.
///
/// An endpoint carries no ARN and no caller-chosen id, so its id is a freshly minted
/// uuid — the same source as an `AppInstanceArn`'s — which fits the modelled
/// `String64`.
pub fn new_endpoint_id() -> String {
    new_uuid_v4()
}

/// What the `ResourceArn` of an `AppInstanceUserEndpoint` has to name: an Amazon
/// Pinpoint application, the resource that delivers the push notification for every
/// endpoint type (`RegisterAppInstanceUserEndpoint$Type` documents `ResourceArn` as the
/// Pinpoint app ARN for APNS, APNS_SANDBOX and GCM alike). Pinpoint publishes
/// `mobiletargeting` as its ARN namespace and writes an application as
/// `apps/<application-id>`.
const PINPOINT_APPLICATION: ::service_common::arn::Requirement =
    ::service_common::arn::Requirement {
        vendor: "mobiletargeting",
        resource_type: Some("apps"),
        label: "an Amazon Pinpoint application",
    };

/// The length of an Amazon Pinpoint application id: Pinpoint mints a project id as 32
/// hexadecimal characters, which is the `apps/<application-id>` segment of its ARN.
const PINPOINT_APPLICATION_ID_LENGTH: usize = 32;

/// The `EndpointState` the `ResourceArn` of a register implies.
///
/// `EndpointState` is read-only: the service computes it from the Pinpoint application
/// the `ResourceArn` names, which is what delivers the push notification for every
/// endpoint type. An ARN that names an application Pinpoint could have minted carries
/// an endpoint that can deliver, so it is `ACTIVE` and reports no reason; any other
/// `ResourceArn` is an invalid Pinpoint ARN, which leaves the endpoint registered and
/// `INACTIVE` carrying the reason the model declares for exactly that input.
pub fn endpoint_state_of(resource_arn: &str) -> EndpointState {
    if names_pinpoint_application(resource_arn) {
        EndpointState {
            status: ENDPOINT_ACTIVE.to_string(),
            status_reason: None,
        }
    } else {
        EndpointState {
            status: ENDPOINT_INACTIVE.to_string(),
            status_reason: Some(INVALID_PINPOINT_ARN.to_string()),
        }
    }
}

/// `EndpointStatus` for an endpoint that can receive messages.
const ENDPOINT_ACTIVE: &str = "ACTIVE";

/// `EndpointStatus` for an endpoint that cannot receive messages.
const ENDPOINT_INACTIVE: &str = "INACTIVE";

/// The `EndpointStatusReason` the model declares for an endpoint whose `ResourceArn` is
/// not a Pinpoint ARN that names an application.
const INVALID_PINPOINT_ARN: &str = "INVALID_PINPOINT_ARN";

/// Whether `resource_arn` names an Amazon Pinpoint application, read against the one ARN
/// parse in [`::service_common::arn`].
///
/// A value that is not an ARN, one of another vendor, one of another Pinpoint resource
/// type, one in the `apps` space whose id is not an id Pinpoint mints
/// ([`names_pinpoint_application_id`]) and `apps` with no id at all name no Pinpoint
/// application.
fn names_pinpoint_application(resource_arn: &str) -> bool {
    PINPOINT_APPLICATION.admits(resource_arn)
        && ::service_common::arn::resource_id_of_type(resource_arn, "apps")
            .is_some_and(names_pinpoint_application_id)
}

/// Whether `application_id` is an id Pinpoint mints for an application: exactly
/// [`PINPOINT_APPLICATION_ID_LENGTH`] hexadecimal characters.
fn names_pinpoint_application_id(application_id: &str) -> bool {
    application_id.len() == PINPOINT_APPLICATION_ID_LENGTH
        && application_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The fingerprint of a create request: a sha256 over LENGTH-PREFIXED fields, so no
/// value containing a separator can impersonate a different split of the same bytes, and
/// an absent optional member is distinct from an empty one.
///
/// `fields` is every member of the request the create persists, in one fixed order — a
/// `ClientRequestToken` stands for ONE request, so a token presented again is compared
/// against the fingerprint of the request it first carried.
pub fn request_fingerprint(fields: &[Option<&str>]) -> String {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    for field in fields {
        match field {
            Some(value) => {
                hasher.update(b"s");
                hasher.update(value.len().to_be_bytes());
                hasher.update(value.as_bytes());
            }
            None => hasher.update(b"n"),
        }
    }
    ::hex::encode(hasher.finalize())
}

/// The fingerprint of the register request an endpoint was minted by: every member the
/// register call persists, through [`request_fingerprint`].
pub fn endpoint_request_fingerprint(
    endpoint_type: &str,
    resource_arn: &str,
    attributes: &EndpointAttributes,
    name: Option<&str>,
    allow_messages: Option<&str>,
) -> String {
    request_fingerprint(&[
        Some(endpoint_type),
        Some(resource_arn),
        Some(attributes.device_token.as_str()),
        attributes.voip_device_token.as_deref(),
        name,
        allow_messages,
    ])
}

/// The number of days a retention-settings structure puts in effect, or `None` when it
/// names none.
///
/// Every level of the structure is optional (`AppInstanceRetentionSettings$Channel\
/// RetentionSettings`, `ChannelRetentionSettings$RetentionDays`), and omitting the
/// number of days is how a caller turns retention off, so "no days anywhere in the
/// structure" and "no retention in effect" are the same answer.
pub fn retention_days_in_effect(settings: &AppInstanceRetentionSettings) -> Option<i32> {
    settings
        .channel_retention_settings
        .as_ref()
        .and_then(|channel| channel.retention_days)
}

/// A stored instant as the wire timestamp the model declares (`Timestamp`).
pub fn wire_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, as epoch MILLISECONDS — the resolution the shared app-instance admin directory
/// records a promotion at, so both services that read it agree on the instant without
/// either having to agree on a Rust clock type.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// An epoch-milliseconds instant as the wire timestamp the model declares.
pub fn wire_timestamp_of_millis(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// One page of a listing, plus the token that resumes after it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Encode a state key as an opaque `NextToken`, scoped to the listing that issued it
/// and authenticated under `token_key`.
///
/// The cursor travels inside the token (there is no page kept on the side to look up),
/// so what makes it a token this listing ISSUED is the authenticator
/// [`::service_common::page_tokens::PageTokenKey::mint`] puts over it: the store's key
/// is drawn from the system's randomness and never leaves the process, so no caller can
/// produce the authenticator for a cursor of its own and no listing can be resumed at a
/// page position it never handed out. Beside the cursor the authenticated bytes carry a
/// version marker and the listing's `scope`, so a token one listing minted does not
/// resume another one over the same store's key.
///
/// Hex-coded rather than base64: the token travels back in a query string
/// (`?next-token=...`), and hex needs no escaping there, while base64's `+` would
/// arrive as a space.
pub fn encode_page_token(token_key: &PageTokenKey, scope: &str, key: &str) -> String {
    // `\n` separates the fields: a scope is an operation name or an ARN and a key is
    // an ARN, so neither can contain one.
    ::hex::encode(token_key.mint(&format!("{TOKEN_VERSION}\n{scope}\n{key}")))
}

/// Decode a `NextToken` issued for `scope` back into the state key it resumes after;
/// `None` when the token is not one this listing issued — a value that is not hex, one
/// whose authenticator this store's key does not verify (a value a caller assembled,
/// one another store minted, one changed on the way back), and one this store minted
/// for a DIFFERENT listing.
pub fn decode_page_token(token_key: &PageTokenKey, scope: &str, token: &str) -> Option<String> {
    let raw = ::hex::decode(token).ok()?;
    let decoded = token_key.cursor_of(&raw)?;
    let mut fields = decoded.splitn(3, '\n');
    let (Some(TOKEN_VERSION), Some(token_scope), Some(key)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    (token_scope == scope).then(|| key.to_string())
}

/// One page of a key-ordered map: the entries after `after`, at most `max_results`
/// (bounded by [`MAX_PAGE_SIZE`]) of them, each projected to a response shape.
///
/// The page starts AT the cursor — `BTreeMap::range` seeks to it in O(log n) rather
/// than re-scanning the consumed prefix — so walking a whole collection costs one
/// pass in total, not one pass per page. The returned `next_token` is `Some` only
/// when entries remain, so the final page omits it, which is how a paginating client
/// knows to stop. `scope` identifies the listing in the token it issues and
/// `token_key` is the store's key that authenticates it; see [`encode_page_token`].
pub fn paginate<'a, V, T>(
    entries: &'a BTreeMap<String, V>,
    max_results: Option<i32>,
    after: Option<&str>,
    token_key: &PageTokenKey,
    scope: &str,
    project: impl Fn(&'a V) -> T,
) -> Page<T> {
    // Defence in depth, not validation: the boundary validator has already refused a
    // `MaxResults` outside the model's `range(min: 1, max: 50)` by the time a handler
    // runs, so this clamp is a no-op on every request that reaches here. It is kept so
    // that no page can ever be materialised larger than the modelled maximum.
    debug_assert!(
        max_results.is_none_or(|requested| (1..=MAX_PAGE_SIZE).contains(&requested)),
        "MaxResults {max_results:?} should have been refused at the boundary"
    );
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let mut items = Vec::new();
    let mut last_key: Option<&str> = None;
    let mut truncated = false;

    let start = match after {
        Some(cursor) => Bound::Excluded(cursor),
        None => Bound::Unbounded,
    };
    for (key, value) in entries.range::<str, _>((start, Bound::Unbounded)) {
        if items.len() == limit {
            truncated = true;
            break;
        }
        items.push(project(value));
        last_key = Some(key.as_str());
    }

    Page {
        items,
        next_token: truncated
            .then(|| last_key.map(|key| encode_page_token(token_key, scope, key)))
            .flatten(),
    }
}
