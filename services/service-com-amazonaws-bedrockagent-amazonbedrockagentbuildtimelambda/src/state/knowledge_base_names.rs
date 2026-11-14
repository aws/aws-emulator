// SPDX-License-Identifier: CC0-1.0

//! A knowledge base's NAME as the account's own namespace: which knowledge base holds a
//! name, and the refusal a second one under it gets.
//!
//! `CreateKnowledgeBase` takes the knowledge base's `name` and answers with the
//! `knowledgeBaseId` it minted, so the name is not the resource's identity — but it is
//! still the account's to hold once: a create under a name another knowledge base of the
//! account already has is refused with the modelled `ConflictException`, which is what a
//! CloudFormation replacement of a knowledge base under the SAME `Name` runs into
//! (recorded: the replacement's create fails because the name is taken, and the stack
//! rolls back rather than ending with two knowledge bases sharing one name).
//!
//! An `UpdateKnowledgeBase` is held to the same rule: that call takes a new required
//! `name`, so a rename onto a name another knowledge base holds would leave two of them
//! under it, and the knowledge base being renamed is of course not its own obstacle.

use super::*;

impl ComAmazonawsBedrockagentAmazonbedrockagentbuildtimelambdaState {
    /// The knowledge base of this account that holds `name`, or `None` when the name is
    /// free.
    ///
    /// A knowledge base a delete has been accepted for is on its way out of the store, so
    /// it no longer holds its name: the same reading of "is this knowledge base still
    /// there" every other operation takes ([`Self::live_knowledge_base`]).
    pub fn knowledge_base_named(&self, name: &str) -> Option<&KnowledgeBaseRecord> {
        self.knowledge_bases
            .values()
            .find(|record| record.name == name && !record.is_deleting())
    }

    /// The same, for a rename: the OTHER knowledge base that holds `name`, which is what
    /// stops an update from leaving two of them under one name.
    pub fn knowledge_base_named_other_than(
        &self,
        name: &str,
        knowledge_base_id: &str,
    ) -> Option<&KnowledgeBaseRecord> {
        self.knowledge_base_named(name)
            .filter(|record| record.knowledge_base_id != knowledge_base_id)
    }
}

/// The `ConflictException` message a create or an update answers when the name it asks for
/// is one this account's knowledge bases already hold.
///
/// The sentence is this service's — the one its other conflicts are worded in
/// ([`resource_status_conflict_message`]) — with two variable parts: the operation the
/// caller invoked and the name it asked for.
pub fn knowledge_base_name_conflict_message(operation: &str, name: &str) -> String {
    format!(
        "Couldn't perform {operation} operation. A knowledge base with the name {name} \
         already exists. Specify a different name and retry your request."
    )
}
