import Foundation
import Observation
import Security

/// App settings. Secrets (the device key and the admin update token) live in
/// the Keychain; everything else in UserDefaults.
@Observable
final class Settings {
    static let shared = Settings()

    enum AudioQuality: String, CaseIterable, Identifiable {
        case low, medium, high
        var id: String { rawValue }
        /// AAC bitrate for each choice. At 16 kHz mono the encoder accepts at
        /// most 48 kbps; 64 kbps and up make the file fail to open, and the
        /// recording then captures nothing.
        var bitRate: Int {
            switch self {
            case .low: return 24_000
            case .medium: return 32_000
            case .high: return 48_000
            }
        }
    }

    var baseURL: String { didSet { defaults.set(baseURL, forKey: Keys.baseURL) } }
    var deviceKey: String { didSet { Keychain.set(deviceKey, for: Keys.deviceKey) } }
    var updateToken: String { didSet { Keychain.set(updateToken, for: Keys.updateToken) } }
    var audioQuality: AudioQuality { didSet { defaults.set(audioQuality.rawValue, forKey: Keys.audioQuality) } }
    var defaultParticipants: Int { didSet { defaults.set(defaultParticipants, forKey: Keys.defaultParticipants) } }
    var reduceMotion: Bool { didSet { defaults.set(reduceMotion, forKey: Keys.reduceMotion) } }
    /// Stable per-install identifier, sent when creating a recording.
    let deviceId: String

    var isConfigured: Bool { !baseURL.trimmingCharacters(in: .whitespaces).isEmpty }

    private let defaults: UserDefaults

    private enum Keys {
        static let baseURL = "baseURL"
        static let deviceId = "deviceId"
        static let audioQuality = "audioQuality"
        static let defaultParticipants = "defaultParticipants"
        static let reduceMotion = "reduceMotion"
        static let deviceKey = "scribe_device_key"
        static let updateToken = "scribe_update_token"
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        baseURL = defaults.string(forKey: Keys.baseURL) ?? ""
        deviceKey = Keychain.get(Keys.deviceKey) ?? ""
        updateToken = Keychain.get(Keys.updateToken) ?? ""
        audioQuality = AudioQuality(rawValue: defaults.string(forKey: Keys.audioQuality) ?? "") ?? .medium
        let p = defaults.integer(forKey: Keys.defaultParticipants)
        defaultParticipants = p > 0 ? p : 2
        reduceMotion = defaults.bool(forKey: Keys.reduceMotion)
        #if DEBUG
        // UI tests start already connected to a local server.
        let env = ProcessInfo.processInfo.environment
        if let url = env["SCRIBE_TEST_BASE_URL"] {
            baseURL = url
            deviceKey = env["SCRIBE_TEST_KEY"] ?? ""
        }
        #endif
        if let id = defaults.string(forKey: Keys.deviceId) {
            deviceId = id
        } else {
            let id = "ios-" + UUID().uuidString.prefix(8).lowercased()
            defaults.set(id, forKey: Keys.deviceId)
            deviceId = id
        }
    }

    /// The base URL as typed, without a trailing slash.
    var normalizedBaseURL: String {
        var s = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.hasSuffix("/") { s.removeLast() }
        return s
    }
}

/// Minimal generic-password Keychain wrapper.
enum Keychain {
    private static let service = "com.dwmmholdings.scribe"

    static func get(_ account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func set(_ value: String, for account: String) {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(base as CFDictionary)
        guard !value.isEmpty else { return }
        var add = base
        add[kSecValueData as String] = Data(value.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        SecItemAdd(add as CFDictionary, nil)
    }
}
