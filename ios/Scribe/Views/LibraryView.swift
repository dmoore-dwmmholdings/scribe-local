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

    private var pendingLocal: [LocalRecording] {
        LocalRecordings.shared.items.filter { !$0.completed }.sorted { $0.createdAt > $1.createdAt }
    }

    var body: some View {
        NavigationStack {
            TabScreen("Library", accessory: {
                Text("\(store.recordings.count)").font(.mono(11)).foregroundStyle(Theme.textMuted)
            }) {
                VStack(alignment: .leading, spacing: 10) {
                    if let err = store.authError {
                        Label(err, systemImage: "lock.trianglebadge.exclamationmark")
                            .font(.footnote).foregroundStyle(Theme.amber).padding(.bottom, 4)
                    }
                    if !pendingLocal.isEmpty {
                        SectionLabel("On this phone").padding(.top, 4)
                        ForEach(pendingLocal) { r in Card(padding: 14) { LocalRecordingRow(recording: r) } }
                        if let why = UploadQueue.shared.blocked {
                            Button { UploadQueue.shared.kick() } label: {
                                Label(why, systemImage: "arrow.clockwise").font(.footnote).foregroundStyle(Theme.amber)
                            }
                        }
                    }
                    if !store.tags.isEmpty { tagChips }
                    if !Settings.shared.isConfigured {
                        empty("No server yet", "Set up your Scribe server in Settings. Recordings wait on this phone until then.", "server.rack")
                    } else if store.recordings.isEmpty && pendingLocal.isEmpty && !store.loading {
                        empty("No recordings yet", "Recordings you make appear here once they upload.", "waveform")
                    }
                    if !shown.isEmpty {
                        SectionLabel(tag.map { "#\($0)" } ?? "Recordings").padding(.top, 8)
                    }
                    ForEach(shown) { r in
                        NavigationLink(value: r) {
                            Card(padding: 14) { RecordingRow(recording: r) }
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("recording-row")
                        .contextMenu {
                            Button(role: .destructive) { pendingDelete = r } label: { Label("Delete", systemImage: "trash") }
                        }
                    }
                    if let tag, shown.isEmpty {
                        Text("No recordings tagged “\(tag)”.").font(.footnote).foregroundStyle(Theme.textMuted)
                    }
                }
            }
            .navigationDestination(for: Recording.self) { RecordingDetailView(recordingId: $0.id, initial: $0) }
            .refreshable { await store.refresh() }
            .task {
                // Keep statuses moving while anything is still on its way: a
                // recording that finished processing should not sit at
                // PROCESSING until someone pulls to refresh.
                while !Task.isCancelled {
                    await store.refresh()
                    let busy = store.recordings.contains { $0.status == .processing || $0.status == .uploading }
                        || !pendingLocal.isEmpty
                    try? await Task.sleep(for: .seconds(busy ? 5 : 60))
                }
            }
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

    private func empty(_ title: String, _ body: String, _ icon: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 34)).foregroundStyle(Theme.accent.opacity(0.7))
            Text(title).font(.headline).foregroundStyle(Theme.textPrimary)
            Text(body).font(.footnote).foregroundStyle(Theme.textMuted).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 60)
    }

    private var tagChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Chip(label: "All", active: tag == nil) { tag = nil }
                ForEach(store.tags, id: \.self) { t in Chip(label: t, active: tag == t) { tag = (tag == t ? nil : t) } }
            }
        }
        .padding(.vertical, 4)
    }
}

struct RecordingRow: View {
    let recording: Recording

    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Theme.accent.opacity(0.07))
                .frame(width: 44, height: 44)
                .overlay(Image(systemName: "waveform").foregroundStyle(Theme.accent))
            VStack(alignment: .leading, spacing: 5) {
                Text(recording.title?.isEmpty == false ? recording.title! : "Untitled recording")
                    .font(.system(size: 16, weight: .semibold)).foregroundStyle(Theme.textPrimary).lineLimit(1)
                HStack(spacing: 6) {
                    if let d = recording.createdDate { Text(d.formatted(.dateTime.month(.abbreviated).day().hour().minute())) }
                    if let ms = recording.durationMs { Text("· \(formatClock(ms: ms))") }
                    StatusBadge(status: recording.status)
                }
                .font(.mono(11, weight: .medium)).foregroundStyle(Theme.textMuted)
                if let tags = recording.tags, !tags.isEmpty {
                    Text(tags.map { "#\($0)" }.joined(separator: " ")).font(.caption2).foregroundStyle(Theme.textDim)
                }
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(Theme.textDim)
        }
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
            Text(label).font(.mono(10, weight: .bold))
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
