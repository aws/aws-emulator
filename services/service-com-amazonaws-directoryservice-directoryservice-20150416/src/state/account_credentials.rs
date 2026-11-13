// SPDX-License-Identifier: CC0-1.0

//! What a directory holds in place of an account's password, and how a credentialed call is
//! authenticated against it.
//!
//! An operation of this service that carries a `UserName`/`Password` pair asks the service
//! to act on that ACCOUNT's behalf — `EnableSso` and `DisableSso` are the two — so the
//! service authenticates the pair against the directory before it acts, and a pair the
//! directory does not hold is the modelled `AuthenticationFailedException`. Deciding that
//! needs something to check the presented password against.
//!
//! What is held is a VERIFIER, never the password. `CreateDirectoryRequest$Password`,
//! `CreateMicrosoftADRequest$Password` and `ResetUserPasswordRequest$NewPassword` are all
//! `@sensitive` and no operation reports one back, so a store that held the password itself
//! would be holding a secret this emulator never has to reproduce — and every reader of its
//! state would hold it too. A digest answers the only question the service asks of it ("is
//! this the password that account's password was set to?"), and the password cannot be read
//! back out of it. This is the same shape a domain controller keeps: the account's password
//! hash, not its password.
//!
//! The digest is domain-separated by the account name the password was set for
//! ([`Directory::user_key`], so the same fold an account lookup uses), which is what keeps
//! one account's verifier from admitting another's password.

use super::*;

use ::sha2::{Digest, Sha256};

/// What a directory holds for one account instead of its password: a value a presented
/// password can be CHECKED against and that the password cannot be read back out of.
///
/// Compared as a whole ([`Self::admits`]) rather than inspected: nothing outside this type
/// reads the digest, and no operation reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordVerifier(String);

impl PasswordVerifier {
    /// The verifier for `password` as the password of the account `user_name`.
    ///
    /// SHA-256 over the account name and the password, with a separator between them, so a
    /// name and password that run together cannot collide with another pair; the account
    /// name is folded exactly as an account LOOKUP folds it, so a verifier made for `Admin`
    /// is the one checked for `ADMIN`.
    #[must_use]
    pub fn of(user_name: &str, password: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(Directory::user_key(user_name).as_bytes());
        digest.update([0]);
        digest.update(password.as_bytes());
        Self(::hex::encode(digest.finalize()))
    }

    /// Whether `password` is the password this verifier was made from, presented for the
    /// account `user_name`.
    #[must_use]
    pub fn admits(&self, user_name: &str, password: &str) -> bool {
        *self == Self::of(user_name, password)
    }
}

/// Why the credential pair `user_name`/`password` is not an account of `directory` — the
/// sentence the modelled `AuthenticationFailedException` carries — or `None` when the
/// directory authenticates it.
///
/// THE one place this decision is made, so the operations that take such a pair
/// (`EnableSso`, `DisableSso`) cannot authenticate differently.
///
/// The rule: a pair is admitted when the directory holds an account of that name
/// ([`Directory::user`], the same case-insensitive fold every account lookup uses) whose
/// password was set to the presented password — the password its create was called with,
/// or the one `ResetUserPassword` has since set. Everything else is a pair the directory
/// does not hold:
///
/// * a name no account of the directory is held under — including every name on a directory
///   that holds no account of its own, which is what an AD Connector is: the accounts it
///   serves live in the self-managed directory it connects to, and this service holds none
///   of them;
/// * the right account with another password;
/// * HALF a pair, which names no account to authenticate: a password with no account to
///   present it for, or an account with no password presented for it.
///
/// The diagnostic is [`AUTHENTICATION_FAILED`] for every one of them, so the refusal reports
/// which half was wrong to nobody — neither member is echoed, and the two cases a caller
/// could otherwise tell apart (an account that exists and one that does not) read the same.
#[must_use]
pub fn credential_defect(
    directory: &Directory,
    user_name: Option<&str>,
    password: Option<&str>,
) -> Option<&'static str> {
    let (Some(user_name), Some(password)) = (user_name, password) else {
        return Some(AUTHENTICATION_FAILED);
    };
    let authenticated = directory.user(user_name).is_some_and(|account| {
        account
            .password
            .as_ref()
            .is_some_and(|verifier| verifier.admits(user_name, password))
    });
    (!authenticated).then_some(AUTHENTICATION_FAILED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a verifier admits the password it was made from, presented for the account it
    /// was made for, and nothing else.
    #[test]
    fn a_verifier_admits_one_password_of_one_account() {
        let verifier = PasswordVerifier::of("Admin", "Sim2Real-Pw1!");

        // The account name is folded the way a lookup folds it, so every spelling of the
        // name presents the same account.
        for spelling in ["Admin", "admin", "ADMIN", "aDmIn"] {
            assert!(
                verifier.admits(spelling, "Sim2Real-Pw1!"),
                "case {spelling:?}: expected the password to be admitted"
            );
        }

        // Another password, another account, and a pair that only runs together the same
        // way are each refused.
        for (case, user_name, password) in [
            ("another password", "Admin", "Sim2Real-Pw2!"),
            ("the password one character short", "Admin", "Sim2Real-Pw1"),
            ("another account", "Administrator", "Sim2Real-Pw1!"),
            ("a name that absorbs the password", "AdminSim2Real-Pw1!", ""),
            ("an empty password", "Admin", ""),
        ] {
            assert!(
                !verifier.admits(user_name, password),
                "case {case:?}: expected {user_name:?} / {password:?} to be refused"
            );
        }
    }

    /// RULE: the verifier is not the password — the password cannot be read out of it, and
    /// two accounts with the same password hold different verifiers.
    #[test]
    fn a_verifier_holds_no_password() {
        let password = "Sim2Real-Pw1!";
        let verifier = PasswordVerifier::of("Admin", password);
        let rendered = format!("{verifier:?}");
        assert!(
            !rendered.contains(password),
            "expected the verifier not to carry the password, got {rendered}"
        );
        assert_ne!(
            PasswordVerifier::of("Admin", password),
            PasswordVerifier::of("Administrator", password),
            "expected two accounts' verifiers to differ for one password"
        );
    }
}
