import Foundation
import Observation

/// Placeholder until the Upload queue item: recordings wait in LocalRecordings.
@Observable
final class UploadQueue {
    static let shared = UploadQueue()
    func kick() {}
}
