import SwiftUI

/// Opens a recording, optionally playing from a moment.
struct RecordingLink: Hashable {
    let id: String
    let title: String?
    let seekMs: Int?
}

struct SearchView: View {
    enum Range: String, CaseIterable { case all = "All", week = "This week" }

    @State private var query = ""
    @State private var range: Range = .all
    @State private var speaker: EnrolledSpeaker?
    @State private var speakers: [EnrolledSpeaker] = []
    @State private var hits: [SearchHit] = []
    @State private var searching = false
    @State private var error: String?
    @State private var searched = false

    var body: some View {
        NavigationStack {
            List {
                filters.listRowBackground(Color.clear).listRowInsets(EdgeInsets())
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(Theme.amber)
                }
                if searched && hits.isEmpty && !searching && error == nil {
                    ContentUnavailableView.search(text: query)
                        .listRowBackground(Color.clear)
                }
                if !hits.isEmpty {
                    Section("\(hits.count) results") {
                        ForEach(Array(hits.enumerated()), id: \.offset) { _, hit in
                            NavigationLink(value: RecordingLink(id: hit.recordingId, title: hit.recordingTitle, seekMs: hit.startMs)) {
                                HitRow(hit: hit, query: query)
                            }
                            .listRowBackground(Theme.surface)
                        }
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.bg)
            .navigationTitle("Search")
            .searchable(text: $query, prompt: "Search across all meetings…")
            .onSubmit(of: .search) { Task { await run() } }
            .task(id: "\(query)|\(range.rawValue)|\(speaker?.id ?? "")") {
                // Debounce typing; an empty box clears the results.
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled else { return }
                await run()
            }
            .task { speakers = (try? await APIClient.shared.speakers()) ?? [] }
            .overlay { if searching && hits.isEmpty { ProgressView() } }
            .navigationDestination(for: RecordingLink.self) { link in
                RecordingDetailView(recordingId: link.id, initial: nil, startAtMs: link.seekMs)
            }
        }
    }

    private var filters: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Range.allCases, id: \.self) { r in
                    Chip(label: r.rawValue, active: range == r) { range = r }
                }
                if !speakers.isEmpty {
                    Menu {
                        Button("Anyone") { speaker = nil }
                        ForEach(speakers) { s in Button(s.displayName) { speaker = s } }
                    } label: {
                        Text(speaker.map { "Said by \($0.displayName)" } ?? "Said by…")
                            .font(.footnote)
                            .padding(.horizontal, 12).padding(.vertical, 6)
                            .background(speaker != nil ? Theme.accent.opacity(0.15) : Color.white.opacity(0.05), in: Capsule())
                            .foregroundStyle(speaker != nil ? Theme.accent : Theme.textMuted)
                    }
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 6)
        }
    }

    @MainActor
    private func run() async {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { hits = []; searched = false; error = nil; return }
        searching = true
        defer { searching = false }
        do {
            let from = range == .week ? Calendar.current.date(byAdding: .day, value: -7, to: Date()) : nil
            hits = try await APIClient.shared.search(q, from: from, speaker: speaker?.id, limit: 40)
            error = nil
        } catch is CancellationError {
            return
        } catch {
            self.error = error.localizedDescription
        }
        searched = true
    }
}

struct HitRow: View {
    let hit: SearchHit
    let query: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(hit.recordingTitle ?? "Untitled recording").font(.subheadline.weight(.medium))
                    .foregroundStyle(Theme.textPrimary).lineLimit(1)
                Spacer()
                Text("\(Int((hit.score * 100).rounded()))%").font(.caption2).foregroundStyle(Theme.textDim)
            }
            Text(highlighted).font(.footnote).foregroundStyle(Theme.textBody).lineLimit(3)
            HStack(spacing: 6) {
                if let ms = hit.startMs { Text("▶ \(formatClock(ms: ms))").foregroundStyle(Theme.accent) }
                if let who = hit.speaker { Text(who).foregroundStyle(Theme.textMuted) }
            }
            .font(.caption)
        }
        .padding(.vertical, 2)
    }

    /// The snippet with the query's words marked.
    private var highlighted: AttributedString {
        var s = AttributedString(hit.text)
        for term in query.lowercased().split(separator: " ") where term.count > 1 {
            var start = s.startIndex
            while let r = s[start...].range(of: term, options: .caseInsensitive) {
                s[r].backgroundColor = Theme.accent.opacity(0.28)
                s[r].foregroundColor = Color(hex: 0xFFD9BF)
                start = r.upperBound
            }
        }
        return s
    }
}
