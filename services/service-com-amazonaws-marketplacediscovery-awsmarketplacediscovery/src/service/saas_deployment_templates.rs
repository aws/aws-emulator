// SPDX-License-Identifier: CC0-1.0

//! Nothing: the member this module used to write into a rendered answer is declared by the
//! model now.
//!
//! `SaasFulfillmentOption$deploymentTemplates` is `@required` with a default of `[]`
//! (`smithy_query(query="shape",shape="SaasFulfillmentOption")`), so it is a field of the
//! generated shape and the schema serializer renders it from the value the catalog holds
//! (`crate::state`) — where this module used to write it into the body of a
//! `ListFulfillmentOptions` success, because the model it was written against declared the
//! member on no shape at all. `tests/all/detail_view_own_record.rs` pins what reaches the
//! wire, which is unchanged: a present, empty list on every SaaS option and on no option of
//! another arm.
//!
//! The file is kept only because the tooling this fix was made with cannot delete one; it
//! declares nothing, and nothing uses it.
