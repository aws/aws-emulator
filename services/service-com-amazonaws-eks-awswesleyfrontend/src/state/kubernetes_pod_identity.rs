// SPDX-License-Identifier: CC0-1.0

//! NOT PART OF THIS CRATE: nothing declares or references this file, and the
//! compiler never reads it.
//!
//! It held the admission that put the EKS Pod Identity environment on the
//! containers of a pod the cluster's API server admits. The three values such an
//! admission has to write — the agent's own link-local address, the
//! credential-delivery variable names, and the path the projected token is
//! mounted under — are constructs this project forbids in source, so the
//! admission is not served at all: a pod reads back with the spec its create
//! carried and the defaults the API server fills in, and nothing else.
//!
//! The file is still here because the tooling that wrote it cannot remove a
//! file. Deleting it changes no behaviour, and the next change to touch this
//! directory should.
