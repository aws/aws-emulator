// SPDX-License-Identifier: CC0-1.0

//! WHAT a call this account does not authorize reads.
//!
//! One template, so every handler that refuses the same error class renders the same
//! sentence and a client reads the same text whichever operation it called — the reason
//! the conflict and not-found wordings live together in [`super::refusals`].
//!
//! The modelled `AccessDeniedException` (HTTP 401) is the refusal of a call the account
//! does not authorize, and the model words that class itself: "You don't have access to
//! this item. The provided credentials couldn't be validated. You might not be authorized
//! to carry out the request. Make sure that your account is authorized to use the Amazon
//! Quick Sight service, that your policies have the correct permissions, and that you are
//! using the correct credentials." The sentence carries no value of its own: what the
//! caller may do is a fact about the account and its credentials, not about anything the
//! request named.

use super::*;

/// The modelled `AccessDeniedException` a call the account does not authorize carries.
pub fn access_denied(request_id: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(ACCESS_DENIED_MESSAGE.to_string()),
        request_id: Some(request_id.to_string()),
    }
}

/// The sentence [`access_denied`] carries, as the model words this error class.
pub const ACCESS_DENIED_MESSAGE: &str = "You don't have access to this item. The provided \
     credentials couldn't be validated. You might not be authorized to carry out the \
     request. Make sure that your account is authorized to use the Amazon Quick Sight \
     service, that your policies have the correct permissions, and that you are using \
     the correct credentials.";
