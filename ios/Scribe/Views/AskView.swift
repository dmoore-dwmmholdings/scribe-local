import SwiftUI

/// Questions answered from the meetings, as a conversation: every earlier turn
/// goes back as history, so "why?" has something to refer to.
struct AskView: View {
    struct Turn: Identifiable {
        let id = UUID()
        let question: String
        var answer: String?
        var citations: [Citation] = []
        var failed = false
    }

    private static let suggestions = [
        "What did we decide about the record screen?",
        "List every action item from this week",
        "Summarize my most recent meeting",
    ]

    @State private var turns: [Turn] = []
    @State private var draft = ""
    @State private var thinking = false
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 18) {
                        if turns.isEmpty { hero } else { thread }
                        if thinking {
                            HStack(spacing: 8) { ProgressView(); Text("Thinking…").foregroundStyle(Theme.textMuted) }
                                .font(.footnote).id("thinking")
                        }
                    }
                    .padding(16)
                }
                .onChange(of: turns.count) { _, _ in withAnimation { proxy.scrollTo(turns.last?.id, anchor: .top) } }
            }
            .background(Theme.bg)
            .navigationTitle("Ask")
            .toolbar {
                if !turns.isEmpty {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("New chat") { turns = []; draft = "" }.disabled(thinking)
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { composer }
            .navigationDestination(for: RecordingLink.self) { link in
                RecordingDetailView(recordingId: link.id, initial: nil, startAtMs: link.seekMs)
            }
        }
    }

    private var hero: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Ask your meetings").font(.title2.weight(.semibold)).foregroundStyle(Theme.textPrimary)
            Text("Ask anything about your recordings, then keep talking — follow-ups remember what you already asked. Answers link back to the moment in the audio.")
                .font(.callout).foregroundStyle(Theme.textMuted)
            Text("TRY ASKING").font(.caption.weight(.semibold)).foregroundStyle(Theme.textDim).padding(.top, 8)
            ForEach(Self.suggestions, id: \.self) { s in
                Button { ask(s) } label: {
                    HStack {
                        Text(s).foregroundStyle(Theme.textPrimary).multilineTextAlignment(.leading)
                        Spacer()
                        Image(systemName: "chevron.right").foregroundStyle(Theme.textDim)
                    }
                    .padding(12)
                    .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var thread: some View {
        ForEach(turns) { turn in
            VStack(alignment: .leading, spacing: 10) {
                Text(turn.question)
                    .font(.callout.weight(.medium)).foregroundStyle(Theme.textPrimary)
                    .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.accentSoft, in: RoundedRectangle(cornerRadius: 12))
                if let a = turn.answer {
                    Text(a).font(.callout).foregroundStyle(turn.failed ? Theme.amber : Theme.textBody).textSelection(.enabled)
                }
                if !turn.citations.isEmpty {
                    Text("SOURCES").font(.caption2.weight(.semibold)).foregroundStyle(Theme.textDim)
                    ForEach(Array(turn.citations.enumerated()), id: \.offset) { _, c in
                        NavigationLink(value: RecordingLink(id: c.recordingId, title: c.recordingTitle, seekMs: c.startMs)) {
                            HStack(alignment: .top, spacing: 8) {
                                Text("▶").foregroundStyle(Theme.accent)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(c.recordingTitle ?? "Untitled recording").font(.caption.weight(.medium))
                                        .foregroundStyle(Theme.textPrimary).lineLimit(1)
                                    Text([c.speaker, c.startMs.map { formatClock(ms: $0) }].compactMap { $0 }.joined(separator: " · ")
                                         + " — " + c.snippet)
                                        .font(.caption).foregroundStyle(Theme.textMuted).lineLimit(2)
                                }
                            }
                            .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 10))
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .id(turn.id)
        }
    }

    private var composer: some View {
        HStack(spacing: 10) {
            TextField(turns.isEmpty ? "Ask a question…" : "Ask a follow-up…", text: $draft, axis: .vertical)
                .lineLimit(1...4)
                .focused($focused)
                .padding(10)
                .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
                .onSubmit { ask(draft) }
            Button { ask(draft) } label: {
                Image(systemName: "arrow.up.circle.fill").font(.system(size: 32))
            }
            .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty || thinking)
            .accessibilityLabel("Send")
        }
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(.ultraThinMaterial)
    }

    private func ask(_ text: String) {
        let q = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty, !thinking else { return }
        // The thread before this question is the history; a failed turn is
        // shown but never sent back, so a server error cannot poison follow-ups.
        let history: [AskTurn] = turns.filter { !$0.failed && $0.answer != nil }.flatMap {
            [AskTurn(role: .user, content: $0.question), AskTurn(role: .assistant, content: $0.answer!)]
        }
        draft = ""
        focused = false
        turns.append(Turn(question: q))
        let index = turns.count - 1
        thinking = true
        Task {
            defer { thinking = false }
            do {
                let res = try await APIClient.shared.ask(AskRequest(question: q, history: history, topK: 6))
                turns[index].answer = res.answer
                turns[index].citations = res.citations
            } catch {
                turns[index].answer = "Could not get an answer: \(error.localizedDescription)"
                turns[index].failed = true
            }
        }
    }
}
