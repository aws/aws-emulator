// SPDX-License-Identifier: CC0-1.0

//! What IAM STORES for a role's trust policy, and so what every read of that role reports.
//!
//! A trust policy — the `AssumeRolePolicyDocument` a role carries — is a policy document a caller
//! hands over as a string, and IAM does not keep the caller's bytes: it RENDERS the policy and
//! holds that rendering, which is the one rendering every service with a policy store in this
//! emulator reports ([`::service_common::iam_policy_rendering::rendered_rewriting`]) —
//!
//! * the whitespace BETWEEN the document's JSON tokens is gone, so a document written with a
//!   space after each `:` and `,` is reported without them;
//! * an element the policy language types as "String or Array of strings" (a statement's
//!   `Action`/`Resource` and the principal lists) that arrived as a one-element array of one
//!   string is reported as that string; and
//! * an AWS ACCOUNT principal named by the account's id alone is reported as the ARN of that
//!   account's root user, `arn:<partition>:iam::<account-id>:root` — which is why a client that
//!   compares a role's trust principal with the account root (a `PassRole` gate, a policy diff)
//!   sees the root ARN whichever of the two spellings the document was written in.
//!
//! Everything else is the caller's own data and survives: the member order, the statement count,
//! the one-element `Statement` array, the type and spelling of every other value, and every byte
//! inside a string literal. Applying the rendering to what it produced returns it unchanged, so a
//! client that puts back the document it read stores the same text again.
//!
//! The rule is about SAVING a policy, so it belongs to the writers of the member — `CreateRole`,
//! `UpdateAssumeRolePolicy` and `AcquireRole`, which renders one from a role template — and the
//! readers (`GetRole`, `ListRoles`, `GetAccountAuthorizationDetails`) report the stored text,
//! percent-encoded as every response carries a policy document
//! ([`super::policy_document_reports`]). A trust document IAM itself writes for a
//! service-linked role is not a caller's document and is held as the service writes it.

/// The key a policy's `Principal`/`NotPrincipal` element names an AWS ACCOUNT principal under
/// (`::service_common::iam_policy_rendering::PRINCIPAL_ELEMENTS` lists the four keys such an
/// element takes; this is the one that names an account, a user or a role).
const ACCOUNT_PRINCIPAL: &str = "AWS";

/// The digits an AWS account id carries: every account id is exactly twelve of them, which is
/// the one form the account's own identifier takes.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The trust policy IAM stores for `document`: the policy RENDERED — the whitespace between its
/// JSON tokens dropped, a one-element string-or-array element written as its string, and every
/// ACCOUNT principal named by an account's id alone written as that account's root-user ARN.
///
/// Applying this to what it produced returns it unchanged, so a client that puts back the
/// document it read stores the same text again.
#[must_use]
pub fn stored_trust_policy(document: &str) -> String {
    ::service_common::iam_policy_rendering::rendered_rewriting(
        document,
        &::service_common::iam_policy_rendering::ElementValues {
            element: ACCOUNT_PRINCIPAL,
            written_as: &account_root_arn_of,
        },
    )
}

/// The root-user ARN of the account `value` names by its id alone, or `None` for a value that is
/// not an account id — an ARN (a root ARN included, which is what makes the rewrite a fixed
/// point), the `*` wildcard, or anything else, each of which is stored as it was written.
///
/// The PARTITION is the one every ARN this emulator mints carries, read off the account root ARN
/// the crate already builds ([`super::account_root_arn`]) rather than written here, so an account
/// is named in the same partition as the roles that trust it.
fn account_root_arn_of(value: &str) -> Option<String> {
    if value.len() != ACCOUNT_ID_DIGITS || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let root = super::account_root_arn();
    let partition = ::service_common::arn::parse(&root).map(|arn| arn.partition.to_string())?;
    Some(format!("arn:{partition}:iam::{value}:root"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the stored trust policy is the policy RENDERED — the whitespace between its JSON
    /// tokens gone, a one-element string-or-array element written as its string, and an account
    /// principal named by the account's id alone written as that account's root-user ARN.
    #[test]
    fn a_submitted_trust_policy_is_stored_rendered() {
        // The rows differ along what the rule generalises over: which account is named (this
        // emulator's own and another), where the principal sits (alone, in a list, under the
        // complement element), the whitespace of the submitted text, the list form of a
        // one-element value, and the values the rule does not touch (an ARN, a wildcard, a
        // service principal, an id under another element).
        let rows: [(&str, &str, &str); 10] = [
            (
                "this emulator's own account, named by its id",
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#,
            ),
            (
                "another account, named by its id",
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"sts:AssumeRole"}]}"#,
            ),
            (
                "a root ARN already, which is what the rule writes",
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "a list of account ids keeps its list form",
                r#"{"Statement":[{"Principal":{"AWS":["210987654321","123456789012"]}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:root","arn:aws:iam::123456789012:root"]}}]}"#,
            ),
            (
                "a one-element list is written as the string it holds",
                r#"{"Statement":[{"Principal":{"AWS":["210987654321"]}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "a one-element action list likewise",
                r#"{"Statement":[{"Action":["sts:AssumeRole"],"Principal":{"Service":"ec2.amazonaws.com"}}]}"#,
                r#"{"Statement":[{"Action":"sts:AssumeRole","Principal":{"Service":"ec2.amazonaws.com"}}]}"#,
            ),
            (
                "the complement element names one too",
                r#"{"Statement":[{"NotPrincipal":{"AWS":"210987654321"}}]}"#,
                r#"{"Statement":[{"NotPrincipal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "an indented document is stored without the whitespace between its tokens",
                "{\r\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Principal\": {\"AWS\": \"210987654321\"}\n\t\t}\n\t]\n}",
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "a role ARN, a wildcard and a service principal are stored as written",
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:role/r","*"],"Service":"ec2.amazonaws.com"}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:role/r","*"],"Service":"ec2.amazonaws.com"}}]}"#,
            ),
            (
                "an account id under another element is not a principal",
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
            ),
        ];

        for (case, submitted, stored) in rows {
            let actual = stored_trust_policy(submitted);
            assert_eq!(
                actual, stored,
                "case {case:?}: expected {stored:?}, got {actual:?}"
            );
            let again = stored_trust_policy(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: the stored form must be a fixed point, got {again:?}"
            );
        }
    }

    /// RULE: only a value that is an account id — twelve decimal digits and nothing else — is
    /// written as a root ARN.
    #[test]
    fn only_a_twelve_digit_id_names_an_account() {
        // Rows: the published length, one digit either side of it, the empty value, a
        // non-decimal character inside one, and a padded one.
        let rows: [(&str, &str, bool); 7] = [
            ("twelve digits", "210987654321", true),
            ("eleven digits", "21098765432", false),
            ("thirteen digits", "2109876543210", false),
            ("no digits at all", "", false),
            ("a letter among the digits", "21098765432a", false),
            ("a space among the digits", "21098765432 ", false),
            ("twelve zeroes", "000000000000", true),
        ];

        for (case, value, names_an_account) in rows {
            let actual = account_root_arn_of(value);
            assert_eq!(
                actual.is_some(),
                names_an_account,
                "case {case:?}: expected an account ARN? {names_an_account}, got {actual:?}"
            );
            if names_an_account {
                let root = crate::state::account_root_arn();
                let partition = ::service_common::arn::parse(&root)
                    .expect("the account root ARN parses")
                    .partition
                    .to_string();
                let expected = format!("arn:{partition}:iam::{value}:root");
                assert_eq!(
                    actual.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
