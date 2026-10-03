import Foundation
import Observation

/// One recording's detail: loads it, polls while the server is still working
/// on it, and carries out the actions the detail screen offers.
@Observable
final class RecordingDetailModel {
    let recordingId: String
    private(set) var detail: RecordingDetail?
    private(set) var templates: [SummaryTemplate] = []
    private(set) var error: String?
    private(set) var busy = false
    /// The summary view on screen, by template id.
    var selectedTemplate: String?

    private var pollTask: Task<Void, Never>?

    init(recordingId: String) { self.recordingId = recordingId }

    var recording: Recording? { detail?.recording }
    var speakers: [RecordingSpeaker] { detail?.speakers ?? [] }
    var utterances: [Utterance] { detail?.utterances ?? [] }
    var summaries: [Summary] { detail?.allSummaries ?? [] }

    var activeSummary: Summary? {
        let all = summaries
        if let t = selectedTemplate, let s = all.first(where: { ($0.template ?? "general") == t }) { return s }
        return all.first
    }

    var isWorking: Bool {
        guard let s = recording?.status else { return false }
        return s == .processing || s == .uploading
    }

    /// The name a transcript line is shown under: the line's own resolved
    /// name, then the recording speaker's, then "Speaker N".
    func speakerName(_ localIdx: Int?) -> String {
        guard let i = localIdx else { return "Unknown" }
        if let name = speakers.first(where: { $0.localIdx == i })?.displayName, !name.isEmpty { return name }
        return "Speaker \(i)"
    }

    func name(for u: Utterance) -> String {
        if let n = u.speakerName, !n.isEmpty { return n }
        return speakerName(u.localIdx)
    }

    /// Speaking time per speaker, from utterance spans, largest first.
    var talkTime: [(localIdx: Int?, name: String, ms: Int, share: Double)] {
        var totals: [Int?: Int] = [:]
        for u in utterances { totals[u.localIdx, default: 0] += max(0, u.endMs - u.startMs) }
        let sum = max(1, totals.values.reduce(0, +))
        return totals.map { (k, v) in (k, speakerName(k), v, Double(v) / Double(sum)) }
            .sorted { $0.ms > $1.ms }
    }

    // MARK: Loading

    @MainActor
    func load(quiet: Bool = false) async {
        do {
            let d = try await APIClient.shared.recording(recordingId)
            detail = d
            error = nil
            LibraryStore.shared.upsert(d.recording)
            schedulePoll()
        } catch is CancellationError {
        } catch {
            if !quiet || detail == nil { self.error = error.localizedDescription }
        }
        if templates.isEmpty { templates = (try? await APIClient.shared.summaryTemplates()) ?? [] }
    }

    /// Re-fetch every few seconds while the server is still processing, and
    /// stop once it is not — a ready recording does not change on its own.
    @MainActor
    private func schedulePoll() {
        pollTask?.cancel()
        guard isWorking else { return }
        pollTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(4))
            guard !Task.isCancelled else { return }
            await self?.load(quiet: true)
        }
    }

    func stopPolling() { pollTask?.cancel() }

    // MARK: Actions

    @MainActor
    private func run(_ work: () async throws -> Void) async -> String? {
        busy = true
        defer { busy = false }
        do {
            try await work()
            await load(quiet: true)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    @MainActor func reprocess() async -> String? {
        await run { _ = try await APIClient.shared.reprocess(recordingId) }
    }

    @MainActor func rediarize() async -> String? {
        await run { _ = try await APIClient.shared.rediarize(recordingId) }
    }

    /// State the speaker count and, when asked, redo speaker detection with it —
    /// the words are kept, so this is quick next to a full reprocess.
    @MainActor func setParticipants(_ count: Int?, redo: Bool) async -> String? {
        await run {
            try await APIClient.shared.setParticipants(recordingId, count: count)
            if redo { _ = try await APIClient.shared.rediarize(recordingId) }
        }
    }

    @MainActor func summarize(template: String) async -> String? {
        selectedTemplate = template
        return await run {
            try await APIClient.shared.resummarize(recordingId, template: template)
            // The summary is written by the worker; poll until it shows up.
            if var d = detail { d.recording.status = .processing; detail = d }
        }
    }

    @MainActor func setTags(_ tags: [String]) async -> String? {
        await run { _ = try await APIClient.shared.setTags(recordingId, tags: tags) }
    }

    @MainActor func edit(_ u: Utterance, text: String) async -> String? {
        await run { _ = try await APIClient.shared.editUtterance(recordingId: recordingId, utteranceId: u.id, text: text) }
    }
}
