# Contacts CLI implementation plan

Build a practical development-data workflow: find contacts, inspect/export their fields, make targeted batch changes, verify generated scenarios, and clean up a recorded run.

Depth: full. Read-only browsing can be incremental, but bulk writes and cleanup need explicit targeting, durable progress, and safe recovery from uncertain outcomes. This plan and all commits stay local; no push, GitHub issue, or pull request is authorised.

## Baseline and scope

Baseline: `64cd10c` on the existing `main` checkout, with a clean working tree when this plan began. Rust CLI, no new application or backend changes.

Current behaviour: login/profile selection, authenticated contact list/detail TUI, plain tables, deterministic JSON scenario generation, and journaled scenario application. Account service `ProfileId`, app user `ExternalId`, public account ID, and numeric Gecko headers are distinct; preserve the checks in `src/app_identity.rs`.

The staging UI exposes search, saved/advanced filters, export/import, contact field options, bulk labels/consents, and event/organisation membership. We are implementing the selected development workflows below. Sending email/SMS, campaign administration, general CSV import, and account provisioning remain outside this iteration.

Source anchors: `src/main.rs` dispatches commands; `src/contacts.rs` owns reads and privacy masking; `src/app.rs`/`src/tui.rs` own interactive browsing; `src/scenarios/{fixture,apply}.rs` own fixture validation and apply journals. Backend contracts are inspected read-only in the local Gecko-API and Gecko-Web checkouts; only CLI-Tools is edited.

## Delivery steps

Each implementation step gets focused tests, formatting, strict Clippy, an independent reviewer pass, fixes for supported findings, and a separate local commit. Record review targets/findings in `docs/review-log.md`. Keep this table current.

| Step | Outcome | Status |
| --- | --- | --- |
| 0 | Save and review this plan | Complete |
| 1 | Server-side search, field/label/date filters, saved filter selection, sorting, JSON output | Complete |
| 2 | Field catalog, configurable export columns, CSV and explicit all-page export | Complete |
| 3 | Targeted bulk label and consent changes with preview and resumable progress | Complete |
| 4 | Add selected contacts to existing organisations/events | Complete |
| 5 | Verify a recorded scenario against its expected values/resources | Complete |
| 6 | Preview and clean up resources recorded as created by a scenario run | In progress |

### 1. Search, filters, and JSON

Keep `contacts` backward compatible: no new flags opens the existing TUI; `--plain` still prints a table. Add a query argument group shared by the initial request and every subsequent TUI page. Search and conditions execute on Gecko, not on the currently loaded page. Support a useful validated subset of field comparisons (including missing values), label IDs, creation dates, saved filter IDs, and deterministic sorting. Reuse the backend's documented-in-source condition shapes; reject unsupported conditions before requests. Provide discovery of saved filters so users need not guess their IDs. JSON output is noninteractive and contains masked, structured contacts plus pagination; diagnostics stay on stderr.

Verify request composition, combined criteria, unchanged filtering while paging, invalid arguments, and JSON privacy/parseability. Confirm read-only staging query behaviour when a valid session is available.

### 2. Field catalog and export

Expose `fields list` with field ID, label, type, required/sensitive flags and available choices when supplied by Gecko. Allow explicit column selection for plain/JSON/CSV export, including custom fields by ID. Fetch values from their actual field IDs and preserve typed JSON values; do not infer custom fields from display positions. Sensitive values remain masked in every output format.

Exports default to the requested page. `--all` explicitly walks all pages with stable ordering and repeated-page protection. Stream or bound work where practical; stop with a nonzero exit on an incomplete export. Do not silently claim a partial export is complete. CSV escapes quotes, separators, and multiline values. Output-file writes create new private files and avoid publishing partial successful-looking files.

Verify custom field mappings, null/typed values, privacy, CSV escaping, capped pagination, repeated pages, and errors after a successful page.

### 3. Labels and consents

Add discovery of label and consent IDs, and commands that act on explicit contact IDs or the contacts recorded in an existing scenario journal. Default to a preview; an explicit execution flag submits writes. Do not support implicit whole-account updates. Check target account/profile and reject missing or foreign resources before writes. Preserve unrelated labels and consent entries according to Gecko's actual additive/update contracts.

Use a private journal bound to API target, selected profile, operation and input IDs. Hold an exclusive lock for the full preview/execution/journal lifecycle; reject another process using the same journal. Record each write before sending, checkpoint confirmed success, and refuse to retry an uncertain result automatically. Report completed/pending counts without leaking contact data. A completed rerun makes no writes.

Contract clarification: ordinary contact updates replace label/consent collections. Use one-contact `/contacts/mass_action` requests with an explicit `conditions.contact_ids` array and only a label-add or consent add/remove action. Label removal is outside this increment: Gecko's permission mapping rejects that mass action and full-list replacement cannot preserve concurrent changes. The maintained backend processes this size synchronously under its default threshold; a returned queue ID remains uncertain until reconciled. Read back the desired state before checkpointing.

Verify exact payloads and target IDs, preservation of unrelated state, zero writes for previews/invalid targets, and interruption/resume behaviour. Mutation verification uses local mock APIs; shared staging records are not changed as part of development checks.

### 4. Organisations and events

Reuse the batch targeting/preview/journal flow to add contacts to an existing organisation or event, including explicit attendance status where supported. Read back and report the actual attendance status (for example, waitlisted instead of registered); never checkpoint the requested status as achieved without verification. Read existing memberships to avoid duplicate creation. Validate destination IDs and API contracts before submitting. Accept a scenario journal as the contact source so generated scenarios can be linked to existing development resources without changing fixture schema v1.

Verify destination/participant payloads, existing-membership handling, per-contact failures, preview, and journal recovery. Do not create or delete shared organisations/events.

### 5. Scenario verification

`scenario verify FIXTURE --profile-id ...` reads the matching apply journal and compares recorded contacts and created resources to the fixture. Confirm target and fingerprint, report missing resources, incomplete/uncertain operations, mismatched values and expected duplicate/missing emails; return a nonzero exit on failure. Resolve standard/custom field IDs using the same contracts as apply, without making writes. Mask sensitive mismatch details and treat values that cannot be verified under current permissions as unverified rather than passed.

Verify exact success, missing contact, changed field, duplicate/missing email cases, wrong fixture/target, incomplete journal and restricted visibility.

### 6. Scenario cleanup

`scenario cleanup FIXTURE --profile-id ...` previews the exact recorded created IDs. Require an explicit execution flag to remove them. Never find cleanup targets by broad labels, names or email domains. Revalidate journal target/fingerprint and current ownership evidence before deletion; stop on pending/ambiguous apply outcomes. Delete generated contacts before generated groups/custom fields, preserving existing standard fields and shared destinations. If safe ownership/use cannot be established for a shared resource, keep it and report the reason.

Before checking apply state, acquire the same exclusive apply lock and retain it through cleanup reads, deletes, deletion checkpoints and lifecycle updates. This excludes apply, cleanup and other scenario lifecycle operations on that journal from running concurrently. Track deletion progress separately from apply checkpoints; preserve the original creation evidence. Treat already-missing resources as complete after an authenticated read. Record uncertain delete outcomes and require reconciliation before continuing. Prevent apply from recreating a cleaned-up run accidentally.

Verify preview has zero writes, only recorded IDs are deleted in dependency order, wrong target/edited fixture is rejected, partial deletion resumes safely, missing resources are handled, cleanup cannot delete unrelated or reused resources, and a concurrent apply/cleanup is rejected before any API mutation.

## Shared decisions and constraints

- Keep one authenticated app API request path where reuse becomes necessary, retaining `/auth/check`, public-to-numeric identity mapping, header checks, timeouts and no cross-origin redirects.
- Keep existing scenario fixture v1 readable and deterministic. Reuse journal provenance; any journal extension must tolerate existing writer-produced records without weakening target checks.
- IDs used in paths are validated; user-provided conditions are parsed/encoded, not concatenated into query syntax without validation.
- Preserve CLI exit codes for scripting and keep machine output free of spinners/status prose.
- New mutations are opt-in after a concrete preview; this is product behaviour, not a request to pause implementation for approval.
- Use local mock HTTP tests for mutation contracts and meaningful failure cases. Run existing required Rust checks for each step; do not duplicate broad checks after a clean pass without changed code or a new concern.
- Independent review is requested for every implementation step. Pause edits to a mutable review target; investigate the next step read-only while review runs.
- API details for later steps must be confirmed from maintained local source before implementation. If an endpoint cannot express a promised operation safely, record evidence and revise only the affected step, preserving the rest of the work.

## Completion record

Step 0: local commit `4d70704`. Step 1: 74 tests, formatting and strict Clippy pass; all reviewer findings resolved. Staging smoke check stopped at expired-token HTTP 401 before querying contacts. Boolean and zero/empty equality are explicitly rejected to avoid Gecko's unsupported comparison semantics.

Step 1 commit: `369513d`. Step 2: 80 tests, formatting and strict Clippy pass; EXPORT-01 resolved. Exports buffer at most the explicit row limit (100,000 by default), so an oversized export fails instead of silently truncating.

Step 2 commit: `e5fa2fa`. Step 3: 89 tests, formatting and strict Clippy pass; BATCH-01 resolved by omitting an unsupported extra label-removal command. Label addition and consent grant/revoke are complete.

Step 3 commit: `eb466f2`. Step 4: 94 tests, formatting and strict Clippy pass; MEMBER-01 resolved. Session containers require selection of a concrete session-time ID; actual attendance outcomes are recorded.

Step 4 commit: `118f42f`. Step 5: 100 tests, formatting and strict Clippy pass; independent review found no supported defects.
