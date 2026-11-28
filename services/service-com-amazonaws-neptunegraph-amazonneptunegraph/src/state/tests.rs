// SPDX-License-Identifier: CC0-1.0

//! Unit cover for this service's store: the id minting, the record lifecycles and the
//! refusal wording `crate::state` states once for every operation to reach.

use super::*;

/// Every minted id matches the modelled `GraphId` pattern and no id repeats.
#[test]
fn minted_ids_match_the_modelled_pattern_and_are_unique() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2_000 {
        let id = state.next_graph_id();
        assert!(
            is_graph_id(&id),
            "id {id:?}: expected a match for ^g-[a-z0-9]{{10}}$"
        );
        assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
    }
}

/// Every minted VPC endpoint id matches the modelled `VpcEndpointId` pattern
/// `^vpce-[0-9a-f]{17}$` and no id repeats.
#[test]
fn minted_vpc_endpoint_ids_match_the_modelled_pattern_and_are_unique() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2_000 {
        let id = state.next_vpc_endpoint_id();
        let suffix = id
            .strip_prefix("vpce-")
            .unwrap_or_else(|| panic!("id {id:?}: expected the vpce- prefix"));
        assert_eq!(
            suffix.len(),
            17,
            "id {id:?}: expected 17 characters after the prefix, got {}",
            suffix.len()
        );
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')),
            "id {id:?}: expected a match for ^vpce-[0-9a-f]{{17}}$"
        );
        assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
    }
}

/// The query store is BOUNDED at its growth site: it refuses a query beyond
/// [`MAX_QUERIES`] and a text longer than [`MAX_QUERY_STRING_CHARS`], and it
/// reclaims a query the clock has made due even when nobody cancels it.
#[test]
fn the_query_store_is_bounded_at_its_growth_site() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let now = Instant::now();

    // Filling the store to the cap is accepted, spread over two graphs so the
    // bound is on the store rather than on one graph's bucket.
    for index in 0..MAX_QUERIES {
        let graph = if index % 2 == 0 { "g-0000000000" } else { "g-0000000001" };
        let result = state.insert_query(Query::running(
            format!("q-{index}"),
            graph,
            "MATCH (n) RETURN n",
            now,
        ));
        assert!(
            result.is_ok(),
            "case query {index}: expected the store to accept it, got {result:?}"
        );
    }
    assert_eq!(
        state.query_count(),
        MAX_QUERIES,
        "case filled to the cap: expected {MAX_QUERIES} queries, got {}",
        state.query_count()
    );

    // One more is refused, and replacing an entry already filed is not growth.
    let over = state.insert_query(Query::running("q-one-too-many", "g-0000000000", "RETURN 1", now));
    assert!(
        over.is_err(),
        "case one past the cap: expected a refusal, got {over:?}"
    );
    let replacement = state.insert_query(Query::running("q-0", "g-0000000000", "RETURN 2", now));
    assert!(
        replacement.is_ok(),
        "case replacing an entry at the cap: expected acceptance, got {replacement:?}"
    );
    assert_eq!(
        state.query_count(),
        MAX_QUERIES,
        "case replacing an entry at the cap: the count must not grow, got {}",
        state.query_count()
    );

    // A text at the bound is accepted and one past it is refused.
    let mut empty = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    for (case, chars, accepted) in [
        ("one character", 1, true),
        ("at the bound", MAX_QUERY_STRING_CHARS, true),
        ("one past the bound", MAX_QUERY_STRING_CHARS + 1, false),
    ] {
        let text = "x".repeat(chars);
        let result = empty.insert_query(Query::running("q-sized", "g-0000000000", text, now));
        assert_eq!(
            result.is_ok(),
            accepted,
            "case {case:?}: expected accepted={accepted}, got {result:?}"
        );
    }

    // And the clock reclaims a query whose own timeout has run out, with no handler
    // having to sleep for it.
    state.reap_terminated_queries(now + DEFAULT_QUERY_TIMEOUT);
    assert_eq!(
        state.query_count(),
        0,
        "case timeout out: expected an empty store, got {}",
        state.query_count()
    );
    assert!(
        state.queries.is_empty(),
        "case timeout out: the per-graph buckets must go too, got {:?}",
        state.queries.keys().collect::<Vec<_>>()
    );
}

/// A cancel can only SHORTEN a query's life: it brings the end forward when it is
/// accepted early, and a late cancel cannot carry the query past the timeout it
/// was already going to end at.
#[test]
fn a_cancel_only_shortens_a_querys_life() {
    let now = Instant::now();
    let tick = Duration::from_millis(1);

    let mut early = Query::running("q-early", "g-0000000000", "RETURN 1", now);
    early.begin_cancellation(now);
    assert!(
        !early.has_terminated(now + CANCELLATION_SPAN - tick),
        "case cancelled at once: the span is never zero, so the query is still stopping"
    );
    assert!(
        early.has_terminated(now + CANCELLATION_SPAN),
        "case cancelled at once: expected the cancellation to end it a span later"
    );

    let mut late = Query::running("q-late", "g-0000000000", "RETURN 1", now);
    late.begin_cancellation(now + DEFAULT_QUERY_TIMEOUT - tick);
    assert!(
        late.has_terminated(now + DEFAULT_QUERY_TIMEOUT),
        "case cancelled late: a cancel must not carry the query past its timeout"
    );

    // A query nobody cancels is reported for as long as it may run: its timeout, which
    // is the request's own when it asked for one, is the only thing that ends it.
    let requested = Duration::from_millis(250);
    let bounded = Query::running("q-bounded", "g-0000000000", "RETURN 1", now)
        .with_timeout(requested);
    assert!(
        !bounded.has_terminated(now + requested - tick),
        "case a requested timeout: the query runs until its timeout is out"
    );
    assert!(
        bounded.has_terminated(now + requested),
        "case a requested timeout: expected the timeout to end it"
    );
    assert!(
        !Query::running("q-default", "g-0000000000", "RETURN 1", now)
            .has_terminated(now + requested),
        "case the default timeout: a query nobody cancelled must still be in flight"
    );
}

/// Every minted export-task id matches the modelled `ExportTaskId` pattern
/// `^t-[a-z0-9]{10}$` and no id repeats — the same contract the graph-id generator
/// has, over the generator this slice added.
#[test]
fn minted_export_task_ids_match_the_modelled_pattern_and_are_unique() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2_000 {
        let id = state.next_export_task_id();
        assert!(
            is_task_id(&id),
            "id {id:?}: expected a match for ^t-[a-z0-9]{{10}}$"
        );
        assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
    }
}

/// An export task's progression is owed by the CLOCK, not by the read: it reports
/// `INITIALIZING` until the first span has passed, `EXPORTING` while the run is in
/// flight, and then the terminal `SUCCEEDED` with no reason to explain — and
/// settling again changes nothing.
///
/// The rows span the instants a read can fall on, including each span's own bound
/// and one tick short of it, so no phase can be skipped, and one far beyond the last
/// phase, so a task first read long after it started still lands on the status it
/// would have reached.
#[test]
fn an_export_task_settles_through_its_run_into_the_modelled_success_state() {
    let now = Instant::now();
    let tick = Duration::from_millis(1);

    // (case, when the task is read, the status due by then, the progress reported)
    let cases: &[(&str, Duration, ExportTaskStatus, i32)] = &[
        ("read at once", Duration::ZERO, ExportTaskStatus::Initializing, 0),
        ("read one tick in", tick, ExportTaskStatus::Initializing, 0),
        (
            "read one tick before the run is due",
            EXPORT_TASK_SPAN - tick,
            ExportTaskStatus::Initializing,
            0,
        ),
        (
            "read when the run falls due",
            EXPORT_TASK_SPAN,
            ExportTaskStatus::Exporting,
            0,
        ),
        (
            "read halfway through the run",
            EXPORT_TASK_SPAN + EXPORT_TASK_SPAN / 2,
            ExportTaskStatus::Exporting,
            50,
        ),
        (
            "read one tick before the run is over",
            EXPORT_TASK_SPAN * 2 - tick,
            ExportTaskStatus::Exporting,
            MAX_IN_FLIGHT_PROGRESS_PERCENTAGE,
        ),
        (
            "read when the run is over",
            EXPORT_TASK_SPAN * 2,
            ExportTaskStatus::Succeeded,
            COMPLETE_PROGRESS_PERCENTAGE,
        ),
        (
            "read long after the run is over",
            EXPORT_TASK_SPAN * 20,
            ExportTaskStatus::Succeeded,
            COMPLETE_PROGRESS_PERCENTAGE,
        ),
    ];
    for (case, after, expected, progress) in cases {
        let mut task = sample_export_task("t-0123456789", "g-0000000000");
        task.started_at = now;
        task.exported_vertices = 7;
        task.exported_edges = 3;
        task.settles_at = ExportTaskStatus::Initializing
            .settles_in()
            .map(|span| now + span);

        task.settle(now + *after);
        assert_eq!(
            task.status, *expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            task.status
        );
        assert_eq!(
            task.status_reason, None,
            "case {case:?}: a run that was not stopped short has nothing to \
                 explain, got {:?}",
            task.status_reason
        );
        let details = task.details(now + *after);
        assert_eq!(
            details.progress_percentage, *progress,
            "case {case:?}: expected progressPercentage {progress}, got {}",
            details.progress_percentage
        );
        // The elements are reported as WRITTEN only once the run is over.
        let expected_counts = if *expected == ExportTaskStatus::Succeeded {
            (Some(7), Some(3))
        } else {
            (None, None)
        };
        assert_eq!(
            (details.num_vertices_written, details.num_edges_written),
            expected_counts,
            "case {case:?}: expected the written-element counts {expected_counts:?}, \
                 got {:?}",
            (details.num_vertices_written, details.num_edges_written)
        );
        assert_eq!(
            task.elapsed_seconds(now + *after),
            i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            "case {case:?}: expected the whole seconds since the task started, got {}",
            task.elapsed_seconds(now + *after)
        );

        // Settling again at the same instant leaves the record where it is: each
        // transition is owed once, and a read never moves a record on by itself.
        task.settle(now + *after);
        assert_eq!(
            task.status, *expected,
            "case {case:?}: a second settle must not change the status, expected \
                 {expected:?}, got {:?}",
            task.status
        );
    }
}

/// Which status an export task is owed, how long after the status before it, whether
/// it can still be cancelled and the reason it carries are all decided by the status
/// it is in — one row per status this slice stores.
#[test]
fn an_export_task_status_decides_what_the_clock_still_owes_it() {
    /// One row: a stored status and everything it decides.
    struct Row {
        status: ExportTaskStatus,
        cancellable: bool,
        terminal: bool,
        settles_into: Option<ExportTaskStatus>,
        settles_in: Option<Duration>,
        reason: Option<&'static str>,
    }

    let cases = [
        Row {
            status: ExportTaskStatus::Initializing,
            cancellable: true,
            terminal: false,
            settles_into: Some(ExportTaskStatus::Exporting),
            settles_in: Some(EXPORT_TASK_SPAN),
            reason: None,
        },
        Row {
            status: ExportTaskStatus::Exporting,
            cancellable: true,
            terminal: false,
            settles_into: Some(ExportTaskStatus::Succeeded),
            settles_in: Some(EXPORT_TASK_SPAN),
            reason: None,
        },
        Row {
            status: ExportTaskStatus::Succeeded,
            cancellable: false,
            terminal: true,
            settles_into: None,
            settles_in: None,
            reason: None,
        },
        Row {
            status: ExportTaskStatus::Cancelling,
            cancellable: false,
            terminal: false,
            settles_into: Some(ExportTaskStatus::Cancelled),
            settles_in: Some(CANCELLATION_SPAN),
            reason: None,
        },
        Row {
            status: ExportTaskStatus::Cancelled,
            cancellable: false,
            terminal: true,
            settles_into: None,
            settles_in: None,
            reason: Some(EXPORT_CANCELLED_REASON),
        },
    ];
    for row in cases {
        let status = row.status;
        let actual = status.is_cancellable();
        assert_eq!(
            actual, row.cancellable,
            "case {status:?}: expected cancellable={}, got {actual}",
            row.cancellable
        );
        let actual = status.is_terminal();
        assert_eq!(
            actual, row.terminal,
            "case {status:?}: expected terminal={}, got {actual}",
            row.terminal
        );
        let actual = status.settles_into();
        assert_eq!(
            actual, row.settles_into,
            "case {status:?}: expected it to settle into {:?}, got {actual:?}",
            row.settles_into
        );
        let actual = status.settles_in();
        assert_eq!(
            actual, row.settles_in,
            "case {status:?}: expected it to settle in {:?}, got {actual:?}",
            row.settles_in
        );
        let actual = status.reason();
        assert_eq!(
            actual, row.reason,
            "case {status:?}: expected the reason {:?}, got {actual:?}",
            row.reason
        );
    }
}

/// A cancelled export task reports the modelled transient status until the clock
/// makes its end due, and then stops `CANCELLED` — not the `SUCCEEDED` outcome the
/// uncancelled task would have reached.
///
/// The rows span the instants a read can fall on, including the span's own bound and
/// one tick short of it, so the transient status cannot be skipped.
#[test]
fn a_cancelled_export_task_settles_into_the_modelled_cancelled_state() {
    let now = Instant::now();
    let tick = Duration::from_millis(1);

    // (case, when the task is read, whether the cancellation is through by then)
    let cases: &[(&str, Duration, bool)] = &[
        ("read at once", Duration::ZERO, false),
        ("read one tick in", tick, false),
        (
            "read one tick before the span is out",
            CANCELLATION_SPAN - tick,
            false,
        ),
        ("read when the span is out", CANCELLATION_SPAN, true),
        ("read a span later", CANCELLATION_SPAN * 2, true),
    ];
    for (case, after, through) in cases {
        let mut task = sample_export_task("t-0123456789", "g-0000000000");
        task.settles_at = Some(now + EXPORT_TASK_SPAN);
        task.begin_cancellation(now);
        assert_eq!(
            task.status,
            ExportTaskStatus::Cancelling,
            "case {case:?}: an accepted cancellation reports CANCELLING, got {:?}",
            task.status
        );

        task.settle(now + *after);
        let (expected, expected_reason) = if *through {
            (
                ExportTaskStatus::Cancelled,
                Some(EXPORT_CANCELLED_REASON.to_string()),
            )
        } else {
            (ExportTaskStatus::Cancelling, None)
        };
        assert_eq!(
            task.status, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            task.status
        );
        assert_eq!(
            task.status_reason, expected_reason,
            "case {case:?}: expected the reason {expected_reason:?}, got {:?}",
            task.status_reason
        );
    }
}

/// What an export WRITES is the elements the request's filter selects out of the
/// source graph's stored data: all of them when no filter is supplied, exactly the
/// elements carrying a label the filter names when one is, and nothing when the
/// filter names no label on that side.
///
/// The rows span the forms a filter takes — absent, empty, one side only, both
/// sides, a label the graph does not hold, a label shared by several elements — over
/// a graph holding multi-labelled nodes and edges of two labels.
#[test]
fn an_export_writes_the_elements_its_filter_selects_out_of_the_stored_graph() {
    let as_of = SystemTime::UNIX_EPOCH;
    let mut data = GraphData::empty(as_of);
    for (id, labels) in [
        ("n1", ["person"].as_slice()),
        ("n2", ["person", "employee"].as_slice()),
        ("n3", ["city"].as_slice()),
    ] {
        data.insert_node(
            id,
            GraphNode {
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                properties: BTreeMap::new(),
            },
            as_of,
        )
        .expect("the sample graph takes its nodes");
    }
    for (id, label, from, to) in [
        ("e1", "knows", "n1", "n2"),
        ("e2", "knows", "n2", "n1"),
        ("e3", "livesIn", "n1", "n3"),
    ] {
        data.insert_edge(
            id,
            GraphEdge {
                label: label.to_string(),
                from_node: from.to_string(),
                to_node: to.to_string(),
                properties: BTreeMap::new(),
            },
            as_of,
        )
        .expect("the sample graph takes its edges");
    }

    /// A per-label filter map over `labels`, each label selecting every property of
    /// the elements carrying it (`ExportFilterElement`'s `properties` is optional).
    fn per_label(labels: &[&str]) -> HashMap<String, crate::ops::types::ExportFilterElement> {
        labels
            .iter()
            .map(|label| {
                (
                    (*label).to_string(),
                    crate::ops::types::ExportFilterElement { properties: None },
                )
            })
            .collect()
    }

    // (case, the filter the request carries, the vertices written, the edges written)
    let cases: Vec<(&str, Option<ExportFilter>, i64, i64)> = vec![
        ("no filter at all exports every element", None, 3, 3),
        (
            "an empty filter object exports nothing",
            Some(ExportFilter {
                vertex_filter: None,
                edge_filter: None,
            }),
            0,
            0,
        ),
        (
            "a vertex filter alone exports no edge",
            Some(ExportFilter {
                vertex_filter: Some(per_label(&["person"])),
                edge_filter: None,
            }),
            2,
            0,
        ),
        (
            "an edge filter alone exports no vertex",
            Some(ExportFilter {
                vertex_filter: None,
                edge_filter: Some(per_label(&["knows"])),
            }),
            0,
            2,
        ),
        (
            "both sides filtered on one label each",
            Some(ExportFilter {
                vertex_filter: Some(per_label(&["city"])),
                edge_filter: Some(per_label(&["livesIn"])),
            }),
            1,
            1,
        ),
        (
            "a secondary label of a multi-labelled node",
            Some(ExportFilter {
                vertex_filter: Some(per_label(&["employee"])),
                edge_filter: Some(per_label(&["knows", "livesIn"])),
            }),
            1,
            3,
        ),
        (
            "a label the graph does not hold selects nothing",
            Some(ExportFilter {
                vertex_filter: Some(per_label(&["product"])),
                edge_filter: Some(per_label(&["bought"])),
            }),
            0,
            0,
        ),
        (
            "a filter naming no label at all selects nothing",
            Some(ExportFilter {
                vertex_filter: Some(HashMap::new()),
                edge_filter: Some(HashMap::new()),
            }),
            0,
            0,
        ),
    ];
    for (case, filter, vertices, edges) in cases {
        let actual = exported_element_counts(&data, filter.as_ref());
        assert_eq!(
            actual,
            (vertices, edges),
            "case {case:?}: expected {:?} vertices and edges written, got {actual:?}",
            (vertices, edges)
        );
    }

    // A graph holding nothing exports nothing, whatever the filter says.
    let empty = GraphData::empty(as_of);
    let actual = exported_element_counts(&empty, None);
    assert_eq!(
        actual,
        (0, 0),
        "case an empty graph: expected nothing written, got {actual:?}"
    );
}

/// `destination` is an Amazon S3 URI: the `s3://` scheme and a bucket. The rows
/// span the scheme (absent, another one, upper-cased), the bucket segment (named,
/// missing, empty) and the key prefix.
#[test]
fn is_s3_uri_accepts_only_an_amazon_s3_uri() {
    for (candidate, accepted) in [
        ("s3://export-bucket", true),
        ("s3://export-bucket/key", true),
        ("s3://export-bucket/a/b/c", true),
        ("s3://export.bucket.with.dots/prefix", true),
        ("s3://a", true),
        ("s3://", false),
        ("s3:///key", false),
        ("export-bucket/key", false),
        ("https://export-bucket.s3.amazonaws.com/key", false),
        ("file:///tmp/export-bucket", false),
        ("S3://export-bucket", false),
        ("", false),
    ] {
        let actual = is_s3_uri(candidate);
        assert_eq!(
            actual, accepted,
            "case {candidate:?}: expected accepted={accepted}, got {actual}"
        );
    }
}

/// The id shape test, over values that break each clause of the pattern.
#[test]
fn is_graph_id_rejects_anything_the_modelled_pattern_rejects() {
    for (candidate, accepted) in [
        ("g-0123456789", true),
        ("g-abcdefghij", true),
        ("g-zz99zz99zz", true),
        ("g-tooshort", false),
        ("g-waytoolongforten", false),
        ("g-ABCDEFGHIJ", false),
        ("gs-0123456789", false),
        ("0123456789", false),
        ("", false),
    ] {
        let actual = is_graph_id(candidate);
        assert_eq!(
            actual, accepted,
            "case {candidate:?}: expected accepted={accepted}, got {actual}"
        );
    }
}

/// Only the OUTER anchors of a `@pattern` are dropped, over a row per form an
/// expression can take: anchored at both ends, at one end, not at all, an escaped
/// trailing `$`, and anchors inside an alternation.
#[test]
fn unanchored_drops_only_the_outer_anchors() {
    // (case, pattern, the expression the requirement must report)
    let cases: &[(&str, &str, &str)] = &[
        // The modelled `GraphIdentifier` / `GraphId` pattern.
        ("anchored at both ends", "^g-[a-z0-9]{10}$", "g-[a-z0-9]{10}"),
        // The modelled `SnapshotId` pattern.
        ("another anchored pattern", "^gs-[a-z0-9]{10}$", "gs-[a-z0-9]{10}"),
        ("anchored at the start only", "^g-[a-z0-9]+", "g-[a-z0-9]+"),
        ("anchored at the end only", "[a-z0-9]+$", "[a-z0-9]+"),
        ("no anchors at all", "[a-z0-9]+", "[a-z0-9]+"),
        ("an escaped trailing dollar", "^cost\\$", "cost\\$"),
        ("an escaped backslash before the anchor", "^path\\\\$", "path\\\\"),
        ("anchors inside an alternation", "^(^a$|^b$)$", "(^a$|^b$)"),
        ("empty", "", ""),
    ];
    for (case, pattern, expected) in cases {
        let actual = unanchored(pattern);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The `ValidationException` envelope counts the clauses it carries, singular for
/// one and plural beyond.
#[test]
fn the_validation_envelope_counts_its_clauses() {
    let clause = |index: usize| {
        constraint_clause(
            &format!("member{index}"),
            Some("bad"),
            &pattern_requirement("^[a-z]+$"),
        )
    };
    for count in 1..=3 {
        let clauses: Vec<String> = (0..count).map(clause).collect();
        let actual = validation_envelope(&clauses);
        let expected_prefix = if count == 1 {
            "1 validation error detected: ".to_string()
        } else {
            format!("{count} validation errors detected: ")
        };
        assert!(
            actual.starts_with(&expected_prefix),
            "case {count} clauses: expected a message opening {expected_prefix:?}, \
                 got {actual:?}"
        );
        assert_eq!(
            actual.matches("failed to satisfy constraint").count(),
            count,
            "case {count} clauses: expected {count} clauses, got {actual:?}"
        );
    }
}

/// An absent member is reported as the null it was; a supplied one quotes its value.
#[test]
fn a_constraint_clause_states_the_value_it_rejected() {
    // (case, the value the request carried, the subject the clause must open with)
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("a supplied value", Some("not-a-valid-id"), "Value 'not-a-valid-id' at 'graphIdentifier'"),
        ("an empty value", Some(""), "Value '' at 'graphIdentifier'"),
        ("an absent member", None, "Value null at 'graphIdentifier'"),
    ];
    for (case, value, expected_subject) in cases {
        let actual = constraint_clause(
            "graphIdentifier",
            *value,
            &pattern_requirement("^g-[a-z0-9]{10}$"),
        );
        assert!(
            actual.starts_with(expected_subject),
            "case {case:?}: expected a clause opening {expected_subject:?}, got {actual:?}"
        );
    }
}

/// The bound on what one stored record's tags may hold, at the limit and one past
/// it for the entry count, the key length and the value length — and the CLAUSE each
/// refusal states, written out here rather than built from the service's own
/// builders, so the wording is pinned independently of the code under test.
#[test]
fn tag_bounds_accept_the_limit_and_refuse_one_past_it() {
    let map = |pairs: Vec<(String, String)>| -> HashMap<String, String> {
        pairs.into_iter().collect()
    };
    let many = |count: usize| {
        map((0..count)
            .map(|index| (format!("key{index}"), "v".to_string()))
            .collect())
    };

    // (case, tags, the clause the refusal states, or `None` when the map is accepted)
    let cases: Vec<(&str, HashMap<String, String>, Option<String>)> = vec![
        ("no tags", map(Vec::new()), None),
        ("at the entry limit", many(MAX_TAGS), None),
        (
            "one entry past the limit",
            many(MAX_TAGS + 1),
            Some(format!(
                "Value '{} entries' at 'tags' failed to satisfy constraint: Member \
                     must have length less than or equal to {MAX_TAGS}",
                MAX_TAGS + 1
            )),
        ),
        (
            "a key at the length limit",
            map(vec![("k".repeat(MAX_TAG_KEY_CHARS), "v".to_string())]),
            None,
        ),
        (
            "a key one character past the limit",
            map(vec![("k".repeat(MAX_TAG_KEY_CHARS + 1), "v".to_string())]),
            Some(format!(
                "Value at 'tags' failed to satisfy constraint: Map keys must satisfy \
                     constraint: [Member must have length less than or equal to \
                     {MAX_TAG_KEY_CHARS}, Member must have length greater than or equal \
                     to {MIN_TAG_KEY_CHARS}, Member must satisfy regular expression \
                     pattern: {}]",
                unanchored(TAG_KEY_PATTERN)
            )),
        ),
        (
            "a value at the length limit",
            map(vec![("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS))]),
            None,
        ),
        (
            "a value one character past the limit",
            map(vec![("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS + 1))]),
            Some(format!(
                "Value at 'tags.k' failed to satisfy constraint: Member must have \
                     length less than or equal to {MAX_TAG_VALUE_CHARS}"
            )),
        ),
        (
            "two oversized values: the entry refused is the first in key order",
            map(vec![
                ("b".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS + 1)),
                ("a".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS + 9)),
            ]),
            Some(format!(
                "Value at 'tags.a' failed to satisfy constraint: Member must have \
                     length less than or equal to {MAX_TAG_VALUE_CHARS}"
            )),
        ),
    ];
    for (case, tags, expected) in cases {
        let actual = bound_tags(&tags);
        assert_eq!(
            actual.err(),
            expected,
            "case {case:?}: expected {expected:?}"
        );
    }
}

/// Either half of a tag entry is refused when it opens with the reserved prefix,
/// however that prefix is cased, and accepted when the prefix merely occurs
/// somewhere else in the text — stating the clause in the same form the bounds above
/// do: a key against the member the map sits at, a value at its own entry's path.
#[test]
fn a_tag_in_the_reserved_namespace_is_refused_on_either_half() {
    let map = |key: &str, value: &str| -> HashMap<String, String> {
        [(key.to_string(), value.to_string())].into_iter().collect()
    };
    // The prefix rule restated independently of the service, so the expectations
    // below are not read out of the implementation under test.
    let reserved = |text: &str| text.to_lowercase().starts_with("aws:");

    // (case, key, value). The rows differ along which half carries the prefix, how
    // that prefix is cased, and where in the text it sits; what each row must answer
    // follows from the rule, so no row carries a hand-written verdict.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("neither half reserved", "owner", "finance"),
        ("the key is reserved", "aws:owner", "finance"),
        ("the value is reserved", "owner", "aws:finance"),
        ("both halves reserved", "aws:owner", "aws:finance"),
        ("the prefix upper-cased", "AWS:owner", "finance"),
        ("the prefix mixed-cased", "Aws:owner", "finance"),
        ("the value's prefix upper-cased", "owner", "AWS:finance"),
        ("the key is exactly the prefix", "aws:", "finance"),
        ("the prefix inside the key, not at its start", "my:aws:owner", "finance"),
        ("the prefix inside the value, not at its start", "owner", "my-aws:finance"),
        ("the letters without the colon", "awsome", "awsome"),
        ("a key shorter than the prefix", "a", "b"),
        ("an empty value", "owner", ""),
        (
            "halves whose prefix-length byte boundary falls inside a character",
            "tāā",
            "ā✓",
        ),
    ];
    for (case, key, value) in cases {
        // A reserved KEY is refused before a reserved value: the entry cannot be
        // stored under that name at all.
        let expected = if reserved(key) {
            Some(format!(
                "Value '{key}' at 'tags' failed to satisfy constraint: Map keys must \
                     satisfy constraint: [Member must not be prefixed with aws:]"
            ))
        } else if reserved(value) {
            Some(format!(
                "Value '{value}' at 'tags.{key}' failed to satisfy constraint: Member \
                     must not be prefixed with aws:"
            ))
        } else {
            None
        };
        let actual = bound_tags(&map(key, value));
        assert_eq!(
            actual.clone().err(),
            expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The post-call entry count `TagResource` checks, at the limit and one past it,
/// stating the count it measured in the form a collection's size is reported in.
#[test]
fn the_tag_entry_bound_accepts_the_limit_and_refuses_one_past_it() {
    for total in [0, 1, MAX_TAGS - 1, MAX_TAGS, MAX_TAGS + 1, MAX_TAGS * 2] {
        let expected = (total > MAX_TAGS).then(|| {
            format!(
                "Value '{total} entries' at 'tags' failed to satisfy constraint: \
                     Member must have length less than or equal to {MAX_TAGS}"
            )
        });
        let actual = bound_tag_count(total);
        assert_eq!(
            actual.err(),
            expected,
            "case total={total}: expected {expected:?}"
        );
    }
}

/// A missing graph is reported scoped to the account the lookup ran in, for every
/// well-formed identifier.
#[test]
fn graph_not_found_names_the_account_and_the_requested_id() {
    for id in ["g-0000000000", "g-zzzzzzzzzz", "g-a1b2c3d4e5"] {
        let actual = graph_not_found(id).message;
        let expected = format!(
            "No graph found for customer [{}] with graph id [{id}]",
            caller_account_id()
        );
        assert_eq!(
            actual, expected,
            "case {id:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Every account-scoped namespace words its miss the same way: the kind of resource it
/// searched for, the account it searched in, and then the identifier clause the operation
/// that took the lookup words — the resource kind and `id` on every read, the bare
/// `task id` on a task cancel.
#[test]
fn a_missing_resource_words_its_operations_identifier_clause() {
    // (the resource kind it reports, the identifier clause the operation words, the
    // wording function, identifiers that name nothing)
    type Case = (
        &'static str,
        &'static str,
        fn(&str) -> ResourceNotFoundException,
        Vec<&'static str>,
    );
    let task_ids = vec!["t-0000000000", "t-a1b2c3d4e5", "t-zzzzzzzzzz"];
    let cases: Vec<Case> = vec![
        (
            "graph",
            "graph id",
            graph_not_found,
            vec!["g-0000000000", "g-a1b2c3d4e5", "g-zzzzzzzzzz"],
        ),
        (
            "snapshot",
            "snapshot id",
            snapshot_not_found,
            vec!["gs-0000000000", "gs-a1b2c3d4e5", "gs-zzzzzzzzzz"],
        ),
        (
            "import task",
            "import task id",
            import_task_not_found,
            task_ids.clone(),
        ),
        (
            "import task",
            "task id",
            import_task_not_found_on_cancel,
            task_ids.clone(),
        ),
        (
            "export task",
            "export task id",
            export_task_not_found,
            task_ids.clone(),
        ),
        (
            "export task",
            "task id",
            export_task_not_found_on_cancel,
            task_ids.clone(),
        ),
        (
            "resource",
            "resource arn",
            resource_not_found,
            vec![
                "arn:aws:neptune-graph:us-east-1:000000000000:graph/g-0000000000",
                "arn:aws-cn:neptune-graph:cn-north-1:000000000000:graph-snapshot/gs-0123456789",
            ],
        ),
    ];

    for (resource, clause, word_it, identifiers) in cases {
        for identifier in identifiers {
            let actual = word_it(identifier).message;
            let expected = format!(
                "No {resource} found for customer [{}] with {clause} [{identifier}]",
                caller_account_id()
            );
            assert_eq!(
                actual, expected,
                "case {resource}/{clause}/{identifier}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // One message per kind written out in full, so the clause text itself is pinned
    // and not only its parameterisation. Only the account segment is read from the
    // framework, because that is the one part no test may hard-code.
    let account = caller_account_id();
    let literal = [
        (
            graph_not_found("g-0123456789").message,
            format!("No graph found for customer [{account}] with graph id [g-0123456789]"),
        ),
        (
            snapshot_not_found("gs-0123456789").message,
            format!(
                "No snapshot found for customer [{account}] with snapshot id [gs-0123456789]"
            ),
        ),
        (
            import_task_not_found("t-0123456789").message,
            format!(
                "No import task found for customer [{account}] with import task id \
                     [t-0123456789]"
            ),
        ),
        (
            export_task_not_found("t-0123456789").message,
            format!(
                "No export task found for customer [{account}] with export task id \
                     [t-0123456789]"
            ),
        ),
        (
            import_task_not_found_on_cancel("t-0123456789").message,
            format!("No import task found for customer [{account}] with task id [t-0123456789]"),
        ),
        (
            export_task_not_found_on_cancel("t-0123456789").message,
            format!("No export task found for customer [{account}] with task id [t-0123456789]"),
        ),
        (
            resource_not_found("arn:aws:neptune-graph:us-east-1:000000000000:graph/g-0123456789")
                .message,
            format!(
                "No resource found for customer [{account}] with resource arn \
                     [arn:aws:neptune-graph:us-east-1:000000000000:graph/g-0123456789]"
            ),
        ),
    ];
    for (actual, expected) in literal {
        assert_eq!(
            actual, expected,
            "case the whole message: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A missing private graph endpoint is reported from the addressing pair the
/// request carried, for every well-formed pair.
#[test]
fn private_endpoint_not_found_names_the_graph_then_the_vpc() {
    for (graph_id, vpc_id) in [
        ("g-0000000000", "vpc-1"),
        ("g-a1b2c3d4e5", "vpc-0000000000000001e"),
        ("g-zzzzzzzzzz", "vpc-abcdef0123456789"),
    ] {
        let actual = private_endpoint_not_found(graph_id, vpc_id).message;
        let expected =
            format!("Private Graph Endpoint to Graph: {graph_id} from VPC: {vpc_id} does not exist");
        assert_eq!(
            actual, expected,
            "case {graph_id}/{vpc_id}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The ARN lookup the tagging operations resolve their subject through is EXACT:
/// only the ARN the service minted for a resource names it, so the same resource id
/// under another region, account or partition names no resource in this store.
#[test]
fn tags_of_resolves_only_the_minted_arn() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let id = "g-0123456789";
    let arn = ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{id}"));
    let mut graph = sample_graph(id, &arn, "us-east-1");
    graph.tags.insert("env".to_string(), "prod".to_string());
    state.insert_graph(graph);

    // (case, the ARN a request names, whether it resolves to the stored graph)
    let cases: Vec<(&str, String, bool)> = vec![
        ("the minted ARN", arn.clone(), true),
        (
            "another region",
            ::engine::emulator_arn(ARN_NAMESPACE, "eu-west-1", &format!("graph/{id}")),
            false,
        ),
        (
            "another account",
            format!("arn:aws:{ARN_NAMESPACE}:us-east-1:999999999999:graph/{id}"),
            false,
        ),
        (
            "another resource type",
            ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph-snapshot/{id}")),
            false,
        ),
        (
            "another graph id",
            ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", "graph/g-9999999999"),
            false,
        ),
        ("not an ARN of this service", "arn:aws:s3:::bucket".to_string(), false),
    ];
    for (case, candidate, resolves) in cases {
        let actual = state.tags_of(&candidate, "us-east-1").is_some();
        assert_eq!(
            actual, resolves,
            "case {case:?} ({candidate:?}): expected resolves={resolves}, got {actual}"
        );
        let actual_mut = state.tags_of_mut(&candidate, "us-east-1").is_some();
        assert_eq!(
            actual_mut, resolves,
            "case {case:?} ({candidate:?}) mutably: expected resolves={resolves}, \
                 got {actual_mut}"
        );
    }
}

/// Every lookup and listing is scoped to the Region of the current request: a
/// record of one Region answers only there, and the same NAME in two Regions is
/// two records.
#[test]
fn every_lookup_is_scoped_to_the_region_of_the_request() {
    let now = Instant::now();
    // One graph per Region, all sharing the name `shared`, plus a second graph in
    // one of them so a Region can hold more than one.
    let placements: &[(&str, &str, &str)] = &[
        ("g-0000000001", "us-east-1", "shared"),
        ("g-0000000002", "eu-west-1", "shared"),
        ("g-0000000003", "ap-southeast-2", "shared"),
        ("g-0000000004", "eu-west-1", "other"),
    ];
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    for (id, region, name) in placements {
        let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("graph/{id}"));
        let mut graph = sample_graph(id, &arn, region);
        graph.name = (*name).to_string();
        state.insert_graph(graph);
        state.insert_snapshot(Snapshot {
            id: format!("gs{}", id.trim_start_matches('g')),
            name: (*name).to_string(),
            arn: ::engine::emulator_arn(
                ARN_NAMESPACE,
                region,
                &format!("graph-snapshot/gs{}", id.trim_start_matches('g')),
            ),
            region: (*region).to_string(),
            source_graph_id: (*id).to_string(),
            status: SnapshotStatus::Available,
            snapshot_create_time: SystemTime::UNIX_EPOCH,
            kms_key_identifier: None,
            tags: BTreeMap::new(),
            settles_at: None,
            source_configuration: sample_configuration(),
            data: GraphData::empty(SystemTime::UNIX_EPOCH),
        });
    }

    for (id, region, name) in placements {
        let snapshot_id = format!("gs{}", id.trim_start_matches('g'));
        // The id resolves in its own Region ...
        assert!(
            state.settled_graph(id, region, now).is_some(),
            "case {id:?} in {region:?}: expected the graph to resolve in its own Region"
        );
        assert!(
            state.settled_snapshot(&snapshot_id, region, now).is_some(),
            "case {snapshot_id:?} in {region:?}: expected the snapshot to resolve in \
                 its own Region"
        );
        // ... and in no other.
        for (_, other_region, _) in placements.iter().filter(|(_, r, _)| r != region) {
            let graph = state.settled_graph(id, other_region, now);
            assert!(
                graph.is_none(),
                "case {id:?} from {other_region:?}: expected no graph, got {:?}",
                graph.map(|graph| graph.id.clone())
            );
            let snapshot = state.settled_snapshot(&snapshot_id, other_region, now);
            assert!(
                snapshot.is_none(),
                "case {snapshot_id:?} from {other_region:?}: expected no snapshot, \
                     got {:?}",
                snapshot.map(|snapshot| snapshot.id.clone())
            );
        }
        // The name index is per Region: `shared` names a DIFFERENT graph in each.
        let named = state.graph_id_named(name, region);
        assert_eq!(
            named,
            Some(*id),
            "case name {name:?} in {region:?}: expected {id:?}, got {named:?}"
        );
        let named_snapshot = state.snapshot_id_named(name, region);
        assert_eq!(
            named_snapshot,
            Some(snapshot_id.as_str()),
            "case snapshot name {name:?} in {region:?}: expected {snapshot_id:?}, \
                 got {named_snapshot:?}"
        );
    }

    // A listing reports the Region's own graphs, in id order, and nothing else.
    for (region, expected) in [
        ("us-east-1", vec!["g-0000000001"]),
        ("eu-west-1", vec!["g-0000000002", "g-0000000004"]),
        ("ap-southeast-2", vec!["g-0000000003"]),
        ("sa-east-1", Vec::new()),
    ] {
        let actual: Vec<&str> = state
            .graphs_in(region, None)
            .map(|graph| graph.id.as_str())
            .collect();
        assert_eq!(
            actual, expected,
            "case listing {region:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A delete reaches only the Region it was signed for.
    assert!(
        state.remove_graph("g-0000000002", "us-east-1").is_none(),
        "case deleting an eu-west-1 graph from us-east-1: expected no removal"
    );
    assert!(
        state.remove_graph("g-0000000002", "eu-west-1").is_some(),
        "case deleting an eu-west-1 graph from eu-west-1: expected the removal"
    );
    let named = state.graph_id_named("shared", "us-east-1");
    assert_eq!(
        named,
        Some("g-0000000001"),
        "case after deleting eu-west-1's 'shared': expected us-east-1's graph to \
             keep its name, got {named:?}"
    );
}

/// The captured source-graph configuration a stored snapshot of
/// [`sample_graph`] carries.
fn sample_configuration() -> GraphConfiguration {
    GraphConfiguration::from(&sample_graph("g-0123456789", "arn", "us-east-1"))
}

/// A stored graph with the members every test here needs and nothing else set.
fn sample_graph(id: &str, arn: &str, region: &str) -> Graph {
    Graph {
        id: id.to_string(),
        name: format!("name-for-{id}"),
        arn: arn.to_string(),
        region: region.to_string(),
        status: GraphStatus::Available,
        status_reason: None,
        create_time: SystemTime::UNIX_EPOCH,
        compute: GraphCompute::provisioned(MIN_PROVISIONED_MEMORY),
        endpoint: "endpoint".to_string(),
        public_connectivity: false,
        replica_count: 1,
        kms_key_identifier: None,
        vector_search_configuration: None,
        deletion_protection: false,
        source_snapshot_id: None,
        build_number: GRAPH_SOFTWARE_BUILD_NUMBER.to_string(),
        tags: BTreeMap::new(),
        pending: None,
        deletes_at: None,
        data: GraphData::empty(SystemTime::UNIX_EPOCH),
    }
}

/// A node with the labels and property keys a case names, which is all the data
/// invariants below look at.
fn sample_node(labels: &[&str], property_keys: &[&str]) -> GraphNode {
    GraphNode {
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        properties: property_keys
            .iter()
            .map(|key| ((*key).to_string(), serde_json::Value::from(1)))
            .collect(),
    }
}

/// The data a graph holds accepts a node and an edge between stored nodes,
/// refuses an edge with an endpoint the graph does not hold, derives a node's
/// outgoing edge labels from the edges, and empties on `clear`.
#[test]
fn graph_data_writes_keep_the_edges_joined_to_stored_nodes() {
    let created_at = SystemTime::UNIX_EPOCH;
    let written_at = created_at + Duration::from_secs(1);
    let mut data = GraphData::empty(created_at);

    // (case, the edge to store, whether the data accepts it) — an edge is
    // refused for EITHER endpoint, so there is a row for each.
    data.insert_node("n1", sample_node(&["Person"], &["name"]), written_at)
        .expect("case stored node: expected the write to be accepted");
    data.insert_node("n2", sample_node(&["Person", "Author"], &[]), written_at)
        .expect("case second stored node: expected the write to be accepted");
    for (case, from_node, to_node, accepted) in [
        ("both endpoints stored", "n1", "n2", true),
        ("unknown source", "n9", "n2", false),
        ("unknown target", "n1", "n9", false),
    ] {
        let outcome = data.insert_edge(
            "e1",
            GraphEdge {
                label: "KNOWS".to_string(),
                from_node: from_node.to_string(),
                to_node: to_node.to_string(),
                properties: BTreeMap::new(),
            },
            written_at,
        );
        assert_eq!(
            outcome.is_ok(),
            accepted,
            "case {case:?}: expected accepted={accepted}, got {outcome:?}"
        );
    }

    assert_eq!(
        data.outgoing_edge_labels("n1"),
        BTreeSet::from(["KNOWS".to_string()]),
        "case outgoing labels of the edge's source: got {:?}",
        data.outgoing_edge_labels("n1")
    );
    assert!(
        data.outgoing_edge_labels("n2").is_empty(),
        "case outgoing labels of the edge's target: expected none, got {:?}",
        data.outgoing_edge_labels("n2")
    );

    // A write recomputes the statistics as of the instant it was made; a read
    // leaves them where they are.
    assert_eq!(
        data.statistics_computed_at, written_at,
        "case statistics after a write: expected {written_at:?}, got {:?}",
        data.statistics_computed_at
    );
    assert_eq!(
        data.outgoing_edge_labels("n1").len(),
        1,
        "case reading the data twice: a read must not change it"
    );
    assert_eq!(
        data.statistics_computed_at, written_at,
        "case reading the data: expected the statistics time to stand still"
    );

    let cleared_at = written_at + Duration::from_secs(1);
    data.clear(cleared_at);
    assert!(
        data.nodes.is_empty() && data.edges.is_empty(),
        "case cleared: expected no nodes and no edges, got {} nodes and {} edges",
        data.nodes.len(),
        data.edges.len()
    );
    assert_eq!(
        data.statistics_computed_at, cleared_at,
        "case cleared: expected {cleared_at:?}, got {:?}",
        data.statistics_computed_at
    );
}

/// Each map of a graph's data is bounded at its growth site, and a write that
/// REPLACES an entry is accepted at the bound.
#[test]
fn graph_data_is_bounded_at_every_growth_site() {
    let written_at = SystemTime::UNIX_EPOCH;
    let mut data = GraphData::empty(written_at);
    for index in 0..MAX_NODES_PER_GRAPH {
        data.insert_node(&format!("n{index}"), GraphNode::default(), written_at)
            .unwrap_or_else(|reason| panic!("case node {index}: refused with {reason}"));
    }
    let refused = data.insert_node("one-too-many", GraphNode::default(), written_at);
    assert!(
        refused.is_err(),
        "case node past the bound: expected a refusal, got {refused:?}"
    );
    let replacement =
        data.insert_node("n0", sample_node(&["Person"], &["name"]), written_at);
    assert!(
        replacement.is_ok(),
        "case replacing a node at the bound: expected acceptance, got {replacement:?}"
    );
    assert_eq!(
        data.nodes.len(),
        MAX_NODES_PER_GRAPH,
        "case at the bound: expected {MAX_NODES_PER_GRAPH} nodes, got {}",
        data.nodes.len()
    );

    // One element's properties are bounded too, so the properties a graph holds
    // — and the work a report over them costs — stay bounded by the ceilings.
    let keys: Vec<String> = (0..=MAX_PROPERTIES_PER_ELEMENT)
        .map(|index| format!("p{index}"))
        .collect();
    let over_propertied = GraphNode {
        labels: BTreeSet::new(),
        properties: keys
            .iter()
            .map(|key| (key.clone(), serde_json::Value::from(1)))
            .collect(),
    };
    let refused_properties = data.insert_node("n0", over_propertied, written_at);
    assert!(
        refused_properties.is_err(),
        "case a node past the property bound: expected a refusal, got {refused_properties:?}"
    );
}

/// The transition is owed at a future instant, so a read taken immediately
/// still sees the transient status — and it settles into the status the pending
/// transition names, which is not always `AVAILABLE`.
#[test]
fn a_pending_transition_settles_only_once_it_is_due() {
    let now = Instant::now();
    // (case, the transient status reported until the transition falls due, the
    // status it settles into) — one row per transition this service accepts.
    for (case, transient, settled) in [
        ("provisioning", GraphStatus::Creating, GraphStatus::Available),
        ("reconfiguring", GraphStatus::Updating, GraphStatus::Available),
        (
            "snapshotting",
            GraphStatus::Snapshotting,
            GraphStatus::Available,
        ),
        ("stopping", GraphStatus::Stopping, GraphStatus::Stopped),
        ("starting", GraphStatus::Starting, GraphStatus::Available),
        ("resetting", GraphStatus::Resetting, GraphStatus::Available),
    ] {
        let mut graph = sample_graph("g-0123456789", "arn", "us-east-1");
        graph.begin_transition(transient, settled, now);

        graph.settle(now);
        assert_eq!(
            graph.status, transient,
            "case {case:?} not-yet-due: expected {transient:?}, got {:?}",
            graph.status
        );

        graph.settle(now + PROVISIONING_SPAN);
        assert_eq!(
            graph.status, settled,
            "case {case:?} due: expected {settled:?}, got {:?}",
            graph.status
        );
        assert!(
            graph.pending.is_none(),
            "case {case:?} due: the settled transition must be cleared, got {:?}",
            graph.pending
        );
    }
}

/// An accepted deletion reports `DELETING` and keeps the record \u2014 and the name it
/// reserves \u2014 until the deletion the clock owes falls due, whatever status the
/// graph was resting in when it was deleted.
#[test]
fn a_deleted_graph_is_reclaimed_only_once_the_deletion_is_due() {
    let now = Instant::now();
    // (case, the resting status the graph was deleted from) — a deletion is
    // accepted from either status a graph rests in.
    for (case, resting) in [
        ("serving", GraphStatus::Available),
        ("stopped", GraphStatus::Stopped),
    ] {
        let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
        let mut graph = sample_graph("g-0123456789", "arn", "us-east-1");
        graph.status = resting;
        let name = graph.name.clone();
        state.insert_graph(graph);

        state
            .settled_graph_mut("g-0123456789", "us-east-1", now)
            .expect("the stored graph resolves")
            .begin_deletion(now);

        let in_flight = state.settled_graph("g-0123456789", "us-east-1", now);
        assert_eq!(
            in_flight.map(|graph| graph.status),
            Some(GraphStatus::Deleting),
            "case {case:?} not-yet-due: expected a graph reporting DELETING, got {:?}",
            in_flight.map(|graph| graph.status)
        );
        let reserved = state.graph_id_named(&name, "us-east-1");
        assert_eq!(
            reserved,
            Some("g-0123456789"),
            "case {case:?} not-yet-due: the name must stay reserved, got {reserved:?}"
        );

        let settled = state
            .settled_graph("g-0123456789", "us-east-1", now + DELETION_SPAN)
            .cloned();
        assert!(
            settled.is_none(),
            "case {case:?} due: the record must be gone, got {settled:?}"
        );
        let freed = state.graph_id_named(&name, "us-east-1");
        assert!(
            freed.is_none(),
            "case {case:?} due: the name must be free again, got {freed:?}"
        );
    }
}

/// The name a final snapshot takes: after the graph where that fits and is free,
/// otherwise after the snapshot's own id — and always a name the modelled
/// `SnapshotName` constraints accept. The names it has to avoid are the ones its
/// OWN Region holds, so a row is checked against a snapshot of another Region too.
#[test]
fn a_final_snapshots_name_satisfies_the_modelled_name_constraints() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    state.insert_snapshot(sample_snapshot(
        "gs-0000000000",
        "taken-graph-final-snapshot",
        "us-east-1",
    ));
    // The same name in ANOTHER Region is not in the way: names are per Region.
    state.insert_snapshot(sample_snapshot(
        "gs-0000000001",
        "elsewhere-graph-final-snapshot",
        "eu-west-1",
    ));
    let longest_graph_name = "a".repeat(MAX_RESOURCE_NAME_CHARS);

    // (case, the graph's name, whether the name may be built from the graph's own)
    let cases: &[(&str, &str, bool)] = &[
        ("a free name", "plain-graph", true),
        ("one character", "a", true),
        (
            "a name already held by a stored snapshot",
            "taken-graph",
            false,
        ),
        (
            "a name held only by a snapshot of another Region",
            "elsewhere-graph",
            true,
        ),
        (
            "a graph name that leaves no room for the suffix",
            longest_graph_name.as_str(),
            false,
        ),
        // `GraphName` reserves `g-` and `SnapshotName` reserves `gs-`, so a graph
        // may legitimately carry a name that no snapshot may carry.
        (
            "a graph name opening with the prefix snapshot names reserve",
            "gs-data",
            false,
        ),
        (
            "a graph name that is the reserved prefix without its hyphen",
            "gs",
            false,
        ),
        (
            "a graph name opening with the reserved letters and no hyphen",
            "gsx-data",
            true,
        ),
        (
            "a graph name opening with the prefix GRAPH ids reserve, hyphenless",
            "g0-x",
            true,
        ),
    ];
    for (case, graph_name, after_the_graph) in cases {
        let minted = state.free_final_snapshot_name(graph_name, "gs-1234567890", "us-east-1");
        assert_eq!(
            minted == format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}"),
            *after_the_graph,
            "case {case:?}: expected named-after-the-graph={after_the_graph}, got {minted:?}"
        );
        assert!(
            !minted.is_empty() && minted.len() <= MAX_RESOURCE_NAME_CHARS,
            "case {case:?}: expected a name within 1..{MAX_RESOURCE_NAME_CHARS} characters, \
                 got {minted:?}"
        );
        assert!(
            resource_name_is_well_formed(&minted, SNAPSHOT_NAME_RESERVED_PREFIX),
            "case {case:?}: expected a name the modelled pattern accepts, got {minted:?}"
        );
        let clash = state.snapshot_id_named(&minted, "us-east-1");
        assert!(
            clash.is_none(),
            "case {case:?}: expected a name no stored snapshot of this Region holds, \
                 got {minted:?} ({clash:?})"
        );
    }
}

/// A stored snapshot with the members these tests need and nothing else set.
fn sample_snapshot(id: &str, name: &str, region: &str) -> Snapshot {
    Snapshot {
        id: id.to_string(),
        name: name.to_string(),
        arn: ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("graph-snapshot/{id}")),
        region: region.to_string(),
        source_graph_id: "g-0123456789".to_string(),
        status: SnapshotStatus::Available,
        snapshot_create_time: SystemTime::UNIX_EPOCH,
        kms_key_identifier: None,
        tags: BTreeMap::new(),
        settles_at: None,
        source_configuration: sample_configuration(),
        data: GraphData::empty(SystemTime::UNIX_EPOCH),
    }
}

/// Every minted snapshot id matches the modelled `SnapshotId` pattern, no id
/// repeats, and the graph and snapshot id spaces stay disjoint.
#[test]
fn minted_snapshot_ids_match_the_modelled_pattern_and_are_unique() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2_000 {
        let id = state.next_snapshot_id();
        assert!(
            is_snapshot_id(&id),
            "id {id:?}: expected a match for ^gs-[a-z0-9]{{10}}$"
        );
        assert!(
            !is_graph_id(&id),
            "id {id:?}: a snapshot id must not also be a graph id"
        );
        assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
    }
}

/// The snapshot-id shape test, over values that break each clause of the pattern.
#[test]
fn is_snapshot_id_rejects_anything_the_modelled_pattern_rejects() {
    for (candidate, accepted) in [
        ("gs-0123456789", true),
        ("gs-abcdefghij", true),
        ("gs-zz99zz99zz", true),
        ("gs-tooshort", false),
        ("gs-waytoolongforten", false),
        ("gs-ABCDEFGHIJ", false),
        ("g-0123456789", false),
        ("0123456789", false),
        ("", false),
    ] {
        let actual = is_snapshot_id(candidate);
        assert_eq!(
            actual, accepted,
            "case {candidate:?}: expected accepted={accepted}, got {actual}"
        );
    }
}

/// The shared resource-name rule, with a row per clause of the modelled pattern
/// `^(?!<prefix>)[a-z][a-z0-9]*(-[a-z0-9]+)*$` and a column per reserved prefix
/// (`GraphName`'s `g-`, `SnapshotName`'s `gs-`): the lookahead is the only clause
/// the two names differ on.
#[test]
fn the_resource_name_rule_differs_only_in_the_reserved_prefix() {
    // (candidate, accepted as a graph name, accepted as a snapshot name)
    let cases: &[(&str, bool, bool)] = &[
        ("a", true, true),
        ("g", true, true),
        ("gs", true, true),
        ("my-snapshot-1", true, true),
        ("snapshot9", true, true),
        // `g-` is reserved for graph ids only, `gs-` for snapshot ids only.
        ("g-abc", false, true),
        ("gs-abc", true, false),
        ("1snapshot", false, false),
        ("-snapshot", false, false),
        ("My-Snapshot", false, false),
        ("my_snapshot", false, false),
        ("my--snapshot", false, false),
        ("my-snapshot-", false, false),
        ("", false, false),
    ];
    for (name, as_graph, as_snapshot) in cases {
        let actual_graph = resource_name_is_well_formed(name, "g-");
        assert_eq!(
            actual_graph, *as_graph,
            "case {name:?} as a graph name: expected accepted={as_graph}, \
                 got {actual_graph}"
        );
        let actual_snapshot = resource_name_is_well_formed(name, "gs-");
        assert_eq!(
            actual_snapshot, *as_snapshot,
            "case {name:?} as a snapshot name: expected accepted={as_snapshot}, \
                 got {actual_snapshot}"
        );
    }
}

/// The ARN lookup resolves a snapshot as well as a graph: both are taggable, and
/// each resolves only through the ARN the service minted for it.
#[test]
fn tags_of_resolves_a_stored_snapshot_by_its_minted_arn() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let graph_id = "g-0123456789";
    let graph_arn =
        ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{graph_id}"));
    state.insert_graph(sample_graph(graph_id, &graph_arn, "us-east-1"));

    let snapshot_id = "gs-0123456789";
    let snapshot_arn = ::engine::emulator_arn(
        ARN_NAMESPACE,
        "us-east-1",
        &format!("graph-snapshot/{snapshot_id}"),
    );
    state.insert_snapshot(Snapshot {
        id: snapshot_id.to_string(),
        name: "my-snapshot".to_string(),
        arn: snapshot_arn.clone(),
        region: "us-east-1".to_string(),
        source_graph_id: graph_id.to_string(),
        status: SnapshotStatus::Available,
        snapshot_create_time: SystemTime::UNIX_EPOCH,
        kms_key_identifier: None,
        tags: [("env".to_string(), "prod".to_string())].into_iter().collect(),
        settles_at: None,
        source_configuration: sample_configuration(),
        data: GraphData::empty(SystemTime::UNIX_EPOCH),
    });

    // (case, the ARN a request names, the tag entries it must resolve to)
    let cases: Vec<(&str, String, Option<usize>)> = vec![
        ("the snapshot's minted ARN", snapshot_arn.clone(), Some(1)),
        ("the graph's minted ARN", graph_arn, Some(0)),
        (
            "the snapshot id under the graph resource type",
            ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("graph/{snapshot_id}")),
            None,
        ),
        (
            "the snapshot ARN in another region",
            ::engine::emulator_arn(
                ARN_NAMESPACE,
                "eu-west-1",
                &format!("graph-snapshot/{snapshot_id}"),
            ),
            None,
        ),
    ];
    for (case, candidate, expected) in cases {
        let actual = state.tags_of(&candidate, "us-east-1").map(BTreeMap::len);
        assert_eq!(
            actual, expected,
            "case {case:?} ({candidate:?}): expected {expected:?} tag entries, \
                 got {actual:?}"
        );
        let actual_mut = state
            .tags_of_mut(&candidate, "us-east-1")
            .map(|tags| tags.len());
        assert_eq!(
            actual_mut, expected,
            "case {case:?} ({candidate:?}) mutably: expected {expected:?} tag \
                 entries, got {actual_mut:?}"
        );
    }
}

/// A snapshot's pending transition is owed at a future instant too, so a read
/// taken immediately still sees `CREATING`.
#[test]
fn a_pending_snapshot_transition_settles_only_once_it_is_due() {
    let now = Instant::now();
    let mut snapshot = Snapshot {
        id: "gs-0123456789".to_string(),
        name: "example".to_string(),
        arn: "arn".to_string(),
        region: "us-east-1".to_string(),
        source_graph_id: "g-0123456789".to_string(),
        status: SnapshotStatus::Creating,
        snapshot_create_time: SystemTime::UNIX_EPOCH,
        kms_key_identifier: None,
        tags: BTreeMap::new(),
        settles_at: Some(now + PROVISIONING_SPAN),
        source_configuration: sample_configuration(),
        data: GraphData::empty(SystemTime::UNIX_EPOCH),
    };

    snapshot.settle(now);
    assert_eq!(
        snapshot.status,
        SnapshotStatus::Creating,
        "case not-yet-due: expected CREATING, got {:?}",
        snapshot.status
    );

    snapshot.settle(now + PROVISIONING_SPAN);
    assert_eq!(
        snapshot.status,
        SnapshotStatus::Available,
        "case due: expected AVAILABLE, got {:?}",
        snapshot.status
    );
    assert!(
        snapshot.settles_at.is_none(),
        "case due: the settled transition must be cleared, got {:?}",
        snapshot.settles_at
    );
}

/// Every status this slice stores renders as one of the values the model
/// declares, so no handler can answer a status outside the modelled enum.
#[test]
fn every_stored_status_is_a_modelled_enum_value() {
    for status in [
        GraphStatus::Creating,
        GraphStatus::Available,
        GraphStatus::Updating,
        GraphStatus::Snapshotting,
        GraphStatus::Importing,
        GraphStatus::Stopping,
        GraphStatus::Stopped,
        GraphStatus::Starting,
        GraphStatus::Deleting,
        GraphStatus::Resetting,
    ] {
        let wire = status.as_wire();
        assert!(
            crate::ops::types::GRAPH_STATUS_VALUES.contains(&wire),
            "case {status:?}: {wire:?} is not a modelled GraphStatus value"
        );
    }
    for status in [
        ExportTaskStatus::Initializing,
        ExportTaskStatus::Exporting,
        ExportTaskStatus::Succeeded,
        ExportTaskStatus::Cancelling,
        ExportTaskStatus::Cancelled,
    ] {
        let wire = status.as_wire();
        assert!(
            crate::ops::types::EXPORT_TASK_STATUS_VALUES.contains(&wire),
            "case {status:?}: {wire:?} is not a modelled ExportTaskStatus value"
        );
    }
    for status in [SnapshotStatus::Creating, SnapshotStatus::Available] {
        let wire = status.as_wire();
        assert!(
            crate::ops::types::SNAPSHOT_STATUS_VALUES.contains(&wire),
            "case {status:?}: {wire:?} is not a modelled SnapshotStatus value"
        );
    }
    for status in [
        ImportTaskStatus::Initializing,
        ImportTaskStatus::AnalyzingData,
        ImportTaskStatus::Importing,
        ImportTaskStatus::RollingBack,
        ImportTaskStatus::Succeeded,
        ImportTaskStatus::Cancelling,
        ImportTaskStatus::Cancelled,
        ImportTaskStatus::Failed,
    ] {
        let wire = status.as_wire();
        assert!(
            crate::ops::types::IMPORT_TASK_STATUS_VALUES.contains(&wire),
            "case {status:?}: {wire:?} is not a modelled ImportTaskStatus value"
        );
    }
}

/// Every minted import-task id matches the modelled `TaskId` pattern and no id
/// repeats, which is what lets a task be read back by the id it was minted with.
#[test]
fn minted_import_task_ids_match_the_modelled_pattern_and_are_unique() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..2_000 {
        let id = state.next_import_task_id();
        assert!(
            is_task_id(&id),
            "id {id:?}: expected a match for ^t-[a-z0-9]{{10}}$"
        );
        assert!(seen.insert(id.clone()), "id {id:?}: minted twice");
    }
}

/// An import task's run walks the stages of a load and then stops where the load it
/// read ends: a task whose source was read analyses and imports its data and
/// `SUCCEEDED`s, one whose load met an error it could not skip releases what it read
/// and `FAILED`s, one whose source was never read stops without reporting a stage, and
/// a cancelled one settles into `CANCELLED`. Each status is owed at a FUTURE instant
/// and applied exactly once.
#[test]
fn an_import_task_settles_through_the_stages_of_the_run_it_read() {
    // (case, the load the task read, the statuses it goes through in order)
    let cases: Vec<(&str, ImportLoad, Vec<ImportTaskStatus>)> = vec![
        (
            "a load that read its source and met no error",
            sample_import_load(None, 0),
            vec![
                ImportTaskStatus::AnalyzingData,
                ImportTaskStatus::Importing,
                ImportTaskStatus::Succeeded,
            ],
        ),
        (
            "a load that halted on an error it could not skip",
            sample_import_load(Some("the load halted"), 1),
            vec![
                ImportTaskStatus::AnalyzingData,
                ImportTaskStatus::Importing,
                ImportTaskStatus::RollingBack,
                ImportTaskStatus::Failed,
            ],
        ),
        (
            "a load whose source could not be read",
            ImportLoad::unread(SOURCE_UNREADABLE_REASON),
            vec![ImportTaskStatus::Failed],
        ),
    ];
    for (case, load, stages) in cases {
        let now = Instant::now();
        let mut task = sample_import_task("t-0123456789", "g-0123456789");
        let expected_reason = load.failure_reason.clone();
        task.load = load;
        task.begin_run(now);

        task.settle(now);
        assert_eq!(
            task.status,
            ImportTaskStatus::Initializing,
            "case {case:?} not-yet-due: expected INITIALIZING, got {:?}",
            task.status
        );

        for (phase, expected) in stages.iter().enumerate() {
            // Every stage of a run lasts one span, so the n-th falls due n spans in.
            let at = now + IMPORT_TASK_SPAN * u32::try_from(phase + 1).expect("a small phase");
            task.settle(at);
            assert_eq!(
                task.status, *expected,
                "case {case:?} after {} span(s): expected {expected:?}, got {:?}",
                phase + 1,
                task.status
            );
        }

        assert_eq!(
            task.status_reason,
            if task.status == ImportTaskStatus::Failed {
                expected_reason
            } else {
                None
            },
            "case {case:?}: a run that loaded its data has nothing to explain and one that \
             stopped reports why, got {:?}",
            task.status_reason
        );
        assert!(
            task.settles_at.is_none(),
            "case {case:?}: a task that has stopped is owed nothing more"
        );

        // A read long after the run reports the status it would have reached, never a
        // stage it had already left.
        let last = *stages.last().expect("every run has a terminal status");
        task.settle(now + IMPORT_TASK_SPAN * 40);
        assert_eq!(
            task.status, last,
            "case {case:?} read late: expected {last:?}, got {:?}",
            task.status
        );
    }
}

/// A cancelled task winds down into `CANCELLED` carrying the reason that status
/// reports, and the elements its load had read are released rather than written.
#[test]
fn a_cancelled_import_task_settles_into_cancelled_and_loads_nothing() {
    let now = Instant::now();
    let mut task = sample_import_task("t-0123456789", "g-0123456789");
    task.load = sample_import_load(None, 0);
    task.begin_run(now);
    task.settle(now + IMPORT_TASK_SPAN);
    let cancelled_at = now + IMPORT_TASK_SPAN;
    task.begin_cancellation(cancelled_at);

    assert_eq!(
        task.status,
        ImportTaskStatus::Cancelling,
        "case an accepted cancellation: expected CANCELLING, got {:?}",
        task.status
    );
    assert!(
        task.load.data.nodes.is_empty() && task.load.data.edges.is_empty(),
        "case an accepted cancellation: a cancelled load writes nothing, so it holds no \
         element, got {} node(s) and {} edge(s)",
        task.load.data.nodes.len(),
        task.load.data.edges.len()
    );

    task.settle(cancelled_at + CANCELLATION_SPAN);
    assert_eq!(
        task.status,
        ImportTaskStatus::Cancelled,
        "case a cancellation that has come due: expected CANCELLED, got {:?}",
        task.status
    );
    assert_eq!(
        task.status_reason.as_deref(),
        Some(IMPORT_CANCELLED_REASON),
        "case a cancellation that has come due: expected the cancellation reason, got {:?}",
        task.status_reason
    );
}

/// Only a task whose load is in flight can be cancelled, and only a stopped one
/// is terminal — one row per status this slice stores.
#[test]
fn cancellability_is_decided_by_the_stored_status() {
    // (status, cancellable, terminal)
    for (status, cancellable, terminal) in [
        (ImportTaskStatus::Initializing, true, false),
        (ImportTaskStatus::AnalyzingData, true, false),
        (ImportTaskStatus::Importing, true, false),
        (ImportTaskStatus::RollingBack, false, false),
        (ImportTaskStatus::Succeeded, false, true),
        (ImportTaskStatus::Cancelling, false, false),
        (ImportTaskStatus::Cancelled, false, true),
        (ImportTaskStatus::Failed, false, true),
    ] {
        let actual = status.is_cancellable();
        assert_eq!(
            actual, cancellable,
            "case {status:?}: expected cancellable={cancellable}, got {actual}"
        );
        let actual_terminal = status.is_terminal();
        assert_eq!(
            actual_terminal, terminal,
            "case {status:?}: expected terminal={terminal}, got {actual_terminal}"
        );
    }
}

/// A load that read its source, carrying `errors` errors and failing for `failure`.
fn sample_import_load(failure: Option<&str>, errors: i32) -> ImportLoad {
    ImportLoad {
        data: ImportedData {
            nodes: [("n1".to_string(), GraphNode::default())]
                .into_iter()
                .collect(),
            edges: BTreeMap::new(),
        },
        statement_count: 0,
        dictionary_entry_count: 1,
        error_count: errors,
        error_details: failure.map(str::to_string),
        source_read: true,
        failure_reason: failure.map(str::to_string),
    }
}

/// A stored import task with the members every test here needs and nothing else
/// set.
fn sample_import_task(id: &str, graph_id: &str) -> ImportTask {
    ImportTask {
        id: id.to_string(),
        region: "us-east-1".to_string(),
        graph_id: graph_id.to_string(),
        source: "s3://example-bucket/data/".to_string(),
        format: None,
        parquet_type: None,
        blank_node_handling: None,
        fail_on_error: true,
        role_arn: "arn:aws:iam::000000000000:role/example".to_string(),
        import_options: None,
        status: ImportTaskStatus::Initializing,
        status_reason: None,
        attempt_number: 1,
        load: ImportLoad::unread(SOURCE_UNREADABLE_REASON),
        delivered: false,
        start_time: SystemTime::UNIX_EPOCH,
        started_at: Instant::now(),
        stopped_progress: None,
        settles_at: None,
    }
}

/// A stored export task with the members every test here needs and nothing else
/// set.
fn sample_export_task(id: &str, graph_id: &str) -> ExportTask {
    let now = Instant::now();
    ExportTask {
        id: id.to_string(),
        arn: ::engine::emulator_arn(ARN_NAMESPACE, "us-east-1", &format!("export-task/{id}")),
        region: "us-east-1".to_string(),
        source_graph_id: graph_id.to_string(),
        role_arn: "arn:aws:iam::000000000000:role/example".to_string(),
        format: "CSV".to_string(),
        parquet_type: None,
        destination: "s3://example-bucket/export/".to_string(),
        kms_key_identifier:
            "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012"
                .to_string(),
        export_filter: None,
        status: ExportTaskStatus::Initializing,
        status_reason: None,
        exported_vertices: 0,
        exported_edges: 0,
        cancelled_progress: None,
        tags: BTreeMap::new(),
        start_time: SystemTime::UNIX_EPOCH,
        started_at: now,
        settles_at: None,
    }
}

/// An import task and an export task are addressed in ONE modelled id space
/// (`TaskId`'s `^t-[a-z0-9]{10}$`), so no id minted for either is ever an id the
/// other already holds — asserted by alternating the two generators, which is what
/// forces each to skip what the other has filed.
#[test]
fn minted_task_ids_are_unique_across_the_import_and_export_task_maps() {
    let mut state = ComAmazonawsNeptunegraphAmazonneptunegraphState::default();
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..1_000 {
        let (kind, id) = if index % 2 == 0 {
            let id = state.next_import_task_id();
            state.insert_import_task(sample_import_task(&id, "g-0123456789"));
            ("an import task", id)
        } else {
            let id = state.next_export_task_id();
            state.insert_export_task(sample_export_task(&id, "g-0123456789"));
            ("an export task", id)
        };
        assert!(
            is_task_id(&id),
            "case {kind} ({id:?}): expected a match for ^t-[a-z0-9]{{10}}$"
        );
        assert!(
            seen.insert(id.clone()),
            "case {kind} ({id:?}): minted twice across the shared task-id space"
        );
    }
}

/// The provisioned memory a graph created with an import gets, over a row per form
/// the request's bounds can take (neither, one, both, equal) — including the
/// published range's own bounds.
#[test]
fn import_provisioned_memory_is_the_floor_of_the_range_the_request_allows() {
    // (case, minProvisionedMemory, maxProvisionedMemory, the chosen value)
    let cases: [(&str, Option<i32>, Option<i32>, i32); 7] = [
        (
            "neither bound supplied",
            None,
            None,
            UNBOUNDED_IMPORT_PROVISIONED_MEMORY,
        ),
        ("only a minimum", Some(128), None, 128),
        (
            "only a maximum",
            None,
            Some(2048),
            DEFAULT_MIN_PROVISIONED_MEMORY,
        ),
        ("both bounds", Some(256), Some(1024), 256),
        ("both bounds equal", Some(512), Some(512), 512),
        // The published range of `ProvisionedMemory` is 16..24576
        // (`smithy_query(query="shape",shape="ProvisionedMemory")`).
        ("at the bottom of the published range", Some(16), None, 16),
        (
            "at the top of the published range",
            Some(24_576),
            Some(24_576),
            24_576,
        ),
    ];
    for (case, min, max, expected) in cases {
        let actual = import_provisioned_memory(min, max);
        assert_eq!(
            actual, expected,
            "case {case:?} (min={min:?}, max={max:?}): expected {expected}, \
                 got {actual}"
        );
    }
}

/// The one rule every operation that accepts an import `source` enforces, over a
/// row per clause: what a location may look like, blankness in each of its forms,
/// and the stored length at the bound and one character past it.
#[test]
fn the_import_source_rule_bounds_blankness_and_the_stored_length() {
    let prefix = "s3://example-bucket/";
    let filler = MAX_IMPORT_SOURCE_CHARS - prefix.len();
    // (case, the candidate source, whether the rule accepts it)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "an S3 prefix",
            "s3://example-bucket/data/".to_string(),
            true,
        ),
        (
            "a Neptune database endpoint",
            "neptune-db://cluster.example.com:8182".to_string(),
            true,
        ),
        (
            "a Neptune snapshot ARN",
            "arn:aws:rds:us-east-1:000000000000:cluster-snapshot:nightly".to_string(),
            true,
        ),
        ("a single character", "s".to_string(), true),
        (
            "at the stored-length bound",
            format!("{prefix}{}", "d".repeat(filler)),
            true,
        ),
        (
            "one character past the stored-length bound",
            format!("{prefix}{}", "d".repeat(filler + 1)),
            false,
        ),
        ("the empty string", String::new(), false),
        ("nothing but spaces", "   ".to_string(), false),
        ("nothing but a tab and a newline", "\t\n".to_string(), false),
    ];
    for (case, candidate, accepted) in cases {
        let requirement = import_source_requirement(&candidate);
        assert_eq!(
            requirement.is_none(),
            accepted,
            "case {case:?} ({} characters): expected accepted={accepted}, got {:?}",
            candidate.chars().count(),
            requirement
        );
    }
}
