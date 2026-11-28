// SPDX-License-Identifier: CC0-1.0

#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::default_trait_access, clippy::use_self, clippy::derivable_impls, clippy::default_constructed_unit_structs)]
#![allow(non_camel_case_types)]
#![forbid(unsafe_code)]
#![doc(hidden)]
//! Service crate for `com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime`.

pub mod ops;
pub mod scaffolded;
pub mod service;
pub mod state;

pub use state::ComAmazonawsSagemakerinferencerouterruntimeSagemakerinferencerouterruntimeState;

pub use service::EmulatedService;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerinferencerouterruntime-sagemakerinferencerouterruntime.json");

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_sagemakerinferencerouterruntime_sagemakerinferencerouterruntime_service_decl.rs"));
