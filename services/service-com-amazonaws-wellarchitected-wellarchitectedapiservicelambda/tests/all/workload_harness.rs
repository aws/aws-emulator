// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every workload suite in this crate shares.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;
use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    AnswerRecord, ChoiceAnswerRecord,
};

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
pub(crate) struct Client {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body),
/// the wire error code when the answer is a failure, and the `x-amzn-errortype` header
/// EXACTLY as the answer carried it (empty when it carried none), for a suite about the
/// form of that header rather than the shape it names.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) error_code: String,
    pub(crate) error_type_header: String,
}

/// The Region this harness signs every request in — the credential scope in
/// [`Client::send`]'s `authorization` header, which is how a handler learns a request's
/// Region. Named so a fixture that has to build an ARN itself puts it in the Region a
/// resource minted over the wire lands in.
pub(crate) const SIGNED_REGION: &str = "us-west-2";

/// The error SHAPE a wire value names, resolved the way every restJson1 client resolves
/// it: the text before the first ':' — what follows it locates the namespace that declares
/// the shape and is not part of its name — and after any '#', for a namespaced name.
pub(crate) fn shape_named(raw: &str) -> String {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace
        .split(':')
        .next()
        .unwrap_or(after_namespace)
        .trim()
        .to_string()
}

impl Client {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// This emulator's service state for the Region this harness signs its requests in
    /// ([`SIGNED_REGION`]) — the store the handlers resolve for those requests — for a test
    /// that has to reach a ceiling the HTTP boundary would take tens of thousands of
    /// requests to reach. The boundary itself is still crossed over HTTP for everything
    /// the test asserts about behaviour.
    pub(crate) fn store(&self) -> state::RegionalServiceState {
        self.store_in(SIGNED_REGION)
    }

    /// One named Region's store, for a fixture that has to seed a Region other than the
    /// one [`SIGNED_REGION`] names.
    pub(crate) fn store_in(&self, region: &str) -> state::RegionalServiceState {
        state::region_state(&*self.engine, region)
            .expect("the shared engine must hand out this service's store")
    }

    /// Fill the store straight through the state the handlers share, so a test can reach
    /// the workload quota without issuing ten thousand HTTP requests. The boundary itself
    /// is still crossed over HTTP.
    pub(crate) fn seed_workloads(&self, count: usize) {
        let state = self.store();
        let mut store = state.lock();
        for index in 0..count {
            let workload_id = store.mint_workload_id();
            let workload_name = format!("seeded-{index:06}");
            store.workload_name_index.insert(
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::normalize_unique_name(
                    &workload_name,
                ),
                workload_id.clone(),
            );
            store.workloads.insert(
                workload_id.clone(),
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::WorkloadRecord {
                    workload_arn: format!(
                        "arn:aws:wellarchitected:us-west-2:000000000000:workload/{workload_id}"
                    ),
                    workload_id,
                    workload_name,
                    description: "a seeded workload".to_string(),
                    environment: "PRODUCTION".to_string(),
                    updated_at: std::time::SystemTime::now(),
                    account_ids: Vec::new(),
                    aws_regions: vec!["us-east-1".to_string()],
                    non_aws_regions: Vec::new(),
                    pillar_priorities: Vec::new(),
                    architectural_design: None,
                    review_owner: Some("seed-owner".to_string()),
                    is_review_owner_update_acknowledged: false,
                    industry_type: None,
                    industry: None,
                    notes: None,
                    improvement_status: "NOT_APPLICABLE".to_string(),
                    lenses: Vec::new(),
                    owner: "000000000000".to_string(),
                    tags: std::collections::BTreeMap::new(),
                    applications: Vec::new(),
                    discovery_config: None,
                    jira_configuration: None,
                    answers: std::collections::BTreeMap::new(),
                    lens_review_records: std::collections::BTreeMap::new(),
                    profiles: Vec::new(),
                },
            );
        }
    }

    /// Record an answer to one question of one lens review straight through the shared
    /// state, so a test has a stored answer for the reads to report. The answers
    /// `UpdateAnswer` records over the wire are exercised by `super::answer_updates`;
    /// this seeds one for the suites that are about a DIFFERENT read.
    pub(crate) fn record_answer(
        &self,
        workload_id: &str,
        lens: &str,
        question_id: &str,
        answer: service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord,
    ) {
        let state = self.store();
        let mut store = state.lock();
        let workload = store
            .workloads
            .get_mut(workload_id)
            .expect("the workload under test must exist");
        workload
            .answers
            .entry(
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::lens_alias(
                    lens,
                )
                .to_string(),
            )
            .or_default()
            .insert(question_id.to_string(), answer);
    }

    /// Record an answer to one question of one REVIEW TEMPLATE's lens review straight
    /// through the shared state, so a test has a stored answer for the reads to report.
    /// `UpdateReviewTemplateAnswer` — the operation that answers a template question
    /// over the wire — is exercised over HTTP by `super::review_template_answer_updates`;
    /// this seed is for the reads that need many answered questions at once.
    pub(crate) fn record_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
        answer: service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord,
    ) {
        let state = self.store();
        let mut store = state.lock();
        let template = store
            .review_templates
            .get_mut(template_arn)
            .expect("the review template under test must exist");
        template
            .answers
            .entry(
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::lens_alias(
                    lens,
                )
                .to_string(),
            )
            .or_default()
            .insert(question_id.to_string(), answer);
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in(SIGNED_REGION, method, uri, body).await
    }

    /// The same request, signed for `region` instead of [`SIGNED_REGION`] — for a suite
    /// about what one Region's endpoint reports of another Region's resources.
    pub(crate) async fn send_in(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/wellarchitected/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        // The SHAPE the header names, resolved the way a restJson1 client resolves it:
        // the text before the first ':' (the locator this service's wire appends is not
        // part of the name) and after any '#' (a namespaced name).
        let error_code = shape_named(&error_type_header);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if error_code.is_empty() {
            shape_named(
                body.get("__type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        } else {
            error_code
        };
        Answer {
            status,
            body,
            error_code,
            error_type_header,
        }
    }

    pub(crate) async fn create(&self, body: Value) -> Answer {
        self.send("POST", "/workloads", Some(body)).await
    }

    pub(crate) async fn get(&self, workload_id: &str) -> Answer {
        self.send("GET", &format!("/workloads/{workload_id}"), None)
            .await
    }

    pub(crate) async fn update(&self, workload_id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/workloads/{workload_id}"), Some(body))
            .await
    }

    pub(crate) async fn delete(&self, workload_id: &str, token: &str) -> Answer {
        self.send(
            "DELETE",
            &format!("/workloads/{workload_id}?ClientRequestToken={token}"),
            None,
        )
        .await
    }

    pub(crate) async fn list(&self, body: Value) -> Answer {
        self.send("POST", "/workloadsSummaries", Some(body)).await
    }

    pub(crate) async fn create_review_template(&self, body: Value) -> Answer {
        self.send("POST", "/reviewTemplates", Some(body)).await
    }

    /// `GET /reviewTemplates/{TemplateArn}`: the ARN is a non-greedy `@httpLabel`, so it
    /// travels percent-encoded in one path segment, exactly as an SDK sends it.
    pub(crate) async fn get_review_template(&self, template_arn: &str) -> Answer {
        let encoded = encode_path_segment(template_arn);
        self.send("GET", &format!("/reviewTemplates/{encoded}"), None)
            .await
    }

    /// `ListReviewTemplates`: `GET /reviewTemplates`, with both paging members bound to
    /// the query string. `query` is passed as sent, empty for a request that names none.
    pub(crate) async fn list_review_templates(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/reviewTemplates".to_string()
        } else {
            format!("/reviewTemplates?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// `DeleteReviewTemplate`: `DELETE /reviewTemplates/{TemplateArn}` with the
    /// `@idempotencyToken` bound to `@httpQuery("ClientRequestToken")`. The ARN is a
    /// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment,
    /// exactly as an SDK sends it.
    pub(crate) async fn delete_review_template(&self, template_arn: &str, token: &str) -> Answer {
        let encoded = encode_path_segment(template_arn);
        self.send(
            "DELETE",
            &format!("/reviewTemplates/{encoded}?ClientRequestToken={token}"),
            None,
        )
        .await
    }

    /// `UpdateReviewTemplate`: `PATCH /reviewTemplates/{TemplateArn}`. The ARN is a
    /// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment, exactly
    /// as an SDK sends it.
    pub(crate) async fn update_review_template(&self, template_arn: &str, body: Value) -> Answer {
        let encoded = encode_path_segment(template_arn);
        self.send("PATCH", &format!("/reviewTemplates/{encoded}"), Some(body))
            .await
    }

    /// `UpdateReviewTemplateLensReview`:
    /// `PATCH /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}`. Both labels are
    /// non-greedy, so each travels percent-encoded in one path segment, exactly as an
    /// SDK sends it.
    pub(crate) async fn update_review_template_lens_review(
        &self,
        template_arn: &str,
        lens: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
    }

    /// `GetReviewTemplateLensReview`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}`.
    pub(crate) async fn get_review_template_lens_review(
        &self,
        template_arn: &str,
        lens: &str,
    ) -> Answer {
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            None,
        )
        .await
    }

    /// `UpdateReviewTemplateAnswer`:
    /// `PATCH /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers/{QuestionId}`.
    /// Every label is non-greedy, so each travels percent-encoded in one path segment,
    /// exactly as an SDK sends it.
    pub(crate) async fn update_review_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
    }

    /// `GetReviewTemplateAnswer`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers/{QuestionId}`.
    pub(crate) async fn get_review_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
    ) -> Answer {
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
    }

    /// `ListReviewTemplateAnswers`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers`, with every
    /// filter and paging member bound to the query string. `query` is passed as sent,
    /// empty for a request that names none.
    pub(crate) async fn list_review_template_answers(
        &self,
        template_arn: &str,
        lens: &str,
        query: &str,
    ) -> Answer {
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        };
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers{suffix}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            None,
        )
        .await
    }

    /// `GetProfileTemplate`: `GET /profileTemplate`. The input declares no member, so the
    /// request carries nothing at all.
    pub(crate) async fn get_profile_template(&self) -> Answer {
        self.send("GET", "/profileTemplate", None).await
    }

    /// `CreateProfile`: `POST /profiles`.
    pub(crate) async fn create_profile(&self, body: Value) -> Answer {
        self.send("POST", "/profiles", Some(body)).await
    }

    /// `GetProfile`: `GET /profiles/{ProfileArn}`, with the optional `ProfileVersion`
    /// bound to the query string. The ARN is a non-greedy `@httpLabel`, so it travels
    /// percent-encoded in one path segment, exactly as an SDK sends it.
    pub(crate) async fn get_profile(
        &self,
        profile_arn: &str,
        profile_version: Option<&str>,
    ) -> Answer {
        let encoded = encode_path_segment(profile_arn);
        let uri = match profile_version {
            Some(version) => format!(
                "/profiles/{encoded}?ProfileVersion={}",
                encode_path_segment(version)
            ),
            None => format!("/profiles/{encoded}"),
        };
        self.send("GET", &uri, None).await
    }

    /// `UpdateProfile`: `PATCH /profiles/{ProfileArn}`. The ARN is a non-greedy
    /// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK
    /// sends it.
    pub(crate) async fn update_profile(&self, profile_arn: &str, body: Value) -> Answer {
        self.send(
            "PATCH",
            &format!("/profiles/{}", encode_path_segment(profile_arn)),
            Some(body),
        )
        .await
    }

    /// `DeleteProfile`: `DELETE /profiles/{ProfileArn}?ClientRequestToken=…`. The ARN is a
    /// non-greedy `@httpLabel` and the token an `@httpQuery` member, so both travel
    /// percent-encoded exactly as an SDK sends them.
    pub(crate) async fn delete_profile(&self, profile_arn: &str, token: &str) -> Answer {
        self.send(
            "DELETE",
            &format!(
                "/profiles/{}?ClientRequestToken={}",
                encode_path_segment(profile_arn),
                encode_path_segment(token)
            ),
            None,
        )
        .await
    }

    /// `ListProfiles`: `GET /profileSummaries`, with every filter and paging member bound
    /// to the query string. `query` is passed as sent, empty for a request that names
    /// none.
    pub(crate) async fn list_profiles(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/profileSummaries".to_string()
        } else {
            format!("/profileSummaries?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// Record a profile ANOTHER account owns straight through the shared state, so a test
    /// has a profile the caller does not own for `ProfileOwnerType` to select. Nothing
    /// over the wire mints one — `CreateProfile` attributes a profile to the calling
    /// account and the profile-share operations are still stubs with their own issues —
    /// while the LIST under test still crosses the HTTP boundary. Answers the stored ARN.
    ///
    /// The ARN is built from the service's own `ARN_NAMESPACE` and the Region this
    /// harness signs its requests in, so it satisfies the modelled `ProfileArn` pattern
    /// and sits in the same Region as a profile minted over the wire; only the account
    /// segment differs, because the profile belongs to another account.
    pub(crate) fn seed_foreign_profile(&self, name: &str, owner: &str, resource_id: &str) -> String {
        let state = self.store();
        let mut store = state.lock();
        let profile_arn = format!(
            "arn:aws:{}:{SIGNED_REGION}:{owner}:profile/{resource_id}",
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::ARN_NAMESPACE
        );
        let now = SystemTime::now();
        store.profiles.insert(
            profile_arn.clone(),
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::ProfileRecord {
                profile_arn: profile_arn.clone(),
                profile_version: "1".to_string(),
                profile_name: name.to_string(),
                profile_description: "a profile another account owns".to_string(),
                questions: std::collections::BTreeMap::new(),
                owner: owner.to_string(),
                created_at: now,
                updated_at: now,
                tags: std::collections::BTreeMap::new(),
            },
        );
        profile_arn
    }

    /// `CreateWorkloadShare`: `POST /workloads/{WorkloadId}/shares`.
    pub(crate) async fn create_share(&self, workload_id: &str, body: Value) -> Answer {
        self.send(
            "POST",
            &format!("/workloads/{workload_id}/shares"),
            Some(body),
        )
        .await
    }

    /// `UpdateWorkloadShare`: `PATCH /workloads/{WorkloadId}/shares/{ShareId}`.
    pub(crate) async fn update_share(
        &self,
        workload_id: &str,
        share_id: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!("/workloads/{workload_id}/shares/{share_id}"),
            Some(body),
        )
        .await
    }

    /// `ListWorkloadShares`: `GET /workloads/{WorkloadId}/shares`, with every filter and
    /// paging member bound to the query string. `query` is passed as sent, empty for a
    /// request that names none.
    pub(crate) async fn list_shares(&self, workload_id: &str, query: &str) -> Answer {
        let uri = if query.is_empty() {
            format!("/workloads/{workload_id}/shares")
        } else {
            format!("/workloads/{workload_id}/shares?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// `ListShareInvitations`: `GET /shareInvitations`, with every filter and paging
    /// member bound to the query string. `query` is passed as sent, empty for a request
    /// that names none.
    pub(crate) async fn list_share_invitations(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/shareInvitations".to_string()
        } else {
            format!("/shareInvitations?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// `UpdateShareInvitation`: `PATCH /shareInvitations/{ShareInvitationId}`. The id is a
    /// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment, exactly
    /// as an SDK sends it.
    pub(crate) async fn update_share_invitation(
        &self,
        share_invitation_id: &str,
        body: Value,
    ) -> Answer {
        let encoded = encode_path_segment(share_invitation_id);
        self.send("PATCH", &format!("/shareInvitations/{encoded}"), Some(body))
            .await
    }

    /// Move the stored profile `profile_arn` to `profile_version`, as a revision of that
    /// profile would.
    ///
    /// Minting a profile's next version follows the service's own counter
    /// (`UpdateProfile` moves a profile to it), so a suite that needs a profile whose
    /// latest version is AHEAD of the version a workload's association pinned — associating
    /// pins the version the profile is at — at a version spelling of its own choosing seeds
    /// that revision here, exactly as the suites above seed a stored answer whose writer is
    /// a stub. The operation under test and the reads of its result still cross the HTTP
    /// boundary.
    pub(crate) fn revise_profile_version(&self, profile_arn: &str, profile_version: &str) {
        let state = self.store();
        let mut store = state.lock();
        let profile = store
            .profiles
            .get_mut(profile_arn)
            .expect("the profile under test must exist");
        profile.profile_version = profile_version.to_string();
    }

    /// Record a share of `workload_id` with `shared_with` at `status` straight through
    /// the shared state, and answer its `(ShareId, ShareInvitationId)`.
    ///
    /// A share invitation is offered to the principal a resource was shared WITH, so a
    /// test of the invitation reads needs a share whose recipient is the CALLER — and
    /// that share has no HTTP path here: `CreateWorkloadShare` refuses a `SharedWith`
    /// naming the principal doing the sharing, and this emulator attributes every
    /// request to its one account, so the other account that would offer the share
    /// cannot sign a request. The seed records exactly what `CreateWorkloadShare`
    /// records — `shared_by` is the stored workload's owning account — and the READ
    /// under test still crosses the HTTP boundary.
    pub(crate) fn record_workload_share(
        &self,
        workload_id: &str,
        shared_with: &str,
        permission_type: &str,
        status: &str,
    ) -> (String, String) {
        let state = self.store();
        let mut store = state.lock();
        let shared_by = store
            .workloads
            .get(workload_id)
            .expect("the workload under test must exist")
            .owner
            .clone();
        let share_id = store.mint_share_id();
        let share_invitation_id = store.mint_share_invitation_id();
        store
            .workload_shares
            .entry(workload_id.to_string())
            .or_default()
            .insert(
                share_id.clone(),
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::WorkloadShareRecord {
                    share_id: share_id.clone(),
                    share_invitation_id: share_invitation_id.clone(),
                    workload_id: workload_id.to_string(),
                    shared_by,
                    shared_with: shared_with.to_string(),
                    permission_type: permission_type.to_string(),
                    status: status.to_string(),
                    status_message: None,
                },
            );
        (share_id, share_invitation_id)
    }
}

/// Percent-encode a value so it survives as ONE path segment or ONE query value:
/// everything RFC 3986 does not leave unreserved is escaped, so an ARN's `:` and `/` reach
/// the server's label decoder intact. The one encoder in this crate's tests; it escapes
/// BYTES, so a non-ASCII value (a tag key carrying the letters `TagKey`'s pattern allows)
/// travels as its UTF-8 octets.
pub(crate) fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create a review template applying `lenses` and answer its `TemplateArn` — the one
/// review-template seeding helper the suites in this crate share.
pub(crate) async fn review_template(client: &Client, name: &str, lenses: &[&str]) -> String {
    let answer = client
        .create_review_template(json!({
            "TemplateName": name,
            "Description": "a review template under test",
            "Lenses": lenses,
            "ClientRequestToken": format!("tok-{name}"),
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `AnswerRecord` that records `choices` as the status of each choice it speaks about,
/// whether the question applies and the reason it does not — the one answer builder the
/// suites in this crate share. An answer's `SelectedChoices` is DERIVED from those statuses
/// on every read, so a seeded answer records no separate list of them; the answer's own
/// notes and each choice's notes are what `UpdateAnswer` / `UpdateReviewTemplateAnswer`
/// write over the wire, so a seeded answer carries none.
pub(crate) fn answered(
    choices: &[(&str, &str)],
    is_applicable: bool,
    reason: Option<&str>,
) -> AnswerRecord {
    AnswerRecord {
        choice_answers: choices
            .iter()
            .map(|(choice_id, status)| {
                (
                    (*choice_id).to_string(),
                    ChoiceAnswerRecord {
                        status: (*status).to_string(),
                        reason: None,
                        notes: None,
                    },
                )
            })
            .collect(),
        is_applicable,
        reason: reason.map(ToString::to_string),
        notes: None,
    }
}

/// An `AnswerRecord` that records `selected` as the best practices in place out of
/// `choices` — the question's WHOLE choice set — and every other choice of the question
/// as `UNSELECTED`.
///
/// The key space an answer's per-choice statuses live in is the question's own choice set,
/// which is what `UpdateAnswer` / `UpdateReviewTemplateAnswer` record an answer over
/// ("the values entered replace the previously selected choices", the `SelectedChoices`
/// doc), so a seeded answer speaks about the same choices a written one would.
pub(crate) fn answered_over(
    choices: &[String],
    selected: &[&str],
    is_applicable: bool,
    reason: Option<&str>,
) -> AnswerRecord {
    let statuses: Vec<(&str, &str)> = choices
        .iter()
        .map(|choice_id| {
            let status = if selected.contains(&choice_id.as_str()) {
                state::CHOICE_STATUS_SELECTED
            } else {
                state::CHOICE_STATUS_UNSELECTED
            };
            (choice_id.as_str(), status)
        })
        .collect();
    answered(&statuses, is_applicable, reason)
}

/// Every `ChoiceId` one question of the AWS-official framework lens offers, in the lens's
/// own order, from the catalog that lens's content IS
/// (`state::FRAMEWORK_LENS_QUESTIONS`).
///
/// A seeded answer has to speak about the question's OWN choices, and a suite about some
/// other read should not have to restate the framework's published best-practice count per
/// question to seed one; `super::question_choice_catalog` is the suite that pins the ids
/// and the catalog itself over the wire.
pub(crate) fn framework_choice_ids(question_id: &str) -> Vec<String> {
    let question = state::FRAMEWORK_LENS_QUESTIONS
        .iter()
        .find(|question| question.question_id == question_id)
        .unwrap_or_else(|| panic!("the framework lens must ask {question_id:?}"));
    (0..question.best_practices.len())
        .map(|position| question.choice_id(position))
        .collect()
}

/// Which of a question's best practices a seeded answer records as in place. The plans
/// span the `Risk` the service derives from an applicable answer, and each is stated over
/// the question's OWN choice set rather than a fixed number of choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InPlace {
    /// Every best practice the question asks about: nothing is left unselected, so the
    /// question is rated `NONE`.
    Every,
    /// None of them, with every choice left unselected: the answer says nothing about any
    /// of the question's best practices, so the question is back to `UNANSWERED`.
    Nothing,
    /// All but the last: one best practice is still unselected, so the question is rated
    /// `MEDIUM` (for a question offering more than one choice).
    AllButLast,
    /// All but the last, which is recorded `NOT_APPLICABLE` rather than unselected:
    /// nothing is left unselected, so the question is rated `NONE`.
    AllButLastWhichDoesNotApply,
    /// None of them in place, with the last recorded `NOT_APPLICABLE` rather than
    /// unselected: the answer DOES speak about that choice, so the question is rated —
    /// with no best practice in place, at `HIGH`.
    NothingAndTheLastDoesNotApply,
}

/// A seeded answer to one question of the framework lens that records `in_place` out of
/// that question's own choices ([`framework_choice_ids`]), with the question applying.
pub(crate) fn framework_answer(question_id: &str, in_place: InPlace) -> AnswerRecord {
    let choices = framework_choice_ids(question_id);
    let ids: Vec<&str> = choices.iter().map(String::as_str).collect();
    let all_but_last: Vec<&str> = ids
        .iter()
        .take(ids.len().saturating_sub(1))
        .copied()
        .collect();
    match in_place {
        InPlace::Every => answered_over(&choices, &ids, true, None),
        InPlace::Nothing => answered_over(&choices, &[], true, None),
        InPlace::AllButLast => answered_over(&choices, &all_but_last, true, None),
        InPlace::AllButLastWhichDoesNotApply => {
            let statuses: Vec<(&str, &str)> = ids
                .iter()
                .map(|choice_id| {
                    let status = if all_but_last.contains(choice_id) {
                        state::CHOICE_STATUS_SELECTED
                    } else {
                        "NOT_APPLICABLE"
                    };
                    (*choice_id, status)
                })
                .collect();
            answered(&statuses, true, None)
        }
        InPlace::NothingAndTheLastDoesNotApply => {
            let statuses: Vec<(&str, &str)> = ids
                .iter()
                .map(|choice_id| {
                    let status = if all_but_last.contains(choice_id) {
                        state::CHOICE_STATUS_UNSELECTED
                    } else {
                        "NOT_APPLICABLE"
                    };
                    (*choice_id, status)
                })
                .collect();
            answered(&statuses, true, None)
        }
    }
}

/// A seeded answer that says one question of the framework lens does NOT apply to the
/// reviewed resource, for the `AnswerReason` given: the question is rated
/// `NOT_APPLICABLE` whatever its choices record.
pub(crate) fn framework_answer_that_does_not_apply(question_id: &str, reason: &str) -> AnswerRecord {
    answered_over(
        &framework_choice_ids(question_id),
        &[],
        false,
        Some(reason),
    )
}

/// The ARN the service REPORTS for an AWS-OFFICIAL lens: partition-global and owned by no
/// customer account — `arn:aws:wellarchitected::aws:lens/<alias>`. Spelled out here
/// (rather than read back out of the code under test) with the account placeholder taken
/// from the emulator's one source of truth, so a caller would fail if an owner account
/// appeared in it.
///
/// `region` is the Region the case asked in, which an AWS-official lens's ARN does not
/// name: the one lens is the same lens in every Region. A caller may still NAME the lens
/// by a Region-qualified ARN ([`regional_lens_arn`]), which resolves to the same lens.
pub(crate) fn official_lens_arn(region: &str, alias: &str) -> String {
    let arn = format!("arn:aws:wellarchitected::aws:lens/{alias}");
    assert!(
        !arn.contains(engine::emulator_account_id()),
        "an AWS-official lens ARN names no owner account, got {arn:?}"
    );
    assert!(
        !arn.contains(region) || region.is_empty(),
        "an AWS-official lens ARN names no Region, got {arn:?} for {region:?}"
    );
    arn
}

/// An AWS-official lens ARN as a CALLER may spell it, naming the Region it signed for —
/// the form the `LensAlias` doc shows ("the lens ARN, such as
/// `arn:aws:wellarchitected:us-east-1::lens/serverless`"). Accepted on input, and naming
/// the same lens as the bare alias and as [`official_lens_arn`].
pub(crate) fn regional_lens_arn(region: &str, alias: &str) -> String {
    format!("arn:aws:wellarchitected:{region}::lens/{alias}")
}

/// A minimal valid `CreateWorkload` request; callers override the members their case
/// varies. `AwsRegions` and `ReviewOwner` are present because the CreateWorkload doc
/// requires both even though the model does not.
pub(crate) fn minimal_create(name: &str, token: &str) -> Value {
    json!({
        "WorkloadName": name,
        "Description": "a workload under test",
        "Environment": "PRODUCTION",
        "Lenses": ["wellarchitected"],
        "AwsRegions": ["us-east-1"],
        "ReviewOwner": "owner@example.com",
        "ClientRequestToken": token,
    })
}

/// One member of an answer's body as a string, empty when the body carries none. The one
/// member reader the suites in this crate share.
pub(crate) fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A minimal valid `CreateProfile` request: `ProfileName`, `ProfileDescription`,
/// `ProfileQuestions` and `ClientRequestToken` are all `@required`
/// (`smithy_query(query="operation", operation="CreateProfile")`). The one profile request
/// builder this crate's suites share.
///
/// The answer names a question the published profile template asks and a choice that
/// question offers, which is what a profile may record: both ids are read off
/// `state::PROFILE_TEMPLATE_QUESTIONS`, the table `GetProfileTemplate` serves, so this
/// request stays valid however that table is revised.
pub(crate) fn minimal_profile(name: &str) -> Value {
    json!({
        "ProfileName": name,
        "ProfileDescription": "a profile under test",
        "ProfileQuestions": [{
            "QuestionId": template_question(0),
            "SelectedChoiceIds": [template_choice(0, 0)],
        }],
        "ClientRequestToken": format!("tok-{name}"),
    })
}

/// The `QuestionId` of the question at `index` in the table the profile template publishes
/// (`state::PROFILE_TEMPLATE_QUESTIONS`, served by `GetProfileTemplate` — the only place a
/// client discovers one). The index wraps, so a fixture asking for "a fourth question"
/// gets a published one however many the template asks.
pub(crate) fn template_question(index: usize) -> &'static str {
    let published = state::PROFILE_TEMPLATE_QUESTIONS;
    published[index % published.len()].question_id
}

/// The `ChoiceId` of the choice at `choice` among those the question
/// [`template_question`] names at `index` offers. Both indices wrap, for the same reason.
pub(crate) fn template_choice(index: usize, choice: usize) -> &'static str {
    let published = state::PROFILE_TEMPLATE_QUESTIONS;
    let question = &published[index % published.len()];
    question.choices[choice % question.choices.len()].choice_id
}

/// A profile minted over HTTP: its `ProfileArn` and the `ProfileVersion` the service put
/// it at, both checked against the modelled forms
/// (`^arn:aws[-a-z]*:wellarchitected:[a-z]{2}(-gov)?-[a-z]+-\d:\d{12}:profile/[a-z0-9]+$`
/// and `@length(1, 32)` `^[A-Za-z0-9-]+$`). The one profile-minting helper this crate's
/// suites share.
pub(crate) async fn created_profile(client: &Client, name: &str) -> (String, String) {
    let answer = client.create_profile(minimal_profile(name)).await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateProfile 200, got {answer:?}"
    );
    let arn = member(&answer, "ProfileArn").to_string();
    let version = member(&answer, "ProfileVersion").to_string();
    assert!(
        arn.starts_with(&format!(
            "arn:aws:wellarchitected:us-west-2:{}:profile/",
            engine::emulator_account_id()
        )),
        "case {name:?}: unexpected ProfileArn {arn:?}"
    );
    assert!(
        (1..=32).contains(&version.len())
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {name:?}: ProfileVersion must match ^[A-Za-z0-9-]{{1,32}}$, got {version:?}"
    );
    (arn, version)
}

/// The `Workload` a `GetWorkload` reports, as a JSON object.
pub(crate) async fn stored_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    answer.body.get("Workload").cloned().unwrap_or(Value::Null)
}

/// How many workloads the store holds, read off the shared state — for an assertion about
/// what a refused write RECORDED, which no list projection of the store can answer. Every
/// request such a test asserts about still crosses the HTTP boundary. The one workload
/// counter this crate's suites share.
pub(crate) fn stored_workload_count(client: &Client) -> usize {
    let state = client.store();
    let store = state.lock();
    store.workloads.len()
}

/// How many review templates the store holds, the counterpart of
/// [`stored_workload_count`] — the one template counter this crate's suites share.
pub(crate) fn stored_template_count(client: &Client) -> usize {
    let state = client.store();
    let store = state.lock();
    store.review_templates.len()
}

/// How many custom lenses the store holds, the counterpart of [`stored_workload_count`] —
/// the one lens counter this crate's suites share. It is what tells a replayed import from
/// a second one, which no read of one lens can answer.
pub(crate) fn stored_lens_count(client: &Client) -> usize {
    let state = client.store();
    let store = state.lock();
    store.lenses.len()
}

/// The `Workload$Profiles` a workload reports, empty for a workload that reports none.
pub(crate) async fn profiles_of(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    stored_workload(client, workload_id, case)
        .await
        .get("Profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `LensReview$Profiles` the workload's review of one lens reports: "the profiles
/// associated with the workload" (`smithy_query(query="doc", shape="LensReview",
/// member="Profiles")`), so a review has to agree with `GetWorkload` about them.
pub(crate) async fn lens_review_profiles(
    client: &Client,
    workload_id: &str,
    lens: &str,
    case: &str,
) -> Vec<Value> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("Profiles"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Workload$UpdatedAt` a workload reports, as the epoch seconds restJson1 carries it.
pub(crate) async fn updated_at_of(client: &Client, workload_id: &str, case: &str) -> f64 {
    stored_workload(client, workload_id, case)
        .await
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the workload must report UpdatedAt"))
}

/// Move the stored workload's `UpdatedAt` back by `age`, so a write the handler makes to
/// it is observable whatever the clock's granularity between two in-process requests.
/// Only the SEED reaches state directly; the write under test is the handler's.
pub(crate) fn age_workload(client: &Client, workload_id: &str, age: Duration) {
    let state = client.store();
    let mut store = state.lock();
    let workload = store
        .workloads
        .get_mut(workload_id)
        .expect("the workload under test must exist");
    workload.updated_at = SystemTime::now() - age;
}

/// Move the stored profile's `UpdatedAt` back by `age`, so a write the handler makes to it
/// is observable whatever the clock's granularity between two in-process requests — and so
/// a write it must NOT make cannot hide behind that granularity either. Only the SEED
/// reaches state directly; the write under test is the handler's.
pub(crate) fn age_profile(client: &Client, profile_arn: &str, age: Duration) {
    let state = client.store();
    let mut store = state.lock();
    let profile = store
        .profiles
        .get_mut(profile_arn)
        .expect("the profile under test must exist");
    profile.updated_at = SystemTime::now() - age;
}

/// The refusal a lookup that resolved nothing gets: 404 `ResourceNotFoundException`, with
/// the modelled `ResourceType` of the resource that was missing and the identifier AS SENT.
pub(crate) fn assert_not_found(
    case: &str,
    answer: &Answer,
    resource_type: &str,
    resource_id: &str,
    subject: &str,
) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        resource_type,
        "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the identifier as sent, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[ResourceNotFound] No {subject} was found in account {}.",
            engine::emulator_account_id()
        ),
        "case {case:?}: unexpected Message, got {answer:?}"
    );
}

/// The refusal a request that NAMES lens content the service cannot resolve gets — a lens
/// review, a question of one — : `400 ValidationException` whose `Reason` is
/// `FIELD_VALIDATION_FAILED`, whose `Fields` carries ONE entry naming the member the value
/// came in, and whose `Message` is that entry's sentence behind this service's
/// `[Validation]` tag.
///
/// The counterpart of [`assert_not_found`] for the values a request carries, as opposed to
/// the resources it identifies, and of [`assert_fields_failed`] for the ones the service
/// MATCHES against lens content rather than resolving.
pub(crate) fn assert_field_validation(
    case: &str,
    answer: &Answer,
    member_name: &str,
    sentence: &str,
) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Reason"),
        "FIELD_VALIDATION_FAILED",
        "case {case:?}: expected Reason FIELD_VALIDATION_FAILED, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!("[Validation] {sentence}"),
        "case {case:?}: unexpected Message, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Fields").cloned().unwrap_or(Value::Null),
        json!([{ "Name": member_name, "Message": sentence }]),
        "case {case:?}: Fields must name the member that carried the value, got {answer:?}"
    );
}

/// The refusal a request whose VALUE was matched against lens content and found outside it
/// gets — a choice the question does not offer, a pillar the lens does not define: `400
/// ValidationException` whose `Reason` is `FIELD_VALIDATION_FAILED`, whose `Message` states
/// only the CLASS of the failure and so carries no value from the request or the store, and
/// whose `Fields` carries ONE entry naming the member the value came in with `detail` — the
/// rule the value broke and the values that content does define.
///
/// The counterpart of [`assert_field_validation`] for that class, which words its finding
/// the other way round; the one envelope assertion this crate's suites share for it.
pub(crate) fn assert_fields_failed(case: &str, answer: &Answer, member_name: &str, detail: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Reason"),
        "FIELD_VALIDATION_FAILED",
        "case {case:?}: expected Reason FIELD_VALIDATION_FAILED, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        "[Validation] The validation of one or more fields has failed.",
        "case {case:?}: unexpected Message, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Fields").cloned().unwrap_or(Value::Null),
        json!([{ "Name": member_name, "Message": detail }]),
        "case {case:?}: Fields must name the member that carried the value and hand back \
         the valid values, got {answer:?}"
    );
}

/// The refusal a request that asks to APPLY a lens the catalog does not resolve gets: `400
/// ValidationException` naming the lens as sent, with `Reason` carried and carrying no
/// per-member finding.
pub(crate) fn assert_unresolvable_lens(case: &str, answer: &Answer, lens: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[Validation] Failed to get lenses {lens}. User might be not authorized to \
             access lens."
        ),
        "case {case:?}: unexpected Message, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Reason").cloned(),
        Some(Value::String(String::new())),
        "case {case:?}: Reason must be carried without a classification, got {answer:?}"
    );
    assert!(
        answer.body.get("Fields").is_none(),
        "case {case:?}: this refusal carries no per-member finding, got {answer:?}"
    );
}

/// A minimal valid `CreateReviewTemplate` request; callers override the members their
/// case varies. The one review-template request builder this crate's suites share.
pub(crate) fn minimal_template(name: &str, token: &str) -> Value {
    json!({
        "TemplateName": name,
        "Description": "a review template under test",
        "Lenses": ["wellarchitected"],
        "ClientRequestToken": token,
    })
}

/// The `TemplateArn` a successful `CreateReviewTemplate` answered with, checked against
/// the modelled `TemplateArn` pattern and its `@length(50, 250)`.
pub(crate) fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    let arn = answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: TemplateArn must be {expected_prefix}<32 hex>, got {arn:?}"
    );
    assert!(
        (50..=250).contains(&arn.len()),
        "case {case:?}: TemplateArn must satisfy @length(50, 250), got {} characters in {arn:?}",
        arn.len()
    );
    arn
}

/// The `WorkloadId` a successful `CreateWorkload` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
pub(crate) fn created_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkload 200, got {answer:?}"
    );
    let id = answer
        .body
        .get("WorkloadId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: WorkloadId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}
