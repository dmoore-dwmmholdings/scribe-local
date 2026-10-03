import Foundation
import Network
import Observation
import UIKit

/// Sends recordings on this phone to the server: creates the server recording,
/// uploads each segment as it closes, and completes the recording once it has
/// stopped and every segment is up. Only then is the phone's copy deleted.
///
/// One uploader works through the recordings oldest first. A failure backs off
/// (2 s doubling to 5 min) and tries again; unlike the old app it never gives up
/// on a segment, because a recording that stops retrying is a recording lost.
/// It wakes when a segment closes, when the app comes to the front, and when the
/// network comes back.
@Observable
final class UploadQueue {
    static let shared = UploadQueue()

    private(set) var running = false
    /// Why the queue is not making progress, when it is not.
    private(set) var blocked: String?

    private var task: Task<Void, Never>?
    private var backoff: Duration = .seconds(2)
    private let monitor = NWPathMonitor()
    private var wake: CheckedContinuation<Void, Never>?

    private init() {
        monitor.pathUpdateHandler = { [weak self] path in
            if path.status == .satisfied { DispatchQueue.main.async { self?.kick() } }
        }
        monitor.start(queue: DispatchQueue(label: "com.dwmmholdings.scribe.netpath"))
        NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            self?.kick()
        }
    }

    /// Something may be ready to send. Starts the uploader, or wakes it from a
    /// back-off so it tries again now.
    @MainActor
    func kick() {
        backoff = .seconds(2)
        if let wake {
            self.wake = nil
            wake.resume()
        }
        guard task == nil else { return }
        task = Task { @MainActor in
            running = true
            await drain()
            running = false
            task = nil
        }
    }

    @MainActor
    private func drain() async {
        // Keep going for a while if the app is sent to the background mid-upload.
        let bg = UIApplication.shared.beginBackgroundTask(withName: "scribe-upload")
        defer { UIApplication.shared.endBackgroundTask(bg) }

        while let next = nextWork() {
            guard Settings.shared.isConfigured else {
                blocked = "No server is set up. Recordings are kept on this phone until there is one."
                return
            }
            do {
                try await process(next)
                blocked = nil
                backoff = .seconds(2)
            } catch let e as APIError where e.isUnauthorized {
                // Retrying will not help until the key or tailnet login changes.
                blocked = "The server rejected this phone. Check the connection in Settings."
                LocalRecordings.shared.update(next) { $0.lastError = self.blocked }
                return
            } catch is CancellationError {
                return
            } catch {
                blocked = "Waiting to upload: \(error.localizedDescription)"
                LocalRecordings.shared.update(next) { $0.lastError = error.localizedDescription }
                await sleep(backoff)
                backoff = min(backoff * 2, .seconds(300))
            }
        }
    }

    /// The oldest recording with something left to do.
    @MainActor
    private func nextWork() -> String? {
        LocalRecordings.shared.items
            .sorted { $0.createdAt < $1.createdAt }
            .first { $0.pendingSegments > 0 || ($0.finished && !$0.completed) || $0.serverId == nil }?
            .id
    }

    @MainActor
    private func process(_ id: String) async throws {
        guard var rec = LocalRecordings.shared.get(id) else { return }
        let store = LocalRecordings.shared

        // Stopped before any audio was captured: there is nothing to send, and
        // the server would refuse to complete it, so this would retry forever.
        if rec.finished && rec.segments.isEmpty {
            if let sid = rec.serverId { try? await APIClient.shared.deleteRecording(sid) }
            store.remove(id)
            return
        }

        if rec.serverId == nil {
            let created = try await APIClient.shared.createRecording(CreateRecordingRequest(
                title: rec.title, participantsExpected: rec.participants,
                deviceId: Settings.shared.deviceId, audioFormat: "m4a", sampleRate: 16_000))
            store.update(id) { $0.serverId = created.id }
            rec.serverId = created.id
        }
        guard let serverId = rec.serverId else { return }

        for seg in rec.segments.sorted(by: { $0.seq < $1.seq }) where !seg.uploaded {
            let url = store.fileURL(rec, seg)
            if FileManager.default.fileExists(atPath: url.path) {
                _ = try await APIClient.shared.uploadSegment(
                    recordingId: serverId, seq: seg.seq, file: url,
                    startMs: seg.startMs, durationMs: seg.durationMs)
            }
            // A file that is gone cannot be sent again; record it as done so the
            // rest of the recording is not held hostage by it.
            store.update(id) { r in
                if let i = r.segments.firstIndex(where: { $0.seq == seg.seq }) { r.segments[i].uploaded = true }
                r.lastError = nil
            }
            try? FileManager.default.removeItem(at: url)
            RecordingSession.shared.segmentUploaded(localId: id)
        }

        guard let now = store.get(id), now.finished, now.pendingSegments == 0, !now.completed else { return }
        do {
            _ = try await APIClient.shared.complete(serverId, durationMs: now.durationMs,
                                                    marks: now.marks.isEmpty ? nil : now.marks)
        } catch let e as APIError where e.status == 409 {
            // Already complete — a previous attempt got through before the reply was lost.
        }
        store.update(id) { $0.completed = true }
        store.remove(id)
        await LibraryStore.shared.refresh()
    }

    @MainActor
    private func sleep(_ d: Duration) async {
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            wake = c
            Task { @MainActor in
                try? await Task.sleep(for: d)
                if let w = self.wake { self.wake = nil; w.resume() }
            }
        }
    }
}
