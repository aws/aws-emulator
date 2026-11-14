// SPDX-License-Identifier: CC0-1.0

//! The CONNECTION FUNCTION slice of this service's state.
//!
//! A connection function is its own resource kind — addressed by a minted `Id` rather than
//! by its name, and projected as a `ConnectionFunctionSummary` rather than a
//! `FunctionSummary` — so everything about how one is persisted and projected lives here
//! rather than in the parent module: the record each stage carries, the ARN and the path a
//! connection function is addressed by, the lookups every read goes through, and the
//! modelled errors the connection function operations answer with. The parent
//! [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map
//! (`connection_functions`) and this module holds everything that reads or writes it.
//!
//! The STAGE vocabulary is shared with the CloudFront function surface rather than
//! duplicated: `ConnectionFunctionSummary$Stage` targets the same modelled `FunctionStage`
//! enum (`DEVELOPMENT` | `LIVE`) a `FunctionMetadata$Stage` does, so
//! [`super::FUNCTION_STAGE_DEVELOPMENT`], [`super::FUNCTION_STAGE_LIVE`],
//! [`super::DEFAULT_FUNCTION_STAGE`] and [`super::FUNCTION_STATUS_UNPUBLISHED`] are the
//! values named here too.
//!
//! One thing here IS owed by the CLOCK: a connection function this service accepts exists
//! in `DEVELOPMENT` immediately, but a PUBLISH of it is asynchronous, so the stage a
//! publish writes reports [`CONNECTION_FUNCTION_STATUS_PUBLISHING`] until the parent's
//! `settle` materialises the status that publish settles to
//! ([`StoredConnectionFunctionStage::published_due`]).

use std::time::SystemTime;

use crate::ops::types::{
    ConnectionFunctionSummary, DistributionConfig, EntityAlreadyExists, EntityNotFound,
    FunctionConfig,
};

use super::{
    ComAmazonawsCloudfrontCloudfront20200531State, TagSet, MAX_FUNCTIONS,
    MAX_FUNCTION_CODE_BYTES, PROPAGATION_DELAY,
};

/// `ConnectionFunctionSummary$Status` for the stage a publish has just written.
///
/// Publishing a connection function is ASYNCHRONOUS: the call that starts it reports the
/// status the copy is IN rather than the one it settles to, so a caller that waits for the
/// publish to finish has a transient status to wait on. What it settles to is read off the
/// stored distributions ([`super::FUNCTION_STATUS_ASSOCIATED`] /
/// [`super::FUNCTION_STATUS_UNASSOCIATED`]) once the clock owes it
/// ([`StoredConnectionFunctionStage::settle`]). The model targets `Status` at plain
/// `string` and states no value list, so the word is spelled out here once.
pub const CONNECTION_FUNCTION_STATUS_PUBLISHING: &str = "PUBLISHING";

/// The prefix every connection function id this service mints carries.
///
/// `DescribeConnectionFunction`'s `Identifier` is "The connection function's identifier",
/// which the three namespaces a connection function answers to share (its id, its name and
/// its ARN), so a minted id is given a prefix of its own for the same reason
/// [`super::DISTRIBUTION_TENANT_ID_PREFIX`] and [`super::CONNECTION_GROUP_ID_PREFIX`] have
/// one: the model states no id pattern, so the shape is this emulator's choice, and a
/// resolution order ([`ComAmazonawsCloudfrontCloudfront20200531State::connection_function`])
/// decides the rest.
pub const CONNECTION_FUNCTION_ID_PREFIX: &str = "cf_";

/// The most connection functions this emulator holds, refused at with the modelled
/// `EntityLimitExceeded` ("The entity limit has been exceeded").
///
/// CloudFront publishes a quota of 100 "Functions per Amazon Web Services account"
/// ([`MAX_FUNCTIONS`]) and states no separate number for a connection function, so the
/// bound is that same count rather than a second one invented here. It is an entry count
/// checked before anything is minted: a connection function record is what a later read
/// answers from, so it cannot be evicted.
pub const MAX_CONNECTION_FUNCTIONS: usize = MAX_FUNCTIONS;

/// The largest connection function code this emulator accepts, refused at with the
/// modelled `EntitySizeLimitExceeded` ("The entity size limit was exceeded").
///
/// The code travels as the same `FunctionBlob` (up to 40960 bytes on the wire) a CloudFront
/// function's does, and CloudFront's published quota for the "Maximum size of the function
/// code and included libraries" is 10 KB ([`MAX_FUNCTION_CODE_BYTES`]) — which is exactly
/// what makes this refusal reachable: the boundary validator rejects anything past the
/// modelled bound, and this quota rejects code that is within the model but past what
/// CloudFront runs.
pub const MAX_CONNECTION_FUNCTION_CODE_BYTES: usize = MAX_FUNCTION_CODE_BYTES;

/// One stage of one connection function, as this emulator persists it.
///
/// A connection function's two stages — `DEVELOPMENT` and `LIVE` — each carry their OWN
/// code, configuration, status and version token, as a CloudFront function's do: a publish
/// COPIES one stage into the other, so an edit to one stage is not an edit to the other and
/// each stage answers reads with what was last copied or written into it.
#[derive(Debug, Clone)]
pub struct StoredConnectionFunctionStage {
    /// Which stage this record is: [`super::FUNCTION_STAGE_DEVELOPMENT`] or
    /// [`super::FUNCTION_STAGE_LIVE`], reported as `ConnectionFunctionSummary$Stage`.
    pub stage: String,
    /// The configuration as submitted ("Contains configuration information about a
    /// CloudFront function"). Stored whole, so a read reports what was stored rather than
    /// re-deriving anything from a request.
    pub config: FunctionConfig,
    /// The connection function code as submitted, which `GetConnectionFunction` answers
    /// with byte for byte.
    pub code: Vec<u8>,
    /// `ConnectionFunctionSummary$Status` — "The connection function status."
    pub status: String,
    /// The version token the last mutation of this stage handed back, and the single source
    /// of truth for every `If-Match` check against it.
    pub etag: String,
    /// `ConnectionFunctionSummary$CreatedTime` — "The connection function created time."
    pub created_time: SystemTime,
    /// `ConnectionFunctionSummary$LastModifiedTime` — "The connection function last
    /// modified time."
    pub last_modified_time: SystemTime,
    /// The instant the PUBLISH that wrote this stage is owed its settled status, for a
    /// stage still reporting [`CONNECTION_FUNCTION_STATUS_PUBLISHING`]; `None` for a stage
    /// no publish is in flight for (a `DEVELOPMENT` stage, or a `LIVE` stage whose publish
    /// has already settled). Stored rather than slept for.
    pub published_due: Option<SystemTime>,
}

impl StoredConnectionFunctionStage {
    /// Apply the publish transition the clock owes at `now`, where `associated` is whether
    /// a stored distribution names this connection function.
    ///
    /// Idempotent and monotonic, as every other `settle` of this service is: it only ever
    /// moves the in-progress status to the settled one, and only once the deadline has
    /// passed, so a read that calls it cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime, associated: bool) {
        let Some(due) = self.published_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = if associated {
            super::FUNCTION_STATUS_ASSOCIATED.to_string()
        } else {
            super::FUNCTION_STATUS_UNASSOCIATED.to_string()
        };
        self.published_due = None;
    }

    /// The `ConnectionFunctionSummary` projection every connection function operation
    /// returns for this stage, read from the one stored record.
    ///
    /// The name, the id and the ARN belong to the connection function rather than to one
    /// stage, so they are passed in by [`StoredConnectionFunction`]: both stages of one
    /// connection function report the SAME id and ARN.
    pub fn to_summary(&self, name: &str, id: &str, arn: &str) -> ConnectionFunctionSummary {
        ConnectionFunctionSummary {
            name: name.to_string(),
            id: id.to_string(),
            connection_function_config: self.config.clone(),
            connection_function_arn: arn.to_string(),
            status: self.status.clone(),
            stage: self.stage.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
        }
    }
}

/// One connection function, as this emulator persists it.
///
/// The record is the single source of truth for the id and the ARN minted for it, the name
/// it was created under, each stage's code, configuration, status, timestamps and version
/// token, and the tags it carries — so the create and every later read answer from it
/// rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredConnectionFunction {
    /// `ConnectionFunctionSummary$Id` — "The connection function ID", minted at creation
    /// and the key this record is held under: it is what `DeleteConnectionFunction`,
    /// `UpdateConnectionFunction` and `PublishConnectionFunction` address the resource by.
    pub id: String,
    /// `ConnectionFunctionSummary$Name` — "The connection function name", unique per
    /// account, which is what makes a repeated name an `EntityAlreadyExists`.
    pub name: String,
    /// `ConnectionFunctionSummary$ConnectionFunctionArn` — "The connection function Amazon
    /// Resource Name (ARN)". Minted at creation and the same for both stages.
    pub arn: String,
    /// The `DEVELOPMENT` stage, which exists for the whole life of the connection function:
    /// a create puts the code and the configuration it was given into it.
    pub development: StoredConnectionFunctionStage,
    /// The `LIVE` stage, which exists only once `PublishConnectionFunction` has copied the
    /// `DEVELOPMENT` stage into it — `None` until then, which is what makes a read of the
    /// `LIVE` stage of a never-published connection function an entity that is not found.
    pub live: Option<StoredConnectionFunctionStage>,
    /// The tags submitted with the create, in the SAME representation every tagged resource
    /// of this service uses, so one tagging rule covers them all.
    pub tags: TagSet,
}

impl StoredConnectionFunction {
    /// The stage `stage` names, or `None` when this connection function has no such stage —
    /// either because it has never been published, or because the value names no stage at
    /// all.
    ///
    /// The one lookup every connection function read goes through: the
    /// `ConnectionFunctionSummary` and the version token a read reports both come from the
    /// stage record it answers.
    pub fn stage(&self, stage: &str) -> Option<&StoredConnectionFunctionStage> {
        match stage {
            super::FUNCTION_STAGE_DEVELOPMENT => Some(&self.development),
            super::FUNCTION_STAGE_LIVE => self.live.as_ref(),
            _ => None,
        }
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// The connection function `identifier` names, or `None` when the store holds none.
    ///
    /// `DescribeConnectionFunction` and `GetConnectionFunction` name their resource by "The
    /// connection function's identifier", which it answers to by id, by name or by ARN, so
    /// all three resolve to the one record: the id is the store's key, and the name and the
    /// ARN are compared against the STORED values rather than re-derived out of the
    /// request. The id is tried FIRST, so a minted id always names the record it was minted
    /// for.
    pub fn connection_function(&self, identifier: &str) -> Option<&StoredConnectionFunction> {
        if let Some(function) = self.connection_functions.get(identifier) {
            return Some(function);
        }
        self.connection_functions
            .values()
            .find(|function| function.name == identifier || function.arn == identifier)
    }

    /// The connection function created under `name`, or `None` when the store holds none.
    ///
    /// `CreateConnectionFunction` models `EntityAlreadyExists` ("The entity already exists.
    /// You must provide a unique entity"), so a name names at most ONE connection function
    /// and this is what makes a repeated name a conflict rather than a second resource.
    pub fn connection_function_named(&self, name: &str) -> Option<&StoredConnectionFunction> {
        self.connection_functions
            .values()
            .find(|function| function.name == name)
    }

    /// The connection function `arn` names, or `None` when this store holds none.
    ///
    /// The ARN a connection function carries is the one minted when it was created, so the
    /// lookup compares against the STORED value rather than re-deriving an identifier out
    /// of the request. ONE ARN lookup for this resource kind, which the tag read goes
    /// through.
    pub fn connection_function_by_arn(&self, arn: &str) -> Option<&StoredConnectionFunction> {
        self.connection_functions
            .values()
            .find(|function| function.arn == arn)
    }

    /// Allocate a connection function id no stored connection function holds:
    /// [`CONNECTION_FUNCTION_ID_PREFIX`] followed by a minted opaque body, drawn from the
    /// same mint as every other namespace-prefixed id this service generates
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::mint_opaque_id_body`]) so the three
    /// namespaces are written in one form.
    pub fn allocate_connection_function_id(&mut self) -> String {
        loop {
            let candidate = format!(
                "{CONNECTION_FUNCTION_ID_PREFIX}{}",
                self.mint_opaque_id_body()
            );
            if !self.connection_functions.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// Whether any distribution this store holds names the connection function `id`.
    ///
    /// A `ConnectionFunctionAssociation` carries "The association's ID", which is the
    /// connection function's own `Id`, so the question is answered against the STORED
    /// configurations rather than against any request: it is what decides whether a
    /// published stage reports [`super::FUNCTION_STATUS_ASSOCIATED`] or
    /// [`super::FUNCTION_STATUS_UNASSOCIATED`].
    pub fn connection_function_is_associated(&self, id: &str) -> bool {
        self.distributions
            .values()
            .any(|distribution| references_connection_function(&distribution.config, id))
    }

    /// Copy the `DEVELOPMENT` stage of the connection function `id` names into its `LIVE`
    /// stage, and report the summary of the stage that was written — or `None` when this
    /// store holds no connection function under that id, checked BEFORE the mint so a
    /// version token is never spent on a resource that is not there.
    ///
    /// A publish is a COPY: the stage it writes carries the code and the configuration the
    /// `DEVELOPMENT` stage holds at this instant, keeps the created time of the connection
    /// function the copy was taken from, is last modified now, and gets a FRESH version
    /// token of its own, so an `If-Match` against the `LIVE` stage from before the publish
    /// no longer matches. The `DEVELOPMENT` stage is left exactly as it was — a publish does
    /// not edit the stage it reads, and the operation hands back no new token for it.
    ///
    /// The publish is ASYNCHRONOUS, so the stage it writes reports
    /// [`CONNECTION_FUNCTION_STATUS_PUBLISHING`] and the CLOCK owes it the status it settles
    /// to — read then off the stored distributions
    /// ([`Self::connection_function_is_associated`]), so a connection function a
    /// distribution names settles as associated and one nothing names as unassociated. The
    /// deadline is the span every other propagation of this service is owed over
    /// ([`PROPAGATION_DELAY`]) and is stored rather than slept for.
    pub fn publish_connection_function(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<ConnectionFunctionSummary> {
        if !self.connection_functions.contains_key(id) {
            return None;
        }
        let etag = self.next_etag();
        let function = self.connection_functions.get_mut(id)?;
        let published = StoredConnectionFunctionStage {
            stage: super::FUNCTION_STAGE_LIVE.to_string(),
            config: function.development.config.clone(),
            code: function.development.code.clone(),
            status: CONNECTION_FUNCTION_STATUS_PUBLISHING.to_string(),
            etag,
            created_time: function.development.created_time,
            last_modified_time: now,
            published_due: Some(now + PROPAGATION_DELAY),
        };
        let summary = published.to_summary(&function.name, &function.id, &function.arn);
        function.live = Some(published);
        Some(summary)
    }

    /// Apply the publish transition the clock owes every stored connection function at
    /// `now` — the slice of the parent's `settle` this module owns.
    ///
    /// Which status a settling publish lands on is read off the stored DISTRIBUTIONS, so
    /// the connection functions that are due are collected first and written after: the
    /// answer comes from the same one rule a publish reads
    /// ([`Self::connection_function_is_associated`]), never from the request being served.
    pub fn settle_connection_functions(&mut self, now: SystemTime) {
        let due: Vec<(String, bool)> = self
            .connection_functions
            .iter()
            .filter(|(_, function)| {
                function
                    .live
                    .as_ref()
                    .is_some_and(|live| live.published_due.is_some_and(|due| now >= due))
            })
            .map(|(id, _)| (id.clone(), self.connection_function_is_associated(id)))
            .collect();
        for (id, associated) in due {
            if let Some(live) = self
                .connection_functions
                .get_mut(&id)
                .and_then(|function| function.live.as_mut())
            {
                live.settle(now, associated);
            }
        }
    }
}

/// Whether `value` has the shape of a connection function id this service mints
/// ([`CONNECTION_FUNCTION_ID_PREFIX`] followed by an opaque base62 body,
/// [`super::is_opaque_id_body`]).
///
/// `ListConnectionFunctions`'s `Marker` is a resume point "the value of `NextMarker` from
/// the current page's response", which is an id this service handed out, and the model
/// constrains it no further than `string` — so this is what tells a resume point the service
/// issued from a value a client made up.
pub fn is_connection_function_id(value: &str) -> bool {
    value
        .strip_prefix(CONNECTION_FUNCTION_ID_PREFIX)
        .is_some_and(super::is_opaque_id_body)
}

/// Whether `config` attaches the connection function `id` names.
///
/// A distribution attaches a connection function through its `ConnectionFunctionAssociation`,
/// whose `Id` is "The association's ID" — the connection function's own `Id` — and a
/// configuration carries at most one, so a distribution is attached to the function exactly
/// when that member names it. ONE rule, read from the STORED configuration, behind both the
/// status a published stage reports and the listing of the distributions a connection function
/// serves, so the two can never disagree about an attachment.
pub fn references_connection_function(config: &DistributionConfig, id: &str) -> bool {
    config
        .connection_function_association
        .as_ref()
        .is_some_and(|association| association.id == id)
}

/// The ARN of a connection function. CloudFront is a GLOBAL service, so the ARN's region
/// segment is empty: `arn:aws:cloudfront::<account>:connection-function/<Id>`. The resource
/// part is the minted id, which is the identifier every mutating connection function
/// operation addresses one by.
pub fn connection_function_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("connection-function/{id}"))
}

/// The path of a connection function resource under this service's API version. The
/// resource is addressed by ID, as `GetConnectionFunction`'s `{Identifier}` label and
/// `DeleteConnectionFunction`'s `{Id}` label are.
pub fn connection_function_path(id: &str) -> String {
    format!("/2020-05-31/connection-function/{id}")
}

/// The sentence a connection function this store does not hold is refused with, in the form
/// CloudFront words a missing named resource — "The specified <resource> does not exist.",
/// as [`super::CONNECTION_GROUP_NOT_FOUND_MESSAGE`] and
/// [`super::NO_SUCH_FUNCTION_EXISTS_MESSAGE`] carry it for the neighbouring resource kinds;
/// the shape's own documentation states only its shorter generic form.
///
/// The WHOLE sentence is fixed: what the refusal states is that no such connection function
/// exists, which is a property of the store rather than of the value submitted, so neither
/// the identifier asked about nor the stage it was asked for is any part of it.
pub const CONNECTION_FUNCTION_NOT_FOUND_MESSAGE: &str =
    "The specified connection function does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for a connection function this store does not
/// hold — no connection function answering to the identifier (by its id, by the name it was
/// created under or by the ARN minted for it), or one that has never been published to the
/// stage the request names.
///
/// Takes neither identifier nor stage, so every connection function read and every
/// connection function mutation refuses this condition with the one fixed sentence
/// ([`CONNECTION_FUNCTION_NOT_FOUND_MESSAGE`]).
pub fn connection_function_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(CONNECTION_FUNCTION_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a connection function name that is already taken with,
/// on the wire: the WIRE text for `EntityAlreadyExists` on THIS resource kind, worded as
/// [`super::CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`] words it for the neighbouring one,
/// and fixed whole for the reason that one is — the sentence the MODEL's documentation of
/// the shape uses ("The entity already exists. You must provide a unique entity.")
/// documents the condition for every kind sharing the code and is not served, and nothing
/// from the request or from the store belongs in the served text.
pub const CONNECTION_FUNCTION_ALREADY_EXISTS_MESSAGE: &str =
    "A connection function with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a connection function name that is
/// already taken.
///
/// Takes no name, for the reason [`super::connection_group_already_exists`] takes none:
/// every refusal of this condition reads the same
/// ([`CONNECTION_FUNCTION_ALREADY_EXISTS_MESSAGE`]).
pub fn connection_function_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(CONNECTION_FUNCTION_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}
