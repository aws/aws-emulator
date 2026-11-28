// SPDX-License-Identifier: CC0-1.0

//! The campaign-template catalog, over this service's own `rpcv2Cbor` wire.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! * `SearchCampaignTemplates` with no query "returns the whole catalog in the default
//!   order", reports `TotalResults` over the whole matching set rather than the page, and
//!   pages honestly: a page holds at most `MaxResults`, a token is offered only while
//!   templates remain, and the pages together are the sequence exactly once;
//! * a facet narrows the catalog as its own member documents — values within one facet
//!   OR-ed, distinct facets AND-ed, and the two facets whose documentation says an
//!   unlabelled template matches every value treated that way;
//! * a free-text query matches any word of a template's name or description, matches the
//!   last word as a prefix once it is three characters long, and ranks a template whose
//!   name holds the query as a phrase first;
//! * `GetCampaignTemplate` reads one template of that same catalog, and a template whose
//!   `EffectiveDateBegin` has not been reached "does not exist to a partner": the search
//!   omits it and the read answers `ResourceNotFoundException`;
//! * a continuation token is bound to the query it was issued for.
//!
//! The catalog itself is the fixture: the expectations are computed from
//! `state::catalog()` and `state::is_effective` rather than restated, so a row cannot
//! quietly describe a catalog the emulator does not serve.

use service_com_amazonaws_partnercentralmarketing_awspartnerdemandgenerator::ops::types::{
    CampaignTemplateSummary, GetCampaignTemplateInput, GetCampaignTemplateOutput,
    SearchCampaignTemplatesInput, SearchCampaignTemplatesOutput, GET_CAMPAIGN_TEMPLATE_INPUT_SCHEMA,
    SEARCH_CAMPAIGN_TEMPLATES_INPUT_SCHEMA,
};
use service_com_amazonaws_partnercentralmarketing_awspartnerdemandgenerator::state;

use super::campaign_lifecycle::{
    assert_refusal, effective_templates, upcoming_template, Answer, Emulator, CATALOG,
};

/// An unfiltered `SearchCampaignTemplates` of this catalog.
fn search_input() -> SearchCampaignTemplatesInput {
    SearchCampaignTemplatesInput {
        catalog: CATALOG.to_string(),
        ..Default::default()
    }
}

/// Dispatch one search, requiring the modelled success.
async fn search(emulator: &Emulator, case: &str, input: &SearchCampaignTemplatesInput) -> Answer {
    let answered = emulator
        .call(
            "SearchCampaignTemplates",
            SEARCH_CAMPAIGN_TEMPLATES_INPUT_SCHEMA,
            input,
        )
        .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: SearchCampaignTemplates expected 200, got {} with body {}",
        answered.status,
        answered.render()
    );
    answered
}

/// One search's `TotalResults` and the template ids of its page, in order.
async fn searched(
    emulator: &Emulator,
    case: &str,
    input: &SearchCampaignTemplatesInput,
) -> (i32, Vec<String>) {
    let output = search(emulator, case, input)
        .await
        .output::<SearchCampaignTemplatesOutput>(case);
    let ids = output
        .campaign_templates
        .iter()
        .map(|summary| summary.id.clone())
        .collect();
    (output.total_results, ids)
}

/// Dispatch one `GetCampaignTemplate`.
async fn get_template(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "GetCampaignTemplate",
            GET_CAMPAIGN_TEMPLATE_INPUT_SCHEMA,
            &GetCampaignTemplateInput {
                catalog: CATALOG.to_string(),
                identifier: identifier.to_string(),
            },
        )
        .await
}

#[tokio::test]
async fn the_catalog_is_browsed_read_and_paged() {
    let emulator = Emulator::new();
    let expected: Vec<String> = effective_templates()
        .iter()
        .map(|record| record.id.to_string())
        .collect();
    assert!(
        expected.len() >= 2,
        "the catalog must carry more than one template for this suite to page it, got {expected:?}"
    );

    // With no query the whole catalog comes back, in the catalog's own order, with the
    // total over the whole matching set.
    let (total, ids) = searched(&emulator, "the whole catalog", &search_input()).await;
    assert_eq!(
        ids, expected,
        "the catalog's own browsing order is what a query-less search reports, got {ids:?}"
    );
    assert_eq!(
        usize::try_from(total).unwrap_or_default(),
        expected.len(),
        "TotalResults counts the whole matching set, got {total}"
    );

    // A page holds at most what was asked for, a token is offered only while templates
    // remain, and the pages together are the sequence exactly once.
    let mut page = search_input();
    page.max_results = Some(1);
    let mut walked: Vec<String> = Vec::new();
    let mut pages = 0;
    loop {
        let output = search(&emulator, "one template per page", &page)
            .await
            .output::<SearchCampaignTemplatesOutput>("one template per page");
        assert!(
            output.campaign_templates.len() <= 1,
            "paging: a page of one must hold at most one template, got {:?}",
            output.campaign_templates
        );
        assert_eq!(
            usize::try_from(output.total_results).unwrap_or_default(),
            expected.len(),
            "paging: every page reports the same total over the whole matching set"
        );
        walked.extend(output.campaign_templates.iter().map(|row| row.id.clone()));
        pages += 1;
        assert!(
            pages <= expected.len() + 1,
            "paging: expected to finish in {} pages, walked {walked:?}",
            expected.len()
        );
        match output.next_token {
            Some(token) => page.next_token = Some(token),
            None => break,
        }
    }
    assert_eq!(
        walked, expected,
        "paging: the pages together are the whole catalog, in order, got {walked:?}"
    );

    // Every template the search reports reads back in full, member for member.
    for record in effective_templates() {
        let case = record.id;
        let answered = get_template(&emulator, record.id).await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: GetCampaignTemplate expected 200, got {} with body {}",
            answered.status,
            answered.render()
        );
        let template = answered
            .output::<GetCampaignTemplateOutput>(case)
            .campaign_template;
        assert_eq!(
            (
                template.id.as_str(),
                template.name.as_str(),
                template.collection.as_str(),
                template.language_code.as_deref(),
                template.description.as_deref(),
                template.industry.as_deref(),
                template.use_case.as_deref(),
                template.setup_duration.as_deref(),
            ),
            (
                record.id,
                record.name,
                record.collection,
                Some(record.language_code),
                Some(record.description),
                record.industry,
                record.use_case,
                Some(record.setup_duration),
            ),
            "case {case:?}: the read must report the catalog's own template, got {template:?}"
        );
        assert!(
            template.effective_date_begin.is_some()
                && template.created_at.is_some()
                && template.updated_at.is_some(),
            "case {case:?}: a template reports its publication and audit instants, got {:?}, \
             {:?} and {:?}",
            template.effective_date_begin,
            template.created_at,
            template.updated_at
        );
        let asset_templates = template.asset_templates.unwrap_or_default();
        assert_eq!(
            asset_templates
                .iter()
                .map(|asset| asset.id.as_str())
                .collect::<Vec<&str>>(),
            record
                .asset_templates
                .iter()
                .map(|asset| asset.id)
                .collect::<Vec<&str>>(),
            "case {case:?}: the template's ordered asset blueprints must be reported"
        );
        for (reported, seeded) in asset_templates.iter().zip(record.asset_templates) {
            let fields = reported.fields.clone().unwrap_or_default();
            assert_eq!(
                fields
                    .iter()
                    .map(|field| field.field_id.as_str())
                    .collect::<Vec<&str>>(),
                seeded
                    .fields
                    .iter()
                    .map(|field| field.field_id)
                    .collect::<Vec<&str>>(),
                "case {case:?}: blueprint {} must report its editable fields",
                seeded.id
            );
            assert_eq!(
                fields
                    .iter()
                    .filter_map(|field| field.campaign_setting.as_deref())
                    .collect::<Vec<&str>>(),
                seeded
                    .fields
                    .iter()
                    .filter_map(|field| field.campaign_setting)
                    .collect::<Vec<&str>>(),
                "case {case:?}: blueprint {} must report which campaign settings its fields \
                 show",
                seeded.id
            );
        }
        assert_eq!(
            template
                .reference_materials
                .unwrap_or_default()
                .iter()
                .map(|material| material.name.clone())
                .collect::<Vec<String>>(),
            record
                .reference_materials
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the template's reference materials must be reported"
        );
    }

    // An id the catalog holds nothing under is not found.
    assert_refusal(
        "an id no template carries",
        &get_template(&emulator, "tmpl-0000000000000").await,
        404,
        "ResourceNotFoundException",
        "Campaign template tmpl-0000000000000 does not exist.",
    );

    // "Until then the template does not exist to a partner: SearchCampaignTemplates
    // omits it and GetCampaignTemplate returns ResourceNotFoundException."
    let upcoming = upcoming_template().id;
    assert!(
        !ids.iter().any(|id| id == upcoming),
        "a template whose EffectiveDateBegin is in the future must be omitted, got {ids:?}"
    );
    assert_refusal(
        "a template that is not effective yet",
        &get_template(&emulator, upcoming).await,
        404,
        "ResourceNotFoundException",
        &format!("Campaign template {upcoming} does not exist."),
    );
}

#[tokio::test]
async fn a_facet_narrows_the_catalog_as_its_own_member_documents() {
    let emulator = Emulator::new();

    // Each row: the facet values the request names, and the rule that decides which
    // templates they admit — written here independently of the handler, and applied to
    // the catalog the emulator serves.
    type Rule = fn(&state::CampaignTemplateRecord) -> bool;
    type Narrow = fn(&mut SearchCampaignTemplatesInput);
    type FacetCase = (&'static str, Narrow, Rule);
    let rows: Vec<FacetCase> = vec![
        (
            "one industry",
            |input| input.industry = Some(vec!["FINANCIAL_SERVICES".to_string()]),
            |record| record.industry == Some("FINANCIAL_SERVICES"),
        ),
        (
            "two industries, OR-ed within the facet",
            |input| {
                input.industry = Some(vec![
                    "FINANCIAL_SERVICES".to_string(),
                    "RETAIL".to_string(),
                ])
            },
            |record| matches!(record.industry, Some("FINANCIAL_SERVICES") | Some("RETAIL")),
        ),
        (
            "an industry no template carries",
            |input| input.industry = Some(vec!["ENERGY".to_string()]),
            |_| false,
        ),
        (
            "one language, matched by the template's LanguageCode",
            |input| input.language = Some(vec!["es".to_string()]),
            |record| record.language_code == "es",
        ),
        (
            "one collection",
            |input| input.collection = Some(vec!["BUILD_WITH_AI".to_string()]),
            |record| record.collection == "BUILD_WITH_AI",
        ),
        (
            "one AWS service",
            |input| input.aws_services = Some(vec!["Amazon Bedrock".to_string()]),
            |record| record.aws_services.contains(&"Amazon Bedrock"),
        ),
        (
            "one use case",
            |input| input.use_case = Some(vec!["Generative AI".to_string()]),
            |record| record.use_case == Some("Generative AI"),
        ),
        (
            // "A template that names no customer segment matches every value."
            "one customer segment",
            |input| input.customer_segments = Some(vec!["ISV".to_string()]),
            |record| record.customer_segments.is_empty() || record.customer_segments.contains(&"ISV"),
        ),
        (
            // The same rule for themes.
            "one theme",
            |input| input.themes = Some(vec!["SECURITY".to_string()]),
            |record| record.themes.is_empty() || record.themes.contains(&"SECURITY"),
        ),
        (
            "two facets, AND-ed with each other",
            |input| {
                input.collection = Some(vec!["GENERAL".to_string()]);
                input.language = Some(vec!["en".to_string()]);
            },
            |record| record.collection == "GENERAL" && record.language_code == "en",
        ),
    ];

    for (case, narrow, rule) in rows {
        let mut input = search_input();
        narrow(&mut input);
        let expected: Vec<String> = effective_templates()
            .iter()
            .filter(|record| rule(record))
            .map(|record| record.id.to_string())
            .collect();
        let (total, ids) = searched(&emulator, case, &input).await;
        assert_eq!(
            ids, expected,
            "case {case:?}: expected the templates the facet admits, got {ids:?}"
        );
        assert_eq!(
            usize::try_from(total).unwrap_or_default(),
            expected.len(),
            "case {case:?}: TotalResults counts the filtered set, got {total}"
        );
    }
}

#[tokio::test]
async fn a_query_matches_any_word_and_the_last_word_as_a_prefix() {
    let emulator = Emulator::new();

    // The template ids this suite names, found in the catalog by the word each template
    // is known by rather than written out, so a row cannot name a template the catalog
    // does not hold.
    let by_name = |needle: &str| {
        effective_templates()
            .iter()
            .find(|record| record.name.to_lowercase().contains(needle))
            .map(|record| record.id.to_string())
            .unwrap_or_else(|| panic!("no catalog template's name holds {needle:?}"))
    };
    let migration = by_name("migration");
    let security = by_name("security posture");
    let nova = by_name("nova");

    // Each row: the query, and the templates it must report. The query's own form is
    // what varies — one word, the same word in another case and with punctuation, a word
    // from the description rather than the name, a three-character prefix of a word, a
    // two-character one (too short to match as a prefix), and a word the catalog carries
    // nowhere.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("a word of a name", "migration", vec![migration.clone()]),
        (
            "the same word in another case",
            "MIGRATION",
            vec![migration.clone()],
        ),
        (
            "the same word with punctuation around it",
            "  migration!",
            vec![migration.clone()],
        ),
        (
            "a three-character prefix of a word",
            "nov",
            vec![nova.clone()],
        ),
        ("a two-character prefix, too short to match", "no", vec![]),
        (
            "a word only a description carries",
            "lakehouse",
            vec![by_name("data platform")],
        ),
        ("a word the catalog carries nowhere", "quantum", vec![]),
    ];

    for (case, query, expected) in rows {
        let mut input = search_input();
        input.search_text = Some(query.to_string());
        let (total, ids) = searched(&emulator, case, &input).await;
        assert_eq!(
            ids, expected,
            "case {case:?}: expected the templates carrying {query:?}, got {ids:?}"
        );
        assert_eq!(
            usize::try_from(total).unwrap_or_default(),
            expected.len(),
            "case {case:?}: TotalResults counts the matching set, got {total}"
        );
    }

    // "A template whose name holds the query as a phrase ranks first": both templates
    // below carry both words, and only one carries them as a phrase in its name.
    let mut phrase = search_input();
    phrase.search_text = Some("security posture".to_string());
    let (_total, ranked) = searched(&emulator, "a phrase in a name", &phrase).await;
    assert_eq!(
        ranked.first(),
        Some(&security),
        "a name holding the query as a phrase ranks first, got {ranked:?}"
    );
    assert!(
        ranked.contains(&migration),
        "a template carrying the words outside its name still matches, got {ranked:?}"
    );

    // A query naming a word several templates carry reports them all, ordered by id so
    // the sequence is total and a page of it is resumable.
    let mut shared = search_input();
    shared.search_text = Some("customers".to_string());
    let (total, ids) = searched(&emulator, "a word several templates carry", &shared).await;
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "templates of equal rank are ordered by id, got {ids:?}"
    );
    assert!(
        total >= 2,
        "the query must match more than one template for this to mean anything, got {total}"
    );
}

#[tokio::test]
async fn a_search_token_is_bound_to_the_query_that_issued_it() {
    let emulator = Emulator::new();
    let mut first = search_input();
    first.max_results = Some(1);
    let token = search(&emulator, "a first page", &first)
        .await
        .output::<SearchCampaignTemplatesOutput>("a first page")
        .next_token
        .expect("a first page of one template offers a continuation");

    let message = "The NextToken is not a continuation token this operation issued for this \
                   request's filters and sort order.";
    for (case, mut changed) in [
        ("the same token under a free-text query", {
            let mut input = search_input();
            input.search_text = Some("migration".to_string());
            input
        }),
        ("the same token under a facet filter", {
            let mut input = search_input();
            input.collection = Some(vec!["GENERAL".to_string()]);
            input
        }),
    ] {
        changed.next_token = Some(token.clone());
        let refused = emulator
            .call(
                "SearchCampaignTemplates",
                SEARCH_CAMPAIGN_TEMPLATES_INPUT_SCHEMA,
                &changed,
            )
            .await;
        assert_refusal(case, &refused, 400, "ValidationException", message);
    }

    let mut forged = search_input();
    forged.next_token =
        Some("Zm9yZ2VkIHRva2VuIHRoYXQgaXMgbG9uZyBlbm91Z2ggdG8gbG9vayByZWFs".to_string());
    let refused = emulator
        .call(
            "SearchCampaignTemplates",
            SEARCH_CAMPAIGN_TEMPLATES_INPUT_SCHEMA,
            &forged,
        )
        .await;
    assert_refusal(
        "a token this store never minted",
        &refused,
        400,
        "ValidationException",
        message,
    );

    // The token the first page issued still resumes its own sequence.
    let mut resumed = search_input();
    resumed.max_results = Some(1);
    resumed.next_token = Some(token);
    let (_total, ids) = searched(&emulator, "the token's own sequence", &resumed).await;
    let expected: Vec<String> = effective_templates()
        .iter()
        .skip(1)
        .take(1)
        .map(|record| record.id.to_string())
        .collect();
    assert_eq!(
        ids, expected,
        "the second page is the next template of the sequence, got {ids:?}"
    );
}

/// The summary a search reports carries the members a client lists a template by, so a
/// caller needs no read per row.
#[tokio::test]
async fn a_summary_carries_what_a_listing_shows() {
    let emulator = Emulator::new();
    let summaries: Vec<CampaignTemplateSummary> =
        search(&emulator, "the whole catalog", &search_input())
            .await
            .output::<SearchCampaignTemplatesOutput>("the whole catalog")
            .campaign_templates;
    for (summary, record) in summaries.iter().zip(effective_templates()) {
        assert_eq!(
            (
                summary.id.as_str(),
                summary.name.as_str(),
                summary.collection.as_str(),
                summary.language_code.as_deref(),
                summary.description.as_deref(),
                summary.industry.as_deref(),
            ),
            (
                record.id,
                record.name,
                record.collection,
                Some(record.language_code),
                Some(record.description),
                record.industry,
            ),
            "the summary of {} must report the catalog's own values, got {summary:?}",
            record.id
        );
    }
}
