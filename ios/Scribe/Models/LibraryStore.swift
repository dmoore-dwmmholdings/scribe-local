import Foundation
import Observation

/// The recordings list, cached on disk so the Library opens instantly and
/// still shows something when the server is out of reach.
@Observable
final class LibraryStore {
    static let shared = LibraryStore()

    private(set) var recordings: [Recording] = []
    private(set) var loading = false
    /// Set when the server refuses this device, so the Library can say why it
    /// is showing a stale list instead of failing quietly.
    private(set) var authError: String?
    private(set) var lastError: String?

    private let cacheURL = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("recordings.json")

    init() {
        if let data = try? Data(contentsOf: cacheURL),
           let cached = try? JSONDecoder().decode([Recording].self, from: data) {
            recordings = cached
        }
    }

    /// Every tag in use, for the filter chips.
    var tags: [String] {
        Array(Set(recordings.flatMap { $0.tags ?? [] })).sorted()
    }

    @MainActor
    func refresh() async {
        guard Settings.shared.isConfigured else { return }
        loading = true
        defer { loading = false }
        do {
            let list = try await APIClient.shared.listRecordings(limit: 200)
            set(list)
            authError = nil
            lastError = nil
        } catch let e as APIError where e.isUnauthorized {
            authError = "The server rejected this device. Check the connection in Settings."
        } catch is CancellationError {
        } catch {
            lastError = error.localizedDescription
        }
    }

    @MainActor
    func delete(_ recording: Recording) async throws {
        try await APIClient.shared.deleteRecording(recording.id)
        set(recordings.filter { $0.id != recording.id })
    }

    @MainActor
    func upsert(_ recording: Recording) {
        if let i = recordings.firstIndex(where: { $0.id == recording.id }) {
            var next = recordings
            next[i] = recording
            set(next)
        } else {
            set([recording] + recordings)
        }
    }

    private func set(_ list: [Recording]) {
        recordings = list
        if let data = try? JSONEncoder().encode(list) { try? data.write(to: cacheURL, options: .atomic) }
    }
}
