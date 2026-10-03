import Foundation
import SwiftUI
import UIKit

/// Turns a recording into a file to share: Markdown, plain text, SRT
/// subtitles, or the full audio.
enum Exporter {
    enum Format: String, CaseIterable, Identifiable {
        case markdown, text, srt
        var id: String { rawValue }
        var label: String {
            switch self {
            case .markdown: return "Markdown"
            case .text: return "Plain text"
            case .srt: return "Subtitles (SRT)"
            }
        }
        var ext: String {
            switch self {
            case .markdown: return "md"
            case .text: return "txt"
            case .srt: return "srt"
            }
        }
    }

    /// Exports live in one directory that is cleared before each new one, not
    /// after sharing: a share target may still be reading the file when the
    /// sheet closes.
    private static var dir: URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("exports", isDirectory: true)
        return d
    }

    private static func freshDirectory() throws -> URL {
        try? FileManager.default.removeItem(at: dir)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static func write(_ d: RecordingDetail, as format: Format, names: (Utterance) -> String) throws -> URL {
        let body: String
        switch format {
        case .markdown: body = markdown(d, names)
        case .text: body = text(d, names)
        case .srt: body = srt(d, names)
        }
        let url = try freshDirectory().appendingPathComponent("\(slug(title(d))).\(format.ext)")
        try body.write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    /// Download the server's full-length WAV — the file every pipeline stage
    /// after transcode works from — reporting whole percentages.
    static func downloadAudio(_ d: RecordingDetail, progress: @escaping (Int) -> Void) async throws -> URL {
        var req = URLRequest(url: try APIClient.shared.audioURL(recordingId: d.recording.id))
        if let auth = APIClient.shared.authorizationHeader { req.setValue(auth, forHTTPHeaderField: "Authorization") }
        let delegate = ProgressDelegate(progress)
        let (tmp, resp) = try await URLSession.shared.download(for: req, delegate: delegate)
        let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else {
            try? FileManager.default.removeItem(at: tmp)
            throw APIError(status: status,
                           message: status == 404 ? "The audio is not ready yet. It exists once processing has prepared it." : "download failed",
                           path: "/audio")
        }
        let date = String(d.recording.createdAt.prefix(10))
        let url = try freshDirectory().appendingPathComponent("\(slug(title(d)))-\(date)-\(d.recording.id.prefix(8)).wav")
        try FileManager.default.moveItem(at: tmp, to: url)
        return url
    }

    // MARK: Formatters (ported from export.ts)

    static func title(_ d: RecordingDetail) -> String {
        d.allSummaries.first?.title ?? d.recording.title ?? "Untitled recording"
    }

    private static func meta(_ d: RecordingDetail, sep: String) -> String {
        var parts: [String] = []
        if let date = d.recording.createdDate { parts.append(date.formatted(date: .abbreviated, time: .shortened)) }
        if let ms = d.recording.durationMs { parts.append(formatClock(ms: ms)) }
        return parts.joined(separator: sep)
    }

    static func markdown(_ d: RecordingDetail, _ name: (Utterance) -> String) -> String {
        var l = ["# \(title(d))", "", "_\(meta(d, sep: " · "))_", ""]
        let s = d.allSummaries.first
        if let t = s?.summary, !t.isEmpty { l += ["## Summary", "", t, ""] }
        if let a = s?.actionItems?.items, !a.isEmpty { l += ["## Action items", ""] + a.map { "- [ ] \($0)" } + [""] }
        if let x = s?.decisions?.items, !x.isEmpty { l += ["## Decisions", ""] + x.map { "- \($0)" } + [""] }
        if let t = s?.topics?.items, !t.isEmpty { l += ["## Topics", "", t.map { "`\($0)`" }.joined(separator: " "), ""] }
        l += ["## Transcript", ""]
        for u in d.utterances {
            l += ["**\(name(u))** _(\(formatClock(ms: u.startMs)))_", u.text.trimmingCharacters(in: .whitespaces), ""]
        }
        return l.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines) + "\n"
    }

    static func text(_ d: RecordingDetail, _ name: (Utterance) -> String) -> String {
        var l = [title(d), meta(d, sep: "  ·  "), ""]
        let s = d.allSummaries.first
        if let t = s?.summary, !t.isEmpty { l += ["SUMMARY", t, ""] }
        if let a = s?.actionItems?.items, !a.isEmpty { l += ["ACTION ITEMS"] + a.map { "  - \($0)" } + [""] }
        if let x = s?.decisions?.items, !x.isEmpty { l += ["DECISIONS"] + x.map { "  - \($0)" } + [""] }
        l += ["TRANSCRIPT", ""]
        for u in d.utterances { l.append("[\(formatClock(ms: u.startMs))] \(name(u)): \(u.text.trimmingCharacters(in: .whitespaces))") }
        return l.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines) + "\n"
    }

    static func srt(_ d: RecordingDetail, _ name: (Utterance) -> String) -> String {
        var cues: [String] = []
        for (i, u) in d.utterances.enumerated() {
            // A zero-length line would never be shown; give it two seconds.
            let end = u.endMs > u.startMs ? u.endMs : u.startMs + 2000
            cues += ["\(i + 1)", "\(srtTime(u.startMs)) --> \(srtTime(end))",
                     "\(name(u)): \(u.text.trimmingCharacters(in: .whitespaces))", ""]
        }
        return cues.joined(separator: "\n")
    }

    private static func srtTime(_ ms: Int) -> String {
        let c = max(0, ms)
        return String(format: "%02d:%02d:%02d,%03d", c / 3_600_000, (c % 3_600_000) / 60_000, (c % 60_000) / 1000, c % 1000)
    }

    private static func slug(_ s: String) -> String {
        let lowered = s.lowercased().map { $0.isLetter || $0.isNumber ? String($0) : "-" }.joined()
        let collapsed = lowered.split(separator: "-").joined(separator: "-")
        let out = String(collapsed.prefix(60))
        return out.isEmpty ? "recording" : out
    }
}

/// Watches a download task's progress.
private final class ProgressDelegate: NSObject, URLSessionTaskDelegate {
    private let report: (Int) -> Void
    private var observation: NSKeyValueObservation?
    private var last = -1

    init(_ report: @escaping (Int) -> Void) { self.report = report }

    func urlSession(_ session: URLSession, didCreateTask task: URLSessionTask) {
        observation = task.progress.observe(\.fractionCompleted) { [weak self] p, _ in
            guard let self else { return }
            let pct = Int(p.fractionCompleted * 100)
            if pct != self.last {
                self.last = pct
                DispatchQueue.main.async { self.report(pct) }
            }
        }
    }
}

/// The system share sheet for one file.
struct ShareSheet: UIViewControllerRepresentable {
    let url: URL
    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }
    func updateUIViewController(_ vc: UIActivityViewController, context: Context) {}
}

/// A file ready to hand to the share sheet.
struct SharedFile: Identifiable {
    let url: URL
    var id: String { url.path }
}
