// SPDX-License-Identifier: CC0-1.0

//! How a read of ONE security group names the error shape of a group it cannot report: it
//! does not.
//!
//! A `GetSecurityGroup` whose `groupId` resolves to no security group of the `networkId` it
//! was asked under is refused with the status the model declares for `UnauthorizedError`
//! (`@httpError(401)`) and this API's fixed refusal sentence for such a read
//! ([`crate::state::SECURITY_GROUP_READ_REFUSED_MESSAGE`]) — and with NO error-shape
//! discriminator, neither the `x-amzn-errortype` header `restJson1` resolves an error shape
//! from nor a `__type` / `code` / `Code` member in the body, so a client classifies it by
//! status.
//!
//! This module states that ONE fact of this API's wire: the status that refusal travels at.
//! The wire form itself — dropping the shape from both places, and nothing else — is
//! [`crate::service::untyped_refusal`], and which operation it fronts is stated in
//! [`crate::service`]. `GetSecurityGroup`'s handler answers this status for exactly one
//! reason: the group the request named is not there ([`crate::ops::get_security_group`]).
//!
//! It reaches that one operation alone: every other refusal of this API keeps the shape its
//! renderer named it with — a network read denies a `networkId` it cannot place with a typed
//! `ForbiddenError`, at the status that shape declares.

/// The status the refusal of an unresolvable security-group read carries: the one the model
/// declares for `UnauthorizedError` (`@httpError(401)`), which is the only reason a security
/// group read answers it.
pub const REFUSED_SECURITY_GROUP_READ_STATUS: u16 = 401;
