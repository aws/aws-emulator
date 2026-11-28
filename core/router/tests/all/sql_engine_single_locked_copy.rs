// SPDX-License-Identifier: CC0-1.0

//! ONE copy of the SQL engine in this workspace, at or past the release it is pinned to.
//!
//! Several service crates run caller SQL on a real in-process engine — the `rusqlite`
//! binding with its `bundled` SQLite — and the vendored telemetry crate links the same
//! binding behind a wide requirement (`>=0.37, <0.42`). Each crate carries its OWN
//! requirement, so what the process actually links is decided by the committed
//! `Cargo.lock`: one locked copy of the binding and one of the native library mean one
//! engine in the build, and a SECOND copy would put an older SQLite back beside the pinned
//! one without a single manifest changing. The release that matters is SQLite 3.54.0, the
//! first whose page cache cannot answer a query with bytes the database never wrote (the
//! advisory `cargo audit` reports against the earlier `libsqlite3-sys`,
//! RUSTSEC-2026-0459); the binding release that first carries it is 0.41, with
//! `libsqlite3-sys` 0.39.
//!
//! So this suite pins two things on the committed files: the lock holds exactly one copy
//! of each, at or past those releases, and no service manifest asks for a requirement that
//! would admit an earlier one. Each check is a function over text, driven once over the
//! committed files and once over synthetic ones, so it is known to REPORT the states it is
//! there to catch rather than only to pass on the tree as it stands.
//!
//! It reads files only; it resolves nothing, starts no router and reaches no network. The
//! release the linked engine REPORTS at run time is pinned beside the data plane that runs
//! on it, by the rds-data crate's `engine_release_floor` suite — this suite is about which
//! copies a build may contain at all.
//!
//! The lock and manifest readers come from `workspace_lockfile_covers_every_manifest`,
//! which already pairs these two files, and so do the two checks themselves: this suite
//! brings the crates, the bound and the refused release, and `python_guest_arithmetic_single_locked_copy`
//! states the same rule for the Python guest's arithmetic through the same pair of readings.

use std::collections::{BTreeMap, BTreeSet};

use crate::workspace_lockfile_covers_every_manifest::{
    committed_lock, locked_copy_reports, locked_packages, pinned_requirement_reports,
    root_and_member_manifests, workspace_dependency_table, Declared, Locked, ReleaseBound,
};

/// The name the lock knows the engine binding by: the crate every SQL data plane here
/// declares.
const ENGINE_BINDING: &str = "rusqlite";

/// The name the lock knows the native SQLite library by. No manifest in this workspace
/// declares it — the binding pulls it in — so the lock entry is the only place it can be
/// pinned.
const NATIVE_LIBRARY: &str = "libsqlite3-sys";

/// One crate a SQL data plane here is built out of: the lowest release a build of this
/// workspace may link, and a release it must NOT link — the one that shipped the SQLite
/// whose page cache could disclose bytes the database never wrote.
#[derive(Debug)]
struct EngineCrate {
    case: &'static str,
    name: &'static str,
    floor: &'static str,
    refused: &'static str,
}

/// The engine the SQL data planes run on, as the lock names it. Both floors are the
/// releases the service manifests' own notes pin ("PINNED at 0.41 … whose
/// `libsqlite3-sys` (0.39) carries a SQLite C library without" the disclosure), and both
/// refused releases are the ones those notes name as carrying it.
const ENGINE_CRATES: [EngineCrate; 2] = [
    EngineCrate {
        case: "the engine binding",
        name: ENGINE_BINDING,
        floor: "0.41.0",
        refused: "0.37.0",
    },
    EngineCrate {
        case: "the native SQLite library it links",
        name: NATIVE_LIBRARY,
        floor: "0.39.0",
        refused: "0.35.0",
    },
];

/// The bound this suite holds each engine crate's locked release to: the release it is pinned
/// at, and everything past it.
fn bound(engine: &EngineCrate) -> ReleaseBound {
    ReleaseBound::AtOrPast(engine.floor)
}

/// The committed lock's packages, the root `[workspace.dependencies]` table a member's
/// `workspace = true` defers to, and the manifest text of every member under `services/` — the
/// crates that declare the engine themselves.
fn lock_and_service_manifests() -> (Locked, BTreeMap<String, Declared>, Vec<(String, String)>) {
    let (root_manifest, members) = root_and_member_manifests();
    let manifests = members
        .into_iter()
        .filter(|(manifest, _)| manifest.starts_with("services/"))
        .collect();
    (
        locked_packages(&committed_lock()),
        workspace_dependency_table(&root_manifest),
        manifests,
    )
}

/// RULE: the committed lock carries exactly ONE copy of the SQL engine binding and one of
/// the native SQLite library it links, each at or past the release this workspace pins —
/// so no build here links a second, older engine beside the pinned one.
///
/// The rows are the two crates a SQL data plane is built out of.
#[test]
fn the_lock_carries_one_copy_of_the_sql_engine_at_or_past_its_pinned_release() {
    let (locked, _, _) = lock_and_service_manifests();
    for engine in &ENGINE_CRATES {
        let reports = locked_copy_reports(&locked, engine.case, engine.name, bound(engine));
        assert!(
            reports.is_empty(),
            "case {:?}: expected one locked copy of {:?} at or past {}, got {} report(s): \
             {reports:#?}",
            engine.case,
            engine.name,
            engine.floor,
            reports.len()
        );
    }
}

/// RULE: the same — stated as the reader, over synthetic locks that each break the rule one
/// way, so the check above is known to fail on a second copy and on an older one rather
/// than only to pass on the tree as it stands.
#[test]
fn a_second_or_older_locked_copy_is_reported() {
    let engine = &ENGINE_CRATES[0];
    // (case, the `[[package]]` entries of a synthetic lock, how many reports it owes)
    let cases = [
        (
            "one copy at the pinned release",
            "[[package]]\nname = \"rusqlite\"\nversion = \"0.41.1\"\n",
            0,
        ),
        (
            "one copy past the pinned release",
            "[[package]]\nname = \"rusqlite\"\nversion = \"0.42.0\"\n",
            0,
        ),
        (
            "two copies, both at or past it",
            "[[package]]\nname = \"rusqlite\"\nversion = \"0.41.1\"\n\
             [[package]]\nname = \"rusqlite\"\nversion = \"0.42.0\"\n",
            1,
        ),
        (
            "two copies, one of them the release the pin replaced",
            "[[package]]\nname = \"rusqlite\"\nversion = \"0.41.1\"\n\
             [[package]]\nname = \"rusqlite\"\nversion = \"0.37.0\"\n",
            2,
        ),
        (
            "one copy, below the pinned release",
            "[[package]]\nname = \"rusqlite\"\nversion = \"0.37.0\"\n",
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
        let reports =
            locked_copy_reports(&locked_packages(lock), engine.case, engine.name, bound(engine));
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
/// The rows are, for each floor: the floor itself, one step past it in each of the three
/// components a release is written in, one step below it, and the release the service
/// manifests name as the one that carried the disclosure.
#[test]
fn the_release_comparison_spans_the_pinned_bound() {
    // (case, the floor, the release, whether a build may link it)
    let cases = [
        ("the binding floor itself", "0.41.0", "0.41.0", true),
        ("one patch past the binding floor", "0.41.0", "0.41.1", true),
        ("one minor past the binding floor", "0.41.0", "0.42.0", true),
        ("one major past the binding floor", "0.41.0", "1.0.0", true),
        ("one minor below the binding floor", "0.41.0", "0.40.9", false),
        ("the binding release that carried it", "0.41.0", "0.37.0", false),
        ("the library floor itself", "0.39.0", "0.39.0", true),
        ("one patch past the library floor", "0.39.0", "0.39.1", true),
        ("one minor past the library floor", "0.39.0", "0.40.0", true),
        ("one major past the library floor", "0.39.0", "1.0.0", true),
        ("one minor below the library floor", "0.39.0", "0.38.9", false),
        ("the library release that carried it", "0.39.0", "0.35.0", false),
    ];
    for case in cases {
        let (name, floor, release, allowed) = case;
        let bound = ReleaseBound::AtOrPast(floor);
        assert_eq!(
            bound.admits(release),
            Some(allowed),
            "case {name:?}: expected release {release:?} against the floor {floor:?} to be \
             allowed = {allowed}, got {:?}",
            bound.admits(release)
        );
    }
    // A release that is not written as numbers is reported unreadable rather than
    // acceptable, so no check above can pass on one.
    assert_eq!(
        ReleaseBound::AtOrPast("0.41.0").admits("not-a-release"),
        None,
        "case \"an unreadable release\": expected no comparison at all, got {:?}",
        ReleaseBound::AtOrPast("0.41.0").admits("not-a-release")
    );
}

/// RULE: every service crate that declares the SQL engine binding requires the locked copy
/// and no earlier release — so a manifest cannot quietly take the build back to the engine
/// the pinned release replaced.
///
/// Checked for every dependency table a manifest declares it in (`[dependencies]` and
/// `[dev-dependencies]` both), against the single locked version the check above pins, with a
/// `workspace = true` declaration read through the root table it defers to.
#[test]
fn every_service_manifest_requires_the_pinned_engine_release() {
    let (locked, workspace_deps, manifests) = lock_and_service_manifests();
    let locked_version = locked
        .get(ENGINE_BINDING)
        .and_then(|versions| versions.first())
        .unwrap_or_else(|| panic!("expected Cargo.lock to carry {ENGINE_BINDING:?}"))
        .clone();
    let refused = ENGINE_CRATES[0].refused;

    let mut declaring: BTreeSet<&str> = BTreeSet::new();
    let mut reports = Vec::new();
    for (manifest_path, text) in &manifests {
        let (declared, manifest_reports) = pinned_requirement_reports(
            manifest_path,
            text,
            &workspace_deps,
            &[ENGINE_BINDING],
            &locked_version,
            refused,
        );
        if declared > 0 {
            declaring.insert(manifest_path.as_str());
        }
        reports.extend(manifest_reports);
    }

    assert!(
        reports.is_empty(),
        "expected every service manifest declaring {ENGINE_BINDING} to require the pinned \
         release, got {} report(s): {reports:#?}",
        reports.len()
    );
    // Not vacuous: the SQL data planes of this emulator declare the engine, so there are
    // requirements to check. Athena, Amazon RDS, the RDS Data API and Redshift Data each
    // run caller SQL on it.
    assert!(
        declaring.len() >= 4,
        "expected at least the 4 SQL data planes of this emulator to declare \
         {ENGINE_BINDING}, got {} ({declaring:?})",
        declaring.len()
    );
}

/// RULE: the same — stated as the reader, over synthetic manifests, so the check above is
/// known to fail on a requirement that admits the release the pin replaced.
///
/// The rows span the declaration forms the tree uses: a bare requirement, an inline table
/// with features, a requirement spread over several lines, a dev-dependency, an exact pin,
/// a renamed dependency, the wide range form the vendored telemetry crate uses — which a
/// service manifest may not take — and a declaration deferred to a root table that does not
/// declare it, which resolves to nothing to read here.
#[test]
fn a_manifest_requirement_admitting_the_replaced_release_is_reported() {
    // (case, the manifest, how many requirements it declares, how many reports it owes)
    let cases = [
        (
            "a bare requirement at the pin",
            "[dependencies]\nrusqlite = \"0.41\"\n",
            1,
            0,
        ),
        (
            "an inline table with features",
            "[dependencies]\nrusqlite = { version = \"0.41\", features = [\"bundled\"] }\n",
            1,
            0,
        ),
        (
            "a requirement spread over several lines",
            "[dependencies]\nrusqlite = { version = \"0.41\", features = [\n  \"bundled\",\n] }\n",
            1,
            0,
        ),
        (
            "the same crate in two tables",
            "[dependencies]\nrusqlite = \"0.41\"\n\n[dev-dependencies]\nrusqlite = \"0.41\"\n",
            2,
            0,
        ),
        (
            "an exact pin on the locked release",
            "[dependencies]\nrusqlite = \"=0.41.1\"\n",
            1,
            0,
        ),
        (
            "the release the pin replaced",
            "[dependencies]\nrusqlite = \"0.37\"\n",
            1,
            2,
        ),
        (
            "a dev-dependency on the release the pin replaced",
            "[dev-dependencies]\nrusqlite = { version = \"0.37\", features = [\"bundled\"] }\n",
            1,
            2,
        ),
        (
            "the wide range the vendored telemetry crate uses",
            "[dependencies]\nrusqlite = { version = \">=0.37, <0.42\" }\n",
            1,
            1,
        ),
        (
            "the binding under another key",
            "[dependencies]\nsql = { version = \"0.37\", package = \"rusqlite\" }\n",
            1,
            2,
        ),
        (
            "a crate that is not the binding",
            "[dependencies]\nsqlparser = \"0.59\"\n",
            0,
            0,
        ),
        (
            // A declaration deferred to a root `[workspace.dependencies]` table that does
            // not declare it resolves to nothing to read here, so it is counted as no
            // requirement rather than reported as one without a version: the unresolvable
            // state is `every_dependency_a_workspace_manifest_declares_is_locked`'s to
            // report.
            "the binding deferred to a root table that does not declare it",
            "[dependencies]\nrusqlite = { workspace = true }\n",
            0,
            0,
        ),
    ];
    for case in cases {
        let (name, manifest, declared, expected) = case;
        let (counted, reports) = pinned_requirement_reports(
            name,
            manifest,
            &BTreeMap::new(),
            &[ENGINE_BINDING],
            "0.41.1",
            "0.37.0",
        );
        assert_eq!(
            counted, declared,
            "case {name:?}: expected {declared} requirement(s) on the engine binding, got \
             {counted}"
        );
        assert_eq!(
            reports.len(),
            expected,
            "case {name:?}: expected {expected} report(s), got {reports:#?}"
        );
    }
}
