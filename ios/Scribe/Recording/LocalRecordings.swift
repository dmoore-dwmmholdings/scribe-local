import Foundation
import Observation

/// One recording as this phone knows it, before and while it uploads.
///
/// Recording works offline: segments are written here first, and the upload
/// queue creates the server recording and sends them whenever it can. Nothing
/// is deleted until the server has the whole recording.
struct LocalRecording: Codable, Identifiable, Hashable {
    struct Segment: Codable, Hashable {
        let seq: Int
        /// File name inside the recording's directory.
        let file: String
        let startMs: Int
        let durationMs: Int
        var uploaded = false
    }

    let id: String
    var serverId: String?
    var title: String?
    var participants: Int?
    let createdAt: Date
    var segments: [Segment] = []
    var marks: [Int] = []
    var durationMs = 0
    /// Recording has stopped; once every segment is up, the server is told.
    var finished = false
    /// The server has been told the recording is complete.
    var completed = false
    var lastError: String?

    var pendingSegments: Int { segments.filter { !$0.uploaded }.count }
}

/// The phone's list of recordings not yet fully on the server, persisted as
/// JSON beside their audio in Application Support.
@Observable
final class LocalRecordings {
    static let shared = LocalRecordings()

    private(set) var items: [LocalRecording] = []
    let root: URL
    private let manifest: URL

    init() {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        root = base.appendingPathComponent("recordings", isDirectory: true)
        manifest = root.appendingPathComponent("manifest.json")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        if let data = try? Data(contentsOf: manifest),
           let list = try? JSONDecoder().decode([LocalRecording].self, from: data) {
            items = list
        }
    }

    func directory(for id: String) -> URL {
        let d = root.appendingPathComponent(id, isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func fileURL(_ r: LocalRecording, _ s: LocalRecording.Segment) -> URL {
        directory(for: r.id).appendingPathComponent(s.file)
    }

    func get(_ id: String) -> LocalRecording? { items.first { $0.id == id } }

    func add(_ r: LocalRecording) { mutate { $0.append(r) } }

    func update(_ id: String, _ change: @escaping (inout LocalRecording) -> Void) {
        mutate { list in
            if let i = list.firstIndex(where: { $0.id == id }) { change(&list[i]) }
        }
    }

    /// Forget a recording and delete its audio — only once the server has it.
    func remove(_ id: String) {
        mutate { $0.removeAll { $0.id == id } }
        try? FileManager.default.removeItem(at: root.appendingPathComponent(id, isDirectory: true))
    }

    /// Every change happens on the main thread, in order; a caller on another
    /// thread is queued there.
    private func mutate(_ change: @escaping (inout [LocalRecording]) -> Void) {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { self.mutate(change) }
            return
        }
        var list = items
        change(&list)
        items = list
        if let data = try? JSONEncoder().encode(list) { try? data.write(to: manifest, options: .atomic) }
    }
}
