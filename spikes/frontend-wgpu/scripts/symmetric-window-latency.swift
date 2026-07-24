#!/usr/bin/env swift

// External, renderer-neutral latency probe for macOS.
//
// The probe posts the same Ctrl+P command-palette input to a prepared native
// window and a prepared terminal-hosted window. It observes both through the
// same ScreenCaptureKit window stream and timestamps the first materially
// changed WindowServer frame. Escape restores the baseline between samples.
//
// This is key-injection -> WindowServer-captured-pixels evidence. It is
// symmetric software presentation evidence, not input-to-photon evidence.

import AppKit
import ApplicationServices
import CoreMedia
import CoreVideo
import Foundation
import Metal
import ScreenCaptureKit

private let schemaVersion = 2
private let paletteKey: CGKeyCode = 35
private let escapeKey: CGKeyCode = 53
private let controlKey: CGKeyCode = 59

// The SDK marks this targeted Accessibility API unavailable to Swift even
// though the stable C symbol remains present in ApplicationServices.
@_silgen_name("AXUIElementPostKeyboardEvent")
private func targetedAXKeyboardEvent(
    _ application: AXUIElement,
    _ keyChar: CGCharCode,
    _ virtualKey: CGKeyCode,
    _ keyDown: Bool
) -> AXError

private struct Config {
    var nativeTitle: String?
    var terminalTitle: String?
    var samples = 1_000
    var trials = 3
    var timeoutMs = 1_000
    var settleMs = 50
    var changeThreshold = 0.005
    var listWindows = false
}

private struct Frame {
    let displayTime: UInt64
    let signature: [UInt8]
}

private struct TrialResult: Codable {
    let frontend: String
    let trial: Int
    let windowID: UInt32
    let windowTitle: String
    let ownerName: String
    let ownerPID: Int32
    let displayID: UInt32?
    let displayRefreshHz: Double?
    let requestedSamples: Int
    let attempts: Int
    let sampleCount: Int
    let misses: Int
    let resetRetries: Int
    let resetFailures: Int
    let p50Ms: Double
    let p95Ms: Double
    let maxMs: Double
    let latenciesMs: [Double]
}

private struct Evidence: Codable {
    let schemaVersion: Int
    let endpoint: String
    let workload: String
    let platform: String
    let availableMetalDevices: [String]
    let sharedDisplayID: UInt32?
    let sharedDisplayRefreshHz: Double?
    let samplesPerTrial: Int
    let pairedTrials: Int
    let timeoutMs: Int
    let settleMs: Int
    let changeThreshold: Double
    let results: [TrialResult]
    let acquisitionCompleted: Bool
    let zeroMissAdmissionPassed: Bool
    let completed: Bool
    let outcome: String
    let notes: String
}

private final class FrameCollector: NSObject, SCStreamOutput, SCStreamDelegate,
    @unchecked Sendable
{
    private let condition = NSCondition()
    private var frames: [Frame] = []
    private var streamError: Error?

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of outputType: SCStreamOutputType
    ) {
        guard outputType == .screen,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(
                  sampleBuffer,
                  createIfNecessary: false
              ) as? [[SCStreamFrameInfo: Any]],
              let metadata = attachments.first,
              let rawStatus = metadata[.status] as? Int,
              SCFrameStatus(rawValue: rawStatus) == .complete,
              let displayTime = (metadata[.displayTime] as? NSNumber)?.uint64Value,
              let imageBuffer = sampleBuffer.imageBuffer
        else {
            return
        }

        let frame = Frame(
            displayTime: displayTime,
            signature: makeSignature(imageBuffer)
        )
        condition.lock()
        frames.append(frame)
        if frames.count > 32 {
            frames.removeFirst(frames.count - 32)
        }
        condition.broadcast()
        condition.unlock()
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        condition.lock()
        streamError = error
        condition.broadcast()
        condition.unlock()
    }

    func latest(after time: UInt64 = 0, timeout: TimeInterval) throws -> Frame? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let error = streamError {
                throw error
            }
            if let frame = frames.last(where: { $0.displayTime >= time }) {
                return frame
            }
            if !condition.wait(until: deadline) {
                return nil
            }
        }
    }

    func changed(
        from baseline: Frame,
        after time: UInt64,
        threshold: Double,
        timeout: TimeInterval
    ) throws -> Frame? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let error = streamError {
                throw error
            }
            if let frame = frames.first(where: {
                $0.displayTime >= time
                    && signatureDifference(baseline.signature, $0.signature) >= threshold
            }) {
                frames.removeAll(where: { $0.displayTime <= frame.displayTime })
                return frame
            }
            if !condition.wait(until: deadline) {
                return nil
            }
        }
    }

    func matching(
        reference: Frame,
        after time: UInt64,
        threshold: Double,
        timeout: TimeInterval
    ) throws -> Frame? {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while true {
            if let error = streamError {
                throw error
            }
            if let frame = frames.first(where: {
                $0.displayTime >= time
                    && signatureDifference(reference.signature, $0.signature) < threshold
            }) {
                frames.removeAll(where: { $0.displayTime <= frame.displayTime })
                return frame
            }
            if !condition.wait(until: deadline) {
                return nil
            }
        }
    }
}

private func makeSignature(_ pixelBuffer: CVPixelBuffer) -> [UInt8] {
    CVPixelBufferLockBaseAddress(pixelBuffer, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(pixelBuffer, .readOnly) }
    guard let base = CVPixelBufferGetBaseAddress(pixelBuffer) else {
        return []
    }

    let width = CVPixelBufferGetWidth(pixelBuffer)
    let height = CVPixelBufferGetHeight(pixelBuffer)
    let bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer)
    let bytes = base.assumingMemoryBound(to: UInt8.self)
    let columns = 64
    let rows = 36
    var result = [UInt8]()
    result.reserveCapacity(columns * rows)

    for row in 0..<rows {
        let y = min(height - 1, (row * height + height / (rows * 2)) / rows)
        for column in 0..<columns {
            let x = min(width - 1, (column * width + width / (columns * 2)) / columns)
            let offset = y * bytesPerRow + x * 4
            let blue = UInt16(bytes[offset])
            let green = UInt16(bytes[offset + 1])
            let red = UInt16(bytes[offset + 2])
            result.append(UInt8((red * 54 + green * 183 + blue * 19) >> 8))
        }
    }
    return result
}

private func signatureDifference(_ lhs: [UInt8], _ rhs: [UInt8]) -> Double {
    guard lhs.count == rhs.count, !lhs.isEmpty else {
        return lhs == rhs ? 0 : 1
    }
    let materiallyChanged = zip(lhs, rhs).reduce(into: 0) { count, pair in
        if abs(Int(pair.0) - Int(pair.1)) >= 12 {
            count += 1
        }
    }
    return Double(materiallyChanged) / Double(lhs.count)
}

private func parseConfig() throws -> Config {
    var config = Config()
    var arguments = Array(CommandLine.arguments.dropFirst())
    while !arguments.isEmpty {
        let argument = arguments.removeFirst()
        func value() throws -> String {
            guard !arguments.isEmpty else {
                throw ProbeError.usage("missing value for \(argument)")
            }
            return arguments.removeFirst()
        }
        switch argument {
        case "--native-title":
            config.nativeTitle = try value()
        case "--terminal-title":
            config.terminalTitle = try value()
        case "--samples":
            guard let parsed = Int(try value()), (1...100_000).contains(parsed) else {
                throw ProbeError.usage("--samples must be in 1...100000")
            }
            config.samples = parsed
        case "--trials":
            guard let parsed = Int(try value()), (1...20).contains(parsed) else {
                throw ProbeError.usage("--trials must be in 1...20")
            }
            config.trials = parsed
        case "--timeout-ms":
            guard let parsed = Int(try value()), (100...10_000).contains(parsed) else {
                throw ProbeError.usage("--timeout-ms must be in 100...10000")
            }
            config.timeoutMs = parsed
        case "--settle-ms":
            guard let parsed = Int(try value()), (0...1_000).contains(parsed) else {
                throw ProbeError.usage("--settle-ms must be in 0...1000")
            }
            config.settleMs = parsed
        case "--change-threshold":
            guard let parsed = Double(try value()), (0.005...0.5).contains(parsed) else {
                throw ProbeError.usage("--change-threshold must be in 0.005...0.5")
            }
            config.changeThreshold = parsed
        case "--list-windows":
            config.listWindows = true
        case "--help", "-h":
            throw ProbeError.usage("")
        default:
            throw ProbeError.usage("unknown argument: \(argument)")
        }
    }
    if !config.listWindows
        && (config.nativeTitle?.isEmpty != false || config.terminalTitle?.isEmpty != false)
    {
        throw ProbeError.usage(
            "--native-title and --terminal-title are required unless --list-windows is used"
        )
    }
    return config
}

private enum ProbeError: Error, CustomStringConvertible {
    case usage(String)
    case runtime(String)

    var description: String {
        switch self {
        case .usage(let message), .runtime(let message):
            return message
        }
    }
}

private let usage = """
usage:
  swift symmetric-window-latency.swift --list-windows
  swift symmetric-window-latency.swift \\
    --native-title "Mandatum GPU Host Spike" \\
    --terminal-title "MANDATUM_SYMMETRIC_TERMINAL" \\
    [--samples 1000] [--trials 3] [--timeout-ms 1000] [--settle-ms 50]

Prepare one native window and one terminal-hosted product window with unique
titles and default key bindings. Keep both unobscured and at a fixed size.
The driver alternates paired trial order, posts Ctrl+P, detects the Command
Palette in ScreenCaptureKit frames, then posts Escape to restore the baseline.
"""

private func shareableWindows() async throws -> [SCWindow] {
    let content = try await SCShareableContent.excludingDesktopWindows(
        true,
        onScreenWindowsOnly: true
    )
    return content.windows.filter {
        $0.owningApplication != nil && $0.title?.isEmpty == false && $0.frame.width > 0
            && $0.frame.height > 0
    }
}

private func matchingWindow(title: String) async throws -> SCWindow {
    let matches = try await shareableWindows().filter {
        $0.title?.localizedCaseInsensitiveContains(title) == true
    }
    guard matches.count == 1, let match = matches.first else {
        let details = matches.map {
            "\($0.windowID):\($0.owningApplication?.applicationName ?? "?"):"
                + "\($0.title ?? "")"
        }.joined(separator: ", ")
        throw ProbeError.runtime(
            "title \(title.debugDescription) matched \(matches.count) windows"
                + (details.isEmpty ? "" : ": \(details)")
        )
    }
    return match
}

private func raiseWindow(title: String, pid: pid_t) throws -> AXUIElement {
    let application = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(
        application,
        kAXWindowsAttribute as CFString,
        &value
    )
    guard status == .success, let windows = value as? [AXUIElement] else {
        throw ProbeError.runtime("cannot enumerate accessibility windows for pid \(pid)")
    }
    let matching = windows.filter { window in
        var titleValue: CFTypeRef?
        return AXUIElementCopyAttributeValue(
            window,
            kAXTitleAttribute as CFString,
            &titleValue
        ) == .success
            && (titleValue as? String)?.localizedCaseInsensitiveContains(title) == true
    }
    guard matching.count == 1, let window = matching.first else {
        throw ProbeError.runtime(
            "accessibility title \(title.debugDescription) matched \(matching.count) windows"
        )
    }
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateAllWindows])
    _ = AXUIElementSetAttributeValue(
        application,
        kAXFrontmostAttribute as CFString,
        kCFBooleanTrue
    )
    guard AXUIElementPerformAction(window, kAXRaiseAction as CFString) == .success else {
        throw ProbeError.runtime("cannot raise window \(title.debugDescription)")
    }
    _ = AXUIElementSetAttributeValue(
        window,
        kAXMainAttribute as CFString,
        kCFBooleanTrue
    )
    _ = AXUIElementSetAttributeValue(
        window,
        kAXFocusedAttribute as CFString,
        kCFBooleanTrue
    )
    _ = AXUIElementSetAttributeValue(
        application,
        kAXFocusedWindowAttribute as CFString,
        window
    )
    let deadline = Date().addingTimeInterval(1)
    while true {
        var focusedValue: CFTypeRef?
        let focusedStatus = AXUIElementCopyAttributeValue(
            application,
            kAXFocusedWindowAttribute as CFString,
            &focusedValue
        )
        var focusedTitleValue: CFTypeRef?
        let focusedTitle: String?
        if let focusedValue,
           CFGetTypeID(focusedValue) == AXUIElementGetTypeID()
        {
            let focusedWindow = unsafeBitCast(focusedValue, to: AXUIElement.self)
            focusedTitle = AXUIElementCopyAttributeValue(
                focusedWindow,
                kAXTitleAttribute as CFString,
                &focusedTitleValue
            ) == .success
                ? focusedTitleValue as? String
                : nil
        } else {
            focusedTitle = nil
        }
        if focusedStatus == .success
            && focusedTitle?.localizedCaseInsensitiveContains(title) == true
        {
            break
        }
        if Date() >= deadline {
            throw ProbeError.runtime(
                "pid \(pid) did not focus window \(title.debugDescription)"
            )
        }
        Thread.sleep(forTimeInterval: 0.02)
    }
    Thread.sleep(forTimeInterval: 0.05)
    return application
}

private func postKey(
    _ key: CGKeyCode,
    control: Bool = false,
    application: AXUIElement
) throws {
    if control {
        guard targetedAXKeyboardEvent(application, 0, controlKey, true) == .success
        else {
            throw ProbeError.runtime("cannot post Control key-down")
        }
    }
    let downStatus = targetedAXKeyboardEvent(application, 0, key, true)
    let upStatus = targetedAXKeyboardEvent(application, 0, key, false)
    let controlUpStatus = control
        ? targetedAXKeyboardEvent(application, 0, controlKey, false)
        : .success
    guard downStatus == .success, upStatus == .success, controlUpStatus == .success else {
        throw ProbeError.runtime(
            "targeted AX keyboard injection failed: down=\(downStatus.rawValue), "
                + "up=\(upStatus.rawValue), controlUp=\(controlUpStatus.rawValue)"
        )
    }
}

private struct DisplayMetadata {
    let id: CGDirectDisplayID
    let refreshHz: Double?
}

private func displayMetadata(for window: SCWindow) -> DisplayMetadata? {
    var displays = [CGDirectDisplayID](repeating: 0, count: 16)
    var count: UInt32 = 0
    let status = displays.withUnsafeMutableBufferPointer {
        CGGetDisplaysWithRect(
            window.frame,
            UInt32($0.count),
            $0.baseAddress,
            &count
        )
    }
    guard status == .success, count > 0 else {
        return nil
    }
    let candidates = displays.prefix(Int(count))
    guard let display = candidates.max(by: {
        let lhsArea = CGDisplayBounds($0).intersection(window.frame)
        let rhsArea = CGDisplayBounds($1).intersection(window.frame)
        return lhsArea.width * lhsArea.height < rhsArea.width * rhsArea.height
    }) else {
        return nil
    }
    return DisplayMetadata(
        id: display,
        refreshHz: CGDisplayCopyDisplayMode(display)?.refreshRate
    )
}

private func currentAbsoluteTime() -> UInt64 {
    mach_absolute_time()
}

private func milliseconds(from start: UInt64, to end: UInt64) -> Double {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    let nanoseconds = Double(end - start) * Double(info.numer) / Double(info.denom)
    return nanoseconds / 1_000_000
}

private func percentile(_ sorted: [Double], _ p: Double) -> Double {
    guard !sorted.isEmpty else { return 0 }
    let index = Int((p / 100 * Double(sorted.count - 1)).rounded())
    return sorted[min(index, sorted.count - 1)]
}

private func runTrial(
    frontend: String,
    trial: Int,
    title: String,
    config: Config
) async throws -> TrialResult {
    let window = try await matchingWindow(title: title)
    guard let owner = window.owningApplication else {
        throw ProbeError.runtime("window \(window.windowID) has no owning application")
    }
    let pid = owner.processID
    var application = try raiseWindow(title: title, pid: pid)
    let display = displayMetadata(for: window)

    // A previous fail-closed trial may have left the palette open. Escape is
    // idempotent for the measured product state, so establish a closed state
    // before choosing this trial's reference frame.
    for _ in 0..<3 {
        try postKey(escapeKey, application: application)
        if config.settleMs > 0 {
            try await Task.sleep(for: .milliseconds(config.settleMs))
        }
        application = try raiseWindow(title: title, pid: pid)
    }

    let filter = SCContentFilter(desktopIndependentWindow: window)
    let streamConfig = SCStreamConfiguration()
    streamConfig.width = max(1, Int(window.frame.width * 2))
    streamConfig.height = max(1, Int(window.frame.height * 2))
    streamConfig.pixelFormat = kCVPixelFormatType_32BGRA
    streamConfig.minimumFrameInterval = CMTime(value: 1, timescale: 120)
    streamConfig.queueDepth = 8
    streamConfig.showsCursor = false
    streamConfig.capturesAudio = false

    let collector = FrameCollector()
    let stream = SCStream(filter: filter, configuration: streamConfig, delegate: collector)
    let queue = DispatchQueue(label: "mandatum.symmetric-latency.frames")
    try stream.addStreamOutput(collector, type: .screen, sampleHandlerQueue: queue)
    try await stream.startCapture()

    let timeout = Double(config.timeoutMs) / 1_000
    guard var closedReference = try collector.latest(timeout: timeout) else {
        throw ProbeError.runtime("no initial ScreenCaptureKit frame for \(title)")
    }
    var samples = [Double]()
    samples.reserveCapacity(config.samples)
    var misses = 0
    var resetRetries = 0
    var resetFailures = 0
    var attempts = 0
    let maximumAttempts = config.samples * 2

    while samples.count < config.samples && attempts < maximumAttempts {
        attempts += 1
        if config.settleMs > 0 {
            try await Task.sleep(for: .milliseconds(config.settleMs))
        }
        let started = currentAbsoluteTime()
        try postKey(
            paletteKey,
            control: true,
            application: application
        )
        if let opened = try collector.changed(
            from: closedReference,
            after: started,
            threshold: config.changeThreshold,
            timeout: timeout
        ) {
            samples.append(milliseconds(from: started, to: opened.displayTime))
            var closed: Frame?
            for resetAttempt in 0..<10 {
                if config.settleMs > 0 {
                    try await Task.sleep(for: .milliseconds(config.settleMs))
                }
                if resetAttempt > 0 {
                    resetRetries += 1
                    application = try raiseWindow(title: title, pid: pid)
                }
                let closeStarted = currentAbsoluteTime()
                try postKey(escapeKey, application: application)
                closed = try collector.matching(
                    reference: closedReference,
                    after: closeStarted,
                    threshold: config.changeThreshold,
                    timeout: timeout
                )
                if closed != nil {
                    break
                }
            }
            if let closed {
                closedReference = closed
            } else {
                resetFailures += 1
                misses += 1
                break
            }
        } else {
            misses += 1
            var recovered: Frame?
            for resetAttempt in 0..<10 {
                if resetAttempt > 0 {
                    resetRetries += 1
                    application = try raiseWindow(title: title, pid: pid)
                }
                let recoveryCheckStarted = currentAbsoluteTime()
                try postKey(escapeKey, application: application)
                recovered = try collector.matching(
                    reference: closedReference,
                    after: recoveryCheckStarted,
                    threshold: config.changeThreshold,
                    timeout: timeout
                )
                if recovered != nil {
                    break
                }
            }
            if let recovered {
                closedReference = recovered
            } else {
                resetFailures += 1
                break
            }
        }
    }

    let sortedSamples = samples.sorted()
    let result = TrialResult(
        frontend: frontend,
        trial: trial,
        windowID: window.windowID,
        windowTitle: window.title ?? "",
        ownerName: owner.applicationName,
        ownerPID: pid,
        displayID: display?.id,
        displayRefreshHz: display?.refreshHz,
        requestedSamples: config.samples,
        attempts: attempts,
        sampleCount: samples.count,
        misses: misses,
        resetRetries: resetRetries,
        resetFailures: resetFailures,
        p50Ms: percentile(sortedSamples, 50),
        p95Ms: percentile(sortedSamples, 95),
        maxMs: sortedSamples.last ?? 0,
        latenciesMs: samples
    )
    try await stream.stopCapture()
    return result
}

private func run() async throws -> Bool {
    let config = try parseConfig()
    guard CGPreflightScreenCaptureAccess() else {
        throw ProbeError.runtime(
            "Screen Recording permission is required; grant it, then restart the invoking terminal"
        )
    }
    guard AXIsProcessTrusted() else {
        throw ProbeError.runtime(
            "Accessibility permission is required to raise target windows and post input"
        )
    }

    if config.listWindows {
        for window in try await shareableWindows().sorted(by: {
            ($0.owningApplication?.applicationName ?? "", $0.title ?? "")
                < ($1.owningApplication?.applicationName ?? "", $1.title ?? "")
        }) {
            print(
                "\(window.windowID)\t"
                    + "\(window.owningApplication?.applicationName ?? "?")\t"
                    + "\(window.owningApplication?.processID ?? 0)\t\(window.title ?? "")"
            )
        }
        return true
    }

    let nativeTitle = config.nativeTitle!
    let terminalTitle = config.terminalTitle!
    var results = [TrialResult]()
    for trial in 1...config.trials {
        let pair = trial.isMultiple(of: 2)
            ? [("terminal", terminalTitle), ("native", nativeTitle)]
            : [("native", nativeTitle), ("terminal", terminalTitle)]
        for (frontend, title) in pair {
            results.append(
                try await runTrial(
                    frontend: frontend,
                    trial: trial,
                    title: title,
                    config: config
                )
            )
        }
    }

    let acquisitionCompleted = results.allSatisfy {
        $0.sampleCount == $0.requestedSamples && $0.resetFailures == 0
    }
    let zeroMissAdmissionPassed = results.allSatisfy {
        $0.misses == 0 && $0.resetFailures == 0
    }
    let displayIDs = Set(results.compactMap(\.displayID))
    let sharedDisplayID = displayIDs.count == 1 && results.allSatisfy { $0.displayID != nil }
        ? displayIDs.first
        : nil
    let sharedRefreshRates = Set(results.compactMap(\.displayRefreshHz))
    let sharedDisplayRefreshHz = sharedRefreshRates.count == 1
        && results.allSatisfy { $0.displayRefreshHz != nil }
        ? sharedRefreshRates.first
        : nil
    let displayPairValidated = sharedDisplayID != nil && sharedDisplayRefreshHz != nil
    let completed = acquisitionCompleted && displayPairValidated
    let outcome = completed ? "ok" : "incomplete"
    let evidence = Evidence(
        schemaVersion: schemaVersion,
        endpoint: "mach_absolute_time immediately before targeted AXUIElementPostKeyboardEvent -> ScreenCaptureKit SCStreamFrameInfo.displayTime",
        workload: "Ctrl+P opens Command Palette; Escape restores baseline",
        platform: "\(ProcessInfo.processInfo.operatingSystemVersionString) \(Runtime.machine)",
        availableMetalDevices: MTLCopyAllDevices().map(\.name),
        sharedDisplayID: sharedDisplayID,
        sharedDisplayRefreshHz: sharedDisplayRefreshHz,
        samplesPerTrial: config.samples,
        pairedTrials: config.trials,
        timeoutMs: config.timeoutMs,
        settleMs: config.settleMs,
        changeThreshold: config.changeThreshold,
        results: results,
        acquisitionCompleted: acquisitionCompleted,
        zeroMissAdmissionPassed: zeroMissAdmissionPassed,
        completed: completed,
        outcome: outcome,
        notes: "Both paths use the same targeted AX keyboard injection, focus verification, timer start, and WindowServer-captured-pixels endpoint. Raw latencies are retained in acquisition order per trial for independent recomputation. Escape resets run outside the timed interval and must return to the previously verified closed signature. Display identity and refresh are recorded per result and must match across the pair. availableMetalDevices describes machine inventory, not the renderer selected by either frontend. This is software presentation evidence, not photon timing. Incomplete acquisition, display mismatch, or reset failure returns a nonzero process status. zeroMissAdmissionPassed reports the separate proposed Phase 7 admission threshold without invalidating completed Phase 6 evidence."
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    print(String(decoding: try encoder.encode(evidence), as: UTF8.self))
    return completed
}

private enum Runtime {
    static var machine: String {
        var value = utsname()
        uname(&value)
        return withUnsafePointer(to: &value.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) {
                String(cString: $0)
            }
        }
    }
}

Task {
    do {
        exit(try await run() ? EXIT_SUCCESS : EXIT_FAILURE)
    } catch let error as ProbeError {
        if case .usage = error {
            if !error.description.isEmpty {
                fputs("error: \(error.description)\n", stderr)
            }
            fputs("\(usage)\n", stderr)
            exit(error.description.isEmpty ? EXIT_SUCCESS : EX_USAGE)
        }
        fputs("error: \(error.description)\n", stderr)
        exit(EXIT_FAILURE)
    } catch {
        fputs("error: \(error)\n", stderr)
        exit(EXIT_FAILURE)
    }
}
RunLoop.main.run()
