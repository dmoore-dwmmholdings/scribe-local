import ExpoModulesCore
import Network

/// Finds Scribe servers on the local network.
///
/// The server advertises `_scribe._tcp` with a TXT record carrying the URL it
/// wants to be reached on — its tailnet address, not the LAN address this
/// browse resolves. So this only ever reads the TXT record; it never connects,
/// and it deliberately does not resolve endpoints. Bonjour is the beacon, the
/// tailnet is the transport.
///
/// `NWBrowser` is used rather than the older `NetServiceBrowser` because it
/// hands back TXT records with the browse results, so a separate resolve step
/// (and its extra failure mode) is not needed.
public class ScribeDiscoveryModule: Module {
  private var browser: NWBrowser?
  /// Keyed by service name so a server that is re-announced does not appear twice.
  private var found: [String: [String: String]] = [:]
  private let queue = DispatchQueue(label: "com.dwmmholdings.scribe.discovery")

  public func definition() -> ModuleDefinition {
    Name("ScribeDiscovery")

    // Browse for `timeoutMs`, then resolve with whatever was seen.
    //
    // A fixed window rather than a live-updating list: pairing is a one-shot
    // action from a button, and a promise is far simpler for the caller than a
    // subscription it has to tear down. Bonjour answers in well under a second
    // on a quiet network, so a few seconds is generous.
    AsyncFunction("discover") { (timeoutMs: Int, promise: Promise) in
      self.queue.async {
        self.stopLocked()
        self.found = [:]

        let params = NWParameters()
        // Bonjour over the local link only; no need to touch the tailnet here.
        params.includePeerToPeer = false

        let browser = NWBrowser(
          for: .bonjourWithTXTRecord(type: "_scribe._tcp", domain: nil),
          using: params
        )
        self.browser = browser

        browser.browseResultsChangedHandler = { [weak self] results, _ in
          guard let self else { return }
          for result in results {
            guard case let .service(name, _, _, _) = result.endpoint else { continue }
            guard case let .bonjour(txt) = result.metadata else {
              // A server with no TXT record cannot tell us its URL, so there is
              // nothing actionable to show the user.
              continue
            }
            var entry: [String: String] = ["name": name]
            for key in ["url", "version", "auth"] {
              if let value = txt[key] { entry[key] = value }
            }
            // Without a URL the entry is unusable — skip rather than surface a
            // row that cannot be tapped.
            if entry["url"] != nil {
              self.found[name] = entry
            }
          }
        }

        browser.stateUpdateHandler = { [weak self] state in
          // A denied local-network permission surfaces here. Fail loudly: the
          // alternative is an empty list that looks like "no servers found",
          // which sends the user hunting for a problem on the server.
          if case let .failed(error) = state {
            guard let self else { return }
            self.queue.async {
              self.stopLocked()
              promise.reject("E_BROWSE_FAILED", "Bonjour browse failed: \(error)")
            }
          }
        }

        browser.start(queue: self.queue)

        self.queue.asyncAfter(deadline: .now() + .milliseconds(timeoutMs)) { [weak self] in
          guard let self, self.browser === browser else { return }
          let results = Array(self.found.values)
          self.stopLocked()
          promise.resolve(results)
        }
      }
    }

    // Stop an in-flight browse (e.g. the user left the screen).
    Function("stop") {
      self.queue.async { self.stopLocked() }
    }

    OnDestroy {
      self.queue.sync { self.stopLocked() }
    }
  }

  /// Cancel and clear the browser. Must be called on `queue`.
  private func stopLocked() {
    browser?.cancel()
    browser = nil
  }
}
