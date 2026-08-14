# Gap Analysis — Scribe vs Plaud

_Generated 2026-06-15. Compares Scribe's current feature set against Plaud (Plaud Note / Note Pro / NotePin hardware + the Plaud app, Web, and Desktop)._

## Framing

Plaud is a **hardware + software** product. The hardware (card recorder, wearable, acoustic phone-call recording, one-press buttons, 64 GB offline buffer) is its moat and is **not replicable in a phone-only software app** — mobile OSes block call-audio APIs, and there's no wearable. Those are marked 🚫.

Everything Plaud does *in software*, however, is squarely replicable — and that's where Plaud is genuinely ahead of Scribe today: **summary templates, mind maps, multidimensional summaries, multi-LLM choice, and automation (AutoFlow)**.

Two structural advantages Scribe already has over Plaud:
- **Live transcription** — Plaud is deliberately post-processing only. Scribe streams live.
- **Self-hosted, no minutes quota** — Plaud monetizes by metered transcription minutes; Scribe has none.

**Legend:** ✅ at parity (or better) · 🟡 partial · ❌ missing & applicable · 🚫 out of scope (hardware/by design)

---

## Feature-by-feature

### Hardware capture
| Plaud feature | Scribe | Notes |
|---|---|---|
| Card/wearable recorder, 64 GB, multi-day battery | 🚫 | Hardware; Scribe uses the phone |
| **Acoustic phone-call recording** | 🚫 | Device captures call audio acoustically; phone OS APIs block this in software |
| One-press / physical highlight button | 🚫 | No hardware; closest analog is the in-app "Mark" button |
| Always-on wearable capture | 🚫 | — |

### Transcription
| Plaud feature | Scribe | Notes |
|---|---|---|
| ASR (Whisper-class) | ✅ | Parakeet TDT default + Whisper option |
| Speaker diarization / labeling | ✅ | Plus cross-recording enrollment (Plaud relabels per-recording) |
| Word/segment timestamps + tap-to-seek | ✅ | — |
| Languages (Plaud: 112, auto-detect) | 🟡 | Scribe's models are multilingual but lack auto-detect + per-recording language UI |
| Custom vocabulary / industry glossaries | 🟡 | Hotwords file only; Plaud ships 10+ built-in glossaries + custom terms |
| Edit transcript | ❌ | Not supported |
| **Re-transcribe** (new language/settings) | ❌ | No re-run-from-audio action |
| Translation | ❌ | Plaud also lacks this — **neither has it** |
| Real-time transcription | ✅+ | **Scribe wins — Plaud has none** |

### AI / intelligence — _Plaud's strongest area_
| Plaud feature | Scribe | Notes |
|---|---|---|
| Auto summary | ✅ | title / summary / action_items / decisions / topics |
| **Summary Templates** (10,000+ presets) | ❌ | Plaud's flagship. Scribe has one hardcoded format. **Highest-value gap.** |
| **Custom summary templates** | ❌ | User-defined prompt/structure |
| **Multidimensional summary** (multiple role-specific summaries from one recording) | ❌ | — |
| **Mind-map generation** | ❌ | Visual map from summary; exportable |
| Ask AI over recording (+ global) | ✅ | `POST /ask`, hybrid retrieval + citations, spans all recordings |
| Auto title | ✅+ | Scribe continuously regenerates and respects manual edits |
| Action items / to-dos | ✅ | Extracted in summary |
| Selectable LLM (GPT/Claude/Gemini) | 🟡 | Provider/model is global config, not a per-summary in-app choice |
| Highlights / key quotes | 🟡 | "Mark" captured live but not surfaced as highlights |
| Multimodal input (attach text/images) | ❌ | Audio only |
| **AutoFlow** (auto transcribe→summarize→email/export) | ❌ | Pipeline runs automatically, but no delivery/automation step |

### Organization, sync, export
| Plaud feature | Scribe | Notes |
|---|---|---|
| Folders / tags | ❌ | None |
| Search | ✅ | Hybrid keyword + semantic |
| Audio-synced transcript + edit | 🟡 | Synced playback ✅; editing ❌ |
| Cloud sync across app/web/desktop | 🟡 | Server is the source of truth; mobile caches offline. No web/desktop client |
| Minutes quota model | ✅+ | N/A — Scribe has no quota (advantage) |
| Export (txt/srt/docx/pdf, audio mp3/wav, mind-map png/md) | ❌ | **No export** |
| Share links | ❌ | No shareable links |
| Platforms: iOS / Android / Web / Desktop | 🟡 | iOS + Android only |

---

## What Scribe already wins on
- **Live transcription** (Plaud has none).
- **No minutes quota / fully self-hosted** — Plaud's core monetization and its privacy tradeoff both disappear.
- **Cross-recording speaker enrollment** (Plaud relabels per recording).
- **Continuous auto-titling** that respects manual edits.

## Path to parity (applicable software gaps, prioritized)

1. **Summary templates** (built-in set + custom) + **multidimensional summaries** — Plaud's flagship; Scribe already has the LLM plumbing, this is mostly prompt + selection + a re-summarize action. _Highest value._
2. **Export** (md/txt/srt now; docx/pdf + mind-map export later) — _quick win, shared with Otter roadmap._
3. **Mind-map generation** — derive from summary topics/structure; render + export.
4. **Re-transcribe / re-summarize** action (pick template or language, re-run).
5. **Inline transcript editing** + **surface highlights** from "Mark".
6. **Folders + tags.**
7. **Language auto-detect + per-recording language selection**; then **translation**.
8. **Custom vocabulary UI** (wrap the existing hotwords mechanism) + **per-summary LLM choice**.
9. **AutoFlow-style automation** (on completion: auto-export / email the summary).
10. **Web / desktop client.**

See [`parity-roadmap.md`](parity-roadmap.md) for the merged, phased plan across both competitors.
