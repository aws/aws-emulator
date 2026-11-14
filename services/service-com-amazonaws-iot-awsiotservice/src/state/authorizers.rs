// SPDX-License-Identifier: CC0-1.0

//! When an authorizer of this account may be DELETED, and how a delete that comes too
//! early is refused.
//!
//! A custom authorizer is a resource this control plane keeps a STATUS on
//! (`AuthorizerDescription$status`, the modelled `AuthorizerStatus` enum `ACTIVE |
//! INACTIVE` — `smithy_query(query="shape",shape="AuthorizerStatus")`), and the status
//! says whether the authorizer is still in service: an `ACTIVE` one authorizes
//! connections, an `INACTIVE` one authorizes nothing. A resource that is still in
//! service cannot be torn out from under the clients using it, so the delete is
//! admissible only once the status says it serves nobody — the same precondition this
//! service enforces on a certificate, which "cannot be deleted if … its status is set to
//! ACTIVE" (`DeleteCertificate`) — and the way to get there is `UpdateAuthorizer`, the
//! operation that carries the `status` member.
//!
//! So the two refusals a delete can state are distinguishable, and neither of them is a
//! success that removed nothing: a name the account holds NO authorizer under is the
//! modelled not-found ([`super::missing_authorizer_message`]), while an authorizer the
//! account DOES hold and has not deactivated is the modelled `InvalidRequestException`
//! ([`undeletable_authorizer_message`]) — which leaves the record exactly as it was, so
//! the caller can read it, deactivate it and delete it.

/// The `status` an authorizer must carry before it can be deleted.
///
/// One of the two values `AuthorizerStatus` admits — the other is
/// [`super::DEFAULT_AUTHORIZER_STATUS`], the status a create that asks for none yields —
/// and the one that means the authorizer authorizes nothing.
pub const INACTIVE_AUTHORIZER_STATUS: &str = "INACTIVE";

/// How this service refuses the delete of an authorizer the account still has in
/// service.
///
/// The fixed text is the class's, and the variable parts come from the request and from
/// the record the account holds: the name the request addressed, and the status that
/// record carries. The sentence names the status to move the authorizer to, because
/// `UpdateAuthorizer` is the operation that gets it there.
pub fn undeletable_authorizer_message(authorizer_name: &str, status: &str) -> String {
    format!(
        "Cannot delete authorizer {authorizer_name} in {status} status. \
         Update status to {INACTIVE_AUTHORIZER_STATUS} and delete."
    )
}

/// How this service refuses a create under a name the account already holds an
/// authorizer under.
///
/// The fixed text is the family's — the name is the authorizer's identity, and an
/// account holds one authorizer per name — and the variable part is the name the request
/// addressed.
pub fn existing_authorizer_message(authorizer_name: &str) -> String {
    format!("Authorizer {authorizer_name} already exist for this account")
}

impl super::RegisteredAuthorizer {
    /// Whether this authorizer is out of service — its persisted status is
    /// [`INACTIVE_AUTHORIZER_STATUS`] — and so may be deleted.
    ///
    /// Read from the record rather than from the create request, so an authorizer
    /// created `ACTIVE` and later updated to `INACTIVE` is deletable and one created
    /// `INACTIVE` and later updated back is not.
    pub fn is_deactivated(&self) -> bool {
        self.status == INACTIVE_AUTHORIZER_STATUS
    }
}
