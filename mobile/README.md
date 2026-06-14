# Scribe Mobile

React Native (Expo) capture client for the self-hosted Scribe meeting
recorder / transcription system.  Records meetings in segmented AAC/m4a,
uploads each segment as it closes, and displays speaker-labelled transcripts,
LLM summaries, and semantic search results — all served from your own hardware.

---

## Prerequisites

### Device / OS
- **iOS 16+** or **Android 12+**
- **Tailscale** app installed and signed into the same tailnet as your Scribe
  server.  The app talks to the server over the tailnet; the Tailscale app
  handles the VPN — no extra configuration needed in Scribe itself.

### Development workstation
- **Node 22** and **npm**
- **Expo CLI**: `npm install -g expo-cli`  (or use `npx expo`)
- **Expo dev client** on your test device (see below) — the app uses native
  modules (`expo-audio`, `expo-secure-store`) that are not available in Expo
  Go.

### Building the Expo dev client
```bash
# iOS (requires a Mac with Xcode 15+)
npx expo run:ios --device

# Android (requires Android SDK / Android Studio)
npx expo run:android --device
```

Once the dev client is installed on the device you can start the metro server
and reload over the air without rebuilding:

```bash
cd mobile
npm install
npx expo start --dev-client
```

---

## Pointing the app at your server

Open the **Settings** tab in the app and fill in:

| Field | Example |
|---|---|
| Base URL | `https://scribe.mango-slug.ts.net` |
| Device API Key | `scribe_sk_abc123…` (issued by `scribe serve`) |

The base URL is the Tailscale MagicDNS HTTPS address of your storage node.
Enable MagicDNS in the Tailscale admin console.  The server's TLS certificate
is auto-provisioned by `tailscale cert` or `tailscale serve` — the device
trusts it out of the box (Let's Encrypt; no custom CA to install).

Tap **Test connection** to verify the device can reach the server.

---

## iOS background-recording caveats

From design §11 / §16:

- **`UIBackgroundModes: ["audio"]`** is declared in `app.json` → recording
  continues when the app is backgrounded or the screen is locked.
- **You must start a recording while the app is in the foreground.**  The OS
  will not grant microphone access to a backgrounded app that wasn't already
  recording.
- **Phone calls interrupt the microphone** (AVAudioSession interruption).  The
  segmented recorder detects this: the in-progress segment is closed and
  emitted, and recording resumes automatically when the call ends.
  At most one segment (≤ 30 s) may be lost if the interruption occurs exactly
  at the start of a new segment before the previous one finished closing.
- Siri activations pause the mic briefly in the same way as calls.

---

## Android background-recording caveats

From design §11 / §16:

- A **foreground service** with `foregroundServiceType="microphone"` keeps
  recording alive when the app is backgrounded (Android 14+ requirement).
  The `FOREGROUND_SERVICE_MICROPHONE` permission is declared in `app.json`.
- The OS shows a persistent **"Recording…" notification** for the lifetime of
  the foreground service — this is mandatory and cannot be suppressed.
- **Do not force-kill the app** while recording; the foreground service will
  be destroyed and the current segment will be lost.
- **Do not attempt to start recording from the background.**  Start the
  recording in the foreground, then background the app.
- If `expo-audio`'s Android background path proves unreliable (it is
  under-documented), the recording core can be swapped to
  `react-native-nitro-sound` (the maintained successor to the deprecated
  `react-native-audio-recorder-player`) with a hand-wired foreground service
  — the segmented recorder interface (`SegmentedRecorder`) is isolated in
  `src/recording/segmentedRecorder.ts` to make this swap straightforward.

---

## Upload protocol

**v1: direct PUT per segment.**  Each 30-second segment is uploaded as a raw
binary PUT to `PUT /recordings/{id}/segments/{seq}?ext=m4a`.

**Production upgrade path (recommended):** swap to tus resumable uploads
(`tus-js-client` v4 or `@cuvent/react-native-better-tus-client`) against a
`rustus` sidecar at `/files`.  The upload queue in
`src/recording/uploadQueue.ts` is protocol-agnostic; only the
`api.uploadSegment()` call in `src/api/client.ts` changes.  See design §11
"Upload" and the comments in those files.

---

## Project structure

```
mobile/
  app/                      expo-router file routes
    _layout.tsx             root layout (hydrates stores, restores queue)
    (tabs)/
      _layout.tsx           tab bar
      index.tsx             Screen 1: Record
      library.tsx           Screen 2: Library
      search.tsx            Screen 4: Search
      ask.tsx               Screen 5: Ask
      settings.tsx          Screen 6: Settings
    recordings/
      [id].tsx              Screen 3: Recording Detail
  src/
    types.ts                TypeScript types mirroring scribe_core::types + API contract
    api/
      client.ts             Typed API client (fetch + auth injection)
    recording/
      segmentedRecorder.ts  Fixed-segment AAC recorder (expo-audio)
      uploadQueue.ts        Offline-first segment upload queue with backoff
      recordingSession.ts   Top-level session orchestrator
    state/
      settingsStore.ts      Zustand store — persisted to SecureStore/AsyncStorage
      recordingsStore.ts    Zustand store — recordings list cache
  app.json                  Expo config (UIBackgroundModes, Android permissions, plugins)
  package.json
  tsconfig.json
```

---

## Running type checks

```bash
cd mobile
npm install
npx tsc --noEmit
```

---

## Phased build notes (design §15)

This is the **Phase 1** mobile client (capture → upload → store).  The
server-side pipeline (transcription, diarisation, LLM summary) is Phase 2–4
and is implemented in the Rust `scribe-worker` binary.  The UI screens for
transcript / summary / search / ask are wired up and will become fully
functional once the server implements those pipeline stages.
