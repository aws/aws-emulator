// SPDX-License-Identifier: CC0-1.0

//! Who may act on a collection's DATA: the `data` access policies, and the decision one
//! of them gives for a principal, a resource and a permission.
//!
//! A data access policy is what "specifies permissions for collections and indexes"
//! (`AccessPolicyType$data`), and AWS's "Data access control for Amazon OpenSearch
//! Serverless" states the rule: a policy's statement names the `Principal` entries it is
//! about and `Rules` whose entries name a `ResourceType` (`collection` or `index`), the
//! `Resource` patterns they apply to (`collection/<name>`, `index/<collection>/<index>`,
//! each with a trailing wildcard allowed) and the `Permission` entries they grant
//! (`aoss:CreateIndex`, `aoss:ReadDocument`, `aoss:*`). A principal may perform an action
//! on a resource when some stored policy grants it; with none, nothing is permitted —
//! "data access policies don't apply to a collection until you create a policy that
//! matches it".
//!
//! That decision is asked by two planes of this emulator, which is why it lives here and
//! not in either of them:
//!
//! * the collection's own REST data plane, for the principal a request is signed by
//!   ([`crate::service::collection_rest`]);
//! * a service that resolves a vector store it is given — Amazon Bedrock's knowledge
//!   base, whose `roleArn` must be able to read the index its `storageConfiguration`
//!   names — asking about a principal that is not the caller at all.
//!
//! The DOCUMENT is read the same way every other policy document of this service is:
//! [`super::policy_document_entries`] walks a statement, or a list of them, and the list
//! spelling and the single-value spelling of `Principal`, `Rules`, `Resource` and
//! `Permission` alike. A pattern is matched by this service's own resource matcher
//! ([`super::resource_pattern_matches`] through [`super::resource_patterns_overlap`]), so
//! the resources a grant covers are the ones a listing's `resource` filter selects on.
//!
//! A `Principal` is compared LITERALLY, as IAM compares one: the grammar gives that
//! element no partial wildcards, only the whole-element `*`. Which spellings name one
//! caller is the asking plane's to decide — an assumed-role session is named by the role
//! it is a session of as well as by its own ARN — so a list of them is passed in and any
//! one matching is the grant.

use ::aws_smithy_types::Document;

use super::ComAmazonawsOpensearchserverlessOpensearchserverlessState as ServiceState;

/// The `AccessPolicyType` whose policies say who may act on a collection's data
/// (`smithy_query(query="shape", shape="AccessPolicyType")` -> `data`).
pub const ACCESS_POLICY_TYPE_DATA: &str = "data";

/// The `ResourceType` a data access policy rule carries for a rule about INDEXES, and the
/// prefix its `Resource` patterns are written with: AWS's data-access documentation spells
/// those patterns `index/<collection>/<index>`.
pub const POLICY_INDEX_RESOURCE_TYPE: &str = "index";

/// The whole-element wildcard a `Principal` may be written as, which names every
/// principal.
const ANY_PRINCIPAL: &str = "*";

/// The permission a data access policy grants for one action on a collection or an index.
///
/// Source: the permissions AWS's "Supported policy permissions" table publishes for a data
/// access policy — the collection-level `aoss:*CollectionItems` permissions and the
/// index-level ones beside them. They are named here because a policy document states them
/// as text and no shape of this service's model carries them.
pub mod permission {
    /// Create an index in the collection (`PUT /<index>`).
    pub const CREATE_INDEX: &str = "aoss:CreateIndex";
    /// Read an index's own definition — its mappings and settings.
    pub const DESCRIBE_INDEX: &str = "aoss:DescribeIndex";
    /// Change an index's mappings or settings.
    pub const UPDATE_INDEX: &str = "aoss:UpdateIndex";
    /// Delete an index.
    pub const DELETE_INDEX: &str = "aoss:DeleteIndex";
    /// Read documents of an index (a get, a search, a count).
    pub const READ_DOCUMENT: &str = "aoss:ReadDocument";
    /// Write documents into an index (an index request, a bulk write, a delete).
    pub const WRITE_DOCUMENT: &str = "aoss:WriteDocument";
    /// Read the collection itself — what the cluster is.
    pub const DESCRIBE_COLLECTION_ITEMS: &str = "aoss:DescribeCollectionItems";
}

/// The `Resource` pattern an index of one collection is named by in a data access policy.
#[must_use]
pub fn index_resource(collection: &str, index: &str) -> String {
    format!("{POLICY_INDEX_RESOURCE_TYPE}/{collection}/{index}")
}

/// The `Resource` pattern a collection itself is named by in a data access policy.
#[must_use]
pub fn collection_resource(collection: &str) -> String {
    format!("{}/{collection}", super::POLICY_COLLECTION_RESOURCE_TYPE)
}

impl ServiceState {
    /// Whether a stored `data` access policy grants one of `principals` the permission
    /// `permission` on `resource` — the one data-access decision this service takes.
    ///
    /// `resource` is a pattern of the form a rule names ([`index_resource`],
    /// [`collection_resource`]); a stored rule covers it when the two patterns denote
    /// overlapping resource sets, so `index/<collection>/*` grants the whole of a
    /// collection's indexes. `false` when no policy grants it, which is what IAM's own
    /// default deny and the data-access rule above both come to.
    pub fn data_access_grants(
        &self,
        principals: &[String],
        resource: &str,
        permission: &str,
    ) -> bool {
        self.access_policies
            .iter()
            .filter(|((kind, _), _)| kind == ACCESS_POLICY_TYPE_DATA)
            .any(|(_, record)| statement_grants(&record.policy, principals, resource, permission))
    }
}

/// Whether any statement of one policy document grants `permission` on `resource` to one of
/// `principals`.
fn statement_grants(
    policy: &Document,
    principals: &[String],
    resource: &str,
    permission: &str,
) -> bool {
    super::policy_document_entries(Some(policy))
        .into_iter()
        .any(|statement| {
            let Document::Object(members) = statement else {
                return false;
            };
            names_a_principal(members.get("Principal"), principals)
                && super::policy_document_entries(members.get("Rules"))
                    .into_iter()
                    .any(|rule| rule_grants(rule, resource, permission))
        })
}

/// Whether a statement's `Principal` element is about one of `principals`.
///
/// The whole-element `*` names every principal; every other entry is compared literally,
/// as IAM compares a principal.
fn names_a_principal(element: Option<&Document>, principals: &[String]) -> bool {
    super::policy_document_entries(element)
        .into_iter()
        .any(|named| match named {
            Document::String(named) => {
                named == ANY_PRINCIPAL || principals.iter().any(|spelling| spelling == named)
            }
            _ => false,
        })
}

/// Whether one `Rules` entry grants `permission` on `resource`.
fn rule_grants(rule: &Document, resource: &str, permission: &str) -> bool {
    let Document::Object(members) = rule else {
        return false;
    };
    let covers = super::policy_document_entries(members.get("Resource"))
        .into_iter()
        .any(|named| match named {
            Document::String(named) => {
                named.len() <= super::MAX_RESOURCE_PATTERN_LENGTH
                    && super::resource_patterns_overlap(named, resource)
            }
            _ => false,
        });
    covers
        && super::policy_document_entries(members.get("Permission"))
            .into_iter()
            .any(|granted| match granted {
                Document::String(granted) => {
                    granted.len() <= super::MAX_RESOURCE_PATTERN_LENGTH
                        && super::resource_patterns_overlap(granted, permission)
                }
                _ => false,
            })
}
