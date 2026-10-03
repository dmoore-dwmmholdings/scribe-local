import AVFoundation
import Observation

/// Streams a recording's audio from the server and tracks which transcript
/// line and word are being spoken.
///
/// `activeLine` and `activeToken` change only when the spoken word changes, so
/// a view that reads them redraws a few times a second rather than on every
/// clock tick.
@Observable
final class Player {
    static let rates: [Float] = [1, 1.25, 1.5, 2]

    private(set) var isPlaying = false
    private(set) var isReady = false
    private(set) var currentMs = 0
    private(set) var durationMs = 0
    private(set) var rate: Float = 1
    private(set) var error: String?
    private(set) var activeLine: Int?
    private(set) var activeToken: Int?

    private var player: AVPlayer?
    private var timeObserver: Any?
    private var statusObservation: NSKeyValueObservation?
    private var endObserver: NSObjectProtocol?
    private var timeline: [Karaoke.Line] = []
    private var tokensById: [Int: [Karaoke.Token]] = [:]
    private var loadedId: String?

    func setTranscript(_ utterances: [Utterance]) {
        timeline = Karaoke.timeline(utterances)
        tokensById = Dictionary(timeline.map { ($0.id, $0.tokens) }, uniquingKeysWith: { a, _ in a })
        updateActive()
    }

    /// Point the player at a recording's audio. Nothing streams until play.
    func load(recordingId: String, durationMs hint: Int?) {
        guard loadedId != recordingId else { return }
        teardown()
        loadedId = recordingId
        if let hint { durationMs = hint }
        guard let url = try? APIClient.shared.audioURL(recordingId: recordingId) else {
            error = "No server is set up."
            return
        }
        var headers: [String: String] = [:]
        if let auth = APIClient.shared.authorizationHeader { headers["Authorization"] = auth }
        // The one way to send a header with AVPlayer's own HTTP loading.
        let asset = AVURLAsset(url: url, options: ["AVURLAssetHTTPHeaderFieldsKey": headers])
        let item = AVPlayerItem(asset: asset)
        let p = AVPlayer(playerItem: item)
        p.automaticallyWaitsToMinimizeStalling = true
        player = p

        statusObservation = item.observe(\.status, options: [.new]) { [weak self] item, _ in
            DispatchQueue.main.async {
                guard let self else { return }
                switch item.status {
                case .readyToPlay:
                    self.isReady = true
                    let d = item.duration.seconds
                    if d.isFinite, d > 0 { self.durationMs = Int(d * 1000) }
                case .failed:
                    self.error = "The audio could not be loaded. It exists once processing has prepared it."
                default: break
                }
            }
        }
        timeObserver = p.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 15), queue: .main) { [weak self] t in
            guard let self else { return }
            self.currentMs = Int(t.seconds * 1000)
            self.updateActive()
        }
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main
        ) { [weak self] _ in
            self?.isPlaying = false
        }
    }

    func toggle() { isPlaying ? pause() : play() }

    func play() {
        guard let player else { return }
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .spokenAudio)
        try? AVAudioSession.sharedInstance().setActive(true)
        player.playImmediately(atRate: rate)
        isPlaying = true
    }

    func pause() {
        player?.pause()
        isPlaying = false
    }

    func seek(toMs ms: Int, play startPlaying: Bool = false) {
        let clamped = max(0, durationMs > 0 ? min(ms, durationMs) : ms)
        currentMs = clamped
        updateActive()
        player?.seek(to: CMTime(value: CMTimeValue(clamped), timescale: 1000),
                     toleranceBefore: .zero, toleranceAfter: CMTime(value: 1, timescale: 10))
        if startPlaying { play() }
    }

    func skip(by ms: Int) { seek(toMs: currentMs + ms) }

    func cycleRate() {
        let i = Player.rates.firstIndex(of: rate) ?? 0
        rate = Player.rates[(i + 1) % Player.rates.count]
        if isPlaying { player?.rate = rate }
    }

    func teardown() {
        if let timeObserver { player?.removeTimeObserver(timeObserver) }
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        statusObservation = nil
        timeObserver = nil
        endObserver = nil
        player?.pause()
        player = nil
        isPlaying = false
        isReady = false
        loadedId = nil
    }

    private func updateActive() {
        let li = Karaoke.lastStarted(timeline, before: currentMs) { $0.startMs }
        var line: Int?
        var token: Int?
        if li >= 0 {
            let l = timeline[li]
            if currentMs <= l.endMs + Karaoke.lingerMs {
                line = l.id
                token = Karaoke.activeToken(l.tokens, at: currentMs)
            }
        }
        if line != activeLine { activeLine = line }
        if token != activeToken { activeToken = token }
    }

    func tokens(forLine id: Int) -> [Karaoke.Token] {
        tokensById[id] ?? []
    }

    deinit { teardown() }
}
