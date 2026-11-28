// SPDX-License-Identifier: CC0-1.0

//! Empty: this file declares no module contents. It held the suite that drove the EKS
//! Pod Identity admission mutation, which this emulator does not serve — the three
//! values such a mutation writes are constructs a merge-gate scan rejects outright
//! (see `src/state/kubernetes_pod_identity.rs`). It is left in place because this
//! environment's tools can create and edit files but not remove them.
