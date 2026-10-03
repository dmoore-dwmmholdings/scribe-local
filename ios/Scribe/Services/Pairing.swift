import Foundation

/// What a `scribe://pair?url=…&key=…` link carries: the server URL and, unless
/// the server admits this phone by its tailnet identity, a device key. The
/// installer prints this as a QR code so nobody types a 64-character key.
struct PairingPayload: Equatable, Identifiable {
    let baseURL: String
    let deviceKey: String
    var id: String { baseURL + deviceKey }

    /// Accepts `scribe://pair?…` and `scribe:///pair?…`, whichever form the QR
    /// encoder produced. Only HTTPS is accepted, except for a loopback address,
    /// so a link cannot point the app at a plaintext server on the network.
    init?(url link: URL) {
        guard link.scheme?.lowercased() == "scribe",
              let comps = URLComponents(url: link, resolvingAgainstBaseURL: false) else { return nil }
        let target = (comps.host ?? "").isEmpty ? comps.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")) : comps.host!
        guard target == "pair" else { return nil }
        let items = comps.queryItems ?? []
        guard let raw = items.first(where: { $0.name == "url" })?.value?.trimmingCharacters(in: .whitespaces),
              let server = URL(string: raw), let host = server.host else { return nil }
        let loopback = host == "localhost" || host == "127.0.0.1"
        guard server.scheme == "https" || (server.scheme == "http" && loopback) else { return nil }
        var base = raw
        while base.hasSuffix("/") { base.removeLast() }
        baseURL = base
        deviceKey = (items.first(where: { $0.name == "key" })?.value ?? "").trimmingCharacters(in: .whitespaces)
    }

    var summary: String {
        let auth = deviceKey.isEmpty
            ? "No device key — the server admits this phone by its tailnet identity."
            : "Device key ending …\(deviceKey.suffix(4))"
        return "\(baseURL)\n\n\(auth)"
    }
}
