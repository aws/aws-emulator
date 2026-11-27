Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.analyze-document-forms-extracts-key-value-pairs textract.analyze-expense-extracts-summary-fields
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract.analyze-document-forms-extracts-key-value-pairs|run#18|AssertionError|assert "JOHN" in linked_value_text and "SMITH" in linked_value_text, (
Issue-Key: textract.analyze-expense-extracts-summary-fields|run#10|AssertionError|assert "TOTAL" in all_block_text, f"expected TOTAL label in OCR text: {all_block_text!r}"
Issue-Commit: 5f47a0ef97da3054161c1821b9739375857f2595

# Textract OCR still misclassifies glyphs inside recognised words: FORMS value text and AnalyzeExpense block text report characters that were never drawn ('JOHN SMILH', 'TO1AL')

> _Note: this issue was identified against the code base at commit `5f47a0ef97da3054161c1821b9739375857f2595`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AnalyzeDocument

## Description
Both textract data-plane feature paths now emit the right STRUCTURE for a rendered page (KEY_VALUE_SET pairs for FORMS, a non-empty SummaryFields list for AnalyzeExpense — both of those assertions pass today), but the recognised TEXT carried by those blocks is not the text drawn on the page: individual glyphs are misclassified, so 'T' comes back as 'L' or '1'. HTTP 200 on every call; no error code, no error message, no non-2xx anywhere — the divergence is entirely in Block$Text / ExpenseDocument.Blocks[].Text content, which is what makes it silent.

This issue reports 1 root cause, grounded in 2 failing checks.

Context on what is already filed: the open issue 20261113T235649Z-textract-raster-reading-is-wrong-end-to-end.md describes glyph misclassification (its Cause 1) and lists both of these check ids under Issue-Trace-Ids, but it does NOT carry either failing replay key: its keys for these two checks are `assert kv_blocks, "no KEY_VALUE_SET blocks returned for a form-like document"` and `assert expense_doc["SummaryFields"], "no SummaryFields extracted from the receipt"`, and BOTH of those assertions now pass — the checks fail later, on the text content of the blocks that are now produced. The replay reported no owner for these two failures (the other 8 failures in this run were all marked OWNED/ALREADY FILED and are skipped). The open 20261115T094519Z-textract-synchronous-blocks-geometry-confidence-and-text-do.md is a WIRE issue about Blocks geometry/confidence/polygon ordering on DetectDocumentText/AnalyzeDocument and carries no behavioral key for these checks; the open 20261110T200812Z (TABLES CELL text), 20261114T190633Z (QUERY_RESULT leading characters), 20261030T113700Z (corrupt-image positive control) and 20261109T200548Z (CloudFormation AWS::Textract::Adapter) issues own their own checks. Neither failure is a refusal, so this is not a reopened refusal.

Cause 1 of 1 — the shared glyph recogniser misreads rendered ASCII characters, so the text of the FORMS VALUE block and of AnalyzeExpense's Blocks is not the text rasterised onto the page.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): Block$Text is documented "The word or line of text that's recognized by Amazon Textract." AnalyzeDocument input {Document required, FeatureTypes required, QueriesConfig, HumanLoopConfig, AdaptersConfig} / output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}; BlockType includes KEY_VALUE_SET and WORD, EntityType includes KEY and VALUE, RelationshipType includes VALUE and CHILD — AnalyzeDocument's documentation states form data is returned as a KEY block linked by a VALUE relationship to its VALUE block, each with CHILD relationships to its WORD blocks. AnalyzeExpense input {Document required} / output {DocumentMetadata, ExpenseDocuments}; ExpenseDocument{ExpenseIndex, SummaryFields, LineItemGroups, Blocks}. The requirement is general and character-exact: for ANY ASCII text rasterised onto a page, every WORD/LINE Text the service returns — including the WORD children of a KEY_VALUE_SET block and the Blocks of an ExpenseDocument — spells exactly the characters drawn, with whitespace-separated drawn words reported as separate word tokens. The strings quoted below are one run's output filling that template, not the requirement.
Emulator: characters are substituted inside otherwise correctly-structured results. (a) AnalyzeDocument(FeatureTypes=["FORMS"]) on a page drawing "Name: John Smith" / "Date: 01/02/2024" produces the KEY block (its text matches NAME) and a VALUE relationship that resolves to a VALUE block, but that block's CHILD words spell 'JOHN SMILH' — the 'T' of SMITH is read as 'L'. (b) AnalyzeExpense on a page drawing "RECEIPT" / "TOTAL $12.34" / "DATE 01/02/2023" returns non-empty SummaryFields and Blocks whose joined text is 'RECEIPT RECEIPT TO1AL $12.34 TO1AL $12.34 DATE01O2/2023 DATE01O2/2023' — 'TOTAL' is read as 'TO1AL' (the second 'T' read as the digit '1'), the drawn space between 'DATE' and '01/02/2023' is lost and '/' in the first position is read as 'O'. The amount '$12.34' is recovered, so the failure is per-glyph classification rather than a missing read path, and the same substitution class appears on two independent feature derivations, so it is in the one shared recogniser, not in either handler.
Why: a client calls Textract to read a document and gets a 200 whose shape, EntityTypes, relationships and SummaryFields all validate, yet the strings contain characters that were never on the page. A form-processing client stores 'JOHN SMILH' as the person's name; an expense client looking for the 'TOTAL' label never finds it and records the receipt with no total even though the amount was read. Nothing in the response signals that recognition failed, so these are silent data-corruption and false-negative paths.
Fix: the per-glyph classification and scoring in the OCR module of the textract crate (services/service-com-amazonaws-textract-textract/src/state/ocr.rs and the glyph/template matching it uses), reached from the page reader that feeds both the FORMS KEY/VALUE word children and AnalyzeExpense's ExpenseDocument.Blocks — rendered ASCII must resolve to the drawn characters, and inter-word spacing must break words, before any WORD text is emitted. The two downstream derivations (the FORMS KEY/VALUE builder and the AnalyzeExpense summary/blocks builder) consume that text and need no change once it is correct.
Traces: textract.analyze-document-forms-extracts-key-value-pairs, textract.analyze-expense-extracts-summary-fields.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 16/26 passed (61.5%), 8 mismatched, 2 errored, 0 skipped, 0 unloadable. Per-service: textract 16/24 (67%) 8 failed; cloudformation__textract 0/2 (both OWNED by 20261109T200548Z and skipped). bootstrap_signal was false — the emulator was serving and textract answered HTTP 200 on every call below. Of the 8 textract failures, 6 were marked OWNED by already-filed issues and skipped; the 2 below had no owner.

[mismatch] textract.analyze-document-forms-extracts-key-value-pairs
  Request: AnalyzeDocument(Document={Bytes: PNG rendering "Name: John Smith" and "Date: 01/02/2024"}, FeatureTypes=["FORMS"]).
  Passed: KEY_VALUE_SET blocks exist, a KEY-tagged block exists, a VALUE-tagged block exists, and a KEY block's CHILD words contain 'NAME'.
  Failed (check line 56): assert "JOHN" in linked_value_text and "SMITH" in linked_value_text
  AssertionError: the NAME key's VALUE relationship did not resolve to the rendered value, got 'JOHN SMILH'
  Expected: the words reached through the KEY block's VALUE relationship and that value block's CHILD relationship spell the drawn value ('JOHN SMITH' for this page). Actual: 'JOHN SMILH' — final 'T' of the drawn word returned as 'L'.

[mismatch] textract.analyze-expense-extracts-summary-fields
  Request: AnalyzeExpense(Document={Bytes: PNG rendering "RECEIPT", "TOTAL $12.34", "DATE 01/02/2023"}).
  Passed: ExpenseDocuments non-empty, SummaryFields non-empty, and the amount assertion "12.34" in the joined block text.
  Failed (check line 28): assert "TOTAL" in all_block_text
  AssertionError: expected TOTAL label in OCR text: 'RECEIPT RECEIPT TO1AL $12.34 TO1AL $12.34 DATE01O2/2023 DATE01O2/2023'
  Expected: ExpenseDocument.Blocks text containing the drawn label ('TOTAL') and the drawn date as drawn ('DATE 01/02/2023'). Actual: 'TO1AL' (T -> '1'), 'DATE01O2/2023' (space between the label and the value dropped, '/' -> 'O').

No diverging HTTP status, error code or error message was observed on either check: both are 200 with value-only divergence in the response body. The quoted strings are one run's output; the requirement is character fidelity to whatever ASCII is drawn, for any page.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, textract group.
2. Run textract.analyze-document-forms-extracts-key-value-pairs: rasterise a PNG containing a labelled field (e.g. "Name: John Smith") plus a second labelled line, call AnalyzeDocument(Document.Bytes, FeatureTypes=["FORMS"]), locate the KEY-tagged KEY_VALUE_SET block whose CHILD words spell the label, follow its VALUE relationship to the value block and join that block's CHILD WORD texts. Observe characters substituted inside the value ('SMITH' read as 'SMILH') while the structure and relationships are correct.
3. Run textract.analyze-expense-extracts-summary-fields: rasterise a receipt page whose lines are "RECEIPT", "TOTAL $12.34", "DATE 01/02/2023", call AnalyzeExpense(Document.Bytes), and join the Text of every ExpenseDocuments[0].Blocks entry. Observe the drawn label 'TOTAL' returned as 'TO1AL', the space between 'DATE' and its value dropped, and '/' returned as 'O', while '$12.34' is recovered correctly.
4. Vary the drawn strings (any ASCII words containing 'T', and any label/value line separated by a space): the substitution follows the glyph, not the string, confirming the defect is in the shared glyph classifier rather than in either feature derivation.
5. After the fix, re-replay both check ids in trace_ids — textract.analyze-document-forms-extracts-key-value-pairs and textract.analyze-expense-extracts-summary-fields — and require both to pass end to end, including the assertions that already pass today (KEY/VALUE structure, non-empty SummaryFields, the '12.34' amount).
