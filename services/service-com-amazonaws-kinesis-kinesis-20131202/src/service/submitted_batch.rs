// SPDX-License-Identifier: CC0-1.0

//! What this service STATES about quoting a collection a refused request carried: the two
//! ceilings, read off its own model.
//!
//! The quoting itself is not here and must not be: a clause about a collection member quotes
//! the collection the request carried — the elements in order, each structure printed over
//! every member its shape declares, an unset member as `null`, a blob as the buffer it was
//! read into — and that rendering, the walk that collects it, and the rule that restores it
//! into the clause all live in the tree's one home for it
//! (`service_common::validation::refusing_violations_with_declared_echoes_rendering_up_to`
//! and `service_common::validation_message::quoting_the_submitted_collection`, reached from
//! [`super::stated_refusal`]). Two services asked how one batch is quoted must not answer
//! differently.
//!
//! What IS this service's own is how much it quotes, because that is a fact about its model
//! rather than about the rendering: the shared walk holds a composite to a child count and a
//! rendering size, and a service whose model declares a collection longer than the walk's own
//! default states its ceilings from that collection's declared maximum. Both numbers below
//! are model constants, so no response this service sends is sized by the request: past them
//! the framework's own count summary stands.

/// The most elements of one collection a clause of this service quotes: twice the largest
/// list its model declares (`PutRecordsInput$Records` is `@length 1..500`), so a list ONE
/// step past its bound — the list such a clause is about — is quoted whole, while a list of a
/// size the caller chose freely is not rendered at all.
const QUOTED_BATCH_ELEMENTS: usize = 2 * MAX_BATCH_RECORDS;

/// The most member POSITIONS the shared walk may visit for one request of this service,
/// which is the ceiling it both stops at and renders a composite from: the elements above
/// times the members one element declares, because every member of every element is a
/// position of its own and a composite renders only from children the walk reached.
pub(super) const QUOTED_BATCH_POSITIONS: usize = QUOTED_BATCH_ELEMENTS * MEMBERS_PER_ELEMENT;

/// The rendering bytes this service's clauses are held to, which is the other ceiling the
/// shared walk takes from the service.
///
/// TWO things are measured against it, which is why it is a multiple of the batch's own
/// rendering rather than that rendering alone: ONE composite's rendering — the quoted batch,
/// about the 128 bytes per element its fixed text plus a `PartitionKey` of the modelled
/// `@length 1..256` takes — and the renderings the walk keeps while it builds that one, which
/// for a batch are the elements' own renderings plus their members'. Four times the batch's
/// rendering covers both, and every factor in it is a model constant, so the text one refused
/// request makes the walk hold is bounded by the model and not by the request.
pub(super) const QUOTED_BATCH_RENDERING_BYTES: usize = 4 * QUOTED_BATCH_ELEMENTS * 128;

/// The `@length` maximum of `PutRecordsInput$Records`
/// (`smithy_query(query="operation",operation="PutRecords")` -> `Records` len 1..500): the
/// largest collection any operation of this service admits, which is what the ceilings above
/// are read from.
const MAX_BATCH_RECORDS: usize = 500;

/// The members one element of that collection declares
/// (`smithy_query(query="shape",shape="com.amazonaws.kinesis#PutRecordsRequestEntry")` ->
/// `Data`, `ExplicitHashKey`, `PartitionKey`), which is how many positions one element costs
/// the walk.
const MEMBERS_PER_ELEMENT: usize = 3;
