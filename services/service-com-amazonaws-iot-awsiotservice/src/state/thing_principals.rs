// SPDX-License-Identifier: CC0-1.0

//! WHICH PRINCIPALS are attached to which thing, and what that attachment holds back.
//!
//! `AttachThingPrincipal` "Attaches the specified principal to the specified thing. A
//! principal can be X.509 certificates, Amazon Cognito identities or federated
//! identities", `DetachThingPrincipal` "Detaches the specified principal from the
//! specified thing" and `ListThingPrincipals` "Lists the principals associated with the
//! specified thing" — three views of ONE relation, and that relation is what this module
//! keeps.
//!
//! It lives on the THING's own record ([`super::RegisteredThing::principals`]) rather than
//! in a map beside the registry, for the reason a group's membership lives on the group
//! ([`super::thing_group_membership`]): an attachment is an attachment OF a thing, so it
//! disappears with the thing, and the thing is the record both ends of the relation are
//! reached through.
//!
//! A principal's IDENTITY is the resource it names and not the string that names it, the
//! way a policy target's is ([`super::policy_attachments`]) — a certificate is named by
//! its ARN, an Amazon Cognito identity by its `<region>:<id>` form — so the relation is
//! keyed by the resource each of those names ([`ThingPrincipal::key`]) and the same
//! certificate named by two ARNs is one principal. The two forms are read by the one
//! reader of these addresses this service has ([`super::parse_policy_target`]), less the
//! thing group, which is a target of a POLICY and no principal of a thing. Only a
//! principal that EXISTS is attached: a certificate ARN is resolved against the
//! certificate store, while a Cognito identity is resolved no further than its form,
//! because identity pools live in another service and this one holds no record of them.
//!
//! SYNTAX and EXISTENCE are separate questions, asked in that order
//! ([`ThingPrincipalLookup`]): a value that is no principal ADDRESS at all names no
//! principal in any account and is a request to be FIXED, while a well-formed address this
//! account holds no principal under is an ordinary lookup MISS — including an ARN of a
//! resource that is no principal of a thing, which addresses something and simply resolves
//! to no principal.
//!
//! Each refusal is worded after the CALL that failed and not after the resource family it
//! could not resolve ([`ThingPrincipalRefusal::message`], [`listing_missing_thing_message`]):
//! the sentence names the action the caller asked for, the principal it named and the thing
//! it addressed.
//!
//! The relation is also what two DELETES read before they remove anything, because AWS IoT
//! requires detach-before-delete at BOTH ends: a thing that still has a principal attached
//! ([`attached_principals_thing_deletion_message`]), and a certificate that still has a
//! thing attached ("A certificate cannot be deleted if it has a policy or IoT thing
//! attached to it", `DeleteCertificate`, refused by
//! [`super::certificates::attached_things_deletion_message`]). So neither end is ever torn
//! out from under the other.

use crate::ops::types::THING_PRINCIPAL_TYPE_VALUES;

/// How many principals one thing may hold.
///
/// The relation is one a CALLER grows — one pair per `AttachThingPrincipal` — so it is
/// bounded for the reason [`super::MAX_POLICIES_PER_TARGET`] bounds the policy attachments:
/// past the ceiling nothing is recorded and the attach is refused rather than one thing's
/// record growing without bound. The operation models no `LimitExceededException`
/// (`smithy_query(query="operation",operation="AttachThingPrincipal")` lists
/// InternalFailure, InvalidRequest, ResourceNotFound, ServiceUnavailable, Throttling and
/// Unauthorized), so the refusal is the modelled `InvalidRequestException` naming the
/// limit; the figure is this emulator's own capacity.
pub const MAX_PRINCIPALS_PER_THING: usize = 10;

/// The `ThingPrincipalType` an attachment is recorded under when the request named none.
///
/// `ThingPrincipalObject$thingPrincipalType` is "The type of the relation you want to
/// specify when you attach a principal to a thing. The value defaults to
/// `NON_EXCLUSIVE_THING`"
/// (`smithy_query(query="shape",shape="ThingPrincipalObject")`), so every recorded
/// attachment carries one whether or not the writer stated it — the same rule
/// [`super::DEFAULT_DYNAMIC_GROUP_QUERY_VERSION`] states for a dynamic group's query.
pub const DEFAULT_THING_PRINCIPAL_TYPE: &str = "NON_EXCLUSIVE_THING";

/// The `ThingPrincipalType` an attachment that holds the thing EXCLUSIVELY is recorded
/// under.
///
/// The other of the two values the enum admits
/// (`smithy_query(query="shape",shape="ThingPrincipalType")`), and the one
/// [`DEFAULT_THING_PRINCIPAL_TYPE`] is not: `thingPrincipalType` is "The type of the
/// relation you want to specify when you attach a principal to a thing", so an attachment
/// recorded under this value is the one a reader of the relation reports as an exclusive
/// attachment of the thing.
pub const EXCLUSIVE_THING_PRINCIPAL_TYPE: &str = "EXCLUSIVE_THING";

/// The resource type segment of a certificate ARN, as [`super::certificate_arn`] mints it.
const CERTIFICATE_RESOURCE_TYPE: &str = "cert";

/// The key prefix a Cognito identity is recorded under, keeping it apart from a
/// certificate of the same name.
const COGNITO_IDENTITY_KEY_PREFIX: &str = "cognito";

/// The resource a thing principal names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThingPrincipal {
    /// `arn:<partition>:iot:<region>:<account>:cert/<certificateId>`: one X.509 device
    /// certificate, carrying its `certificateId`.
    Certificate(String),
    /// `<region>:<id>`: one Amazon Cognito identity, carrying the identity as supplied.
    CognitoIdentity(String),
}

impl ThingPrincipal {
    /// The key the relation records this principal under: the resource it names, so two
    /// ARNs of the same certificate are one principal. The resource type prefixes the key,
    /// so a certificate and a Cognito identity never collide.
    pub fn key(&self) -> String {
        match self {
            Self::Certificate(certificate_id) => {
                format!("{CERTIFICATE_RESOURCE_TYPE}/{certificate_id}")
            }
            Self::CognitoIdentity(identity) => {
                format!("{COGNITO_IDENTITY_KEY_PREFIX}/{identity}")
            }
        }
    }
}

/// One attachment, as the thing's record holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThingPrincipalAttachment {
    /// The principal as every listing of one reports it: the ARN this account holds for the
    /// resource, or the Cognito identity as supplied.
    pub reported: String,
    /// The `ThingPrincipalType` the attachment was recorded under, which an attach naming
    /// none records at [`DEFAULT_THING_PRINCIPAL_TYPE`].
    pub principal_type: String,
}

impl ThingPrincipalAttachment {
    /// Whether this attachment holds the thing EXCLUSIVELY — its recorded
    /// `ThingPrincipalType` is [`EXCLUSIVE_THING_PRINCIPAL_TYPE`].
    ///
    /// Read from the RECORD rather than from a request, so what a listing reports about an
    /// attachment is what the attach that made it asked for, whichever writer of the
    /// relation made it.
    pub fn is_exclusive(&self) -> bool {
        self.principal_type == EXCLUSIVE_THING_PRINCIPAL_TYPE
    }
}

/// A thing principal resolved against this account's state: everything an attachment or a
/// listing needs about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedThingPrincipal {
    /// Which resource it names.
    pub principal: ThingPrincipal,
    /// The key the relation records it under ([`ThingPrincipal::key`]).
    pub key: String,
    /// The principal as every listing reports it.
    pub reported: String,
}

/// Why the relation will not take — or will not report — a thing principal.
///
/// Each variant names the modelled error class the refusal answers with, so every operation
/// that reads or writes the relation refuses one principal for one reason, in one wording,
/// with one status. What the sentence SAYS is the answering operation's own
/// ([`Self::message`]), so each variant carries the parts of the request its sentences name
/// rather than a sentence of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThingPrincipalRefusal {
    /// The request named a thing the registry does not hold:
    /// `ResourceNotFoundException` (HTTP 404).
    MissingThing {
        /// The principal the request carried, as it carried it.
        principal: String,
        /// The `thingName` the request addressed.
        thing_name: String,
    },
    /// The value is no principal ADDRESS at all, so it names no principal in any account
    /// and there is nothing to look for: `InvalidRequestException` (HTTP 400).
    InvalidPrincipalType {
        /// The value the request carried, as it carried it.
        principal: String,
    },
    /// A well-formed principal address naming no principal this account holds:
    /// `ResourceNotFoundException` (HTTP 404).
    MissingPrincipal {
        /// The principal the request carried, as it carried it.
        principal: String,
        /// The `thingName` the request addressed.
        thing_name: String,
    },
    /// The thing already holds [`MAX_PRINCIPALS_PER_THING`] principals:
    /// `InvalidRequestException` (HTTP 400).
    TooManyPrincipals {
        /// The `thingName` the request addressed.
        thing_name: String,
    },
}

/// Which WRITE of the relation is answering a refusal of it.
///
/// The two writes state a refusal in the same sentence less one verb and one preposition —
/// a principal is attached TO a thing and detached FROM it — so which operation is
/// answering is all the wording needs to be the operation's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThingPrincipalWrite {
    /// `AttachThingPrincipal`.
    Attach,
    /// `DetachThingPrincipal`.
    Detach,
}

impl ThingPrincipalWrite {
    /// The verb this write's sentences name the action with.
    fn verb(self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Detach => "detach",
        }
    }

    /// The preposition this write's sentences name the THING with: the direction the write
    /// moves the principal in.
    fn preposition(self) -> &'static str {
        match self {
            Self::Attach => "to",
            Self::Detach => "from",
        }
    }
}

impl ThingPrincipalRefusal {
    /// What the refusal reports, in the wording the operation answering it states it in.
    ///
    /// The two ends of the relation are refused in the two sentences the WRITE words them
    /// in: a `thingName` the registry does not hold says the action failed because the
    /// thing does not exist in the account, and a principal the account holds none of says
    /// the same of the principal. Both name the principal the request carried and the thing
    /// it addressed; neither qualifies a name with the account id, and neither ends in a
    /// period. The recorded wording is `AttachThingPrincipal`'s; the detach states the same
    /// class in its own action's words.
    pub fn message(&self, write: ThingPrincipalWrite) -> String {
        let verb = write.verb();
        let preposition = write.preposition();
        match self {
            Self::MissingThing {
                principal,
                thing_name,
            } => format!(
                "Failed to {verb} the principal {principal} {preposition} thing {thing_name} \
                 because the thing does not exist in your account"
            ),
            Self::MissingPrincipal {
                principal,
                thing_name,
            } => format!(
                "Can't {verb} the principal {principal} {preposition} thing {thing_name} \
                 because the principal does not exist in your account"
            ),
            Self::InvalidPrincipalType { principal } => invalid_principal_type_message(principal),
            Self::TooManyPrincipals { thing_name } => format!(
                "Thing {thing_name} cannot have more than {MAX_PRINCIPALS_PER_THING} \
                 attached principals"
            ),
        }
    }

    /// Whether the refusal is the modelled `ResourceNotFoundException` (HTTP 404) — one end
    /// of the relation names a resource this account does not hold — rather than
    /// `InvalidRequestException` (HTTP 400), a request to be fixed before it is retried.
    pub fn is_missing_resource(&self) -> bool {
        matches!(
            self,
            Self::MissingThing { .. } | Self::MissingPrincipal { .. }
        )
    }
}

/// How this service states that a value bound where it takes a thing principal is no
/// principal ADDRESS at all.
///
/// `AttachThingPrincipal$principal` is "The principal, which can be a certificate ARN (as
/// returned from the CreateCertificate operation) or an Amazon Cognito ID", so a value in
/// neither form names no KIND of principal at all — which is what the sentence says. The
/// fixed text names that, and the only variable part is the value the request sent, echoed
/// verbatim.
pub fn invalid_principal_type_message(principal: &str) -> String {
    format!("Invalid principal type: {principal}")
}

/// How `ListThingPrincipals` states that the thing whose principals it was asked for is one
/// the registry does not hold.
///
/// The LISTING words the miss after the call that failed too, the way the two writes do
/// ([`ThingPrincipalRefusal::message`]), so the sentence names the listing rather than the
/// thing family: fixed text, with the `thingName` the URI label carried as its only variable
/// part, no account qualification and no trailing period. AWS leaves NO space between
/// `thing` and the name, so the template carries none either.
pub fn listing_missing_thing_message(thing_name: &str) -> String {
    format!(
        "Failed to list principals for thing{thing_name} because the thing does not exist \
         in your account"
    )
}

/// How `ListPrincipalThings` states that the value it was asked for the things of names no
/// principal this account holds.
///
/// The listing words the miss after the CALL that failed, the way the two writes of the
/// relation and the listing from the thing's end do
/// ([`ThingPrincipalRefusal::message`], [`listing_missing_thing_message`]): fixed text
/// naming the action and the reason, with the principal the request carried as its only
/// variable part, no account qualification and no trailing period.
pub fn listing_missing_principal_message(principal: &str) -> String {
    format!(
        "Failed to list things for principal {principal} because the principal does not \
         exist in your account"
    )
}

/// Why a value bound where this service takes a thing principal names no principal of this
/// account: the question the RELATION asks of the value, before any operation words it
/// ([`ThingPrincipalRefusal`]).
///
/// Two outcomes, because they are two different conditions for a caller: a value that is no
/// address cannot name a principal however the account grows, while an address the account
/// holds no principal under names one that could be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThingPrincipalLookup {
    /// The value is no principal address at all: neither an ARN nor the `<region>:<id>`
    /// form an Amazon Cognito identity is named in.
    InvalidType,
    /// A well-formed address naming no principal this account holds.
    Missing,
}

impl ThingPrincipalLookup {
    /// The refusal a write of the relation answers this lookup failure with: the value the
    /// request carried and the thing it addressed are what the sentences name.
    pub fn refusal(self, principal: &str, thing_name: &str) -> ThingPrincipalRefusal {
        match self {
            Self::InvalidType => ThingPrincipalRefusal::InvalidPrincipalType {
                principal: principal.to_string(),
            },
            Self::Missing => ThingPrincipalRefusal::MissingPrincipal {
                principal: principal.to_string(),
                thing_name: thing_name.to_string(),
            },
        }
    }
}

/// Which lookup failure a value that reads as no principal of this account is.
///
/// A well-formed ARN ADDRESSES a resource, so a principal the account holds none of under
/// it is a lookup miss — whatever resource type the ARN names, since none of them is a
/// principal of a thing unless the account holds it — while a value that is no ARN and no
/// Cognito identity addresses nothing at all. The fields are read through the one ARN parse
/// the services share (`service_common::arn`), so what counts as an address here is what
/// counts as one everywhere else.
fn principal_address_lookup(principal: &str) -> ThingPrincipalLookup {
    if ::service_common::arn::parse(principal).is_some() {
        ThingPrincipalLookup::Missing
    } else {
        ThingPrincipalLookup::InvalidType
    }
}

/// The message a delete of a THING that still has a principal attached is refused with.
///
/// AWS IoT makes detach-before-delete mandatory, and words this refusal after the thing the
/// request addressed: the fixed text is the class's and the only variable part is the
/// `thingName` the request carried. How MANY principals are attached is not part of it —
/// the sentence says "one or more principals" whether the thing holds one or ten.
pub fn attached_principals_thing_deletion_message(thing_name: &str) -> String {
    format!("Cannot delete. Thing {thing_name} is still attached to one or more principals")
}

impl super::ComAmazonawsIotAwsiotserviceState {
    /// Resolve `principal` against this account's state: the resource it names, the key the
    /// relation records it under and the form every listing reports it in — or the reason it
    /// is no principal of a thing of this account.
    pub fn resolve_thing_principal(
        &self,
        principal: &str,
    ) -> Result<ResolvedThingPrincipal, ThingPrincipalLookup> {
        let named = match super::parse_policy_target(principal) {
            Ok(super::PolicyTarget::Certificate(certificate_id)) => {
                ThingPrincipal::Certificate(certificate_id)
            }
            Ok(super::PolicyTarget::CognitoIdentity(identity)) => {
                ThingPrincipal::CognitoIdentity(identity)
            }
            // A thing GROUP is a target a policy attaches to and no principal a thing is
            // reached through, and an ARN of any other resource names no principal of this
            // account either — but each of them ADDRESSES something, so what separates the
            // two refusals is whether the value is an address at all and not which resource
            // type it names.
            Ok(super::PolicyTarget::ThingGroup(_)) | Err(_) => {
                return Err(principal_address_lookup(principal))
            }
        };
        let reported = match &named {
            ThingPrincipal::Certificate(certificate_id) => {
                let Some(certificate) = self.certificates.get(certificate_id) else {
                    return Err(ThingPrincipalLookup::Missing);
                };
                certificate.certificate_arn.clone()
            }
            ThingPrincipal::CognitoIdentity(identity) => identity.clone(),
        };
        Ok(ResolvedThingPrincipal {
            key: named.key(),
            principal: named,
            reported,
        })
    }

    /// Why the relation would refuse attaching `principal` to the thing named `thing_name`,
    /// or `None` when it would take it.
    ///
    /// READ-ONLY, and the one place the ceiling is read: [`Self::record_thing_principal`]
    /// answers this before it writes, so a caller that has to know whether the attachment is
    /// admissible BEFORE making other writes — the provisioning path, which registers the
    /// thing and the attachment in one call ([`super::thing_provisioning`]) — asks the same
    /// question and cannot drift from what the write then does.
    ///
    /// A pair already recorded is admissible and no nearer the ceiling, because recording it
    /// again adds nothing. Whether the thing EXISTS is not read here: that is the write's own
    /// check, so a caller about to register the thing can ask this first.
    pub fn thing_principal_refusal_for(
        &self,
        thing_name: &str,
        principal: &ResolvedThingPrincipal,
    ) -> Option<ThingPrincipalRefusal> {
        let thing = self.things.get(thing_name)?;
        if thing.principals.contains_key(&principal.key) {
            return None;
        }
        (thing.principals.len() >= MAX_PRINCIPALS_PER_THING).then(|| {
            ThingPrincipalRefusal::TooManyPrincipals {
                thing_name: thing_name.to_string(),
            }
        })
    }

    /// Record that the already resolved `principal` is attached to the thing `thing_name`:
    /// the ONE relation write, so every writer of it — the attach, and the provisioning path
    /// that provisions a device with its certificate — agrees on what it takes.
    ///
    /// IDEMPOTENT: the pair is keyed by the resource the principal names, so a repeated
    /// attach leaves the same one attachment and is no nearer the ceiling. The type the
    /// request named is recorded on it, and an attach naming none records
    /// [`DEFAULT_THING_PRINCIPAL_TYPE`].
    pub fn record_thing_principal(
        &mut self,
        thing_name: &str,
        principal: &ResolvedThingPrincipal,
        thing_principal_type: Option<&str>,
    ) -> Result<(), ThingPrincipalRefusal> {
        if !self.things.contains_key(thing_name) {
            // This writer is handed the principal already RESOLVED, so the form it names in
            // the sentence is the one the relation reports; its callers resolve the thing
            // first, so a request's own value is what a caller actually sees named.
            return Err(ThingPrincipalRefusal::MissingThing {
                principal: principal.reported.clone(),
                thing_name: thing_name.to_string(),
            });
        }
        if let Some(refusal) = self.thing_principal_refusal_for(thing_name, principal) {
            return Err(refusal);
        }
        // A value outside the modelled enum is refused before any handler runs, so this
        // keeps the record inside `ThingPrincipalType` without depending on that.
        let principal_type = thing_principal_type
            .filter(|named| THING_PRINCIPAL_TYPE_VALUES.contains(named))
            .unwrap_or(DEFAULT_THING_PRINCIPAL_TYPE)
            .to_string();
        if let Some(thing) = self.things.get_mut(thing_name) {
            thing.principals.insert(
                principal.key.clone(),
                ThingPrincipalAttachment {
                    reported: principal.reported.clone(),
                    principal_type,
                },
            );
        }
        Ok(())
    }

    /// Attach `principal` to the thing `thing_name`, answering the principal as the relation
    /// recorded it.
    ///
    /// Both ends have to EXIST — an attachment between records this account does not hold
    /// would be a relation over nothing — which is why the thing is resolved before the
    /// principal is: the request addresses the THING, so a request naming neither is told
    /// about the resource it addressed.
    pub fn attach_thing_principal(
        &mut self,
        thing_name: &str,
        principal: &str,
        thing_principal_type: Option<&str>,
    ) -> Result<ResolvedThingPrincipal, ThingPrincipalRefusal> {
        if !self.things.contains_key(thing_name) {
            return Err(ThingPrincipalRefusal::MissingThing {
                principal: principal.to_string(),
                thing_name: thing_name.to_string(),
            });
        }
        let resolved = self
            .resolve_thing_principal(principal)
            .map_err(|lookup| lookup.refusal(principal, thing_name))?;
        self.record_thing_principal(thing_name, &resolved, thing_principal_type)?;
        Ok(resolved)
    }

    /// Detach `principal` from the thing `thing_name`, answering whether an attachment was
    /// there to remove: the inverse of [`Self::attach_thing_principal`], under the same rule
    /// that both ends exist.
    ///
    /// A principal that is not attached removes nothing and succeeds: the detach names no
    /// identity, so the relation holds the same attachments afterwards however often it is
    /// called.
    pub fn detach_thing_principal(
        &mut self,
        thing_name: &str,
        principal: &str,
    ) -> Result<bool, ThingPrincipalRefusal> {
        if !self.things.contains_key(thing_name) {
            return Err(ThingPrincipalRefusal::MissingThing {
                principal: principal.to_string(),
                thing_name: thing_name.to_string(),
            });
        }
        let resolved = self
            .resolve_thing_principal(principal)
            .map_err(|lookup| lookup.refusal(principal, thing_name))?;
        Ok(self
            .things
            .get_mut(thing_name)
            .is_some_and(|thing| thing.principals.remove(&resolved.key).is_some()))
    }

    /// The principals attached to the thing `thing_name`, as a listing of them reports them,
    /// in ASCENDING order of the reported value — the total order that makes a page boundary
    /// over them meaningful, since a page marker names a position in it.
    pub fn thing_principals(&self, thing_name: &str) -> Vec<String> {
        let mut reported: Vec<String> = self
            .things
            .get(thing_name)
            .map(|thing| {
                thing
                    .principals
                    .values()
                    .map(|attachment| attachment.reported.clone())
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        reported
    }

    /// The principals of the thing `thing_name` whose attachment is EXCLUSIVE
    /// ([`ThingPrincipalAttachment::is_exclusive`]), in the same ASCENDING order of the
    /// reported value [`Self::thing_principals`] reports the relation in.
    ///
    /// Derived from the attachments themselves — the type each one was recorded under —
    /// rather than kept as a second relation beside them, so a principal cannot be
    /// exclusively attached in one reader's view and not in another's.
    pub fn thing_principals_with_exclusive_attachment(&self, thing_name: &str) -> Vec<String> {
        let mut reported: Vec<String> = self
            .things
            .get(thing_name)
            .map(|thing| {
                thing
                    .principals
                    .values()
                    .filter(|attachment| attachment.is_exclusive())
                    .map(|attachment| attachment.reported.clone())
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        reported
    }

    /// The names of the things the principal recorded under `key` is attached to, in
    /// registry order.
    ///
    /// Derived from the things' own records, so there is one record of the relation and not a
    /// second copy keyed by the principal — which is what lets a delete of the PRINCIPAL ask
    /// whether a thing still holds it.
    pub fn things_attached_to_principal(&self, key: &str) -> Vec<String> {
        self.things
            .values()
            .filter(|thing| thing.principals.contains_key(key))
            .map(|thing| thing.thing_name.clone())
            .collect()
    }
}
