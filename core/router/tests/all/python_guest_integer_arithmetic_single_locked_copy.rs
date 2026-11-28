// SPDX-License-Identifier: CC0-1.0

//! ONE copy of the Python guest's arbitrary-precision integer arithmetic in this workspace, at
//! the release its substrate pins — because a second copy is a workspace that does not COMPILE,
//! and an emulator that does not compile serves no request at all.
//!
//! Two service crates run caller-supplied Python in this process and reach the interpreter
//! through `services/python-guest-substrate`, which pins that arithmetic exactly
//! (`malachite-bigint = "=0.12.0"`) and says why: the math module the interpreter builds on
//! accepts any `0.x` of it and resolves the newest on its own, and a manifest requirement binds
//! only the edge it is written on — so cargo is free to resolve a SECOND copy, one major-zero
//! line ahead, for that other edge. The committed `Cargo.lock` is the only place that edge can be
//! stated, and the release the line ahead carries declares a `rust-version` ahead of this
//! project's toolchain: with it in the lock, `cargo build --locked` refuses the whole graph
//! before a single crate is compiled, so every target that reaches the substrate — the router
//! binary this emulator is, and the test targets of the service crates that embed it — stops
//! existing.
//!
//! So this suite pins two things on the committed files: the lock carries exactly one copy of
//! each crate of that arithmetic, each one the substrate's own pin accepts, and the substrate's
//! manifest states that pin EXACTLY rather than as a requirement a resolve may walk forward.
//! Neither is read from the implementation: the expected release comes from the manifest the pin
//! lives in, so moving the pin moves the test with it, and a crate hard-coded here could not make
//! a check pass on its own.
//!
//! Each check is a function over text, driven once over the committed files and once over
//! synthetic ones, so it is known to REPORT the states it is there to catch rather than only to
//! pass on the tree as it stands. It reads files only; it resolves nothing, starts no router and
//! reaches no network.
//!
//! The lock and manifest readers come from `workspace_lockfile_covers_every_manifest`, which
//! already pairs these two files, and the shape of the suite follows its sibling
//! `sql_engine_single_locked_copy`, which pins the same kind of rule over the SQL engine.

use std::fs;

use crate::workspace_lockfile_covers_every_manifest::{
    accepts, dependencies_in, is_member_dependency_table, locked_packages, workspace_root, Locked,
};

/// The crate the substrate declares the pin on: the guest's arbitrary-precision integers.
const PINNED_CRATE: &str = "malachite-bigint";

/// The manifest that declares it, relative to the workspace root — the crate that CONTAINS the
/// interpreter, so the services embedding it inherit the pin instead of each repeating it.
const SUBSTRATE_MANIFEST: &str = "services/python-guest-substrate/Cargo.toml";

/// Every crate of that arithmetic the lock holds: the pinned one and the crates it is built out
/// of, which no manifest in this workspace declares — the lock entry is the only place each of
/// them can be pinned. Read off the locked `[[package]]` dependency lists of `malachite-bigint`
/// (`malachite-base`, `malachite-nz`) beside the rationals crate the interpreter itself takes
/// (`malachite-q`), all of which are released as one line.
const ARITHMETIC_CRATES: [&str; 4] = [
    PINNED_CRATE,
    "malachite-base",
    "malachite-nz",
    "malachite-q",
];

/// One report per way `locked` fails the rule for the crate `name`: the lock carries no copy of
/// it, it carries more than one, or a copy it carries is not the release `pin` names.
fn copy_reports(locked: &Locked, name: &str, pin: &str) -> Vec<String> {
    let Some(versions) = locked.get(name) else {
        return vec![format!(
            "case {name:?}: expected Cargo.lock to carry {name:?}, got no [[package]] with that \
             name"
        )];
    };
    let mut reports = Vec::new();
    if versions.len() != 1 {
        reports.push(format!(
            "case {name:?}: expected exactly 1 locked copy of {name:?}, got {} ({versions:?}) — a \
             second copy is the release line this toolchain cannot build",
            versions.len()
        ));
    }
    for version in versions {
        if !accepts(pin, version) {
            reports.push(format!(
                "case {name:?}: expected the locked {name:?} to be the release the substrate pins \
                 ({pin}), got {version:?}"
            ));
        }
    }
    reports
}

/// The requirement the manifest `text` declares on [`PINNED_CRATE`], and one report per way that
/// declaration fails the rule: it is missing, it defers to the root table (which this repository's
/// fixes may not edit), or it is not an EXACT pin, which would let a resolve walk the substrate's
/// own edge forward.
fn pin_reports(manifest_path: &str, text: &str) -> (Option<String>, Vec<String>) {
    let mut reports = Vec::new();
    let mut found = None;
    for (table, key, spec) in dependencies_in(text, is_member_dependency_table) {
        // The crate the key resolves to, so a dependency declared under an alias is read by the
        // crate the lock knows rather than by the name the manifest chose.
        let crate_name = spec.package.clone().unwrap_or(key);
        if crate_name != PINNED_CRATE {
            continue;
        }
        if spec.from_workspace {
            reports.push(format!(
                "case {manifest_path:?} {table}: expected {PINNED_CRATE} to be pinned in this \
                 manifest, got a declaration deferring to the root workspace table"
            ));
            continue;
        }
        let Some(req) = &spec.req else {
            reports.push(format!(
                "case {manifest_path:?} {table}: expected {PINNED_CRATE} to carry a version \
                 requirement, got {spec:?}"
            ));
            continue;
        };
        if !req.trim().starts_with('=') {
            reports.push(format!(
                "case {manifest_path:?} {table}: expected {PINNED_CRATE} to be pinned exactly, \
                 got the requirement {req:?}, which a resolve may walk forward"
            ));
        }
        found = Some(req.trim().to_string());
    }
    if found.is_none() && reports.is_empty() {
        reports.push(format!(
            "case {manifest_path:?}: expected a declaration of {PINNED_CRATE}, got none"
        ));
    }
    (found, reports)
}

/// The committed lock's packages, and the text of the manifest the pin lives in.
fn lock_and_substrate_manifest() -> (Locked, String) {
    let root = workspace_root();
    let locked = locked_packages(
        &fs::read_to_string(root.join("Cargo.lock")).expect("read the workspace Cargo.lock"),
    );
    let manifest = fs::read_to_string(root.join(SUBSTRATE_MANIFEST))
        .unwrap_or_else(|error| panic!("read {SUBSTRATE_MANIFEST}: {error}"));
    (locked, manifest)
}

/// RULE: the substrate's manifest pins the guest's integer arithmetic EXACTLY, and the committed
/// lock carries exactly one copy of every crate of that arithmetic, each at the release that pin
/// names — so no resolve can put the release line this toolchain cannot build back in the graph.
///
/// The rows are the crates of that arithmetic; the release they are held to is the manifest's own
/// pin rather than a literal of this suite's.
#[test]
fn the_lock_carries_one_copy_of_the_guest_integer_arithmetic_at_its_pinned_release() {
    let (locked, manifest) = lock_and_substrate_manifest();
    let (pin, reports) = pin_reports(SUBSTRATE_MANIFEST, &manifest);
    assert!(
        reports.is_empty(),
        "expected {SUBSTRATE_MANIFEST} to pin {PINNED_CRATE} exactly, got {} report(s): \
         {reports:#?}",
        reports.len()
    );
    let pin = pin.unwrap_or_else(|| panic!("expected a requirement on {PINNED_CRATE}"));

    for name in ARITHMETIC_CRATES {
        let reports = copy_reports(&locked, name, &pin);
        assert!(
            reports.is_empty(),
            "case {name:?}: expected one locked copy of {name:?} at the pinned {pin}, got {} \
             report(s): {reports:#?}",
            reports.len()
        );
    }
}

/// RULE: the same — stated as the reader, over synthetic locks that each break the rule one way,
/// so the check above is known to fail on a second copy and on the release line ahead rather than
/// only to pass on the tree as it stands.
#[test]
fn a_second_or_unpinned_locked_copy_is_reported() {
    // (case, the `[[package]]` entries of a synthetic lock, how many reports it owes)
    let cases = [
        (
            "one copy at the pinned release",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0\"\n",
            0,
        ),
        (
            "two copies, the second one line ahead",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0\"\n\
             [[package]]\nname = \"malachite-bigint\"\nversion = \"0.15.0\"\n",
            2,
        ),
        (
            "one copy, one line ahead of the pin",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.15.0\"\n",
            1,
        ),
        (
            "one copy, one patch past the pin",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.1\"\n",
            1,
        ),
        (
            "no copy at all",
            "[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
            1,
        ),
    ];
    for case in cases {
        let (name, lock, expected) = case;
        let reports = copy_reports(&locked_packages(lock), PINNED_CRATE, "=0.12.0");
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

/// RULE: the same, stated as the manifest reader, over synthetic manifests — so the check above is
/// known to fail on a requirement a resolve may walk forward.
///
/// The rows span the declaration forms this workspace's manifests use: an exact pin, a bare caret
/// requirement, an inline table, a renamed dependency, a declaration deferring to the root table,
/// and a manifest that declares the crate not at all.
#[test]
fn a_requirement_a_resolve_may_walk_forward_is_reported() {
    // (case, the manifest, the requirement it is read as, how many reports it owes)
    let cases = [
        (
            "an exact pin",
            "[dependencies]\nmalachite-bigint = \"=0.12.0\"\n",
            Some("=0.12.0"),
            0,
        ),
        (
            "an exact pin in an inline table",
            "[dependencies]\nmalachite-bigint = { version = \"=0.12.0\" }\n",
            Some("=0.12.0"),
            0,
        ),
        (
            "a bare caret requirement",
            "[dependencies]\nmalachite-bigint = \"0.12\"\n",
            Some("0.12"),
            1,
        ),
        (
            "the crate under another key",
            "[dependencies]\nbigint = { version = \"0.12\", package = \"malachite-bigint\" }\n",
            Some("0.12"),
            1,
        ),
        (
            "a declaration deferring to the root table",
            "[dependencies]\nmalachite-bigint = { workspace = true }\n",
            None,
            1,
        ),
        (
            "a manifest that declares something else",
            "[dependencies]\nnix = \"0.31\"\n",
            None,
            1,
        ),
    ];
    for case in cases {
        let (name, manifest, expected_req, expected) = case;
        let (req, reports) = pin_reports(name, manifest);
        assert_eq!(
            req.as_deref(),
            expected_req,
            "case {name:?}: expected the requirement {expected_req:?}, got {req:?}"
        );
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}
