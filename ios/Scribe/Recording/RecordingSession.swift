import Foundation
import Observation
import UIKit

/// The recording in progress, if any: drives the recorder, keeps the marks, and
/// hands each closed segment to the local store for upload.
@Observable
final class RecordingSession {
    static let shared = RecordingSession()

    enum State: Equatable { case idle, recording, paused, interrupted, finished(String) }

    private(set) var state: State = .idle
    private(set) var elapsedMs = 0
    private(set) var level: Float = 0
    private(set) var marks: [Int] = []
    private(set) var error: String?
    private(set) var localId: String?

    private var recorder: SegmentedRecorder?
    private var ticker: Timer?

    var isActive: Bool {
        switch state {
        case .recording, .paused, .interrupted: return true
        default: return false
        }
    }

    @MainActor
    func start(title: String?, participants: Int?) async {
        error = nil
        guard await SegmentedRecorder.requestPermission() else {
            error = "Scribe needs the microphone. Allow it in Settings → Privacy → Microphone."
            return
        }
        let id = UUID().uuidString.lowercased()
        let local = LocalRecording(id: id, title: title?.isEmpty == false ? title : nil,
                                   participants: participants, createdAt: Date())
        LocalRecordings.shared.add(local)

        let rec = SegmentedRecorder(directory: LocalRecordings.shared.directory(for: id),
                                    bitRate: Settings.shared.audioQuality.bitRate)
        rec.onSegment = { seg in
            LocalRecordings.shared.update(id) {
                $0.segments.append(.init(seq: seg.seq, file: seg.url.lastPathComponent,
                                         startMs: seg.startMs, durationMs: seg.durationMs))
            }
            DispatchQueue.main.async { UploadQueue.shared.kick() }
        }
        rec.onLevel = { [weak self] in self?.level = $0 }
        rec.onInterruption = { [weak self] resumed in
            guard let self else { return }
            if resumed { if self.state == .interrupted { self.state = .recording } }
            else if self.state == .recording { self.state = .interrupted }
        }
        do {
            try rec.start()
        } catch {
            self.error = error.localizedDescription
            LocalRecordings.shared.remove(id)
            return
        }
        recorder = rec
        localId = id
        marks = []
        elapsedMs = 0
        state = .recording
        UIApplication.shared.isIdleTimerDisabled = true
        ticker = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            guard let self, let r = self.recorder else { return }
            self.elapsedMs = r.elapsedMs
        }
    }

    func pause() {
        guard state == .recording else { return }
        recorder?.pause()
        state = .paused
    }

    func resume() {
        guard state == .paused || state == .interrupted else { return }
        recorder?.resume()
        state = .recording
    }

    /// Bookmark this moment, in recorded time.
    func mark() {
        guard isActive, let r = recorder else { return }
        let at = r.elapsedMs
        marks.append(at)
        if let id = localId { LocalRecordings.shared.update(id) { $0.marks.append(at) } }
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
    }

    @MainActor
    func stop() {
        guard isActive, let r = recorder, let id = localId else { return }
        ticker?.invalidate()
        ticker = nil
        let total = r.stop()
        recorder = nil
        elapsedMs = total
        level = 0
        UIApplication.shared.isIdleTimerDisabled = false
        // The last segment's save was queued onto main from the writer queue
        // while stop() ran; queue `finished` behind it, so the upload queue can
        // never complete a recording that is missing its tail.
        DispatchQueue.main.async {
            LocalRecordings.shared.update(id) {
                $0.finished = true
                $0.durationMs = total
            }
            UploadQueue.shared.kick()
        }
        state = .finished(id)
    }

    func reset() {
        guard !isActive else { return }
        state = .idle
        elapsedMs = 0
        marks = []
        localId = nil
    }
}
