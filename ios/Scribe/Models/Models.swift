import Foundation

// Types mirroring the server's JSON (scribe_core::types and the API's view
// models). Keys are snake_case on the wire; `APIClient` converts them, so the
// Swift names here are camelCase. UUIDs stay strings, as the server writes them.

enum RecordingStatus: String, Codable {
    case uploading, processing, ready, failed
    case unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = RecordingStatus(rawValue: raw) ?? .unknown
    }
}

struct Recording: Codable, Identifiable, Hashable {
    let id: String
    var title: String?
    let createdAt: String
    var deviceId: String?
    var durationMs: Int?
    var status: RecordingStatus
    var participantsExpected: Int?
    var audioFormat: String?
    var sampleRate: Int?
    var tags: [String]?
    var marks: [Int]?

    var createdDate: Date? { ISO8601.parse(createdAt) }
}

struct EnrolledSpeaker: Codable, Identifiable, Hashable {
    let id: String
    var displayName: String
    let createdAt: String
    var hasVoiceprint: Bool?
    var recordingCount: Int?
}

struct RecordingSpeaker: Codable, Hashable {
    let recordingId: String
    let localIdx: Int
    var speakerId: String?
    var displayName: String?
}

struct Word: Codable, Hashable {
    let w: String
    let startMs: Int
    let endMs: Int
    var conf: Double?
    var localIdx: Int?
}

struct Utterance: Codable, Identifiable, Hashable {
    let id: Int
    let recordingId: String
    var localIdx: Int?
    let startMs: Int
    let endMs: Int
    var text: String
    var words: [Word]
    var speakerName: String?
}

/// A summary list item: the server stores these as JSON and a model may write
/// either plain strings or small objects (`{"owner": …, "task": …}`). Both are
/// kept as display text.
struct SummaryItems: Codable, Hashable {
    var items: [String]

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            items = []
        } else if let values = try? c.decode([JSONValue].self) {
            items = values.map(\.displayText).filter { !$0.isEmpty }
        } else if let value = try? c.decode(JSONValue.self) {
            items = [value.displayText].filter { !$0.isEmpty }
        } else {
            items = []
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(items)
    }
}

struct Summary: Codable, Hashable {
    var title: String?
    var summary: String?
    var actionItems: SummaryItems?
    var topics: SummaryItems?
    var decisions: SummaryItems?
    var template: String?
}

struct SummaryTemplate: Codable, Identifiable, Hashable {
    let id: String
    let label: String
}

enum StageState: String, Codable {
    case pending, queued, running, done, failed
    case unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = StageState(rawValue: raw) ?? .unknown
    }
}

struct StageProgress: Codable, Hashable {
    let kind: String
    let state: StageState
    var startedAt: String?
    var finishedAt: String?
    var attempts: Int
    var error: String?
}

struct PipelineProgress: Codable, Hashable {
    let stages: [StageProgress]
    var current: String?
    let completed: Int
    let total: Int
}

struct RecordingDetail: Codable {
    var recording: Recording
    var speakers: [RecordingSpeaker]
    var utterances: [Utterance]
    /// Older servers send one summary; newer ones one per template.
    var summary: Summary?
    var summaries: [Summary]?
    var progress: PipelineProgress?

    var allSummaries: [Summary] {
        if let s = summaries, !s.isEmpty { return s }
        return summary.map { [$0] } ?? []
    }
}

struct SearchHit: Codable, Hashable {
    let recordingId: String
    var recordingTitle: String?
    var startMs: Int?
    var endMs: Int?
    let text: String
    var speaker: String?
    let score: Double
}

struct Citation: Codable, Hashable {
    let recordingId: String
    var recordingTitle: String?
    var startMs: Int?
    var endMs: Int?
    var speaker: String?
    let snippet: String
}

struct AskTurn: Codable, Hashable {
    enum Role: String, Codable { case user, assistant }
    let role: Role
    let content: String
}

struct AskFilters: Codable {
    var from: String?
    var to: String?
    var speaker: String?
    var recording: String?
}

struct AskRequest: Codable {
    let question: String
    var history: [AskTurn] = []
    var filters: AskFilters?
    var topK: Int?
}

struct AskResponse: Codable {
    let answer: String
    let citations: [Citation]
}

struct HealthResponse: Decodable {
    let status: String
    let version: String
    /// Whether the server's database answered. The server sends a boolean;
    /// accept a string too, so an older or newer server cannot break the test.
    let dbOK: Bool
    var publicBaseUrl: String?
    var auth: String?

    enum CodingKeys: String, CodingKey { case status, version, db, publicBaseUrl, auth }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        status = try c.decode(String.self, forKey: .status)
        version = try c.decode(String.self, forKey: .version)
        if let b = try? c.decode(Bool.self, forKey: .db) { dbOK = b }
        else { dbOK = ((try? c.decode(String.self, forKey: .db)) ?? "").lowercased() == "ok" }
        publicBaseUrl = try c.decodeIfPresent(String.self, forKey: .publicBaseUrl)
        auth = try c.decodeIfPresent(String.self, forKey: .auth)
    }
}

struct CreateRecordingRequest: Codable {
    var title: String?
    var participantsExpected: Int?
    var deviceId: String?
    var audioFormat: String?
    var sampleRate: Int?
}

struct CreateRecordingResponse: Codable {
    let id: String
    let status: RecordingStatus
}

struct UploadSegmentResponse: Codable {
    let seq: Int
    let bytes: Int
    let storageKey: String
}

struct CompleteRecordingRequest: Codable {
    var durationMs: Int?
    var marks: [Int]?
}

struct NameSpeakerRequest: Codable {
    var name: String?
    var speakerId: String?
    var enroll: Bool?
    var replaceVoiceprint: Bool?
}

struct NameSpeakerResponse: Codable {
    let localIdx: Int
    let speakerId: String
    let displayName: String
    var enrolled: Bool?
    var alreadyEnrolledAs: String?
}

struct StatusResponse: Codable {
    let id: String
    let status: String
}

struct TranslateResponse: Codable {
    let lang: String
    let text: String
}

// MARK: Processing schedule

struct DayWindow: Codable, Hashable {
    var enabled: Bool
    var start: Int
    var end: Int
}

struct ScheduleOverride: Codable, Hashable {
    let mode: String
    let until: String
}

struct ProcessingSchedule: Codable, Hashable {
    var enabled: Bool
    var days: [DayWindow]
    var graceMinutes: Int
    var override: ScheduleOverride?
}

struct ScheduleStatus: Codable, Hashable {
    let allowed: Bool
    let reason: String
    var nextChangeSecs: Int?
    var nextChangeAt: String?
    let serverTime: String
}

struct ScheduleBacklog: Codable, Hashable {
    let queued: Int
    let running: Int
    let failed: Int
    let recordingsWaiting: Int
}

struct ProcessingScheduleResponse: Codable {
    let schedule: ProcessingSchedule
    let status: ScheduleStatus
    let backlog: ScheduleBacklog
}

struct OverrideRequest: Codable {
    let mode: String
    var minutes: Int?
}

// MARK: Admin

struct UpdateInfoResponse: Codable {
    let version: String
    let target: String
    let updateEnabled: Bool
    let restartMode: String
    let hasBackup: Bool
}

struct RollbackResponse: Codable {
    let restoredVersion: String
    let restarting: Bool
    let restartInMs: Int
}

// MARK: Helpers

/// Any JSON value, for fields whose shape the server does not fix.
enum JSONValue: Codable, Hashable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else { self = .object(try c.decode([String: JSONValue].self)) }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .bool(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .null: try c.encodeNil()
        }
    }

    var displayText: String {
        switch self {
        case .string(let v): return v
        case .number(let v): return v.rounded() == v ? String(Int(v)) : String(v)
        case .bool(let v): return v ? "yes" : "no"
        case .null: return ""
        case .array(let v): return v.map(\.displayText).joined(separator: ", ")
        case .object(let v):
            return v.keys.sorted().compactMap { k in
                let t = v[k]!.displayText
                return t.isEmpty ? nil : "\(k): \(t)"
            }.joined(separator: "; ")
        }
    }
}

enum ISO8601 {
    private static let withFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let plain = ISO8601DateFormatter()

    static func parse(_ s: String) -> Date? {
        withFraction.date(from: s) ?? plain.date(from: s)
    }

    static func string(_ d: Date) -> String { plain.string(from: d) }
}

/// `83_000` → `1:23`; an hour or more gets an hour field.
func formatClock(ms: Int) -> String {
    let s = max(0, ms) / 1000
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60)
    return h > 0 ? String(format: "%d:%02d:%02d", h, m, sec) : String(format: "%d:%02d", m, sec)
}
