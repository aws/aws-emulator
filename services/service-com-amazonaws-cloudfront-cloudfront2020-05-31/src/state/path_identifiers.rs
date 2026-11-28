// SPDX-License-Identifier: CC0-1.0

//! What an identifier bound to the request PATH has to be before the operation it addresses
//! can resolve it at all.
//!
//! An identifier CloudFront issues names the kind of resource it was issued for, so an
//! operation addressed by one can report a value it cannot resolve either as the resource that
//! is ABSENT or as an identifier it cannot READ ([`super::id_format_invalid_message`]) — the
//! same not-found code and status under two different sentences. HOW MUCH of the submitted
//! value a route reads before it goes near the store is that resource kind's own recorded
//! contract, and the members that carry one are plain `string` `@httpLabel`s in the model (no
//! `@pattern`, no `@length`), so no route may invent a format constraint of its own:
//!
//! * the DISTRIBUTION reads the ALPHABET and nothing more
//!   ([`super::unresolved_distribution_id`]). Every operation addressed by a distribution
//!   identifier — the distribution's own reads and mutations, the invalidations held under one,
//!   the monitoring subscription — carries a value written in the alphabet an entity
//!   identifier is written in ([`super::is_entity_id_form`]) straight through to the store and
//!   reports the distribution ABSENT ([`super::no_such_distribution`]) for every one it holds
//!   no distribution under, because the width this emulator MINTS a distribution identifier at
//!   is one width CloudFront writes one in, never the only width, and WHICH identifier of that
//!   alphabet was ever issued is a property of the STORE alone — while a value written outside
//!   that alphabet is the identifier the route reports it cannot READ;
//! * the field-level encryption CONFIGURATION DOCUMENT routes refuse the form of every value
//!   that could never be an identifier issued for their kind — one written outside the alphabet
//!   an identifier is written in, and one written exactly as an ISSUED identifier (that alphabet
//!   at the issued width) under another kind's namespace letter, which makes it that
//!   other kind's identifier ([`super::unresolved_field_level_encryption_config`],
//!   [`super::unresolved_field_level_encryption_profile`]); every other value is carried
//!   through to the store, while that configuration's resource read reads no form at all and
//!   reports every id it cannot resolve as the configuration that is absent
//!   ([`super::no_such_field_level_encryption_config`]);
//! * the PUBLIC KEY routes read ONE of those two things: an identifier ISSUED FOR ANOTHER KIND
//!   is the identifier they report they cannot read, and every other value — in any characters,
//!   at any width — is carried through to the store ([`unresolved_public_key`]);
//! * the ORIGIN ACCESS CONTROL reads the ALPHABET and nothing more
//!   ([`unresolved_origin_access_control_id`]), the way the distribution does and for the same
//!   reason: its identifiers come from the namespace its kind SHARES with the distributions,
//!   the streaming distributions and the origin access identities, so neither the letter a
//!   value opens with nor the width of its body says which of them issued it, while a value
//!   written outside that alphabet was issued for nothing at all and is the identifier the
//!   route reports it cannot READ.
//!
//! The same split, for the kinds whose identifier form IS decided by the value alone, is made
//! by the free functions beside the refusals they answer with:
//! [`super::unresolved_distribution_id`], [`unresolved_origin_access_control_id`],
//! [`super::unresolved_key_group_id`], the public key's
//! and the two field-level encryption records'. A kind whose recorded contract
//! makes NO such split reads nothing of the value at all and reports the absent resource for
//! every identifier it cannot resolve, which is what the field-level encryption
//! configuration's RESOURCE read does
//! ([`super::no_such_field_level_encryption_config`]).
//!
//! Nothing here echoes the submitted value: both sentences are fixed text.

use super::*;

/// The model's name for the member every operation of the `E`-form family addresses its
/// resource by, except the ones that address a resource held UNDER a distribution
/// (`CreateInvalidation$DistributionId`).
pub const ADDRESSED_BY_ID: &str = "Id";

/// The modelled `NoSuchPublicKey` (HTTP 404) an operation ADDRESSED by a public key `Id` in its
/// path answers an `Id` it cannot resolve.
///
/// These operations model this one not-found code and no argument error
/// (`smithy_query(query="operation",operation="GetPublicKey,GetPublicKeyConfig,UpdatePublicKey,\
/// DeletePublicKey")` reports `AccessDenied` and `NoSuchPublicKey` as the only errors covering an
/// `Id` they cannot resolve), so one code and one status cover two conditions and the MESSAGE is
/// what tells them apart — decided from the value alone, and therefore before the store is
/// consulted. These routes read ONE thing of the value, which their recorded answers are what
/// settles: whether it IS an identifier CloudFront issued for another kind of this service
/// ([`is_issued_for_another_kind`]) — the id alphabet at the issued width under another kind's
/// namespace letter — in which case it names that other kind's record and the route reports the
/// identifier it cannot READ ([`id_format_invalid_message`]).
///
/// EVERY other value is carried through to the store, however it is spelled: `Id` is a plain
/// `string` `@httpLabel` the model constrains no further (no `@pattern`, no `@length`), WHICH
/// identifier of this kind was ever issued is a property of the STORE rather than of the value,
/// and the widths and alphabets this service MINTS in are one form CloudFront writes an
/// identifier in rather than the only one — so a value of the alphabet at another width, a value
/// under a letter no kind issues under, a value carrying lower case or a separator and a value
/// of no identifier shape at all are each resolved against the store, and the key it holds none
/// under is the key that is simply ABSENT. This is what the recorded answers of these routes
/// report: a probe written in lower case around separators, and one of the alphabet written
/// wider than a key's identifier is issued, are each answered with the absent-key sentence.
///
/// Only an identifier that ADDRESSES the operation is judged this way: a public key id borne by a
/// submitted DOCUMENT (a key group's `Items` member, a field-level encryption profile's
/// `PublicKeyId`) addresses nothing, so it is resolved against the store alone and reports the
/// absent key whatever its shape ([`no_such_public_key`]).
pub fn unresolved_public_key(id: &str) -> crate::ops::types::NoSuchPublicKey {
    if is_issued_for_another_kind(PUBLIC_KEY_ID_PREFIX, id) {
        crate::ops::types::NoSuchPublicKey {
            message: Some(id_format_invalid_message(ADDRESSED_BY_ID)),
        }
    } else {
        no_such_public_key()
    }
}

/// The modelled `NoSuchOriginAccessControl` (HTTP 404) an operation ADDRESSED by an origin
/// access control `Id` in its path answers an `Id` it cannot resolve.
///
/// These operations model this one not-found code and no argument error
/// (`smithy_query(query="operation",operation="GetOriginAccessControl,GetOriginAccessControlConfig,UpdateOriginAccessControl,DeleteOriginAccessControl")`
/// reports `AccessDenied` and `NoSuchOriginAccessControl` as the only errors covering an `Id`
/// they cannot resolve), so one code and one status cover two conditions and the MESSAGE is
/// what tells them apart — decided from the value alone, and therefore before the store is
/// consulted.
///
/// What the value has to say for the format refusal to be the answer is the same thing it has
/// to say on the DISTRIBUTION's routes ([`super::unresolved_distribution_id`]), because the two
/// kinds are minted in one form: a value written in the alphabet CloudFront writes an entity
/// identifier in ([`is_entity_id_form`]) is an identifier of that mint, and WHICH one was ever
/// issued — and for which of the kinds that share the namespace letter — is a property of the
/// STORE, so such a value the account holds no control under is the control reported ABSENT
/// ([`no_such_origin_access_control`]). A value written OUTSIDE that alphabet — in lower case,
/// around a separator, as a UUID, as a word or as a domain name — was never an identifier of
/// this mint at all, so no record of any kind could answer to it and the route reports the
/// identifier it cannot READ ([`id_format_invalid_message`]).
///
/// Only an identifier that ADDRESSES the operation is judged this way: a control id borne by a
/// submitted distribution configuration (`Origin$OriginAccessControlId`) addresses nothing
/// here, so it is resolved against the store alone and refused as that configuration's own
/// unusable reference ([`invalid_origin_access_control`]).
pub fn unresolved_origin_access_control_id(id: &str) -> NoSuchOriginAccessControl {
    if is_entity_id_form(id) {
        return no_such_origin_access_control();
    }
    NoSuchOriginAccessControl {
        message: Some(id_format_invalid_message(ADDRESSED_BY_ID)),
    }
}

/// The model's name for the member the operations addressed by the distribution a resource is
/// held under carry that distribution's identifier in — the invalidations of a distribution
/// and its monitoring subscription (`CreateInvalidation$DistributionId`,
/// `GetMonitoringSubscription$DistributionId`), which is the member such a route names when
/// it reports an identifier it cannot read ([`super::unresolved_distribution_id`]).
pub const ADDRESSED_BY_DISTRIBUTION_ID: &str = "DistributionId";
