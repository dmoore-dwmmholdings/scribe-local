import XCTest

/// Drives the app against a local Scribe server and saves a screenshot at each
/// step, so a change is seen working rather than only compiling.
///
/// Run with `ios/run-ui-tests.sh`, which starts nothing itself: it expects a
/// server at SCRIBE_TEST_BASE_URL and writes screenshots to SCREENSHOT_DIR.
final class AppFlowTests: XCTestCase {
    private var app: XCUIApplication!
    private var shots = 0

    override func setUp() {
        continueAfterFailure = true
        app = XCUIApplication()
        let env = ProcessInfo.processInfo.environment
        app.launchEnvironment["SCRIBE_TEST_BASE_URL"] = env["SCRIBE_TEST_BASE_URL"] ?? "http://127.0.0.1:8443"
        app.launchEnvironment["SCRIBE_TEST_KEY"] = env["SCRIBE_TEST_KEY"] ?? ""
        if let audio = env["SCRIBE_TEST_AUDIO_FILE"] { app.launchEnvironment["SCRIBE_TEST_AUDIO_FILE"] = audio }
        addUIInterruptionMonitor(withDescription: "permissions") { alert in
            for label in ["Allow", "OK", "Allow While Using App"] where alert.buttons[label].exists {
                alert.buttons[label].tap()
                return true
            }
            return false
        }
        app.launch()
    }

    private func snap(_ name: String) {
        shots += 1
        let shot = XCUIScreen.main.screenshot()
        let attachment = XCTAttachment(screenshot: shot)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        if let dir = ProcessInfo.processInfo.environment["SCREENSHOT_DIR"] {
            let url = URL(fileURLWithPath: dir).appendingPathComponent(String(format: "%02d-%@.png", shots, name))
            try? shot.pngRepresentation.write(to: url)
        }
    }

    private func tab(_ name: String) {
        let button = app.tabBars.buttons[name]
        if !button.waitForExistence(timeout: 5) {
            print("TABS:", app.tabBars.firstMatch.debugDescription)
        }
        button.tap()
        // The selected tab is reported through the button's value/selection.
        _ = button.wait(for: \.isSelected, toEqual: true, timeout: 3)
    }

    func test1_connection() {
        tab("Settings")
        snap("settings")
        app.buttons["Test connection"].tap()
        let ok = app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'Connected'")).firstMatch
        XCTAssertTrue(ok.waitForExistence(timeout: 15), "Test connection did not report Connected")
        snap("settings-tested")
    }

    func test2_recordUploadAndPlay() {
        tab("Record")
        snap("record-idle")
        app.buttons["Start recording"].tap()
        app.tap() // lets the interruption monitor answer the microphone prompt
        sleep(8)
        snap("record-8s")
        let timer = app.staticTexts.matching(NSPredicate(format: "label MATCHES '^[0-9]+:[0-9]{2}$'")).firstMatch
        XCTAssertTrue(timer.exists)
        XCTAssertNotEqual(timer.label, "0:00", "The recording timer did not move")
        sleep(22)
        snap("record-30s")
        sleep(20) // past one 30 s segment: it uploads, transcribes, and shows live
        snap("record-50s-live")
        app.buttons["Stop"].tap()
        sleep(2)
        snap("record-stopped")

        tab("Library")
        sleep(3)
        snap("library-after-stop")
        // Wait for the server to process it.
        var ready = false
        for _ in 0..<24 {
            sleep(5)
            if app.staticTexts["PROCESSING"].exists || app.staticTexts["UPLOADING"].exists { continue }
            if app.buttons.matching(identifier: "recording-row").count > 0 {
                ready = true
                break
            }
        }
        snap("library-processed")
        XCTAssertTrue(ready, "The recording never finished processing")

        // Open the newest recording and play it.
        app.buttons.matching(identifier: "recording-row").firstMatch.tap()
        sleep(3)
        snap("detail")
        if app.buttons["Play"].waitForExistence(timeout: 10) {
            app.buttons["Play"].tap()
            sleep(4)
            snap("detail-playing")
        } else {
            XCTFail("No Play button on the recording")
        }
    }

    /// Frames of the orb and the edge glow, idle and while recording, to judge
    /// the animation by eye.
    func test3_visuals() {
        tab("Record")
        snap("orb-idle-a")
        Thread.sleep(forTimeInterval: 1.2)
        snap("orb-idle-b")
        app.buttons["Start recording"].tap()
        app.tap()
        if app.buttons["Turn off live transcript"].waitForExistence(timeout: 3) {
            app.buttons["Turn off live transcript"].tap()
        }
        Thread.sleep(forTimeInterval: 3)
        for i in 0..<4 {
            snap("orb-recording-\(i)")
            Thread.sleep(forTimeInterval: 0.4)
        }
        app.buttons["Stop"].tap()
    }

    /// The Live Activity: the Dynamic Island from the Home Screen, and the
    /// Lock Screen presentation from the notification view.
    func test4_liveActivity() {
        tab("Record")
        app.buttons["Start recording"].tap()
        app.tap()
        Thread.sleep(forTimeInterval: 4)
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 2)
        snap("activity-island")
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        // Long-press the island to expand it.
        springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.03)).press(forDuration: 1.2)
        Thread.sleep(forTimeInterval: 1.5)
        snap("activity-island-expanded")
        XCUIDevice.shared.press(.home)
        Thread.sleep(forTimeInterval: 1)
        let top = springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.005))
        top.press(forDuration: 0.1, thenDragTo: springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.7)))
        Thread.sleep(forTimeInterval: 2)
        snap("activity-lockscreen")
        XCUIDevice.shared.press(.home)
        app.activate()
        Thread.sleep(forTimeInterval: 1)
        if app.buttons["Stop"].waitForExistence(timeout: 5) { app.buttons["Stop"].tap() }
    }
}
