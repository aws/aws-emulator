// SPDX-License-Identifier: CC0-1.0

//! WHICH identities a QuickSight account manages.
//!
//! An account's authentication method is what decides where its users come from
//! (`AccountInfo$AuthenticationType`, from the `AuthenticationMethod` its sign-up chose):
//! an account authenticating its callers through Amazon Web Services identities alone, an
//! Active Directory or IAM Identity Center takes its users from there and holds no
//! QuickSight identity store of its own, so no QuickSight-managed user can be registered
//! in it. Only the method that holds QuickSight identities beside the Amazon Web Services
//! ones ([`AUTHENTICATION_METHOD_IAM_AND_QUICKSIGHT`]) gives an account a store of its
//! own.
//!
//! A registration of an identity the account does not manage is refused with the
//! modelled `AccessDeniedException` ([`super::authorization::access_denied`]).

use super::*;

impl ComAmazonawsQuicksightQuicksight20180401State {
    /// Whether `aws_account_id`'s QuickSight account manages QuickSight identities of its
    /// own — the identities an `IdentityType` of `QUICKSIGHT` names.
    ///
    /// Read from the account's own subscription ([`Self::account_info`]), so an account
    /// that signed up through this service is judged on the method it chose and one that
    /// never did on the service default ([`DEFAULT_AUTHENTICATION_METHOD`]).
    pub fn manages_quicksight_users(&self, aws_account_id: &str) -> bool {
        self.account_info(aws_account_id)
            .authentication_type
            .as_deref()
            == Some(AUTHENTICATION_METHOD_IAM_AND_QUICKSIGHT)
    }
}
