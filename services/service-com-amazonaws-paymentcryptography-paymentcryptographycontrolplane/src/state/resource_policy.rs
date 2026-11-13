// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a key carries: which documents this service attaches to one,
//! why it refuses the rest, and the layout an attached document is reported in.
//!
//! One concern: everything `PutResourcePolicy` has to decide about a document before it is
//! stored, so `GetResourcePolicy` reports exactly what some accepted call attached and
//! nothing a reader has to re-judge.
//!
//! The IAM JSON policy language itself is read once for the whole workspace
//! ([`::service_common::iam_policy`] for a document's grammar and its canonical layout,
//! [`::service_common::public_resource_policy`] for whom its grants reach), so this module
//! holds only what is this service's own: which refusal each clause earns.

/// How this service words the refusal of a document that would open a key to everybody:
/// the sentence AWS answers a public resource policy with, entirely FIXED text that names
/// neither the key nor anything of the submitted document, as the modelled
/// `PublicPolicyException` describes it ("the resource-based policy would grant public
/// access to the key", `smithy_query(query="shape",shape="PublicPolicyException")`).
pub const PUBLIC_POLICY_MESSAGE: &str =
    "The resource policy must not grant public access to the resource.";

/// The `Policy` a read reports for a key no call has attached a policy to: the empty JSON
/// object.
///
/// A key with no resource-based policy grants nothing through one, and the read answers
/// that rather than refusing — `GetResourcePolicy` models no failure for a key that exists,
/// and the recorded response of that read on such a key carries exactly this document. It
/// is not a document a caller can ATTACH ([`attachable_policy`] refuses it, since it
/// declares no statement): it is what "no policy" is reported as.
pub const NO_POLICY_DOCUMENT: &str = "{}";

/// Why this service will not attach a resource-based policy a request carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRejection {
    /// The text is not a policy document this service would attach, worded with the
    /// sentence AWS states for the clause it broke.
    Malformed(String),
    /// The document is a policy, and it would grant public access to the key:
    /// [`PUBLIC_POLICY_MESSAGE`], the modelled `PublicPolicyException`.
    Public,
}

/// The document to store for a policy a request asks to attach, or why it is refused.
///
/// Three readings, in the order a caller earns each refusal:
///
/// * the text is a policy document the attaching validator accepts — a JSON object whose
///   member names the policy language defines, carrying a statement element and a version
///   string it defines, naming only actions the language can read
///   ([`::service_common::iam_policy::attached_policy_defect`], whose variants carry AWS's
///   own sentence per clause);
/// * every statement is a RESOURCE-policy statement: it states an effect, names an action
///   and — this being the element that says who the key is opened to — names a principal
///   ([`::service_common::iam_policy::resource_policy_defect`]);
/// * the grants it then makes do not reach the public, which is the one refusal this
///   operation models of its own ("if the policy would grant public access, the request
///   fails with a `PublicPolicyException`", `PutResourcePolicy`'s own documentation).
///
/// Nothing is written by judging, so a refused document leaves the key the policy it had.
/// The answer is the document in the layout an attached policy is REPORTED in
/// ([`::service_common::iam_policy::canonical_document`]): the parsed document
/// re-serialized rather than the caller's bytes echoed, so one key's policy reads the same
/// whatever whitespace it was submitted with.
pub fn attachable_policy(policy: &str) -> Result<String, PolicyRejection> {
    if let Some(defect) = ::service_common::iam_policy::attached_policy_defect(
        policy,
        ::service_common::iam_policy::readable_action,
    ) {
        return Err(PolicyRejection::Malformed(defect.message()));
    }
    if let Some(defect) = ::service_common::iam_policy::resource_policy_defect(policy) {
        return Err(PolicyRejection::Malformed(format!(
            "The resource-based policy is not a valid policy document: {defect}."
        )));
    }
    if ::service_common::public_resource_policy::grants_public_access(policy) {
        return Err(PolicyRejection::Public);
    }
    Ok(::service_common::iam_policy::canonical_document(policy))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-statement document carrying `statement`.
    fn policy(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    /// A statement granting one named account the use of the key.
    const NAMED_ACCOUNT_STATEMENT: &str =
        "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
         \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Resource\":\"*\"}";

    #[test]
    fn an_acceptable_document_is_stored_in_the_reported_layout() {
        let submitted = policy(NAMED_ACCOUNT_STATEMENT);
        let stored = attachable_policy(&submitted).expect("a cross-account grant is attached");
        assert_eq!(
            stored,
            ::service_common::iam_policy::canonical_document(&submitted),
            "an attached document is stored in the one reported layout, got {stored}"
        );
    }

    #[test]
    fn each_clause_earns_its_own_refusal() {
        // The rows differ along what the reading generalises over: which clause the
        // document breaks, and whether it is a policy at all.
        let rows: [(&str, String, Option<PolicyRejection>); 7] = [
            ("a cross-account grant", policy(NAMED_ACCOUNT_STATEMENT), None),
            (
                "a wildcard principal",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                     \"Principal\":\"*\",\"Resource\":\"*\"}",
                ),
                Some(PolicyRejection::Public),
            ),
            (
                "text that is not JSON",
                "not a policy".to_string(),
                Some(PolicyRejection::Malformed(
                    ::service_common::iam_policy::NOT_A_JSON_POLICY_MESSAGE.to_string(),
                )),
            ),
            (
                "no version string",
                format!("{{\"Statement\":[{NAMED_ACCOUNT_STATEMENT}]}}"),
                Some(PolicyRejection::Malformed(
                    ::service_common::iam_policy::INVALID_POLICY_VERSION_MESSAGE.to_string(),
                )),
            ),
            (
                "an element the language does not define",
                format!(
                    "{{\"Version\":\"2012-10-17\",\"Statements\":[{NAMED_ACCOUNT_STATEMENT}]}}"
                ),
                Some(PolicyRejection::Malformed(
                    "Unknown field Statements".to_string(),
                )),
            ),
            (
                "an action the language cannot read",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"decrypt data\",\
                     \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}",
                ),
                Some(PolicyRejection::Malformed(
                    ::service_common::iam_policy::POLICY_INVALID_ACTION_MESSAGE.to_string(),
                )),
            ),
            (
                "a statement naming no principal",
                policy(
                    "{\"Effect\":\"Allow\",\"Action\":\"payment-cryptography:DecryptData\",\
                     \"Resource\":\"*\"}",
                ),
                Some(PolicyRejection::Malformed(
                    "The resource-based policy is not a valid policy document: a statement \
                     names no Principal or NotPrincipal."
                        .to_string(),
                )),
            ),
        ];

        for (case, document, expected) in rows {
            let actual = attachable_policy(&document).err();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?} for {document}"
            );
        }
    }
}
