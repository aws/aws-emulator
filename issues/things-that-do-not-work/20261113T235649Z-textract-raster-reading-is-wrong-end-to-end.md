Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: textract.detect-document-text-reads-rendered-text textract.detect-document-text-from-s3-object-source textract.start-document-text-detection-job-lifecycle textract.analyze-document-forms-extracts-key-value-pairs textract.analyze-document-combined-features-tables-and-forms textract.analyze-document-queries-feature-returns-query-block textract.analyze-expense-extracts-summary-fields
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract.detect-document-text-reads-rendered-text|run#8|AssertionError|assert "SIM2REAL" in joined and token in joined, (
Issue-Key: textract.detect-document-text-from-s3-object-source|run#11|AssertionError|assert "SOURCED" in joined and "DOCUMENT" in joined, f"unexpected OCR output: {line_texts!r}"
Issue-Key: textract.start-document-text-detection-job-lifecycle|run#14|AssertionError|assert "ASYNC" in line_texts and "TEXTRACT" in line_texts and "JOB" in line_texts, (
Issue-Key: textract.analyze-document-forms-extracts-key-value-pairs|run#7|AssertionError|assert kv_blocks, "no KEY_VALUE_SET blocks returned for a form-like document"
Issue-Key: textract.analyze-document-combined-features-tables-and-forms|run#8|AssertionError|assert table_blocks, "no TABLE block detected in the rendered grid"
Issue-Key: textract.analyze-document-queries-feature-returns-query-block|run#12|AssertionError|assert answer_ids, "QUERY block has no ANSWER relationship to a QUERY_RESULT block"
Issue-Key: textract.analyze-expense-extracts-summary-fields|run#7|AssertionError|assert expense_doc["SummaryFields"], "no SummaryFields extracted from the receipt"
Issue-Commit: d1ed6d82cb6e069fe1b5dc1fba14ae1b2ea73c86

# Textract raster reading is wrong end to end: glyphs misread, word breaks lost, and FORMS/TABLES/QUERIES/AnalyzeExpense extract nothing from a rendered page

> _Note: this issue was identified against the code base at commit `d1ed6d82cb6e069fe1b5dc1fba14ae1b2ea73c86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AnalyzeDocument

## Description
Every textract data-plane read path answers HTTP 200 with a structurally valid response whose CONTENT does not describe the submitted document. No refusal, no error code, no non-2xx: the divergence is entirely in the reported text and in the derived block/field sets. This issue bundles 6 distinct root causes, all in the textract crate (services/service-com-amazonaws-textract-textract); a fixer must close ALL SIX before submitting.

Context on what is already filed: the resolved issue 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md claimed these same content causes fixed, and the resolved 20261111T092651Z-textract-synchronous-blocks-do-not-describe-the-submitted--r2.md claimed the synchronous blocks now describe the document; both are in the resolved folder yet the suite still fails on content, so neither covers what is reported here (and neither failure is a refusal, so this is not a reopened refusal). The open issue 20261110T200812Z-textract-analyzedocument-tables-cell-blocks-carry-garbled-text.md carries the combined-features check at the replay key `assert any(expected in t for t in cell_texts)` — a TABLE with garbled CELL text; that assertion is no longer reached, because the check now fails earlier with NO TABLE block at all (Cause 4 below), so that issue's key does not cover this failure. The open 20261030T113700Z (corrupt-image positive control) and 20261109T200548Z (AWS::Textract::Adapter in CloudFormation) issues own their own checks and are skipped here.

Cause 1 of 6 — the glyph recogniser misreads rendered ASCII, so Block$Text is not the text drawn on the page.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks, DetectDocumentTextModelVersion}; BlockType enum includes PAGE, LINE, WORD; Block$Text is documented as "The word or line of text that's recognized by Amazon Textract". The requirement is general: for ANY ASCII text rasterised onto a page, the LINE and WORD blocks report exactly the characters drawn, with Confidence in [0,100] and a BoundingBox inside [0,1]. The strings in the evidence below are one run's output filling that template, not the requirement.
Emulator: structure, Confidence, Geometry and DetectDocumentTextModelVersion are all present and in range, but characters are misclassified: a page drawing "SIM2REAL <token>" / "TEXTRACT CHECK" read back as ['SIV2REAL PVY92SHC', 'DIXTRACT CHECK'] (M read as V, TE read as DIX); a page drawing "S3 SOURCED DOCUMENT" read back as ['W IX)UICEO[XX:UVENT']; the async path over "ASYNC TEXTRACT JOB LINE" read back as 'ASYNC TEXTRACTJO8UNE' (B read as 8, LI read as U). The same corruption appears on the inline-Bytes path, the Document.S3Object path and the asynchronous Get* path, so it is in the one shared recogniser rather than in any handler.
Why: a client calls Textract to read a document. It gets a response that passes every schema, confidence and geometry check it makes, yet the strings are characters that were never on the page, so its matching, indexing and numeric parsing silently run on text the document does not contain. Nothing signals a failure.
Fix: the per-glyph classification and scoring in the OCR module (crate::state::ocr and its glyphs submodule, reached from crate::state::read_submitted_document / read_content) — rendered ASCII must resolve to the drawn characters before any WORD text is emitted. This cause is upstream of Causes 3, 5 and 6, which parse the recognised text.
Traces: textract.detect-document-text-reads-rendered-text, textract.detect-document-text-from-s3-object-source, textract.start-document-text-detection-job-lifecycle.

Cause 2 of 6 — word separation collapses several drawn words into one WORD/LINE token, so word count and word boundaries do not match the page.
AWS: Smithy: BlockType includes LINE and WORD; Block$Text documents a WORD as "The word ... recognized by Amazon Textract", and AWS returns one WORD block per whitespace-separated word on the line, each with its own Geometry and Confidence; a LINE's CHILD relationship resolves to those WORD blocks. For a line drawing four space-separated words AWS returns four WORD blocks.
Emulator: words drawn with an intervening space are emitted as a single token. "ASYNC TEXTRACT JOB LINE" came back as the single LINE text 'ASYNC TEXTRACTJO8UNE', in which "TEXTRACT", "JOB" and "LINE" are fused with no separator; "S3 SOURCED DOCUMENT" came back as 'W IX)UICEO[XX:UVENT', with the last two words fused. Hypothesis: the inter-mark spacing analysis that sets word breaks (crate::state::ocr Separators — the word/column break decision taken off the vertical projection profile) is not finding the space advance at the rendered pitch, so adjacent words are segmented as one run. This is separable from Cause 1: even a correctly classified glyph sequence would still be reported as the wrong number of words.
Why: a client that counts WORD blocks, or that tokenises on word boundaries to locate a field, sees fewer words than the page holds and word strings that span two real words, so per-word geometry (used for redaction or highlighting) covers the wrong region of the page.
Fix: the word/column separation in crate::state::ocr (Separators and the segmentation step that feeds WordLayout), which is what downstream line/word block emission in crate::state::blocks consumes.
Traces: textract.start-document-text-detection-job-lifecycle, textract.detect-document-text-from-s3-object-source.

Cause 3 of 6 — AnalyzeDocument with FeatureTypes=[FORMS] returns no KEY_VALUE_SET block at all for a rendered form page.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}; BlockType includes KEY_VALUE_SET; EntityType includes KEY and VALUE; RelationshipType includes VALUE and CHILD. AnalyzeDocument's documentation states form data is returned in two Block objects of type KEY_VALUE_SET — a KEY block and a VALUE block — the KEY block carrying a VALUE relationship to its value block and CHILD relationships to the WORD blocks of its text. For any page rendering a labelled field, AWS returns that KEY/VALUE pair.
Emulator: the Blocks list contains zero KEY_VALUE_SET blocks; the check fails on `assert kv_blocks, "no KEY_VALUE_SET blocks returned for a form-like document"`. The FORMS derivation (crate::state::form_blocks, invoked from crate::state::blocks when FeatureSet.forms is set) emits a pair only for a line that crate::state::labelled_split accepts, and labelled_split requires `line.colon` — a ':' recognised in the line's character text, with non-empty text on both sides. A page whose label text is not recovered character-for-character therefore yields no field at all, and the whole feature degrades to silence rather than to a partial answer.
Why: a client that asks for FORMS receives a 200 whose block list is indistinguishable from a plain DetectDocumentText answer, so its key/value loop concludes the document carries no form fields — a false negative on exactly the document class the feature exists for.
Fix: AnalyzeDocument's FORMS derivation in crate::state (form_blocks / labelled_split, called from crate::state::blocks): KEY/VALUE pairing must come from the page's recognised words and their geometry, and must emit the KEY and VALUE EntityTypes with the VALUE and CHILD relationships for a label/value pair on a raster page.
Traces: textract.analyze-document-forms-extracts-key-value-pairs (the same absence is asserted later in textract.analyze-document-combined-features-tables-and-forms, which fails earlier on Cause 4).

Cause 4 of 6 — AnalyzeDocument with FeatureTypes=[TABLES] returns no TABLE (and therefore no CELL) block for a table drawn as ruling lines.
AWS: Smithy: BlockType includes TABLE, CELL and MERGED_CELL; Block$RowIndex and Block$ColumnIndex are documented 1-based; RelationshipType includes CHILD. AnalyzeDocument's documentation states "A TABLE Block object contains information about a detected table. A CELL Block object is returned for each cell in a table." For a page rasterising a ruled grid, AWS returns one TABLE block per drawn table with one CELL per drawn cell, each cell's CHILD words being that cell's content words.
Emulator: no TABLE block is produced; the check fails on `assert table_blocks, "no TABLE block detected in the rendered grid"`. crate::state::table_runs builds a table from "a contiguous run of tab-separated lines" — LineLayout::cells, i.e. columns split on U+0009 in the recognised character text — so a rasterised page, which contains ink and not tab characters, produces an empty table-run set by construction. The response is still 200 with AnalyzeDocumentModelVersion, so the omission is silent. Note this is a REGRESSION against open issue 20261110T200812Z, which recorded a TABLE with four CELL children for this same check: the structure that issue describes is now gone.
Why: TABLES is the reason a client calls AnalyzeDocument rather than DetectDocumentText. It gets a successful response with no TABLE/CELL blocks and concludes the scanned document has no tables.
Fix: the TABLES derivation in crate::state (table_runs / table_blocks / cell_texts / cell_words / cell_boxes, reached from crate::state::blocks and shared with crate::state::line_item_groups) — row and column segmentation must be derived from the recognised words' geometry and the page's ruling lines on a raster page, not from tab characters in line text, and must emit TABLE with CELL children carrying 1-based RowIndex/ColumnIndex and CHILD word relationships.
Traces: textract.analyze-document-combined-features-tables-and-forms.

Cause 5 of 6 — a QUERY block comes back with no ANSWER relationship, so no QUERY_RESULT block exists for a document that plainly answers the query.
AWS: Smithy: QueriesConfig{Queries required}, Query{Text required @length 1..200, Alias @length 1..200, Pages}; BlockType includes QUERY and QUERY_RESULT; RelationshipType includes ANSWER. AnalyzeDocument's documentation states a QUERY block contains the query text, alias and link to the associated query-results block, and a QUERY_RESULT block contains the answer to the query plus a confidence score. For a page rendering a labelled value and a query asking for it, AWS links the QUERY block by an ANSWER relationship to a QUERY_RESULT whose Text is the value read off the page.
Emulator: the QUERY block is present and correctly echoes Query.Text and Query.Alias (those assertions pass), but it carries no ANSWER relationship: `assert answer_ids, "QUERY block has no ANSWER relationship to a QUERY_RESULT block"`. crate::state (query blocks, around the answer_for/block_id("QUERY_RESULT") path) answers a query only out of the colon-labelled field index that labelled_split produces, so a page whose label line is not recovered can never yield an answer; a query with no match is documented as getting a QUERY block with no ANSWER, which makes a recognition failure indistinguishable from "the document does not say".
Why: a client using QUERIES reads the answer off QUERY_RESULT. It gets its question echoed with no answer and no error, and must read that as a negative answer about the document — a false negative.
Fix: the QUERIES answer derivation in crate::state (the query-block builder and the field index it searches): a matched query must emit a QUERY_RESULT block carrying the answer text and its measured confidence and link it from the QUERY block with an ANSWER relationship, with the searchable field set built from the page's recognised words rather than only colon-delimited character lines.
Traces: textract.analyze-document-queries-feature-returns-query-block.

Cause 6 of 6 — AnalyzeExpense returns an ExpenseDocument whose SummaryFields list is empty for a rendered receipt.
AWS: Smithy: AnalyzeExpense input {Document required}, output {DocumentMetadata, ExpenseDocuments}; ExpenseDocument{ExpenseIndex, SummaryFields — documented "Any information found outside of a table by Amazon Textract" —, LineItemGroups, Blocks}; ExpenseField{Type, LabelDetection, ValueDetection, PageNumber, Currency, GroupProperties}. For a receipt page whose lines are of the common "LABEL VALUE" form (a total line, a date line), AWS returns at least one SummaryField with its Type and ValueDetection, and Blocks whose text contains those tokens.
Emulator: ExpenseDocuments[0] is present but SummaryFields is empty: `assert expense_doc["SummaryFields"], "no SummaryFields extracted from the receipt"`. crate::state::summary_fields (called from crate::state::expense_documents) accepts a line only when labelled_split finds a recognised ':' on it, so whitespace-separated receipt lines produce no field even when their text is read correctly — a requirement in addition to Cause 1's garbling.
Why: AnalyzeExpense exists so a client does not parse a receipt itself. It gets a 200 with an empty SummaryFields list and records a receipt with no total, vendor or date.
Fix: AnalyzeExpense's summary-field derivation in crate::state::summary_fields / labelled_split (shared with the FORMS path) — a label/value pair must be recognised from whitespace-separated and currency-amount receipt lines, not only colon-delimited ones, and typed into ExpenseField$Type.
Traces: textract.analyze-expense-extracts-summary-fields.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against the emulator booted from this checkout: 16/26 passed (61.5%), 8 mismatched, 2 errored, 0 skipped, 0 unloadable. Per-service: textract 16/24 (67%) 8 failed; cloudformation__textract 0/2 (both OWNED by 20261109T200548Z and skipped). bootstrap_signal was false — the emulator was serving and textract answered HTTP 200 on every call below. Every failure is an assertion on response CONTENT; no diverging status code, error code or error message was observed on these checks.

Cause 1 (glyph misclassification):
[mismatch] textract.detect-document-text-reads-rendered-text — DetectDocumentText(Document={Bytes: PNG}) over a page rendering "SIM2REAL <run token>" and "TEXTRACT CHECK":
  AssertionError: expected run-specific token 'PVY92SHC' in OCR output, got: ['SIV2REAL PVY92SHC', 'DIXTRACT CHECK']
  Expected: the LINE texts spell the drawn characters ("SIM2REAL <token>", "TEXTRACT CHECK"). Actual: 'SIM2REAL' -> 'SIV2REAL', 'TEXTRACT' -> 'DIXTRACT'. (The token value is per-run; the requirement is character fidelity for whatever is drawn.)
[mismatch] textract.detect-document-text-from-s3-object-source — DetectDocumentText(Document={S3Object:{Bucket,Name}}) over an object rendering "S3 SOURCED DOCUMENT":
  AssertionError: unexpected OCR output: ['W IX)UICEO[XX:UVENT']
  Expected: LINE text containing SOURCED and DOCUMENT. Actual: neither appears.
[mismatch] textract.start-document-text-detection-job-lifecycle — StartDocumentTextDetection + GetDocumentTextDetection over an S3 object rendering "ASYNC TEXTRACT JOB LINE": the job reached JobStatus SUCCEEDED (that part passed), then
  AssertionError: unexpected async OCR output: 'ASYNC TEXTRACTJO8UNE'
  Expected: ASYNC, TEXTRACT and JOB all present. Actual: only ASYNC survived.

Cause 2 (word breaks lost): in the two outputs above, 'ASYNC TEXTRACTJO8UNE' fuses the drawn words TEXTRACT / JOB / LINE into one token, and 'W IX)UICEO[XX:UVENT' fuses SOURCED / DOCUMENT; the pages draw them space-separated, so AWS would report one WORD block per drawn word with its own Geometry.

Cause 3 (FORMS):
[mismatch] textract.analyze-document-forms-extracts-key-value-pairs — AnalyzeDocument(Document={Bytes: PNG}, FeatureTypes=["FORMS"]) over a page rendering "Name: John Smith" and "Date: 01/02/2024":
  AssertionError: no KEY_VALUE_SET blocks returned for a form-like document
  Expected: a KEY KEY_VALUE_SET whose CHILD words spell the label and whose VALUE relationship resolves to a block whose CHILD words spell the value. Actual: zero KEY_VALUE_SET blocks of any EntityType.

Cause 4 (TABLES):
[mismatch] textract.analyze-document-combined-features-tables-and-forms — AnalyzeDocument(Document={Bytes: PNG}, FeatureTypes=["TABLES","FORMS"]) over a page drawing a 2-column x 2-row grid with 2px ruling lines (header cells "Item"/"Price", data cells "Widget"/"10.00") plus a "Name: Test User" line:
  AssertionError: no TABLE block detected in the rendered grid  (check line 35)
  Expected: one TABLE block whose CHILD relationship resolves to at least four CELL blocks. Actual: no TABLE block at all — the check no longer reaches the later cell-content assertion that open issue 20261110T200812Z recorded as its key, nor the KEY_VALUE_SET assertions at the end of the same check.

Cause 5 (QUERIES):
[mismatch] textract.analyze-document-queries-feature-returns-query-block — AnalyzeDocument(Document={Bytes: PNG}, FeatureTypes=["QUERIES"], QueriesConfig={Queries:[{Text:"What is the invoice number?", Alias:"INVOICE_NUMBER"}]}) over a page rendering "Invoice Number: INV-99887". Exactly one QUERY block was returned and its Query.Text and Query.Alias echoes both passed; then
  AssertionError: QUERY block has no ANSWER relationship to a QUERY_RESULT block  (check line 42)
  Expected: an ANSWER relationship from the QUERY block to a QUERY_RESULT block whose Text carries the value drawn on the page. Actual: no ANSWER relationship, so no QUERY_RESULT.

Cause 6 (AnalyzeExpense):
[mismatch] textract.analyze-expense-extracts-summary-fields — AnalyzeExpense(Document={Bytes: PNG}) over a page rendering "RECEIPT", "TOTAL $12.34", "DATE 01/02/2023". ExpenseDocuments was non-empty (that assertion passed); then
  AssertionError: no SummaryFields extracted from the receipt  (check line 24)
  Expected: at least one ExpenseField in SummaryFields (a typed total/date with LabelDetection/ValueDetection), and ExpenseDocument.Blocks whose text contains the drawn label and amount. Actual: SummaryFields == [].

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 behavioral suite, textract group.
2. Cause 1: run textract.detect-document-text-reads-rendered-text — rasterise a PNG containing a fresh alphanumeric token plus the words SIM2REAL and TEXTRACT, call DetectDocumentText with Document.Bytes, and compare every LINE/WORD Text against the drawn characters. Repeat through Document.S3Object (textract.detect-document-text-from-s3-object-source) and through StartDocumentTextDetection + GetDocumentTextDetection (textract.start-document-text-detection-job-lifecycle). Each path must report the characters drawn, for any string.
3. Cause 2: on the same responses, count the WORD blocks of a line that draws N space-separated words; expect N WORD blocks with per-word Geometry. Observe drawn words fused into one token (e.g. TEXTRACT/JOB/LINE returned as a single run).
4. Cause 3: run textract.analyze-document-forms-extracts-key-value-pairs — AnalyzeDocument(FeatureTypes=["FORMS"]) on a PNG rendering a labelled field ("Name: John Smith"); observe zero KEY_VALUE_SET blocks instead of a KEY block with a VALUE relationship resolving to the rendered value.
5. Cause 4: run textract.analyze-document-combined-features-tables-and-forms — AnalyzeDocument(FeatureTypes=["TABLES","FORMS"]) on a PNG whose table is drawn with ruling lines; observe no TABLE block at all (and hence no CELL blocks) despite HTTP 200.
6. Cause 5: run textract.analyze-document-queries-feature-returns-query-block — AnalyzeDocument(FeatureTypes=["QUERIES"]) with one Query over a page rendering the queried value as a labelled line; observe the QUERY block has no ANSWER relationship and no QUERY_RESULT block exists.
7. Cause 6: run textract.analyze-expense-extracts-summary-fields — AnalyzeExpense on a rendered receipt whose total line is "TOTAL $12.34"; observe ExpenseDocuments[0].SummaryFields == [].
8. Re-replay all seven check ids in trace_ids after the fix; all must pass, including the assertions each check makes AFTER the one that fails today (cell content and KEY_VALUE_SET in the combined-features check, QUERY_RESULT text in the queries check, the OCR text assertions in the expense check).
