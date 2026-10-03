import AVFoundation

/// Records 16 kHz mono AAC in fixed-length segments, so each closed segment can
/// upload while recording continues and a crash loses at most one.
///
/// Built on an `AVAudioEngine` input tap rather than `AVAudioRecorder`. The old
/// app rotated segments by stopping one recorder and starting the next, which
/// drops a sliver of audio at every boundary and once re-activated the audio
/// session each time. Here the tap never stops: rotation swaps the file the
/// tap's buffers are written to, on the writer queue, so segments butt up
/// exactly. Durations come from frames written, not from a clock.
final class SegmentedRecorder {
    struct Segment {
        let seq: Int
        let url: URL
        let startMs: Int
        let durationMs: Int
    }

    static let sampleRate: Double = 16_000

    /// A closed segment, ready to upload. Called on the writer queue.
    var onSegment: ((Segment) -> Void)?
    /// Input level 0…1, about ten times a second. Called on the main queue.
    var onLevel: ((Float) -> Void)?
    /// An interruption began (false) or recording resumed after one (true).
    var onInterruption: ((Bool) -> Void)?
    /// A segment file could not be opened: audio is not being kept. Main queue.
    var onError: ((String) -> Void)?

    private let engine = AVAudioEngine()
    private let writer = DispatchQueue(label: "com.dwmmholdings.scribe.recorder")
    private let directory: URL
    private let segmentFrames: AVAudioFramePosition
    private let bitRate: Int

    private var converter: AVAudioConverter?
    private var outFormat: AVAudioFormat!
    private var file: AVAudioFile?
    private var fileStartFrame: AVAudioFramePosition = 0
    private var framesWritten: AVAudioFramePosition = 0
    private var seq = 0
    private var paused = false
    private var running = false
    private var lastLevel = Date.distantPast
    private var observers: [NSObjectProtocol] = []

    /// Elapsed recorded audio, excluding pauses — what marks are measured in.
    var elapsedMs: Int { writer.sync { Int(Double(framesWritten) / Self.sampleRate * 1000) } }

    init(directory: URL, segmentSeconds: Double = 30, bitRate: Int) {
        self.directory = directory
        self.segmentFrames = AVAudioFramePosition(segmentSeconds * Self.sampleRate)
        self.bitRate = bitRate
    }

    static func requestPermission() async -> Bool {
        await AVAudioApplication.requestRecordPermission()
    }

    func start() throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .default,
                                options: [.allowBluetoothHFP, .defaultToSpeaker, .mixWithOthers])
        try session.setActive(true)
        observe()
        // Open the first file before the engine runs: if it cannot be created
        // there is no point recording, and saying so beats a timer at 0:00.
        let opened = writer.sync { openNextFile() }
        guard opened else {
            throw NSError(domain: "Scribe", code: 2, userInfo: [NSLocalizedDescriptionKey:
                "The recording file could not be created, so nothing would be kept. Try a lower audio quality in Settings."])
        }
        try startEngine()
        running = true
    }

    func pause() {
        writer.sync { paused = true }
    }

    func resume() {
        writer.sync { paused = false }
        if !engine.isRunning { try? startEngine() }
    }

    /// Stop and close the last segment. Returns the total recorded duration.
    @discardableResult
    func stop() -> Int {
        running = false
        #if DEBUG
        feedTimer?.cancel()
        feedTimer = nil
        #endif
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        observers.forEach { NotificationCenter.default.removeObserver($0) }
        observers.removeAll()
        let total = writer.sync { () -> Int in
            closeFile()
            return Int(Double(framesWritten) / Self.sampleRate * 1000)
        }
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        return total
    }

    // MARK: Engine

    private func startEngine() throws {
        #if DEBUG
        // UI tests feed a file instead of the microphone (the simulator's input
        // is often silent): same conversion, segments and upload from there on.
        if let path = ProcessInfo.processInfo.environment["SCRIBE_TEST_AUDIO_FILE"] {
            try startFileFeed(path)
            return
        }
        #endif
        let input = engine.inputNode
        let inFormat = input.outputFormat(forBus: 0)
        guard inFormat.sampleRate > 0 else {
            throw NSError(domain: "Scribe", code: 1, userInfo: [NSLocalizedDescriptionKey: "No microphone input is available."])
        }
        outFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Self.sampleRate, channels: 1, interleaved: false)
        converter = AVAudioConverter(from: inFormat, to: outFormat)
        input.removeTap(onBus: 0)
        input.installTap(onBus: 0, bufferSize: 4096, format: inFormat) { [weak self] buffer, _ in
            self?.handle(buffer)
        }
        engine.prepare()
        try engine.start()
    }

    #if DEBUG
    private var feedTimer: DispatchSourceTimer?

    private func startFileFeed(_ path: String) throws {
        let file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        outFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Self.sampleRate, channels: 1, interleaved: false)
        converter = AVAudioConverter(from: file.processingFormat, to: outFormat)
        let chunk: AVAudioFrameCount = 4096
        let interval = Double(chunk) / file.processingFormat.sampleRate
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "com.dwmmholdings.scribe.feed"))
        t.schedule(deadline: .now(), repeating: interval)
        t.setEventHandler { [weak self] in
            guard let self, let buf = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: chunk) else { return }
            if file.framePosition >= file.length { file.framePosition = 0 }
            try? file.read(into: buf, frameCount: chunk)
            self.handle(buf)
        }
        t.resume()
        feedTimer = t
    }
    #endif

    private func handle(_ input: AVAudioPCMBuffer) {
        guard let converter, let outFormat else { return }
        reportLevel(input)
        let ratio = outFormat.sampleRate / input.format.sampleRate
        let capacity = AVAudioFrameCount(Double(input.frameLength) * ratio) + 32
        guard let out = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }
        var fed = false
        var err: NSError?
        converter.convert(to: out, error: &err) { _, status in
            if fed { status.pointee = .noDataNow; return nil }
            fed = true
            status.pointee = .haveData
            return input
        }
        guard err == nil, out.frameLength > 0 else { return }
        writer.async { [weak self] in self?.write(out) }
    }

    private func reportLevel(_ buffer: AVAudioPCMBuffer) {
        let now = Date()
        guard now.timeIntervalSince(lastLevel) > 0.1, let data = buffer.floatChannelData?[0] else { return }
        lastLevel = now
        let n = Int(buffer.frameLength)
        var sum: Float = 0
        for i in 0..<n { sum += data[i] * data[i] }
        let rms = sqrt(sum / Float(max(1, n)))
        // -50 dB … 0 dB onto 0 … 1.
        let level = max(0, min(1, (20 * log10(max(rms, 1e-6)) + 50) / 50))
        DispatchQueue.main.async { self.onLevel?(level) }
    }

    // MARK: Files (writer queue)

    private func write(_ buffer: AVAudioPCMBuffer) {
        guard !paused, let file else { return }
        do {
            try file.write(from: buffer)
            framesWritten += AVAudioFramePosition(buffer.frameLength)
            if framesWritten - fileStartFrame >= segmentFrames {
                closeFile()
                openNextFile()
            }
        } catch {
            // A failed write loses this buffer, not the recording.
        }
    }

    /// Open the next segment file. Falls back to lower bitrates if the asked-for
    /// one is refused, and reports when none work rather than dropping audio.
    @discardableResult
    private func openNextFile() -> Bool {
        let url = directory.appendingPathComponent(String(format: "seg-%04d.m4a", seq))
        fileStartFrame = framesWritten
        for rate in [bitRate, 32_000, 24_000] where rate <= bitRate || rate == bitRate {
            let settings: [String: Any] = [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: Self.sampleRate,
                AVNumberOfChannelsKey: 1,
                AVEncoderBitRateKey: rate,
            ]
            if let f = try? AVAudioFile(forWriting: url, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false) {
                file = f
                return true
            }
        }
        file = nil
        DispatchQueue.main.async { self.onError?("Audio could not be saved — the recording file would not open.") }
        return false
    }

    private func closeFile() {
        guard let f = file else { return }
        let url = f.url
        let frames = framesWritten - fileStartFrame
        file = nil // releasing the AVAudioFile finalises the m4a
        guard frames > 0 else {
            try? FileManager.default.removeItem(at: url)
            return
        }
        let seg = Segment(seq: seq,
                          url: url,
                          startMs: Int(Double(fileStartFrame) / Self.sampleRate * 1000),
                          durationMs: Int(Double(frames) / Self.sampleRate * 1000))
        seq += 1
        onSegment?(seg)
    }

    // MARK: Interruptions

    /// A call or Siri stops the engine without telling anyone; left alone, the
    /// recording appears to continue while capturing nothing. Resume when the
    /// system says so, and rebuild after a media-services reset.
    private func observe() {
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            guard let self, self.running,
                  let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            switch type {
            case .began:
                self.onInterruption?(false)
            case .ended:
                // Resume whether or not the system suggests it: a meeting
                // recording that silently stops after a call is the worse error.
                try? AVAudioSession.sharedInstance().setActive(true)
                if (try? self.startEngine()) != nil { self.onInterruption?(true) }
            @unknown default: break
            }
        })
        // A new route (AirPods connecting) changes the input format and stops
        // the engine; reinstall the tap with the new format and carry on.
        observers.append(center.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            guard let self, self.running else { return }
            try? self.startEngine()
        })
        observers.append(center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: .main) { [weak self] _ in
            guard let self, self.running else { return }
            try? AVAudioSession.sharedInstance().setCategory(.playAndRecord, mode: .default,
                                                             options: [.allowBluetoothHFP, .defaultToSpeaker, .mixWithOthers])
            try? AVAudioSession.sharedInstance().setActive(true)
            if (try? self.startEngine()) != nil { self.onInterruption?(true) }
        })
    }
}
