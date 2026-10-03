import SwiftUI

struct RecordingDetailView: View {
    @State private var model: RecordingDetailModel
    private let initial: Recording?

    @State private var filter = ""
    @State private var alert: AlertItem?
    @State private var confirm: Confirm?
    @State private var showParticipants = false
    @State private var showTags = false
    @State private var editing: Utterance?
    @State private var tagging: TagTarget?

    struct TagTarget: Identifiable { let localIdx: Int; var id: Int { localIdx } }
    @State private var player = Player()
    @State private var follow = true

    enum Confirm: Identifiable {
        case reprocess, rediarize
        var id: Int { hashValue }
    }

    init(recordingId: String, initial: Recording?) {
        _model = State(initialValue: RecordingDetailModel(recordingId: recordingId))
        self.initial = initial
    }

    private var recording: Recording? { model.recording ?? initial }
    private var title: String {
        if let t = model.activeSummary?.title, !t.isEmpty { return t }
        if let t = recording?.title, !t.isEmpty { return t }
        return "Untitled recording"
    }

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                header
                if let err = model.error, model.detail == nil {
                    Label(err, systemImage: "exclamationmark.triangle").foregroundStyle(Theme.amber).font(.footnote)
                }
                if let p = model.detail?.progress, model.isWorking || recording?.status == .failed {
                    PipelineProgressView(progress: p)
                } else if recording?.status == .uploading {
                    banner("Uploading — the server starts work once every segment has arrived.", "icloud.and.arrow.up")
                }
                if let tags = recording?.tags, !tags.isEmpty {
                    Text(tags.map { "#\($0)" }.joined(separator: "  ")).font(.caption).foregroundStyle(Theme.textMuted)
                }
                if let marks = recording?.marks, !marks.isEmpty { marksSection(marks) }
                summarySection
                if model.talkTime.count >= 2 { talkTimeSection }
                transcriptSection
            }
            .padding(16)
        }
        // Keep the line being spoken on screen while following.
        .onChange(of: player.activeLine) { _, line in
            guard follow, player.isPlaying, filter.isEmpty, let line else { return }
            withAnimation(.easeInOut(duration: 0.25)) { proxy.scrollTo(line, anchor: .center) }
        }
        }
        .safeAreaInset(edge: .bottom) {
            if recording?.status == .ready || player.isReady {
                PlaybackBar(player: player, marks: recording?.marks ?? [], follow: $follow)
            }
        }
        .onChange(of: model.utterances) { _, u in player.setTranscript(u) }
        .onChange(of: recording?.status) { _, s in
            if s == .ready { player.load(recordingId: model.recordingId, durationMs: recording?.durationMs) }
        }
        .background(Theme.bg)
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .topBarTrailing) { actionsMenu } }
        .task {
            await model.load()
            player.setTranscript(model.utterances)
            if recording?.status == .ready { player.load(recordingId: model.recordingId, durationMs: recording?.durationMs) }
        }
        .refreshable { await model.load() }
        .onDisappear { model.stopPolling(); player.pause() }
        .confirmationDialog(confirmTitle, isPresented: Binding(get: { confirm != nil }, set: { if !$0 { confirm = nil } }),
                            titleVisibility: .visible, presenting: confirm) { c in
            switch c {
            case .reprocess: Button("Reprocess", role: .destructive) { act { await model.reprocess() } }
            case .rediarize: Button("Redo speaker detection") { act { await model.rediarize() } }
            }
        } message: { c in
            switch c {
            case .reprocess: Text("Re-run transcription, speaker detection and the summary from the original audio. This replaces the current transcript.")
            case .rediarize: Text("Work out who spoke when again, and re-label the transcript. The words are kept, so this is much quicker than a full reprocess.")
            }
        }
        .sheet(isPresented: $showParticipants) {
            ParticipantsSheet(current: recording?.participantsExpected ?? model.speakers.count) { count, redo in
                act { await model.setParticipants(count, redo: redo) }
            }
            .presentationDetents([.medium])
        }
        .sheet(isPresented: $showTags) {
            TagsSheet(tags: recording?.tags ?? []) { tags in act { await model.setTags(tags) } }
                .presentationDetents([.medium, .large])
        }
        .sheet(item: $editing) { u in
            EditUtteranceSheet(original: u.text) { text in act { await model.edit(u, text: text) } }
                .presentationDetents([.medium])
        }
        .sheet(item: $tagging) { t in
            SpeakerTagSheet(
                currentLabel: model.speakerName(t.localIdx),
                isNamed: model.speakers.first(where: { $0.localIdx == t.localIdx })?.speakerId != nil,
                onTag: { await model.tagSpeaker(t.localIdx, $0) },
                onUntag: { await model.untagSpeaker(t.localIdx) },
                onNotParticipant: { await model.removeSpeaker(t.localIdx) }
            )
        }
        .alert(item: $alert) { a in Alert(title: Text(a.title), message: Text(a.message)) }
    }

    private var confirmTitle: String {
        switch confirm {
        case .reprocess: return "Reprocess recording?"
        case .rediarize: return "Redo speaker detection?"
        case nil: return ""
        }
    }

    /// Run an action and surface its error, if any.
    private func act(_ work: @escaping () async -> String?) {
        Task {
            if let err = await work() { alert = AlertItem(title: "That did not work", message: err) }
        }
    }

    // MARK: Header and actions

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.title2.weight(.semibold)).foregroundStyle(Theme.textPrimary)
            HStack(spacing: 6) {
                if let d = recording?.createdDate { Text(d.formatted(date: .abbreviated, time: .shortened)) }
                if let ms = recording?.durationMs { Text("· \(formatClock(ms: ms))") }
                if let s = recording?.status { StatusBadge(status: s) }
            }
            .font(.caption).foregroundStyle(Theme.textMuted)
        }
    }

    private var actionsMenu: some View {
        Menu {
            if !model.templates.isEmpty {
                Menu("Summary view") {
                    ForEach(model.templates) { t in
                        Button(t.label) { act { await model.summarize(template: t.id) } }
                    }
                }
            }
            Button { showParticipants = true } label: { Label("Number of speakers…", systemImage: "person.2") }
            Button { confirm = .rediarize } label: { Label("Redo speaker detection", systemImage: "person.wave.2") }
            Button { showTags = true } label: { Label("Tags…", systemImage: "tag") }
            Divider()
            Button(role: .destructive) { confirm = .reprocess } label: { Label("Reprocess", systemImage: "arrow.clockwise") }
        } label: {
            Image(systemName: "ellipsis.circle")
        }
        .disabled(model.busy)
    }

    private func banner(_ text: String, _ icon: String) -> some View {
        Label(text, systemImage: icon).font(.footnote).foregroundStyle(Theme.textMuted)
            .padding(12).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
    }

    // MARK: Summary

    @ViewBuilder private var summarySection: some View {
        if let s = model.activeSummary {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text("SUMMARY").font(.caption.weight(.semibold)).foregroundStyle(Theme.accent)
                    Spacer()
                    if let t = s.template, let label = model.templates.first(where: { $0.id == t })?.label {
                        Text(label).font(.caption2).foregroundStyle(Theme.textMuted)
                    }
                }
                if let text = s.summary, !text.isEmpty {
                    Text(text).font(.callout).foregroundStyle(Theme.textBody).textSelection(.enabled)
                }
                list("Decisions", s.decisions?.items)
                list("Action items", s.actionItems?.items)
                list("Topics", s.topics?.items)
            }
            .padding(14)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 14))
        } else if recording?.status == .ready {
            Button { act { await model.summarize(template: model.templates.first?.id ?? "general") } } label: {
                Label("Generate a summary", systemImage: "sparkles").frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
        }
    }

    @ViewBuilder private func list(_ heading: String, _ items: [String]?) -> some View {
        if let items, !items.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text(heading).font(.footnote.weight(.semibold)).foregroundStyle(Theme.textPrimary)
                ForEach(items, id: \.self) { item in
                    HStack(alignment: .top, spacing: 8) {
                        Text("•").foregroundStyle(Theme.accent)
                        Text(item).font(.footnote).foregroundStyle(Theme.textBody)
                    }
                }
            }
        }
    }

    // MARK: Marks

    private func marksSection(_ marks: [Int]) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("MARKS").font(.caption.weight(.semibold)).foregroundStyle(Theme.textMuted)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(marks, id: \.self) { m in
                        Button { player.seek(toMs: m, play: true) } label: {
                            Label(formatClock(ms: m), systemImage: "flag.fill")
                                .font(.footnote.monospacedDigit())
                                .padding(.horizontal, 10).padding(.vertical, 6)
                                .background(Theme.amber.opacity(0.15), in: Capsule())
                                .foregroundStyle(Theme.amber)
                        }
                        .accessibilityLabel("Play from mark at \(formatClock(ms: m))")
                    }
                }
            }
        }
    }

    // MARK: Talk time

    private var talkTimeSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("TALK TIME").font(.caption.weight(.semibold)).foregroundStyle(Theme.textMuted)
            ForEach(model.talkTime, id: \.name) { row in
                HStack(spacing: 8) {
                    Circle().fill(Theme.speakerColor(row.localIdx)).frame(width: 8, height: 8)
                    Text(row.name).font(.footnote).foregroundStyle(Theme.textPrimary).lineLimit(1)
                    Spacer()
                    Text("\(Int((row.share * 100).rounded()))% · \(formatClock(ms: row.ms))")
                        .font(.caption.monospacedDigit()).foregroundStyle(Theme.textMuted)
                }
                GeometryReader { g in
                    Capsule().fill(Theme.speakerColor(row.localIdx).opacity(0.7))
                        .frame(width: max(4, g.size.width * row.share), height: 4)
                }
                .frame(height: 4)
            }
        }
        .padding(14)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 14))
    }

    // MARK: Transcript

    private var shownUtterances: [Utterance] {
        let q = filter.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return model.utterances }
        return model.utterances.filter { $0.text.lowercased().contains(q) || model.name(for: $0).lowercased().contains(q) }
    }

    @ViewBuilder private var transcriptSection: some View {
        if !model.utterances.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                Text("TRANSCRIPT").font(.caption.weight(.semibold)).foregroundStyle(Theme.textMuted)
                TextField("Find in transcript", text: $filter)
                    .textFieldStyle(.roundedBorder)
                    .textInputAutocapitalization(.never)
                ForEach(shownUtterances) { u in
                    UtteranceRow(
                        utterance: u, name: model.name(for: u),
                        tokens: player.tokens(forLine: u.id),
                        activeToken: player.activeLine == u.id ? player.activeToken : nil,
                        isActive: player.activeLine == u.id,
                        onSeek: { ms in player.seek(toMs: ms, play: true) }
                    )
                    .id(u.id)
                        .contextMenu {
                            Button { player.seek(toMs: u.startMs, play: true) } label: { Label("Play from here", systemImage: "play") }
                            if let idx = u.localIdx {
                                Button { tagging = TagTarget(localIdx: idx) } label: { Label("Who is this?", systemImage: "person.crop.circle.badge.questionmark") }
                            }
                            Button { editing = u } label: { Label("Edit text", systemImage: "pencil") }
                            Button { UIPasteboard.general.string = u.text } label: { Label("Copy", systemImage: "doc.on.doc") }
                        }
                }
                if shownUtterances.isEmpty {
                    Text("Nothing in the transcript matches “\(filter)”.").font(.footnote).foregroundStyle(Theme.textMuted)
                }
            }
        } else if recording?.status == .ready {
            Text("No transcript — there may have been no speech in this recording.")
                .font(.footnote).foregroundStyle(Theme.textMuted)
        }
    }
}

/// One transcript line. Each word is tappable and plays from that word; the
/// word being spoken is lit.
struct UtteranceRow: View, Equatable {
    let utterance: Utterance
    let name: String
    let tokens: [Karaoke.Token]
    let activeToken: Int?
    let isActive: Bool
    let onSeek: (Int) -> Void

    static func == (a: Self, b: Self) -> Bool {
        a.utterance == b.utterance && a.name == b.name && a.activeToken == b.activeToken
            && a.isActive == b.isActive && a.tokens.count == b.tokens.count
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle().fill(Theme.speakerColor(utterance.localIdx)).frame(width: 7, height: 7)
                Text(name).font(.caption.weight(.semibold)).foregroundStyle(Theme.speakerColor(utterance.localIdx))
                Text(formatClock(ms: utterance.startMs)).font(.caption2.monospacedDigit()).foregroundStyle(Theme.textDim)
            }
            .onTapGesture { onSeek(utterance.startMs) }
            if tokens.isEmpty {
                Text(utterance.text).font(.callout).foregroundStyle(Theme.textBody)
            } else {
                FlowLayout(spacing: 4) {
                    ForEach(Array(tokens.enumerated()), id: \.offset) { i, t in
                        Text(t.text)
                            .font(.callout)
                            .foregroundStyle(i == activeToken ? Color(hex: 0xFFD9BF) : Theme.textBody)
                            .padding(.horizontal, i == activeToken ? 2 : 0)
                            .background(i == activeToken ? Theme.accent.opacity(0.28) : .clear, in: RoundedRectangle(cornerRadius: 3))
                            .onTapGesture { onSeek(t.startMs) }
                    }
                }
            }
        }
        .padding(.vertical, 4)
        .padding(.horizontal, isActive ? 8 : 0)
        .background(isActive ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 10))
    }
}

// MARK: Sheets

struct ParticipantsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var count: Int
    @State private var redo = true
    let onSave: (Int?, Bool) -> Void

    init(current: Int, onSave: @escaping (Int?, Bool) -> Void) {
        _count = State(initialValue: max(1, current))
        self.onSave = onSave
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Stepper("\(count) \(count == 1 ? "person" : "people")", value: $count, in: 1...64)
                    Toggle("Redo speaker detection now", isOn: $redo)
                } footer: {
                    Text("Saying how many people spoke is the one correction that reliably improves speaker detection. Redoing it keeps the words, so it is quick.")
                }
                Section {
                    Button("Let Scribe work it out") { onSave(nil, redo); dismiss() }
                }
            }
            .navigationTitle("Number of speakers")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { onSave(count, redo); dismiss() } }
            }
        }
    }
}

struct TagsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var tags: [String]
    @State private var draft = ""
    let onSave: ([String]) -> Void

    init(tags: [String], onSave: @escaping ([String]) -> Void) {
        _tags = State(initialValue: tags)
        self.onSave = onSave
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack {
                        TextField("Add a tag", text: $draft)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                            .onSubmit(add)
                        Button("Add", action: add).disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                }
                Section {
                    ForEach(tags, id: \.self) { Text("#\($0)") }
                        .onDelete { tags.remove(atOffsets: $0) }
                }
            }
            .navigationTitle("Tags")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save") { onSave(tags); dismiss() } }
            }
        }
    }

    private func add() {
        let t = draft.trimmingCharacters(in: .whitespaces).lowercased()
        if !t.isEmpty && !tags.contains(t) { tags.append(t) }
        draft = ""
    }
}

struct EditUtteranceSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var text: String
    let onSave: (String) -> Void

    init(original: String, onSave: @escaping (String) -> Void) {
        _text = State(initialValue: original)
        self.onSave = onSave
    }

    var body: some View {
        NavigationStack {
            TextEditor(text: $text)
                .scrollContentBackground(.hidden)
                .padding()
                .background(Theme.bg)
                .navigationTitle("Edit line")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Save") { onSave(text.trimmingCharacters(in: .whitespacesAndNewlines)); dismiss() }
                            .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
        }
    }
}
