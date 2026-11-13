// SPDX-License-Identifier: CC0-1.0

//! The unit tests of [`super`]'s own items: this module's file, split out of the module
//! root so that root stays within the project's file-length limit.

use super::*;

/// [`CAPABILITY_AUTO_EXPAND`] names one value of the modelled `Capability` enum, whose
/// value set codegen carries as `CAPABILITY_VALUES` — so the constant this crate
/// compares an acknowledgement against cannot drift from the value a caller can
/// actually send.
#[test]
fn the_auto_expand_capability_is_a_modelled_capability_value() {
    let modelled = crate::ops::types::CAPABILITY_VALUES;
    assert!(
        modelled.contains(&CAPABILITY_AUTO_EXPAND),
        "case {CAPABILITY_AUTO_EXPAND:?}: expected the modelled Capability value set to \
         carry it, got {modelled:?}"
    );
}
