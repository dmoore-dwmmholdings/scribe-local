import Foundation

/// A failed API call: the HTTP status, and the server's own message where it
/// sent one (`{"error":{"code","message"}}`).
struct APIError: LocalizedError {
    let status: Int
    let message: String
    let path: String

    var errorDescription: String? {
        switch status {
        case 401, 403: return "The server rejected this device (\(status)). Check the device key, or pair again."
        case 404: return "Not found on the server: \(message)"
        default: return "Server error \(status): \(message)"
        }
    }

    var isUnauthorized: Bool { status == 401 || status == 403 }
}

enum ClientError: LocalizedError {
    case notConfigured
    case badURL(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: return "No server is set up yet. Add one in Settings."
        case .badURL(let s): return "That server address is not a valid URL: \(s)"
        }
    }
}

/// Every call the app makes to the Scribe server.
///
/// Requests carry `Authorization: Bearer <device key>` when a key is set. With
/// an empty key the server can still admit this phone by its tailnet identity,
/// which is why an empty key is sent as no header rather than refused here.
final class APIClient {
    static let shared = APIClient()

    private let settings: Settings
    private let session: URLSession
    private let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()
    private let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        return e
    }()

    init(settings: Settings = .shared, session: URLSession = .shared) {
        self.settings = settings
        self.session = session
    }

    // MARK: URLs and headers

    func url(_ path: String, query: [URLQueryItem] = []) throws -> URL {
        guard settings.isConfigured else { throw ClientError.notConfigured }
        let base = settings.normalizedBaseURL
        guard var comps = URLComponents(string: base + path) else { throw ClientError.badURL(base) }
        if !query.isEmpty { comps.queryItems = (comps.queryItems ?? []) + query }
        guard let u = comps.url else { throw ClientError.badURL(base) }
        return u
    }

    /// The Authorization header for streams that are not made through this
    /// client (the audio player), or nil when no key is set.
    var authorizationHeader: String? {
        settings.deviceKey.isEmpty ? nil : "Bearer \(settings.deviceKey)"
    }

    func audioURL(recordingId: String) throws -> URL { try url("/recordings/\(recordingId)/audio") }

    // MARK: Core request

    private func request<T: Decodable>(
        _ method: String, _ path: String, query: [URLQueryItem] = [],
        body: (any Encodable)? = nil, token: String? = nil, as: T.Type = T.self
    ) async throws -> T {
        var req = URLRequest(url: try url(path, query: query))
        req.httpMethod = method
        let bearer = token ?? (settings.deviceKey.isEmpty ? nil : settings.deviceKey)
        if let bearer { req.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization") }
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try encoder.encode(body)
        }
        let (data, resp) = try await session.data(for: req)
        return try decode(data, resp, path: path)
    }

    private func decode<T: Decodable>(_ data: Data, _ resp: URLResponse, path: String) throws -> T {
        let status = (resp as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            throw APIError(status: status, message: Self.errorMessage(data), path: path)
        }
        if T.self == Empty.self { return Empty() as! T }
        return try decoder.decode(T.self, from: data)
    }

    private static func errorMessage(_ data: Data) -> String {
        struct Envelope: Decodable { struct E: Decodable { let message: String }; let error: E }
        if let e = try? JSONDecoder().decode(Envelope.self, from: data) { return e.error.message }
        return String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    struct Empty: Decodable {}

    // MARK: Health

    /// `GET /health` — unauthenticated, so it proves the server is reachable
    /// and nothing about the key.
    func health() async throws -> HealthResponse { try await request("GET", "/health") }

    // MARK: Recordings

    func createRecording(_ body: CreateRecordingRequest) async throws -> CreateRecordingResponse {
        try await request("POST", "/recordings", body: body)
    }

    func listRecordings(limit: Int = 50, offset: Int = 0, tag: String? = nil) async throws -> [Recording] {
        struct R: Decodable { let recordings: [Recording] }
        var q = [URLQueryItem(name: "limit", value: String(limit)), URLQueryItem(name: "offset", value: String(offset))]
        if let tag, !tag.isEmpty { q.append(URLQueryItem(name: "tag", value: tag)) }
        return try await request("GET", "/recordings", query: q, as: R.self).recordings
    }

    func recording(_ id: String) async throws -> RecordingDetail { try await request("GET", "/recordings/\(id)") }

    func deleteRecording(_ id: String) async throws {
        _ = try await request("DELETE", "/recordings/\(id)", as: Empty.self)
    }

    func complete(_ id: String, durationMs: Int?, marks: [Int]?) async throws -> StatusResponse {
        try await request("POST", "/recordings/\(id)/complete", body: CompleteRecordingRequest(durationMs: durationMs, marks: marks))
    }

    /// `PUT /recordings/{id}/segments/{seq}?ext=…` from a file on disk.
    func uploadSegment(recordingId: String, seq: Int, file: URL, ext: String = "m4a",
                       contentType: String = "audio/mp4", startMs: Int?, durationMs: Int?) async throws -> UploadSegmentResponse {
        let path = "/recordings/\(recordingId)/segments/\(seq)"
        var req = URLRequest(url: try url(path, query: [URLQueryItem(name: "ext", value: ext)]))
        req.httpMethod = "PUT"
        req.setValue(contentType, forHTTPHeaderField: "Content-Type")
        if let auth = authorizationHeader { req.setValue(auth, forHTTPHeaderField: "Authorization") }
        if let startMs { req.setValue(String(startMs), forHTTPHeaderField: "X-Segment-Start-Ms") }
        if let durationMs { req.setValue(String(durationMs), forHTTPHeaderField: "X-Segment-Duration-Ms") }
        let (data, resp) = try await session.upload(for: req, fromFile: file)
        return try decode(data, resp, path: path)
    }

    func summaryTemplates() async throws -> [SummaryTemplate] {
        struct R: Decodable { let templates: [SummaryTemplate] }
        return try await request("GET", "/summary-templates", as: R.self).templates
    }

    func resummarize(_ id: String, template: String) async throws {
        struct B: Encodable { let template: String }
        _ = try await request("POST", "/recordings/\(id)/summarize", body: B(template: template), as: Empty.self)
    }

    func setTags(_ id: String, tags: [String]) async throws -> [String] {
        struct B: Encodable { let tags: [String] }
        struct R: Decodable { let tags: [String] }
        return try await request("PUT", "/recordings/\(id)/tags", body: B(tags: tags), as: R.self).tags
    }

    func tags() async throws -> [String] {
        struct R: Decodable { let tags: [String] }
        return try await request("GET", "/tags", as: R.self).tags
    }

    /// State the speaker count; `nil` clears it.
    func setParticipants(_ id: String, count: Int?) async throws {
        struct B: Encodable {
            let participantsExpected: Int?
            func encode(to encoder: Encoder) throws {
                var c = encoder.container(keyedBy: K.self)
                try c.encode(participantsExpected, forKey: .participantsExpected) // explicit null clears
            }
            enum K: String, CodingKey { case participantsExpected }
        }
        _ = try await request("PUT", "/recordings/\(id)/participants", body: B(participantsExpected: count), as: Empty.self)
    }

    func editUtterance(recordingId: String, utteranceId: Int, text: String) async throws -> Utterance {
        struct B: Encodable { let text: String }
        return try await request("PATCH", "/recordings/\(recordingId)/utterances/\(utteranceId)", body: B(text: text))
    }

    func reprocess(_ id: String) async throws -> StatusResponse { try await request("POST", "/recordings/\(id)/reprocess") }

    /// Re-run speaker detection only, against the stored transcript.
    func rediarize(_ id: String) async throws -> StatusResponse { try await request("POST", "/recordings/\(id)/rediarize") }

    func translate(_ id: String, lang: String) async throws -> TranslateResponse {
        struct B: Encodable { let lang: String }
        return try await request("POST", "/recordings/\(id)/translate", body: B(lang: lang))
    }

    // MARK: Speakers

    func nameSpeaker(recordingId: String, localIdx: Int, _ body: NameSpeakerRequest) async throws -> NameSpeakerResponse {
        try await request("POST", "/recordings/\(recordingId)/speakers/\(localIdx)/name", body: body)
    }

    func unnameSpeaker(recordingId: String, localIdx: Int) async throws {
        _ = try await request("DELETE", "/recordings/\(recordingId)/speakers/\(localIdx)/name", as: Empty.self)
    }

    /// "Not a participant": drop this voice from the recording.
    func removeRecordingSpeaker(recordingId: String, localIdx: Int) async throws {
        _ = try await request("DELETE", "/recordings/\(recordingId)/speakers/\(localIdx)", as: Empty.self)
    }

    func speakers() async throws -> [EnrolledSpeaker] {
        struct R: Decodable { let speakers: [EnrolledSpeaker] }
        return try await request("GET", "/speakers", as: R.self).speakers
    }

    func renameSpeaker(_ id: String, name: String) async throws -> EnrolledSpeaker {
        struct B: Encodable { let name: String }
        return try await request("PATCH", "/speakers/\(id)", body: B(name: name))
    }

    func deleteSpeaker(_ id: String) async throws {
        _ = try await request("DELETE", "/speakers/\(id)", as: Empty.self)
    }

    // MARK: Search and Ask

    func search(_ q: String, from: Date? = nil, to: Date? = nil, speaker: String? = nil,
                recording: String? = nil, limit: Int? = nil) async throws -> [SearchHit] {
        struct R: Decodable { let hits: [SearchHit] }
        var items = [URLQueryItem(name: "q", value: q)]
        if let from { items.append(URLQueryItem(name: "from", value: ISO8601.string(from))) }
        if let to { items.append(URLQueryItem(name: "to", value: ISO8601.string(to))) }
        if let speaker { items.append(URLQueryItem(name: "speaker", value: speaker)) }
        if let recording { items.append(URLQueryItem(name: "recording", value: recording)) }
        if let limit { items.append(URLQueryItem(name: "limit", value: String(limit))) }
        return try await request("GET", "/search", query: items, as: R.self).hits
    }

    func ask(_ body: AskRequest) async throws -> AskResponse { try await request("POST", "/ask", body: body) }

    // MARK: Processing schedule

    func processingSchedule() async throws -> ProcessingScheduleResponse { try await request("GET", "/processing-schedule") }

    func setProcessingSchedule(_ s: ProcessingSchedule) async throws -> ProcessingScheduleResponse {
        try await request("PUT", "/processing-schedule", body: s)
    }

    func setProcessingOverride(_ o: OverrideRequest) async throws -> ProcessingScheduleResponse {
        try await request("POST", "/processing-schedule/override", body: o)
    }

    // MARK: Admin (update token, not the device key)

    func updateInfo() async throws -> UpdateInfoResponse {
        try await request("GET", "/admin/info", token: settings.updateToken)
    }

    func rollbackUpdate() async throws -> RollbackResponse {
        try await request("POST", "/admin/update/rollback", token: settings.updateToken)
    }
}
