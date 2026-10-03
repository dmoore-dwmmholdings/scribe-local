import SwiftUI

struct RecordView: View {
    private var session = RecordingSession.shared
    private var settings = Settings.shared
    @State private var title = ""
    @State private var participants = Settings.shared.defaultParticipants
    @AppStorage("liveTranscript") private var live = true
    @State private var liveText = ""

    private var recording: Bool { session.state == .recording }
    private var liveMode: Bool { session.isActive && live }

    var body: some View {
        ZStack {
            Theme.bg.ignoresSafeArea()
            if session.isActive { EdgeGlow(level: recording ? session.level : 0.05, reduceMotion: settings.reduceMotion).transition(.opacity) }

            VStack(spacing: 0) {
                header
                if liveMode { liveLayout } else { immersiveLayout }
                controls.padding(.bottom, 18)
            }
        }
        .animation(.spring(duration: 0.5), value: liveMode)
        .animation(.easeInOut(duration: 0.4), value: session.isActive)
        .task(id: session.isActive && live ? (session.localId ?? "") : "") { await pollLive() }
    }

    // MARK: Layouts

    private var header: some View {
        HStack(spacing: 8) {
            statusLine
            Spacer()
            if !liveMode {
                Button { live.toggle() } label: {
                    Label("Live", systemImage: live ? "text.bubble.fill" : "text.bubble")
                        .font(.mono(11, weight: .bold))
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(live ? Theme.accent.opacity(0.15) : Color.white.opacity(0.05), in: Capsule())
                        .foregroundStyle(live ? Theme.accent : Theme.textMuted)
                }
                .accessibilityLabel(live ? "Live transcript on" : "Live transcript off")
            }
        }
        .padding(.horizontal, 22).padding(.top, 10)
    }

    private var immersiveLayout: some View {
        VStack(spacing: 22) {
            Spacer(minLength: 10)
            EmberOrb(level: recording ? session.level : 0, active: recording, reduceMotion: settings.reduceMotion)
                .frame(width: 280, height: 280)
            Text(formatClock(ms: session.elapsedMs))
                .font(.system(size: 54, weight: .light, design: .rounded).monospacedDigit())
                .foregroundStyle(session.state == .paused ? Theme.amber : Theme.textPrimary)
                .contentTransition(.numericText())
                .accessibilityLabel(formatClock(ms: session.elapsedMs))
            if !session.isActive { setupCard.padding(.horizontal, 22) }
            messages.padding(.horizontal, 28)
            Spacer(minLength: 10)
        }
    }

    private var liveLayout: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 14) {
                EmberOrb(level: recording ? session.level : 0, active: recording, reduceMotion: settings.reduceMotion)
                    .frame(width: 76, height: 76)
                    .padding(-9)
                Text(formatClock(ms: session.elapsedMs))
                    .font(.system(size: 34, weight: .light, design: .rounded).monospacedDigit())
                    .foregroundStyle(session.state == .paused ? Theme.amber : Theme.textPrimary)
                    .contentTransition(.numericText())
                Spacer()
                Button { live = false } label: {
                    Text("● LIVE").font(.mono(11, weight: .bold)).foregroundStyle(Theme.accent)
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .background(Theme.accentSoft, in: Capsule())
                }
                .accessibilityLabel("Turn off live transcript")
            }
            .padding(.horizontal, 22).padding(.top, 14)

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading) {
                        if liveText.isEmpty {
                            Text("Listening… The words appear here a little behind you, as each half-minute uploads and transcribes.")
                                .font(.callout).foregroundStyle(Theme.textDim)
                        } else {
                            liveParagraph
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                    .padding(.horizontal, 22)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: liveText) { _, _ in withAnimation { proxy.scrollTo("end", anchor: .bottom) } }
            }
            messages.padding(.horizontal, 22)
        }
    }

    /// Older words dim, the newest bright, as one flowing paragraph.
    private var liveParagraph: some View {
        let words = liveText.split(separator: " ")
        let split = max(0, words.count - 24)
        let head = words[..<split].joined(separator: " ")
        let tail = words[split...].joined(separator: " ")
        return (Text(head.isEmpty ? "" : head + " ").foregroundColor(Theme.textDim)
                + Text(tail).foregroundColor(Theme.textPrimary))
            .font(.system(size: 21, weight: .regular))
            .lineSpacing(4)
    }

    private var statusLine: some View {
        HStack(spacing: 8) {
            switch session.state {
            case .recording:
                Circle().fill(Theme.accentDeep).frame(width: 7, height: 7)
                Text("REC · 16 kHz AAC").foregroundStyle(Theme.accent)
            case .paused:
                Text("❚❚ PAUSED").foregroundStyle(Theme.amber)
            case .interrupted:
                Text("INTERRUPTED · RESUMES AFTER").foregroundStyle(Theme.amber)
            case .finished:
                Text("✓ SAVED · UPLOADING").foregroundStyle(Theme.ready)
            case .idle:
                Text(settings.isConfigured ? "READY · 16 kHz AAC" : "READY · NO SERVER YET").foregroundStyle(Theme.textMuted)
            }
            if !session.marks.isEmpty { Text("⚑ \(session.marks.count)").foregroundStyle(Theme.amber) }
        }
        .font(.mono(11, weight: .bold))
        .tracking(1)
    }

    private var setupCard: some View {
        Card(padding: 14) {
            VStack(spacing: 12) {
                TextField("", text: $title, prompt: Text("Title (optional)").foregroundColor(Theme.textDim))
                    .foregroundStyle(Theme.textPrimary)
                Hairline()
                HStack {
                    Text("\(participants) \(participants == 1 ? "person" : "people") speaking")
                        .foregroundStyle(Theme.textPrimary)
                    Spacer()
                    Stepper("", value: $participants, in: 1...20).labelsHidden()
                }
            }
        }
    }

    @ViewBuilder private var messages: some View {
        if let err = session.error {
            Text(err).font(.footnote).foregroundStyle(Theme.amber).multilineTextAlignment(.center)
        } else if case .finished = session.state {
            Text("Saved. It uploads in the background and appears in the Library once processed.")
                .font(.footnote).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
        } else if !settings.isConfigured && !session.isActive {
            Text("No server yet — you can still record; it waits on the phone until there is one.")
                .font(.footnote).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
        }
    }

    // MARK: Controls

    @ViewBuilder private var controls: some View {
        if session.isActive {
            HStack(spacing: 34) {
                control("flag.fill", "Mark", tint: Theme.amber) { session.mark() }
                if recording {
                    control("pause.fill", "Pause", tint: Theme.textPrimary) { session.pause() }
                } else {
                    control("play.fill", "Resume", tint: Theme.accent) { session.resume() }
                }
                control("stop.fill", "Stop", tint: Theme.accentDeep, big: true) {
                    session.stop()
                    title = ""
                }
            }
        } else {
            Button {
                if case .finished = session.state { session.reset() }
                liveText = ""
                Task { await session.start(title: title, participants: participants) }
            } label: {
                ZStack {
                    Circle().fill(Theme.accentGradient).frame(width: 84, height: 84)
                        .shadow(color: Theme.accent.opacity(0.45), radius: 20)
                    Image(systemName: "mic.fill").font(.system(size: 30, weight: .semibold)).foregroundStyle(.white)
                }
            }
            .accessibilityLabel("Start recording")
        }
    }

    private func control(_ icon: String, _ label: String, tint: Color, big: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 6) {
                Image(systemName: icon).font(.system(size: big ? 26 : 20)).foregroundStyle(tint)
                    .frame(width: big ? 72 : 58, height: big ? 72 : 58)
                    .glassBackground(cornerRadius: big ? 36 : 29)
                Text(label).font(.caption).foregroundStyle(Theme.textMuted)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: Live transcript

    /// Poll the server for the transcript so far while recording. Segments are
    /// transcribed as they upload, so text trails the speaker by a segment.
    private func pollLive() async {
        guard session.isActive, live else { return }
        while !Task.isCancelled && session.isActive {
            if let id = session.serverId,
               let d = try? await APIClient.shared.recording(id) {
                let text = d.utterances.map { $0.text.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
                if text != liveText { liveText = text }
            }
            try? await Task.sleep(for: .seconds(2.5))
        }
    }
}
