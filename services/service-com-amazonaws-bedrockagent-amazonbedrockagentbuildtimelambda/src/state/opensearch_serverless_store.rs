// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless vector store a knowledge base is bound to, resolved in the
//! service that OWNS that collection.
//!
//! `CreateKnowledgeBase` documents the vector store as something the caller sets up first
//! ("To create a knowledge base, you must first set up your data sources and configure a
//! supported vector store"), and for an `OPENSEARCH_SERVERLESS` storage configuration what
//! it sets up is a collection, a vector index inside it, and the data access policy that
//! lets the knowledge base's own role reach that index. Amazon Bedrock resolves all three
//! when the knowledge base is created — it reads and writes the index as the role the
//! request names — so a binding its role cannot reach is refused there and then, before any
//! knowledge base is written.
//!
//! The decision is NOT taken here: who may act on a collection's data is the owning
//! service's question and its own state answers it
//! (`::service_com_amazonaws_opensearchserverless_opensearchserverless::state::data_access`),
//! over the very policies `aoss:CreateAccessPolicy` wrote. What lives here is which
//! resource and which permissions a knowledge base takes on its index, and the sentence this
//! service refuses an unreachable binding with.
//!
//! An ARN naming a collection that service holds no record of is not judged: nothing stored
//! says what its policies grant, so such a binding is not refused here — the same reading
//! [`crate::state::service_role`] gives a role the IAM store does not hold.
//!
//! LOCK ORDER: as [`crate::state::vector_store`] states — every function here takes the
//! owning service's lock and holds no other while it does, so each must be called with NO
//! guard of this service's store held.

use super::*;

/// The `KnowledgeBaseStorageType` of a knowledge base whose vectors live in an Amazon
/// OpenSearch Serverless collection
/// (`smithy_query(query="shape",shape="KnowledgeBaseStorageType")`).
pub const OPENSEARCH_SERVERLESS_STORAGE_TYPE: &str = "OPENSEARCH_SERVERLESS";

/// The permissions a knowledge base performs on the vector index it is bound to, which are
/// the ones its role must be granted for the binding to work: it reads the index's own
/// definition to check the field mapping, writes a document per ingested chunk, and reads
/// them back to serve a retrieval.
///
/// Source: the index-level permissions AWS's "Create a vector index in Amazon OpenSearch
/// Service" states for the knowledge-base role's data access policy, narrowed to the ones
/// this service's own operations perform.
const REQUIRED_INDEX_PERMISSIONS: [&str; 3] = [
    ::service_com_amazonaws_opensearchserverless_opensearchserverless::state::data_access::permission::DESCRIBE_INDEX,
    ::service_com_amazonaws_opensearchserverless_opensearchserverless::state::data_access::permission::READ_DOCUMENT,
    ::service_com_amazonaws_opensearchserverless_opensearchserverless::state::data_access::permission::WRITE_DOCUMENT,
];

/// The collection and index an `OPENSEARCH_SERVERLESS` storage configuration binds a
/// knowledge base to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSearchServerlessBinding {
    /// The collection's ARN, as the request spells it.
    pub collection_arn: String,
    /// The vector index's name within that collection.
    pub index: String,
}

/// The binding `storage_configuration` names, or `None` when it names no OpenSearch
/// Serverless index at all.
pub fn opensearch_serverless_binding(
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<OpenSearchServerlessBinding> {
    let configuration = storage_configuration
        .filter(|storage| storage.r#type == OPENSEARCH_SERVERLESS_STORAGE_TYPE)?
        .opensearch_serverless_configuration
        .as_ref()?;
    Some(OpenSearchServerlessBinding {
        collection_arn: configuration.collection_arn.clone(),
        index: configuration.vector_index_name.clone(),
    })
}

/// `None` when nothing stored says the role `role_arn` cannot reach the index
/// `storage_configuration` names; otherwise the `ValidationException` message that refuses
/// the call.
///
/// The collection is resolved by the ARN the request carries, in the owning service's own
/// store, and the data access policies of that service decide whether the role is granted
/// the permissions a knowledge base performs on the index ([`REQUIRED_INDEX_PERMISSIONS`]).
/// A role granted none of them cannot read or write the vectors the knowledge base is for,
/// which is what makes the storage configuration invalid.
///
/// LOCK ORDER: as the module doc says — call with NO guard of this service's store held.
pub fn opensearch_serverless_access_problem<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<String> {
    use ::service_com_amazonaws_opensearchserverless_opensearchserverless::state as collections;

    let binding = opensearch_serverless_binding(storage_configuration)?;
    let state = collections::service_state(engine)?;
    let store = state.lock();
    // A collection this emulator holds no record of says nothing about what its policies
    // grant, so the binding is not judged on one.
    let collection = store.collection_by_arn(&binding.collection_arn)?;
    let resource = collections::data_access::index_resource(&collection.name, &binding.index);
    let principals = [role_arn.to_string()];
    let granted = REQUIRED_INDEX_PERMISSIONS
        .iter()
        .all(|permission| store.data_access_grants(&principals, &resource, permission));
    (!granted).then(|| STORAGE_CONFIGURATION_UNREACHABLE.to_string())
}

/// The `ValidationException` message Amazon Bedrock answers when the vector store refuses
/// the knowledge base's own role.
///
/// Fixed text for this error class: this service's own opening for an invalid storage
/// configuration, and the dependency's refusal — the status and the message the vector store
/// answered the authorization failure with — neither of which is taken from the request.
pub const STORAGE_CONFIGURATION_UNREACHABLE: &str =
    "The knowledge base storage configuration provided is invalid... Dependency error \
     document status code: 403, error message: Bad Authorization";
