// SPDX-License-Identifier: CC0-1.0

//! ONE copy of the Python guest's arbitrary-precision arithmetic, below the release line
//! this project's toolchain cannot compile.
//!
//! The Python execution substrate (`services/python-guest-substrate`, which the Lambda
//! function runtime and the MicroVM guest both narrow their interpreter through) is built
//! against the `malachite` arithmetic the interpreter's own release line requires
//! (`^0.12.0`). The math module that interpreter builds on, `pymath`, declares the SAME
//! arithmetic behind a requirement spanning every `0.x` release (`malachite-bigint = "0"`),
//! and a requirement in a manifest here binds only the edge that manifest declares: a line
//! one major-zero ahead is semver-incompatible with it, so cargo is free to answer `pymath`'s
//! own edge with the newest line, and a from-scratch resolve does. That is a SECOND copy of
//! the arithmetic in the build, whose crates declare `rust-version = "1.100.0"`
//! (`malachite-base`, `malachite-bigint` and `malachite-nz` from 0.15.0) while the platform
//! builds with 1.99.1. Cargo then refuses the workspace before a single crate compiles —
//! `rustc 1.99.1 is not supported by the following packages` — so every test target of every
//! service fails at once, on a resolution no service's code is involved in. The line taken
//! here declares 1.90.0.
//!
//! So this suite pins two things on the committed files: the lock holds exactly ONE copy of
//! each crate of that arithmetic, below the first line the toolchain cannot build, and no
//! manifest here carries a requirement that would admit one of those releases. Neither half
//! stands in for the other — the manifests state the edges this workspace declares, and the
//! committed lock is the only place the edges it does not declare are stated at all, so the
//! drift is caught here rather than in a platform build nobody can rerun locally. Each check
//! is a function over text, driven once over the committed files and once over synthetic ones,
//! so it is known to REPORT the states it is there to catch rather than only to pass on the
//! tree as it stands.
//!
//! It reads files only; it resolves nothing, starts no router and reaches no network.
//!
//! The lock and manifest readers come from `workspace_lockfile_covers_every_manifest`,
//! which already pairs these two files, and so do the two checks themselves — this suite
//! brings the crates, the bound and the release it refuses; `sql_engine_single_locked_copy`
//! states the same shape of rule, with a floor instead of a ceiling, for the SQL engine the
//! data planes link.

use std::collections::{BTreeMap, BTreeSet};

use crate::workspace_lockfile_covers_every_manifest::{
    committed_lock, crate_manifests_under, locked_copy_reports, locked_packages,
    pinned_requirement_reports, root_and_member_manifests, workspace_dependency_table, Declared,
    Locked, ReleaseBound,
};

/// The first release of the arithmetic this project's toolchain cannot compile: from here on
/// its crates declare a `rust-version` ahead of the rustc the platform builds with, which
/// cargo refuses the whole workspace over.
const FIRST_UNSUPPORTED: &str = "0.15.0";

/// The bound this suite holds the arithmetic's locked release to: below the first release the
/// toolchain cannot compile.
const BOUND: ReleaseBound = ReleaseBound::Below(FIRST_UNSUPPORTED);

/// The crates the guest's arbitrary-precision arithmetic is made of, as the lock names them.
/// Only the first is declared by a manifest here — the others are pulled in by it and by the
/// interpreter, so the lock is the only place they are pinned at all.
const ARITHMETIC_CRATES: [&str; 4] = [
    "malachite-bigint",
    "malachite-base",
    "malachite-nz",
    "malachite-q",
];

/// The crate a manifest here declares, and the one the interpreter's math module asks for
/// behind its own `0`-wide requirement.
const DECLARED_CRATE: &str = "malachite-bigint";

/// The manifest that states which release of the arithmetic this workspace takes: the crate
/// the execution substrate is contained in, which the services that embed it inherit from.
const PINNING_MANIFEST: &str = "services/python-guest-substrate/Cargo.toml";

/// Where this emulator's crates live. Read from disk as well as from the root manifest's
/// `members`, because a crate in here that is only a PATH DEPENDENCY of a member — the
/// execution substrate is one — is resolved with the workspace all the same while being
/// listed nowhere, so a reader of the listed members alone would not see its requirements.
const SERVICE_DIRECTORY: &str = "services";

/// Every manifest this workspace resolves from, by the path to read it at: the members the
/// root manifest lists, and every crate directory under `services/`, listed or not.
fn workspace_manifests(members: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut manifests: BTreeMap<String, String> = members.into_iter().collect();
    manifests.extend(crate_manifests_under(SERVICE_DIRECTORY));
    manifests.into_iter().collect()
}

/// The committed lock's packages, the root manifest's text, and every manifest the workspace
/// resolves from with its path.
fn lock_and_manifests() -> (Locked, String, Vec<(String, String)>) {
    let (root_manifest, members) = root_and_member_manifests();
    (
        locked_packages(&committed_lock()),
        root_manifest,
        workspace_manifests(members),
    )
}

/// The release the committed lock carries `name` at — the first of them, which the copy
/// check above pins to be the only one.
fn locked_release(locked: &Locked, name: &str) -> Option<String> {
    locked
        .get(name)
        .and_then(|versions| versions.first())
        .cloned()
}

/// RULE: the committed lock carries exactly ONE copy of each crate of the Python guest's
/// arithmetic, each below the first release this toolchain cannot compile — so no resolve
/// here puts a crate in the build that stops cargo before any crate compiles.
///
/// The rows are the four crates that arithmetic is made of.
#[test]
fn the_lock_carries_one_copy_of_the_guest_arithmetic_below_the_unsupported_release() {
    let (locked, _, _) = lock_and_manifests();
    for name in ARITHMETIC_CRATES {
        let reports = locked_copy_reports(&locked, name, name, BOUND);
        assert!(
            reports.is_empty(),
            "case {name:?}: expected one locked copy of {name:?} below {FIRST_UNSUPPORTED}, \
             got {} report(s): {reports:#?}",
            reports.len()
        );
    }
}

/// RULE: the same — stated as the reader, over synthetic locks that each break the rule one
/// way, so the check above is known to fail on a second copy and on a copy the toolchain
/// cannot compile rather than only to pass on the tree as it stands.
#[test]
fn a_second_or_unsupported_locked_copy_is_reported() {
    // (case, the `[[package]]` entries of a synthetic lock, how many reports it owes)
    let cases = [
        (
            "one copy on the release line taken here",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0\"\n",
            0,
        ),
        (
            "one copy on a line below the unsupported one",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.14.9\"\n",
            0,
        ),
        (
            "two copies, both below the unsupported one",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0\"\n\
             [[package]]\nname = \"malachite-bigint\"\nversion = \"0.14.0\"\n",
            1,
        ),
        (
            "the second copy the wide requirement resolves on its own",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0\"\n\
             [[package]]\nname = \"malachite-bigint\"\nversion = \"0.15.0\"\n",
            2,
        ),
        (
            "one copy, on the unsupported line",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.15.0\"\n",
            1,
        ),
        (
            "one copy, written in a form no release comparison can read",
            "[[package]]\nname = \"malachite-bigint\"\nversion = \"0.12.0-rc.1\"\n",
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
        let reports = locked_copy_reports(&locked_packages(lock), name, DECLARED_CRATE, BOUND);
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}

/// RULE: the same bound, stated as the release comparison the check makes, over releases
/// spanning it in both directions.
///
/// The rows are the line taken here, a step past it in each of the three components a release
/// is written in, the first unsupported release itself, and a step past that one in each
/// component.
#[test]
fn the_release_comparison_spans_the_unsupported_bound() {
    // (case, the release, whether a build here may carry it)
    let cases = [
        ("the line taken here", "0.12.0", true),
        ("one patch past it", "0.12.1", true),
        ("one minor past it", "0.13.0", true),
        ("the last minor below the bound", "0.14.9", true),
        ("a line older than the one taken here", "0.11.0", true),
        ("the first unsupported release", "0.15.0", false),
        ("one patch past the first unsupported", "0.15.1", false),
        ("one minor past the first unsupported", "0.16.0", false),
        ("one major past the first unsupported", "1.0.0", false),
    ];
    for case in cases {
        let (name, release, allowed) = case;
        assert_eq!(
            BOUND.admits(release),
            Some(allowed),
            "case {name:?}: expected release {release:?} below {FIRST_UNSUPPORTED} to be \
             allowed = {allowed}, got {:?}",
            BOUND.admits(release)
        );
    }
    // A release that is not written as numbers is reported unreadable rather than supported,
    // so no check above can pass on one.
    assert_eq!(
        BOUND.admits("not-a-release"),
        None,
        "case \"an unreadable release\": expected no comparison at all, got {:?}",
        BOUND.admits("not-a-release")
    );
}

/// RULE: every manifest in this workspace that requires the guest arithmetic accepts the
/// locked copy and refuses the releases the toolchain cannot compile — so no edge this
/// workspace declares itself can take a release the build cannot compile, and the release the
/// lock records for the edges it does not declare is the one its own manifests agree on.
///
/// Checked over every crate the workspace resolves from — the members the root manifest lists
/// and every crate directory under `services/` — in every dependency table each declares, with
/// `workspace = true` read through the root table it defers to.
#[test]
fn every_manifest_requiring_the_guest_arithmetic_refuses_the_unsupported_release() {
    let (locked, root_manifest, manifests) = lock_and_manifests();
    let workspace_deps = workspace_dependency_table(&root_manifest);

    let mut declaring: BTreeSet<&str> = BTreeSet::new();
    let mut reports = Vec::new();
    for (manifest_path, text) in &manifests {
        for name in ARITHMETIC_CRATES {
            // Each crate paired with ITS OWN locked release, so a manifest requiring one of
            // the siblings is read against the release that sibling is locked at rather than
            // against another crate's. A crate the lock carries no copy of is reported by the
            // copy check above, which is the one reading of that state.
            let Some(version) = locked_release(&locked, name) else {
                continue;
            };
            let (declared, manifest_reports) = pinned_requirement_reports(
                manifest_path,
                text,
                &workspace_deps,
                &[name],
                &version,
                FIRST_UNSUPPORTED,
            );
            if declared > 0 {
                declaring.insert(manifest_path.as_str());
            }
            reports.extend(manifest_reports);
        }
    }

    assert!(
        reports.is_empty(),
        "expected every manifest requiring the guest arithmetic to refuse \
         {FIRST_UNSUPPORTED} and accept the release its crate is locked at, got {} \
         report(s): {reports:#?}",
        reports.len()
    );
    // Not vacuous, and not satisfied by the lock alone: the crate the execution substrate is
    // contained in states the release, so there is a requirement to check.
    assert!(
        declaring.contains(PINNING_MANIFEST),
        "expected {PINNING_MANIFEST} to state the release of the guest arithmetic this \
         workspace takes, got the declaring manifests {declaring:?}"
    );
}

/// RULE: the same — stated as the reader, over synthetic manifests, so the check above is
/// known to fail on a requirement that admits a release the toolchain cannot compile.
///
/// The rows span the declaration forms this tree uses: the exact pin the substrate carries, a
/// caret requirement on the line taken here, an inline table, a dev-dependency, a renamed
/// dependency, a declaration deferred to the root table, a crate that is not the arithmetic —
/// and the `0`-wide form the interpreter's math module declares, which a manifest here may
/// not take because it admits every release of the line the toolchain cannot compile.
#[test]
fn a_manifest_requirement_admitting_the_unsupported_release_is_reported() {
    let workspace_deps: BTreeMap<String, Declared> = [(
        DECLARED_CRATE.to_string(),
        Declared {
            req: Some("0".to_string()),
            ..Declared::default()
        },
    )]
    .into_iter()
    .collect();
    // (case, the manifest, how many requirements it declares, how many reports it owes)
    let cases = [
        (
            "the exact pin the substrate carries",
            "[dependencies]\nmalachite-bigint = \"=0.12.0\"\n",
            1,
            0,
        ),
        (
            "a caret requirement on the line taken here",
            "[dependencies]\nmalachite-bigint = \"0.12\"\n",
            1,
            0,
        ),
        (
            "an inline table with features",
            "[dependencies]\nmalachite-bigint = { version = \"=0.12.0\", features = [\"std\"] }\n",
            1,
            0,
        ),
        (
            "a sibling crate of the arithmetic, pinned",
            "[dependencies]\nmalachite-nz = \"=0.12.0\"\n",
            1,
            0,
        ),
        (
            "the wide form the interpreter's math module declares",
            "[dependencies]\nmalachite-bigint = \"0\"\n",
            1,
            1,
        ),
        (
            "a requirement on the unsupported line itself",
            "[dependencies]\nmalachite-bigint = \"=0.15.0\"\n",
            1,
            2,
        ),
        (
            "a dev-dependency on the unsupported line",
            "[dev-dependencies]\nmalachite-bigint = { version = \"0.15\" }\n",
            1,
            2,
        ),
        (
            "the arithmetic under another key",
            "[dependencies]\nbigint = { version = \"0\", package = \"malachite-bigint\" }\n",
            1,
            1,
        ),
        (
            "a declaration deferred to the root table",
            "[dependencies]\nmalachite-bigint = { workspace = true }\n",
            1,
            1,
        ),
        (
            "a crate that is not the arithmetic",
            "[dependencies]\nnum-bigint = \"0\"\n",
            0,
            0,
        ),
    ];
    for case in cases {
        let (name, manifest, declared, expected) = case;
        let (counted, reports) = pinned_requirement_reports(
            name,
            manifest,
            &workspace_deps,
            &ARITHMETIC_CRATES,
            "0.12.0",
            FIRST_UNSUPPORTED,
        );
        assert_eq!(
            counted, declared,
            "case {name:?}: expected {declared} requirement(s) on the guest arithmetic, got \
             {counted}"
        );
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}
