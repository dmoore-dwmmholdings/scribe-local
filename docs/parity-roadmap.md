# Scribe — Feature-Parity Roadmap

_Merged, prioritized plan from [`gap-analysis-otter.md`](gap-analysis-otter.md) and [`gap-analysis-plaud.md`](gap-analysis-plaud.md). Generated 2026-06-15._

The two competitors overlap heavily once you strip out what doesn't apply to a self-hosted, single-user, software-only tool. This roadmap sequences the **applicable** gaps by value-to-effort, and records what's intentionally excluded.

## Guiding principle

Don't chase SaaS/enterprise/hardware surface area. Scribe's edge is **privacy, self-hosting, live transcription, and an LLM-native design**. Parity work should make Scribe a best-in-class *personal* meeting brain, not a clone of a team product.

---

## Phase 0 — Quick wins ✅ COMPLETE (2026-06-16)

| # | Feature | Closes gap vs | Effort | Notes |
|---|---|---|---|---|
| 0.1 | **Export** (Markdown / plain text / SRT) | Otter + Plaud | S | ✅ Done — client-side from `RecordingDetailResponse`; OS share sheet |
| 0.2 | **Playback speed control** (1×/1.25×/1.5×/2×) | Otter | S | ✅ Done — speed button on the playback bar |
| 0.3 | **Surface "Marks" as reviewable highlights** | Otter + Plaud | S | ✅ Done — marks now sent on `/complete` → `recordings.marks int[]` (migration 0008) → detail HIGHLIGHTS row of tappable `mm:ss` jump chips |
| 0.4 | **Speaker talk-time analytics** | Otter | S | ✅ Done — talk-time breakdown card on the detail screen |

## Phase 1 — Core parity ✅ COMPLETE (2026-06-16)

| # | Feature | Closes gap vs | Effort | Notes |
|---|---|---|---|---|
| 1.1 | **Summary templates** (built-in set) | Plaud (flagship) + Otter | M | ✅ Done — registry (general/standup/interview/1:1/lecture/sales), `POST /recordings/{id}/summarize`, `GET /summary-templates`, `summaries.template` migration, `[llm].summary_template` config, mobile picker + re-summarize. **Needs backend rebuild + redeploy.** Custom user-defined templates still open (1.2 multidimensional builds on this) |
| 1.2 | **Multidimensional summaries** (multiple role views) | Plaud | M | ✅ Done — `summaries` keyed by `(recording_id, template)` (migration 0007), detail returns `summaries[]`; mobile keeps every generated view, switch instantly via the template chip, generate more on demand |
| 1.3 | **Re-transcribe / reprocess** action | Plaud | M | ✅ Done — re-summarize with a template was 1.1; full reprocess via `POST /recordings/{id}/reprocess` (clears derived data incl. stale jobs, re-enqueues the pipeline) + detail-screen ⋯ → "Reprocess transcript". Recovers recordings processed before fixes |
| 1.4 | **Inline transcript editing** | Otter + Plaud | M | ✅ Done — long-press a line → Edit text; `PATCH /recordings/{id}/utterances/{utterance_id}` updates text (keyword `tsv` is a generated column so it reindexes automatically). Semantic chunk embeddings stay until a reprocess (follow-up) |
| 1.5 | **Import existing audio file** | Otter | M | ✅ Done — Library "+" / empty-state link → `expo-document-picker` → create + stream-upload (seq 0) + complete → existing pipeline. Mobile-only, reuses existing endpoints (segment route has body limit disabled). Full transcription of long imports needs the Whisper-chunking backend fix deployed |
| 1.6 | **Tags** (organization) | Otter + Plaud | M | ✅ Done — `recordings.tags text[]` (migration 0006), `PUT /recordings/{id}/tags`, `GET /tags`, `?tag=` list filter; detail-screen tag editor + Library filter chips. (Folders deferred — tags subsume the need for now.) |

## Phase 2 — Depth & polish (2–4 weeks)

| # | Feature | Closes gap vs | Effort | Notes |
|---|---|---|---|---|
| 2.1 | **Mind-map generation** | Plaud | M | ✅ Done (v1) — client-side tree from the active summary (root title → Overview/Topics/Action items/Decisions branches → leaves), rendered as a colored connector tree with Markdown share. Detail ⋯ → "Mind map". Mobile-only, no backend. Follow-up: richer LLM-generated tree |
| 2.2 | **Language auto-detect + per-recording selection** | Otter + Plaud | M | Surface model capability; store language on recording |
| 2.3 | **Translation** | (neither has it — leapfrog) | M | ✅ Done — `POST /recordings/{id}/translate` runs the summary through the LLM into a target language (reuses `/ask`'s LLM path; 503 if unreachable; no storage). Detail ⋯ → "Translate summary" → language picker → result + share |
| 2.4 | **Server-side export + DOCX/PDF + bulk export** | Otter + Plaud | M | `GET /recordings/{id}/export?format=`; bulk archive |
| 2.5 | **Custom vocabulary UI** + **per-summary LLM choice** | Plaud | S/M | Wrap hotwords; expose model picker |

## Phase 3 — Strategic & platform (project-sized)

| # | Feature | Closes gap vs | Effort | Notes |
|---|---|---|---|---|
| 3.1 | **MCP server** (expose Scribe to Claude/other models) | Otter | L | Turns Scribe into a Claude-accessible personal knowledge base — strongly on-brand |
| 3.2 | **AutoFlow-style automation** (on completion: auto-export / email summary) | Plaud | M | Post-pipeline delivery hooks |
| 3.3 | **Web client** (read / search / ask in browser) | Otter + Plaud | L | Reuses the existing HTTP API |
| 3.4 | **Desktop client / capture** | Plaud | L | — |

_Phase 2 status: 2.1 ✅ (mind maps), 2.3 ✅ (translation). Remaining items carry friction: 2.2 language selection is constrained by sherpa-Whisper fixing its language at recognizer-creation (not per-recording); 2.4 DOCX/PDF + bulk export hits mobile binary-file delivery (needs expo-sharing/expo-print → dev-client rebuild); 2.5 custom vocab is a no-op on Whisper (hotwords are transducer-only) and per-summary model choice has little value on a single loaded LM Studio model. Picking up the deferred **0.3 Marks → highlights** next (clean, no ASR/native-module friction)._

## Stretch / reconsider later
- **Meeting-bot auto-join** (Zoom/Meet/Teams) + **calendar capture** — large always-on infra; cuts against the self-hosted model. Only if the use case demands virtual-meeting capture.
- **Slide / screenshot capture.**

## Out of scope by design
- Team workspaces, channels, shared notes, granular permissions (single-user tool).
- CRM (Salesforce/HubSpot), enterprise admin, SSO, per-seat billing.
- Enterprise voice agents (Otter Meeting/Sales/SDR agents).
- All **hardware** capabilities (acoustic phone-call recording, wearable, one-press buttons, on-device storage) — not replicable in a phone-only app.
- Minutes/quota billing model — Scribe is unmetered by design.

---

## Status log
- **2026-06-15** — Roadmap created. Starting Phase 0.1 (Export).
- **2026-06-15** — Shipped Phase 0.1 (export: md/text/srt), 0.2 (playback speed), 0.4 (talk-time analytics) — all mobile-only, no redeploy. Deferred 0.3 (marks→highlights) pending backend mark persistence.
- **2026-06-15** — Completed Phase 1.1 (summary templates) end-to-end: backend (registry + 2 routes + migration 0005 + config) verified compiling (`cargo check --no-default-features` clean; pipeline/core/db/api tests pass), mobile picker + re-summarize wired to the contract, mobile typechecks clean. Requires a backend rebuild + redeploy to activate.
- **2026-06-16** — Completed Phase 1.5 (import existing audio file): mobile-only (`importAudioSegment` streaming upload + Library import entry points), reuses existing create/upload/complete endpoints, typechecks clean, no native rebuild (expo-document-picker already linked). Live via Fast Refresh. Short imports work on the current backend; long imports transcribe fully once the Whisper-chunking ASR fix is deployed.
- **2026-06-16** — Completed Phase 1.4 (transcript editing) + 1.6 (tags). Backend (migration 0006 tags column; `PUT /recordings/{id}/tags`, `GET /tags`, `?tag=` filter, `PATCH .../utterances/{id}`; `tsv` is generated so edits auto-reindex keyword search) verified with `cargo check --tests --no-default-features` (clean). Mobile (Library filter chips + detail tag editor + long-press → Edit text / Rename speaker) typechecks clean. Also fixed `launch-all.ps1` to load the MSVC dev environment (vswhere + Enter-VsDevShell + RUSTUP_TOOLCHAIN) before the real build, so the one-command rebuild works from a plain PowerShell.
- **2026-06-16** — Started Phase 2: shipped 2.1 mind maps (v1, mobile-only — client-side tree from the summary + Markdown share; typechecks clean). No backend/redeploy needed.
- **2026-06-16** — Shipped 2.3 translation (`POST /recordings/{id}/translate`, LLM via the `/ask` path; mobile language picker + result) and closed the deferred 0.3 marks→highlights (migration 0008 `marks int[]`, sent on `/complete`, detail HIGHLIGHTS chips). Both verified `cargo check --tests` clean + mobile typechecks clean. **Phase 0 COMPLETE; Phase 1 COMPLETE; Phase 2: 2.1 + 2.3 done.** Cleanly-scoped backlog is now exhausted — remaining items (2.2 language, 2.4 DOCX/PDF+bulk, 2.5 custom-vocab) carry ASR-architecture / native-module / no-op-on-Whisper friction. Large unverified backend batch (migrations 0005–0008) awaits one `.\scripts\launch-all.ps1`.
- **2026-08-15** — During playback the mobile app highlights each spoken word and keeps that transcript line in view (`mobile/src/playback/karaoke.ts`). The API adds `GET /speakers`, `PATCH /speakers/{id}`, `DELETE /speakers/{id}`, and `DELETE /recordings/{id}/speakers/{local_idx}/name`. `POST .../name` also accepts a `speaker_id`, and it records the voiceprint on a speaker that does not have one. Thus a name that you give in one recording applies to the recordings that you upload subsequently. The mobile app has a tag sheet with the known speakers, and a Speakers screen in `Settings`. `cargo check`, the API test, and `tsc --noEmit` show no errors.
- **2026-06-16** — Completed Phase 1.2 (multidimensional summaries) + 1.3 (reprocess) → **Phase 1 COMPLETE**. Backend (migration 0007 repoints `summaries` PK to `(recording_id, template)`; detail → `summaries[]`; `POST /recordings/{id}/reprocess` + `reset_for_reprocess` clearing derived data incl. stale jobs) verified `cargo check --tests --no-default-features` clean across all crates. Mobile (per-template view switcher on the summary card + ⋯ → reprocess) typechecks clean. **All staged backend work (templates, Whisper transcript fix, tags, editing, multi-summary, reprocess + migrations 0005–0007) deploys with one `.\scripts\launch-all.ps1`.**
