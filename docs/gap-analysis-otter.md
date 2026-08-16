# Gap Analysis — Scribe vs Otter.ai

_Generated 2026-06-15. Compares Scribe's current feature set against Otter.ai (mid-2026, "AI Meeting Intelligence Platform")._

## Framing

Scribe and Otter are **structurally different products**, and that shapes what "parity" means:

| | Scribe | Otter.ai |
|---|---|---|
| Hosting | Self-hosted, single box (+ optional split) | Cloud SaaS |
| Audience | Single user / household | Teams & enterprise |
| Privacy | All data on your hardware; no external calls except your own LLM | Cloud-dependent; reported directory auto-share & bot-join friction |
| Capture | Phone records in person | Meeting bot auto-joins Zoom/Meet/Teams + mobile |
| Monetization | None (you own it) | Per-seat + minute quotas |

So a large slice of Otter's surface area (team workspaces, channels, CRM sync, enterprise agents, per-seat admin) is **deliberately out of scope** — copying it would work against Scribe's reason to exist. The gap analysis below separates *applicable* gaps from *N/A by design*.

**Legend:** ✅ at parity (or better) · 🟡 partial · ❌ missing & applicable · 🚫 out of scope by design

---

## Feature-by-feature

### Capture & recording
| Otter feature | Scribe | Notes |
|---|---|---|
| Live / real-time transcription | ✅ | Scribe streams 15 s segments → provisional transcript during recording |
| Mobile in-person recording | ✅ | Expo app, segmented recorder + resilient upload queue |
| Import existing audio/video file | ❌ | Scribe only records in-app; no "transcribe this file" path |
| Meeting bot auto-join (Zoom/Meet/Teams) | 🚫→stretch | Requires always-on cloud infra; counter to the self-hosted model, but a possible differentiator later |
| Calendar-driven auto-record | ❌ | No calendar integration |
| Chrome extension (capture web meetings) | 🚫 | Browser-meeting capture; out of scope for a phone-first personal tool |
| Slide / screenshot capture | ❌ | No visual capture during meetings |

### Transcription
| Otter feature | Scribe | Notes |
|---|---|---|
| Word-level timestamps | ✅ | Every word carries start/end/conf |
| Speaker diarization | ✅ | VAD → segmentation → embeddings → clustering |
| Speaker ID across meetings | ✅ | 192-dim enrollment. A cosine similarity of 0.5 or more identifies the speaker. The mobile app has a speaker library. You can tag a speaker with a known name, rename that name, or remove it |
| Custom vocabulary | 🟡 | Hotwords file exists but transducer-only + no UI; Otter has in-app term learning |
| Languages | 🟡 | Models are multilingual (Parakeet 100+/Whisper), but no per-recording language UI or auto-detect surfaced. (Otter only supports 6 languages — Scribe's models can actually do *more*, it's just not exposed) |
| Translation | ❌ | No translate-to-target |

### AI / intelligence
| Otter feature | Scribe | Notes |
|---|---|---|
| Auto summary / outline | ✅ | LLM summary: title, summary, action_items, decisions, topics |
| Action-item extraction | ✅ | Extracted into `summaries.action_items` |
| Action-item *assignment* | 🚫 | Multi-user concept; N/A for single user |
| Q&A chat over meetings (Otter AI Chat) | ✅ | `POST /ask` — RAG with inline citations; **Scribe cites sources, a plus** |
| Cross-meeting knowledge base | ✅ | Search + Ask already span all recordings |
| Role-based / templated summaries | ❌ | Single fixed summary format; see Plaud analysis (templates is the bigger gap there) |
| Topic / keyword detection | 🟡 | `topics` in summary, but no keyword index / word-cloud |
| Auto follow-up email generation | ❌ | No generated outbound content |
| Filler removal | ✅+ | Scribe strips uh/um by default — Otter does **not** advertise this |
| Voice agents (Meeting/Sales/SDR) | 🚫 | Enterprise sales tooling; out of scope |

### Playback & review
| Otter feature | Scribe | Notes |
|---|---|---|
| Synchronized transcript highlighting | ✅ | The mobile app highlights each spoken word and keeps that transcript line in view. Tap to seek |
| Variable playback speed | ❌ | No speed control yet |
| Skip silence | ❌ | Not implemented |
| Inline transcript editing | ❌ | Can name speakers; **cannot edit transcript text** |
| Highlights | 🟡 | "Mark" button captures moments while recording, but marks aren't surfaced/reviewable afterward |
| Comments / annotations | 🚫 | Collaboration feature; N/A single user |

### Search & organization
| Otter feature | Scribe | Notes |
|---|---|---|
| Global search across conversations | ✅ | Hybrid keyword + semantic (RRF fusion) — **arguably better than Otter** |
| Folders | ❌ | No organizational hierarchy |
| Tags | ❌ | No tagging |
| Speaker talk-time analytics | ❌ | Data is present (utterance timings + speaker) but not surfaced |

### Collaboration, integrations, platforms
| Otter feature | Scribe | Notes |
|---|---|---|
| Shared notes / channels / permissions | 🚫 | Team features; out of scope |
| CRM (Salesforce/HubSpot), Slack, Notion | 🚫 | Mostly team/SaaS; a Notion/markdown *export* is the in-scope subset |
| Public API / webhooks | 🟡 | Has a typed HTTP API already; no webhooks |
| **MCP server** (expose meetings to Claude) | ❌ | Otter shipped this in 2025; **highly relevant** to Scribe given the LLM-native design |
| Export (txt/docx/pdf/srt/mp3) | ❌ | **Scribe has no export at all** — biggest functional gap |
| Bulk export | ❌ | — |
| Web app | ❌ | Mobile-only; no browser client |
| Desktop app | ❌ | — |

---

## What Scribe already wins on
- **Privacy / self-hosting** — no cloud dependency, no minute quotas, no directory auto-share.
- **Semantic + keyword hybrid search** with citations.
- **Filler removal** out of the box.
- **Live transcription** at parity with Otter (and ahead of Plaud).
- **Multilingual-capable ASR** (the models can do more languages than Otter's 6 — it just needs surfacing).

## Path to parity (applicable gaps, prioritized)

1. **Export** (md/txt/srt now; docx/pdf + bulk later) — closes the single most glaring gap. _Quick win._
2. **Playback speed control** + **surface "Marks" as reviewable highlights** — small, high-perceived-value. _Quick win._
3. **Inline transcript editing** — table stakes for a notes app.
4. **Folders + tags** — organization at scale.
5. **Import existing audio file** — reuse the upload + pipeline path.
6. **Templated / role-based summaries** — see the Plaud roadmap (shared item).
7. **Speaker talk-time analytics** — data already exists; presentation only.
8. **MCP server** — strategic; turns Scribe into a Claude-accessible personal knowledge base.
9. **Web client** — read/search/ask in a browser.
10. _Stretch / likely-never:_ meeting bot auto-join, calendar capture, slide capture.

See [`parity-roadmap.md`](parity-roadmap.md) for the merged, phased plan across both competitors.
