# Native Swift rewrite — working plan

The React Native app is being rewritten as a native SwiftUI app here, in `ios/`.
The old app is the specification: it sits outside the repo at
`../scribe-mobile-rn-old` (also in git history at `91e1ab1`, path `mobile/`).
Read the old file named against each item before writing it.

Each loop pass: take the **first unchecked item**, port it, regenerate
(`xcodegen`), build for the device, fix every error, install on the phone, commit
the `ios/` changes by explicit path, and tick the item here with one line on what
was done. Keep a pass to one item, or part of one, that ends building.

```bash
cd ios && xcodegen && xcodebuild -project Scribe.xcodeproj -scheme Scribe \
  -configuration Debug -destination 'generic/platform=iOS' -derivedDataPath build/dd \
  -allowProvisioningUpdates build
xcrun devicectl device install app --device 12CD3A5C-BC75-5794-BA91-7C86430E1E90 \
  build/dd/Build/Products/Debug-iphoneos/Scribe.app
```

Conventions: SwiftUI, iOS 17, `@Observable` models, `async`/`await` with
`URLSession`, `Codable` types matching the server's JSON (snake_case), the device
key in the Keychain, colours from `Theme`. No third-party packages unless a
feature cannot be done without one.

## Core — "Swift is up" once all of these are ticked

- [x] **Skeleton**: XcodeGen project, theme, five tabs, builds and installs. Bundle
      id `com.dwmmholdings.scribenative` so it sits beside the old app.
- [ ] **Models + API client** — `src/types.ts`, `src/api/client.ts`: Codable types,
      one `APIClient` (bearer auth, `ApiError` with status, `/health`, every route
      the old client calls). Settings store (`src/state/settingsStore.ts`):
      server URL + device id in UserDefaults, device key in Keychain.
- [ ] **Settings + pairing** — `app/(tabs)/settings.tsx`, `src/state/pairing.ts`,
      `modules/scribe-discovery`: URL/key fields, Test connection (health, then an
      authenticated call), Find server (NWBrowser on `_scribe._tcp`, TXT `url`,
      `auth`), `scribe://pair?url=&key=` deep link, audio quality, default
      participants.
- [ ] **Library** — `app/(tabs)/library.tsx`, `src/state/recordingsStore.ts`:
      list with status, duration, tags, pull to refresh, tag filter, delete.
- [ ] **Recording detail** — `app/recordings/[id].tsx` (the big one; split it):
      summary + action items/decisions/topics, transcript by speaker with
      colours, pipeline progress while processing, summary templates, reprocess,
      rediarize, participants.
- [ ] **Playback** — `src/playback/karaoke.ts`, `src/components/PlaybackWave.tsx`:
      stream `/recordings/{id}/audio` with the bearer header, play/pause/seek,
      rate, word highlight, tap a word to seek, marks.
- [ ] **Recording** — `src/recording/segmentedRecorder.ts`, `recordingSession.ts`,
      `modules/scribe-audio-session`, `modules/scribe-bg-timer`, `app/(tabs)/index.tsx`:
      AVAudioSession record + background audio, AAC segments rotated without
      re-activating the session, marks, pause/resume, level meter, interruptions.
- [ ] **Upload queue** — `src/recording/uploadQueue.ts`: create recording, PUT each
      segment as it closes, retry with backoff, survive relaunch (persist the
      queue), complete with duration + marks.
- [ ] **Speakers** — `src/components/SpeakerTagSheet.tsx`, `app/speakers.tsx`: tag a
      speaker by name or enrolled voice, re-learn a voice, untag, not a
      participant; the enrolled library with rename and delete.
- [ ] **Search + Ask** — `app/(tabs)/search.tsx`, `app/(tabs)/ask.tsx`: hybrid search
      with snippets and seek-to; Ask with history and citations.
- [ ] **Export** — `src/util/export.ts`: Markdown, text, SRT via the share sheet; full
      audio (`/recordings/{id}/audio` to a file, then share).

When every core item is ticked, the app builds, and it is installed on the phone:
delete `../scribe-mobile-rn-old`, commit the removal of `mobile/` from the repo
(and the `mobile/` lines in `.gitignore`), update README/docs that point at
`mobile/`, and say so. Do not uninstall the old app from the phone — it may hold
recordings that never uploaded; that is the owner's call.

## After core

- [ ] Live Activity + lock-screen recording controls — `targets/live-activity`,
      `modules/scribe-live-activity`.
- [ ] Import audio (document picker) — `app/(tabs)/index.tsx`.
- [ ] Translate, mind map — `src/components/Translate.tsx`, `MindMap.tsx`.
- [ ] Admin: logs, processing schedule, self-update — `app/admin/*`.
- [ ] Edit an utterance's text; tags editor.
- [ ] Polish pass (design skill): empty states, loading, errors, haptics.
