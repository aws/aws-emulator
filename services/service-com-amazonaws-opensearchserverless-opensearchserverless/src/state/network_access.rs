// SPDX-License-Identifier: CC0-1.0

//! Whether a collection's own endpoints are reachable at all: the `network` security
//! policies that admit traffic to them.
//!
//! A `network` security policy is what "specifies access to collections and their
//! endpoints" (`SecurityPolicyType$network`), and AWS's "Network access for Amazon
//! OpenSearch Serverless" states the rule a collection's endpoints are reached under: the
//! endpoints a collection publishes are reachable only through a network policy whose
//! rules name that collection — "you must have at least one network policy that applies
//! to a collection in order to access its endpoint". A collection no network policy covers
//! publishes an address nothing is admitted to, whichever principal dials it and whatever
//! the data access policies say, which is why this decision is taken before any other.
//!
//! The DOCUMENT is read the same way every other policy document of this service is: a
//! statement, or a list of them, each carrying `Rules` whose entries name a `ResourceType`
//! and the `Resource` patterns they apply to ([`super::policy_document_resources`] walks
//! the same three levels). So the question here is whether a stored policy's rules cover
//! `collection/<name>` — the same covering relation an encryption lookup
//! ([`super::ComAmazonawsOpensearchserverlessOpensearchserverlessState::policy_encryption_for`])
//! is answered by, and nothing is re-implemented for it.
//!
//! What this does NOT decide is which SOURCE a covered collection admits. A policy states
//! its sources as `AllowFromPublic: true`, or as `SourceVPCEs` / `SourceServices`, and
//! every client of this emulator dials the one address this process serves the collection
//! at ([`super::collection_plane`]) — there is no network for a request to arrive from, so
//! no stored source can be compared against one. A covered collection therefore admits the
//! request, and the boundary this emulator can observe is the one the rule above states:
//! covered, or not covered at all.

use super::ComAmazonawsOpensearchserverlessOpensearchserverlessState as ServiceState;

/// The `SecurityPolicyType` whose policies say which traffic reaches a collection's
/// endpoints (`smithy_query(query="shape", shape="SecurityPolicyType")` -> `network`).
pub const SECURITY_POLICY_TYPE_NETWORK: &str = "network";

impl ServiceState {
    /// Whether a stored `network` security policy covers the collection `name`, which is
    /// what admits traffic to the endpoints that collection publishes.
    ///
    /// `false` for a collection no such policy names: every request to its endpoints is
    /// refused, which is what this service's data plane answers for one.
    pub fn network_policy_admits(&self, name: &str) -> bool {
        let resource = format!("{}/{name}", super::POLICY_COLLECTION_RESOURCE_TYPE);
        self.security_policies
            .iter()
            .filter(|((kind, _), _)| kind == SECURITY_POLICY_TYPE_NETWORK)
            .any(|(_, record)| super::covering_statement(&record.policy, &resource).is_some())
    }
}
