# Contacts CLI review log

User scope: implement `docs/implementation-plan.md`, independently review and fix each step, and make separate local commits. No remote publishing.

Baseline: `64cd10c`.

Reviewer reports are preserved below separately from owner dispositions. Each review identifies its frozen commit/diff or plan digest. The owner applies fixes; the reviewer remains read-only.

## Step 0 — plan review, pass 1

Reviewer: `/root/codebase_review` (independent reviewer role). Target: plan SHA-256 `a3e0635e2cf14d610cfe5d08475603b694671f8625aa602d2102e43faf8261ef`, code baseline `64cd10c`. Read-only source inspection.

Original reviewer report:

> Required — PLAN-01: Specify exclusive locking across apply and cleanup. Cleanup could observe cleared pending state while apply still owns its run lock, then race with subsequent writes. Require cleanup to acquire the existing apply lock before checking state and retain it through deletion and lifecycle updates. New batch journals also need exclusive locking. Include a focused concurrent-operation rejection check.
> Observation: Event registration can succeed with a different status, such as waitlisted instead of registered. Step 4 should report and checkpoint the actual outcome.
> Full planning depth and verification scope are proportionate. Existing-journal provenance limitations are adequately covered by the refusal/retention rule.

Owner disposition: PLAN-01 accepted (concurrency/data-integrity defect in proposed workflow). Added shared apply/cleanup locking, batch locks and concurrency checks. Accepted the attendance observation as a contract clarification; added readback/reporting of actual status. No implementation scope removed.

## Step 0 — plan review, pass 2

Target: plan SHA-256 `ea33ff7d277ac99f444abd276f5bbfafb35ace8b72b317f6ab697cad7fe996d8`, baseline `64cd10c`.

Original reviewer report:

> PLAN-01 resolved: exclusive lifecycle locking, shared apply/cleanup locking and concurrent-operation rejection tests are explicit. Attendance clarification resolved: actual status must be read back and recorded. No unresolved findings. Read-only inspection; no live operations.

Owner disposition: accepted. Plan review complete; implementation begins.

## Step 1 — source review, pass 1

Target: baseline `4d70704`, source/README manifest SHA-256 `a5a1c87658386f7f08239e7d2037098089cdc49213e15244f5b77608959611f7` (includes new gecko/query/catalog modules).

Original reviewer report:

> P2 QUERY-01: Floats and booleans are accepted by the CLI but Gecko equality only adds predicates for arrays, strings and integers. Equality with 0/"0"/false also enters the missing-value branch. Normalize supported comparisons or reject them. A recording PHP probe reproduced missing predicates.
> P2 QUERY-02: Gecko removes unknown/deleted/foreign IDs from a combined label condition, broadening a mixed valid/invalid request. Validate all IDs or use independent label conditions. Repeated identical IDs alone are handled correctly by Gecko.
> No further supported findings in query contracts, paging, JSON privacy or authenticated-client extraction. No live operations.

Owner disposition: accepted both. Decimal equality is encoded as text; boolean and zero/empty equality are rejected. Labels become separate AND conditions, so an absent label yields no matches. Added focused regressions and README limits. Initial checks: 73 tests and strict Clippy passed; read-only staging smoke test stopped at expired-token HTTP 401 before contact search.

## Step 1 — source review, pass 2

Target manifest SHA-256 `45b9a529b4e9b7a6323a950806c75c2736909a97dfbb2ac4410d6ef0c3f476a4`.

Original reviewer report:

> QUERY-02 resolved. QUERY-01 partially resolved: decimal contains still reaches Gecko's LIKE helper as a float, causing its predicate to be omitted. Normalize decimal contains too and cover it.

Owner disposition: accepted. Normalize every decimal scalar to text, including contains, and add its regression.

## Step 1 — source review, pass 3

Target manifest SHA-256 `6f7d26fe9f42c8d4c775fb1efaef768b2ab32cc9e0282bb5c65bf71dca9ccb7e`.

Original reviewer report:

> QUERY-01 resolved: decimal contains values now normalize to text, with regression coverage. No unresolved findings. Verified manifest/file hashes; read-only inspection.

Owner disposition: accepted. Step 1 complete with 74 passing tests, formatting and strict Clippy.

## Step 2 — source review, pass 1

Target: baseline `369513d`, manifest SHA-256 `ae550fce44465fb91fca5a2efa5d298251c084df9db87b6a11664a6129f8fb95`.

Original reviewer report:

> P2 EXPORT-01: Built-in phone/last_chat_message exports reuse the TUI's six-column projection, silently yielding empty strings for populated fields omitted from that configuration. Resolve aliases through actual field IDs/current values; require field:ID for ambiguity and cover an omitted populated field.
> No further supported findings in typed values, canonical field mapping, masking, pagination or atomic file publication. Read-only local source inspection.

Owner disposition: accepted. Aliases now resolve the actual field, prefer a unique configured choice when multiple exist, reject ambiguity and return null for absent types. Added a regression with populated fields outside the UI list. Initial 79 tests and strict Clippy passed.

## Step 2 — source review, pass 2

Target manifest SHA-256 `a3823ab3ccc1202629bb16caacc17d2422b1e515a517da1abb3b873a97e98637`.

Original reviewer report:

> EXPORT-01 resolved: aliases read actual values, preserve masking, return null for absent types and reject ambiguity. Regression covers populated fields omitted from the UI list. No remaining findings in this focused follow-up.

Owner disposition: accepted. Step 2 complete with 80 tests, formatting and strict Clippy passing.

## Step 3 — source review, pass 1

Target: baseline `e5fa2fa`, manifest SHA-256 `278ff5b9fff0921b9c4e57a5b0d416f72925fd2f95d64de6b3a6037c56ec0f74`.

Original reviewer report:

> P2 BATCH-01: label-remove sends remove_contact_label, absent from Ability's contact action permission map. Every needed removal fails Forbidden, even for admins, and leaves the journal pending. Omit it unless a supported API safely preserves unrelated labels.
> No further findings in targeting, consents, preflight, locking, readback or recovery. Local backend inspection only; no live operations.

Owner disposition: accepted. Removed the unsupported extra command and operation flag. The selected UI workflow (Add labels) remains implemented; consent grant/revoke use the supported manage_consent action. README/plan record the API limitation. Initial 89 tests and strict Clippy passed.

## Step 3 — source review, pass 2

Target manifest SHA-256 `ff75fdccee6d6c4555756f0a2c5f2f3fe41038cb9c0405ad851e64fabafdfa48`.

Original reviewer report:

> BATCH-01 resolved: unsupported command/removal path are gone. Label addition and consent grant/revoke retain supported payloads and recovery. No remaining findings in this focused follow-up.

Owner disposition: accepted. Step 3 complete with 89 tests, formatting and strict Clippy passing.

## Step 4 — source review, pass 1

Target: baseline `eb466f2`, manifest SHA-256 `e846ac9558c71231a7a8944a9aea57d1913f0032a2846547da95c926007bab5d`.

Original reviewer report:

> P2 MEMBER-01: A session container (type 20) can redirect registration to its child session-time ID, leaving pending after a successful write when readback uses the original ID. Reject containers or resolve/bind the concrete destination before preview. Discovery should expose type and parent ID.
> No further findings in targeting, status readback, preservation or recovery. Read-only local contract inspection.

Owner disposition: accepted. Session containers, missing types and unknown types fail in destination preflight; event discovery includes type/parent_id. Added a zero-write regression. Initial 93 tests and strict Clippy passed.

## Step 4 — source review, pass 2

Target manifest SHA-256 `fdf88521b3a03871ac02fd8af3dc43862b1d8c155850b4fac9a908ef405dc5f7`.

Original reviewer report:

> MEMBER-01 resolved: session containers and missing/unknown types rejected in preflight; discovery exposes type/parent_id, with zero-write coverage. No remaining findings in this focused follow-up.

Owner disposition: accepted. Step 4 complete with 94 tests, formatting and strict Clippy passing.

## Step 5 — source review

Target: baseline `118f42f`, manifest SHA-256 `71f67898257a10365ed06adb3357210b0942724318b531a3a328209239632a5e`.

Original reviewer report:

> No supported findings. Reviewed fixture/journal binding, checkpoint validation, field mapping, permissions, typed values, email counts, restricted-value handling and shared HTTP errors against local contracts. Verified manifest/file hashes and git diff --check; no live operations. Tests/Clippy are owner-run evidence.

Owner disposition: accepted. Step 5 complete with 100 tests, formatting and strict Clippy passing.
