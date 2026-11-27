Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3996f901063d22c400f1655bb242a5d1c6eab093

# The "required personalization input must be supplied" gate is implemented three times, once exhaustively and twice by hand

_Filed: 2026-11-27T05:08:39Z_
_Operation: GetActionRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3996f901063d22c400f1655bb242a5d1c6eab093`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate owns one rule for the personalization input an inference request has to carry: the resolved resource states which input it needs (`RequiredInput`, derived once per resource by `Campaign::required_input` / `Recommender::required_input`), and a request whose that input is absent — read through the crate's one `present()` rule — is refused with `InvalidInputException` naming the member.

The rule is implemented three times, in three different shapes, and `RequiredInput` carries no predicate for it (it exposes only `personalization` and `personalizing_user`, which answer *which value* is used, never *whether it was supplied*):

1. `GetRecommendations` decides it with an exhaustive `match` over `RequiredInput`, covering both the `User` and the `Item` arm.
2. `GetActionRecommendations` decides it with a hand-written `==` comparison against the single `RequiredInput::User` arm. Because it is a comparison and not a `match`, the compiler cannot force it to be updated: if `Campaign::required_input()` ever answered `RequiredInput::Item` (or anything else) for a `PERSONALIZED_ACTIONS` campaign, this handler would silently serve a request that is missing the input its own resolved campaign needs, while the sibling handler keeps refusing — the exact drift mode the already-fixed "recipe-type-to-operation mapping is implemented four times" issue was fixed by routing every handler through one shared predicate.
3. `GetPersonalizedRanking` decides it without consulting `RequiredInput` at all, with a hard-coded `input.user_id.is_empty()` check. That copy is why the `RecipeType::PersonalizedRanking => RequiredInput::User` arm in `Campaign::required_input()` is dead code: the only operation that serves a `PERSONALIZED_RANKING` campaign never asks the campaign what input it needs, and the only operation that asks (`GetRecommendations`) refuses such a campaign one step earlier. So the single fact "a PERSONALIZED_RANKING campaign needs the user input" is now stated in two places, one of which nothing reads and nothing can test.

The three refusal SENTENCES differ and are pinned by tests, so they must stay exactly as they are; what can be stated once is the branch ("is the input this resource requires supplied?"), e.g. a `RequiredInput` accessor answering which member is missing, with each handler keeping its own wording. Every response and every existing test stays unchanged.

## Evidence

Copy 1 — the exhaustive form, `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs:278-295`:
```
        // 3. The personalization input the resolved resource needs.
        let user_id = registry::present(input.user_id.as_deref()).unwrap_or_default();
        let item_id = registry::present(input.item_id.as_deref()).unwrap_or_default();
        match resolved.required_input {
            RequiredInput::User if user_id.is_empty() => {
                return Err(invalid(format!(
                    "userId is required to get recommendations from {}.",
                    resolved.description
                )))
            }
            RequiredInput::Item if item_id.is_empty() => {
                return Err(invalid(format!(
                    "itemId is required to get recommendations from {}.",
                    resolved.description
                )))
            }
            _ => {}
        }
```

Copy 2 — the same rule as a one-armed `==`, `src/ops/get_action_recommendations.rs:237-244`:
```
        let user_id = registry::present(input.user_id.as_deref()).unwrap_or_default();
        if campaign.required_input() == RequiredInput::User && user_id.is_empty() {
            return Err(invalid(format!(
                "userId is required to get action recommendations from campaign {} ({}).",
                campaign.arn,
                campaign.recipe_type.as_str()
            )));
        }
```
its own comment says it is the same rule: `src/ops/get_action_recommendations.rs:230-236` — "`userId` is the personalization input the RESOLVED campaign needs ... it is the input the resolved campaign's recipe type asks for ([`crate::state::Campaign::required_input`]) ... exactly as the sibling `GetRecommendations` reports the input ITS resolved resource needs."

Copy 3 — the same rule with `RequiredInput` bypassed, `src/ops/get_personalized_ranking.rs:231-240`:
```
        //    `userId` is a fact about the request too: this operation serves
        //    `PERSONALIZED_RANKING` campaigns only, so the personalization input it
        //    needs never depends on what the ARN resolves to.
        if input.user_id.is_empty() {
            return Err(invalid(
                "userId is required: it is the user the campaign provides a personalized ranking \
                 for.",
            ));
        }
```

The arm copy 3 renders unreachable, `src/state.rs:447-458`:
```
    /// Which personalization input a `GetRecommendations` call against this campaign
    /// needs: "USER_PERSONALIZATION - userId required, itemId not used RELATED_ITEMS
    /// - itemId required, userId not used" (`GetRecommendations` doc).
    pub fn required_input(&self) -> RequiredInput {
        match self.recipe_type {
            RecipeType::UserPersonalization | RecipeType::PersonalizedRanking => {
                RequiredInput::User
            }
            RecipeType::RelatedItems => RequiredInput::Item,
            RecipeType::PersonalizedActions => RequiredInput::User,
        }
    }
```
(`PersonalizedRanking` can only be reached from `GetPersonalizedRanking`, which never calls this method, and `GetRecommendations` refuses a `PERSONALIZED_RANKING` campaign at `src/ops/get_recommendations.rs:523-530` before step 3 runs — so that arm is unread.)

The helper that exists but answers the other question, `src/state.rs:201-219`: `pub fn personalization<'a>(self, user_id: &'a str, item_id: &'a str) -> &'a str` and `pub fn personalizing_user(self, user_id: &str) -> &str` — neither says whether the required input was supplied.

Test coverage that must stay green and byte-identical: `tests/all/optional_input_refusal_order.rs:189` ("userId is required to get action recommendations from campaign …"), `:231`, `:243` and `:264` (the `GetRecommendations` user/recommender/item sentences), `tests/all/empty_string_member_constraints.rs:404` (the `GetPersonalizedRanking` sentence), `:415`, `:426`, `:437`, `:448`, and `tests/all/request_only_refusal_order.rs:254`, `:427`.

## Steps to Reproduce

1. Read src/ops/get_recommendations.rs:278-295: the gate is an exhaustive `match resolved.required_input` with a `User` arm and an `Item` arm, both refusing when the member the resource needs is empty.
2. Read src/ops/get_action_recommendations.rs:237-244: the same gate again, written as `campaign.required_input() == RequiredInput::User && user_id.is_empty()` — a comparison the compiler never has to revisit when a `RequiredInput` arm is added or a recipe type's required input changes.
3. Read src/ops/get_personalized_ranking.rs:231-240: the same gate a third time, hard-coded on `input.user_id` without consulting `RequiredInput` at all.
4. Read src/state.rs:447-458: `Campaign::required_input()` maps `RecipeType::PersonalizedRanking => RequiredInput::User`. Nothing reads that arm — `GetPersonalizedRanking` (step 3 above) hard-codes the check, and `GetRecommendations` refuses a `PERSONALIZED_RANKING` campaign at src/ops/get_recommendations.rs:523-530 before its own gate runs — so the fact "PERSONALIZED_RANKING needs the user input" is stated twice, once unreadably.
5. Read src/state.rs:201-219: `RequiredInput` exposes `personalization` and `personalizing_user` only, so there is no single predicate for "is the input this resource requires supplied?" for the three copies to share.
6. Conclude: one rule, three implementations, one of them inexhaustive and one bypassing the enum entirely. Hoisting the branch onto `RequiredInput` (each handler keeping its own sentence) leaves every response and every test above unchanged.
