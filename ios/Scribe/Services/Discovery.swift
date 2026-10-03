import Foundation
import Network

/// A Scribe server found on the local network.
struct DiscoveredServer: Identifiable, Hashable {
    /// Bonjour instance name — the server's machine name.
    let name: String
    /// The URL the server wants to be reached on: its tailnet address.
    let url: String
    let version: String?
    /// `tailnet` when the server admits this phone by its tailnet login and no
    /// device key is needed; `token` when a key is still required.
    let auth: String?
    var id: String { name }
    var needsKey: Bool { auth != "tailnet" }
}

enum DiscoveryError: LocalizedError {
    case failed(String)
    var errorDescription: String? {
        switch self {
        case .failed(let why):
            return "Could not search the local network (\(why)). Allow Scribe to find devices on your network in Settings → Privacy → Local Network."
        }
    }
}

/// Finds servers advertising `_scribe._tcp`.
///
/// Only the TXT record is read: it carries the URL the server wants to be
/// reached on, its tailnet address, so nothing here resolves or connects.
/// Bonjour is the beacon, the tailnet the transport. `NWBrowser` hands TXT
/// records back with the browse results, so no separate resolve step is needed.
enum Discovery {
    static func browse(for seconds: Double = 3) async throws -> [DiscoveredServer] {
        try await withCheckedThrowingContinuation { cont in
            let queue = DispatchQueue(label: "com.dwmmholdings.scribe.discovery")
            let params = NWParameters()
            params.includePeerToPeer = false
            let browser = NWBrowser(for: .bonjourWithTXTRecord(type: "_scribe._tcp", domain: nil), using: params)
            var found: [String: DiscoveredServer] = [:]
            var finished = false

            func finish(_ result: Result<[DiscoveredServer], Error>) {
                guard !finished else { return }
                finished = true
                browser.cancel()
                cont.resume(with: result)
            }

            browser.browseResultsChangedHandler = { results, _ in
                for result in results {
                    guard case let .service(name, _, _, _) = result.endpoint,
                          case let .bonjour(txt) = result.metadata,
                          let url = txt["url"] else { continue }
                    found[name] = DiscoveredServer(name: name, url: url, version: txt["version"], auth: txt["auth"])
                }
            }
            // A denied local-network permission arrives here. Report it, rather
            // than an empty list that reads as "no server on this network".
            browser.stateUpdateHandler = { state in
                if case let .failed(error) = state { finish(.failure(DiscoveryError.failed("\(error)"))) }
                if case let .waiting(error) = state { finish(.failure(DiscoveryError.failed("\(error)"))) }
            }
            browser.start(queue: queue)
            queue.asyncAfter(deadline: .now() + seconds) {
                finish(.success(found.values.sorted { $0.name < $1.name }))
            }
        }
    }
}
