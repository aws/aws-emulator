// SPDX-License-Identifier: CC0-1.0

//! Amazon Cognito Federated Identities' published default-quota catalog.

use super::*;

/// One Amazon Cognito Federated Identities API REQUEST-RATE quota: the requests per
/// second that identity-pool operation is served at.
///
/// Every member of such a row but the operation's own name and the value is the same
/// across the class — the `QuotaName` is `Rate of <Api> requests` and the `Description`
/// is the sentence the service publishes for the class with the operation named in it —
/// so one constructor fills them and two rows of the class cannot drift apart.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-identity` service code,
/// and the recorded default-quota listing of that code for which `QuotaCode` names which
/// operation's rate and for the wording. The rate's window is stated in the quota's own
/// name and description, and the rows of this service carry NO `Period` or `UsageMetric`
/// block, which is why none is set here.
fn cognito_identity_request_rate(code: &str, api_name: &str, requests_per_second: f64) -> QuotaRow {
    quota(format!("Rate of {api_name} requests"), requests_per_second)
        .code(code)
        .describe(&format!(
            "The maximum number of {api_name} requests that you can make per second."
        ))
}

/// Amazon Cognito Federated Identities' published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-identity` service code,
/// and the recorded default-quota listing of that code for which `QuotaCode` names which
/// operation's rate and at how many requests per second it is served
/// ([`cognito_identity_request_rate`]): this service publishes a request-rate quota per
/// identity-pool API operation, and beside those the COUNTS and SIZES of an identity pool
/// and of the account that holds it, for which that listing states the published name,
/// text, unit, value and adjustability: the user pool providers and the access-control
/// rules of one identity pool (the first being the quota the identity service enforces as
/// `MAX_USER_POOL_PROVIDERS_PER_IDENTITY_POOL`,
/// `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/state/pool_configuration.rs`),
/// the identity pools of the account, the page a list or lookup call answers with, and the
/// size the two names an identity pool is configured with may reach. A count is not a
/// rate, so none of those rows carries a window or a usage metric; a size is not a count,
/// so those rows carry the unit AWS measures them in rather than `None`. Each carries the
/// service's own published sentence verbatim, a row carrying the quota's own published
/// text or none.
///
/// This catalog publishes a row for every `QuotaCode` that listing reports for this
/// service code. A code whose quota no source available here states would be left to the
/// default-quota-set gap rather than published under a binding this catalog would be
/// guessing at (see [`crate::state::descriptive_quota_code`]), since a code identifies
/// ONE quota.
pub(in crate::state) fn cognito_identity_quotas() -> Vec<QuotaRow> {
    vec![
        cognito_identity_request_rate("L-0BF99E8D", "GetOpenIdTokenForDeveloperIdentity", 50.0),
        cognito_identity_request_rate("L-0D31D6D2", "UntagResource", 5.0),
        quota("User pool providers per identity pool", 50.0)
            .code("L-14403AD5")
            .describe(
                "The maximum number of Amazon Cognito user pool providers per identity pool.",
            ),
        // The rules of role-based access control, which AWS publishes as a hard limit.
        quota("Role-based access control rules", 25.0)
            .code("L-1F1BC060")
            .fixed()
            .describe("The maximum number of rules for role-based access control (RBAC)"),
        // The size an identity pool's own name may reach, published in bytes and as a hard
        // limit. SOURCE for the 128: the identity service's `IdentityPoolName`, whose
        // modelled length is what a pool name may reach
        // (`services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model`).
        quota("Identity pool name size", 128.0)
            .code("L-29CA570C")
            .unit("Bytes")
            .fixed()
            .describe("The maximum size of an identity pool name in bytes."),
        cognito_identity_request_rate("L-50C9AAB8", "ListTagsForResource", 10.0),
        cognito_identity_request_rate("L-5AE9F13D", "GetCredentialsForIdentity", 200.0),
        quota("Identity pools per account", 1000.0)
            .code("L-8692CE1C")
            .describe("The maximum number of identity pools per account."),
        cognito_identity_request_rate("L-A38ADA84", "GetId", 25.0),
        // The size a login provider's name may reach, published in bytes and as a hard
        // limit.
        quota("Login provider name size", 2048.0)
            .code("L-B08F14B9")
            .unit("Bytes")
            .fixed()
            .describe("The maximum size of a login provider name in bytes."),
        cognito_identity_request_rate("L-DAF3EAFF", "TagResource", 5.0),
        cognito_identity_request_rate("L-DBF99350", "ListIdentities", 5.0),
        cognito_identity_request_rate("L-E97D135E", "GetOpenIdToken", 200.0),
        // The page a list or lookup call answers with, which AWS publishes as a hard
        // limit of the API rather than a quota an account can have raised.
        quota("List API call results", 60.0)
            .code("L-EAF710D2")
            .fixed()
            .describe("The maximum number of results from a list or lookup API call."),
    ]
}
