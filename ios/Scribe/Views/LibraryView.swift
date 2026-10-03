import SwiftUI

struct LibraryView: View {
    private var store = LibraryStore.shared
    @State private var tag: String?
    @State private var pendingDelete: Recording?
    @State private var alert: AlertItem?

    private var shown: [Recording] {
        guard let tag else { return store.recordings }
        return store.recordings.filter { ($0.tags ?? []).contains(tag) }
    }

    var body: some View {
        NavigationStack {
            Group {
                if !Settings.shared.isConfigured {
                    ContentUnavailableView("No server yet", systemImage: "server.rack",
                                           description: Text("Set up your Scribe server in Settings."))
                } else if store.recordings.isEmpty && !store.loading && pendingLocal.isEmpty {
                    ContentUnavailableView("No recordings yet", systemImage: "waveform",
                                           description: Text("Recordings you make appear here once they upload."))
                } else {
                    list
                }
            }
            .background(Theme.bg)
            .navigationTitle("Library")
            .navigationDestination(for: Recording.self) { RecordingDetailView(recordingId: $0.id, initial: $0) }
            .refreshable { await store.refresh() }
            .task { await store.refresh() }
            .confirmationDialog("Delete this recording?", isPresented: Binding(
                get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }
            ), titleVisibility: .visible, presenting: pendingDelete) { r in
                Button("Delete recording and audio", role: .destructive) {
                    Task {
                        do { try await store.delete(r) }
                        catch { alert = AlertItem(title: "Could not delete", message: error.localizedDescription) }
                    }
                }
            } message: { _ in
                Text("The transcript, summary and audio are removed from the server. This cannot be undone.")
            }
            .alert(item: $alert) { a in Alert(title: Text(a.title), message: Text(a.message)) }
        }
    }

    private var pendingLocal: [LocalRecording] {
        LocalRecordings.shared.items.filter { !$0.completed }.sorted { $0.createdAt > $1.createdAt }
    }

    private var list: some View {
        List {
            if !pendingLocal.isEmpty {
                Section {
                    ForEach(pendingLocal) { r in LocalRecordingRow(recording: r) }
                        .listRowBackground(Theme.surface)
                    if let why = UploadQueue.shared.blocked {
                        Button { UploadQueue.shared.kick() } label: {
                            Label(why, systemImage: "arrow.clockwise").font(.footnote)
                        }
                        .listRowBackground(Theme.surface)
                    }
                } header: {
                    Text("On this phone")
                }
            }
            if let err = store.authError {
                Label(err, systemImage: "lock.trianglebadge.exclamationmark")
                    .font(.footnote).foregroundStyle(Theme.amber)
                    .listRowBackground(Theme.surface)
            }
            if !store.tags.isEmpty {
                tagChips.listRowBackground(Color.clear).listRowInsets(EdgeInsets())
            }
            ForEach(shown) { r in
                NavigationLink(value: r) { RecordingRow(recording: r) }
                    .listRowBackground(Theme.surface)
                    .swipeActions {
                        Button(role: .destructive) { pendingDelete = r } label: { Label("Delete", systemImage: "trash") }
                    }
            }
            if let tag, shown.isEmpty {
                Text("No recordings tagged “\(tag)”.").foregroundStyle(Theme.textMuted).listRowBackground(Color.clear)
            }
        }
        .scrollContentBackground(.hidden)
    }

    private var tagChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Chip(label: "All", active: tag == nil) { tag = nil }
                ForEach(store.tags, id: \.self) { t in Chip(label: t, active: tag == t) { tag = (tag == t ? nil : t) } }
            }
            .padding(.horizontal, 16).padding(.vertical, 6)
        }
    }
}

struct RecordingRow: View {
    let recording: Recording

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(recording.title?.isEmpty == false ? recording.title! : "Untitled recording")
                .font(.body.weight(.medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            HStack(spacing: 6) {
                if let d = recording.createdDate {
                    Text(d.formatted(date: .abbreviated, time: .shortened))
                }
                if let ms = recording.durationMs { Text("· \(formatClock(ms: ms))") }
                StatusBadge(status: recording.status)
            }
            .font(.caption).foregroundStyle(Theme.textMuted)
            if let tags = recording.tags, !tags.isEmpty {
                Text(tags.map { "#\($0)" }.joined(separator: " ")).font(.caption2).foregroundStyle(Theme.textDim)
            }
        }
        .padding(.vertical, 2)
    }
}

/// A recording still on this phone: recording, uploading, or waiting.
struct LocalRecordingRow: View {
    let recording: LocalRecording

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(recording.title ?? recording.createdAt.formatted(date: .abbreviated, time: .shortened))
                .font(.body.weight(.medium)).foregroundStyle(Theme.textPrimary).lineLimit(1)
            HStack(spacing: 6) {
                if !recording.finished {
                    Text("Recording").foregroundStyle(Theme.accent)
                } else if recording.pendingSegments > 0 {
                    Text("Uploading \(recording.segments.count - recording.pendingSegments) of \(recording.segments.count)")
                } else {
                    Text("Finishing upload")
                }
                if recording.durationMs > 0 { Text("· \(formatClock(ms: recording.durationMs))") }
            }
            .font(.caption).foregroundStyle(Theme.textMuted)
            if recording.finished, recording.segments.count > 0 {
                ProgressView(value: Double(recording.segments.count - recording.pendingSegments),
                             total: Double(recording.segments.count))
                    .tint(Theme.accent)
            }
            if let e = recording.lastError {
                Text(e).font(.caption2).foregroundStyle(Theme.amber).lineLimit(2)
            }
        }
        .padding(.vertical, 2)
    }
}

struct StatusBadge: View {
    let status: RecordingStatus
    var body: some View {
        if status != .ready {
            Text(label).font(.caption2.weight(.semibold))
                .padding(.horizontal, 6).padding(.vertical, 2)
                .background(color.opacity(0.15), in: Capsule())
                .foregroundStyle(color)
        }
    }
    private var label: String {
        switch status {
        case .uploading: return "UPLOADING"
        case .processing: return "PROCESSING"
        case .failed: return "FAILED"
        case .ready: return "READY"
        case .unknown: return "?"
        }
    }
    private var color: Color {
        switch status {
        case .failed: return Theme.accentDeep
        case .ready: return Theme.ready
        default: return Theme.accent
        }
    }
}

struct Chip: View {
    let label: String
    let active: Bool
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Text(label).font(.footnote).lineLimit(1)
                .padding(.horizontal, 12).padding(.vertical, 6)
                .background(active ? Theme.accent.opacity(0.15) : Color.white.opacity(0.05), in: Capsule())
                .overlay(Capsule().stroke(active ? Theme.accent.opacity(0.3) : .clear))
                .foregroundStyle(active ? Theme.accent : Theme.textMuted)
        }
        .buttonStyle(.plain)
    }
}
