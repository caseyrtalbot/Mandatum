#!/usr/bin/env swift

import AppKit
import CoreImage
import CoreMedia
import CoreVideo
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

private let fixedProfile = "casey-m4pro-metal-scale2"
private let scenarios: Set<String> = [
    "typography", "calm-terminal", "dense-workspace", "attention", "palette",
    "full-modal", "welcome", "context-menu", "artifacts", "narrow", "restored",
]

private enum CaptureError: Error, CustomStringConvertible {
    case usage(String)
    case runtime(String)

    var description: String {
        switch self {
        case .usage(let message), .runtime(let message):
            return message
        }
    }
}

private struct Config {
    let profile: String
    let scenario: String
}

private struct CapturedFrame {
    let pixels: CVPixelBuffer
    let scaleFactor: Double
}

private final class OneShotCollector: NSObject, SCStreamOutput, SCStreamDelegate,
    @unchecked Sendable
{
    private let condition = NSCondition()
    private var frame: CapturedFrame?
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
              let imageBuffer = sampleBuffer.imageBuffer
        else {
            return
        }

        let scale = (metadata[.scaleFactor] as? NSNumber)?.doubleValue ?? 0
        condition.lock()
        if frame == nil {
            frame = CapturedFrame(pixels: imageBuffer, scaleFactor: scale)
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

    func wait(timeout: TimeInterval) throws -> CapturedFrame {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        defer { condition.unlock() }
        while frame == nil && streamError == nil {
            if !condition.wait(until: deadline) {
                break
            }
        }
        if let streamError {
            throw streamError
        }
        guard let frame else {
            throw CaptureError.runtime("ScreenCaptureKit produced no complete frame")
        }
        return frame
    }
}

Task {
    do {
        let config = try parseConfig()
        try requireFixedReferenceDisplay()
        try await capture(config)
        exit(0)
    } catch {
        FileHandle.standardError.write(
            Data("visual-regression: \(error)\n".utf8)
        )
        exit(2)
    }
}
dispatchMain()

private func parseConfig() throws -> Config {
    var args = Array(CommandLine.arguments.dropFirst())
    guard args.first == "capture" else {
        throw CaptureError.usage(
            "usage: visual-regression.swift capture --profile <id> --scenario <id>"
        )
    }
    args.removeFirst()
    var profile: String?
    var scenario: String?
    while !args.isEmpty {
        let option = args.removeFirst()
        guard !args.isEmpty else {
            throw CaptureError.usage("missing value for \(option)")
        }
        let value = args.removeFirst()
        switch option {
        case "--profile":
            profile = value
        case "--scenario":
            scenario = value
        default:
            throw CaptureError.usage("unknown option: \(option)")
        }
    }
    guard profile == fixedProfile else {
        throw CaptureError.usage(
            "unsupported profile \(profile ?? "<missing>"); expected \(fixedProfile)"
        )
    }
    guard let scenario, scenarios.contains(scenario) else {
        throw CaptureError.usage(
            "unknown scenario \(scenario ?? "<missing>")"
        )
    }
    return Config(profile: fixedProfile, scenario: scenario)
}

private func requireFixedReferenceDisplay() throws {
    let eligible = NSScreen.screens.filter {
        abs($0.backingScaleFactor - 2.0) < 0.001
            && $0.frame.width >= 800
            && $0.frame.height >= 600
    }
    guard !eligible.isEmpty else {
        let active = NSScreen.screens
            .map {
                "\($0.localizedName) scale=\($0.backingScaleFactor) "
                    + "size=\(Int($0.frame.width))x\(Int($0.frame.height))"
            }
            .joined(separator: ", ")
        throw CaptureError.runtime(
            "profile \(fixedProfile) requires an active 2.0 backing-scale display "
                + "with at least 800x600 logical pixels; active displays: \(active)"
        )
    }
}

private func capture(_ config: Config) async throws {
    let repo = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
    let manifest = repo.appendingPathComponent("spikes/frontend-wgpu/Cargo.toml")
    let title = "Mandatum Visual \(config.scenario)"
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = [
        "cargo", "run", "-q",
        "--manifest-path", manifest.path,
        "--bin", "mandatum-native-lab", "--",
        "--visual-scenario", config.scenario,
        "--font-size", "13",
        "--exit-after", "30",
    ]
    process.currentDirectoryURL = repo
    let output = Pipe()
    process.standardOutput = output
    process.standardError = FileHandle.standardError
    try process.run()
    defer {
        if process.isRunning {
            process.terminate()
        }
    }

    let window = try await waitForWindow(title: title, timeout: 15)
    guard let screen = screenFor(window: window) else {
        throw CaptureError.runtime("could not resolve the scenario window display")
    }
    guard abs(screen.backingScaleFactor - 2.0) < 0.001 else {
        throw CaptureError.runtime(
            "scenario window landed on \(screen.localizedName) at scale "
                + "\(screen.backingScaleFactor), not 2.0"
        )
    }

    try await Task.sleep(for: .milliseconds(750))
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let streamConfig = SCStreamConfiguration()
    streamConfig.width = 1_600
    streamConfig.height = 1_200
    streamConfig.pixelFormat = kCVPixelFormatType_32BGRA
    streamConfig.colorSpaceName = CGColorSpace.sRGB
    streamConfig.minimumFrameInterval = CMTime(value: 1, timescale: 60)
    streamConfig.queueDepth = 3
    streamConfig.showsCursor = false
    streamConfig.capturesAudio = false
    streamConfig.ignoreShadowsSingleWindow = true

    let collector = OneShotCollector()
    let stream = SCStream(
        filter: filter,
        configuration: streamConfig,
        delegate: collector
    )
    let queue = DispatchQueue(label: "mandatum.visual-regression.capture")
    try stream.addStreamOutput(
        collector,
        type: .screen,
        sampleHandlerQueue: queue
    )
    try await stream.startCapture()
    let captured = try collector.wait(timeout: 5)
    try await stream.stopCapture()

    let width = CVPixelBufferGetWidth(captured.pixels)
    let height = CVPixelBufferGetHeight(captured.pixels)
    guard width == 1_600, height == 1_200 else {
        throw CaptureError.runtime(
            "captured \(width)x\(height); fixed profile requires 1600x1200"
        )
    }
    guard abs(captured.scaleFactor - 2.0) < 0.001 else {
        throw CaptureError.runtime(
            "ScreenCaptureKit frame scaleFactor=\(captured.scaleFactor); expected 2.0"
        )
    }

    let candidateDir = repo
        .appendingPathComponent("spikes/frontend-wgpu/visual-candidates")
        .appendingPathComponent(config.profile)
        .appendingPathComponent(config.scenario)
    try FileManager.default.createDirectory(
        at: candidateDir,
        withIntermediateDirectories: true
    )
    let candidate = candidateDir.appendingPathComponent("candidate.png")
    try writePNG(captured.pixels, to: candidate)

    let sourceCommit = try command(
        "/usr/bin/git",
        ["rev-parse", "HEAD"],
        cwd: repo
    ).trimmingCharacters(in: .whitespacesAndNewlines)
    let dirty = !((try command(
        "/usr/bin/git",
        ["status", "--porcelain", "--untracked-files=normal"],
        cwd: repo
    )).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    let displayID = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")]
        as? NSNumber)?.uint32Value ?? 0
    let refresh = CGDisplayCopyDisplayMode(displayID)?.refreshRate ?? 0
    let gpuName = metalDeviceName()
    let executable = repo
        .appendingPathComponent("spikes/frontend-wgpu/target/debug/mandatum-native-lab")
        .path
    let metadata: [String: Any] = [
        "schema_version": 1,
        "profile": config.profile,
        "scenario": config.scenario,
        "theme": "mandatum-dark",
        "captured_at": ISO8601DateFormatter().string(from: Date()),
        "surface": [
            "logical_width": 800,
            "logical_height": 600,
            "physical_width": 1_600,
            "physical_height": 1_200,
            "backing_scale": 2.0,
        ],
        "scene": ["columns": 102, "rows": 35],
        "font": [
            "source": "bundled",
            "family": "JetBrains Mono",
            "size": 13.0,
            "faces": ["Regular", "Bold", "Italic", "BoldItalic"],
        ],
        "display": [
            "id": String(displayID),
            "name": screen.localizedName,
            "refresh_hz": refresh,
        ],
        "gpu": [
            "name": gpuName,
            "backend": "Metal",
            "device_type": "integrated",
        ],
        "source": ["commit": sourceCommit, "dirty": dirty],
        "build": ["profile": "debug", "executable": executable],
        "capture": [
            "api": "ScreenCaptureKit",
            "color_space": "srgb",
            "pixel_format": "rgba8-unpremultiplied",
            "client_surface": true,
            "shows_cursor": false,
            "includes_shadow": false,
        ],
        "fallback_regions": [],
        "acceptance": NSNull(),
    ]
    let metadataData = try JSONSerialization.data(
        withJSONObject: metadata,
        options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
    )
    try metadataData.write(
        to: candidateDir.appendingPathComponent("metadata.json"),
        options: .atomic
    )
    print(candidate.path)
}

private func waitForWindow(title: String, timeout: TimeInterval) async throws -> SCWindow {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        let content = try await SCShareableContent.excludingDesktopWindows(
            true,
            onScreenWindowsOnly: true
        )
        let matches = content.windows.filter {
            $0.title == title && $0.owningApplication != nil
        }
        if matches.count == 1 {
            return matches[0]
        }
        if matches.count > 1 {
            throw CaptureError.runtime("multiple on-screen windows matched \(title)")
        }
        try await Task.sleep(for: .milliseconds(100))
    }
    throw CaptureError.runtime("timed out waiting for native window \(title)")
}

private func screenFor(window: SCWindow) -> NSScreen? {
    NSScreen.screens.max {
        intersectionArea($0.frame, window.frame) < intersectionArea($1.frame, window.frame)
    }
}

private func intersectionArea(_ lhs: CGRect, _ rhs: CGRect) -> CGFloat {
    let intersection = lhs.intersection(rhs)
    return intersection.isNull ? 0 : intersection.width * intersection.height
}

private func writePNG(_ buffer: CVPixelBuffer, to url: URL) throws {
    let ciImage = CIImage(cvPixelBuffer: buffer)
    let context = CIContext(options: [.workingColorSpace: CGColorSpace(name: CGColorSpace.sRGB)!])
    guard let image = context.createCGImage(ciImage, from: ciImage.extent) else {
        throw CaptureError.runtime("could not create captured CGImage")
    }
    guard let destination = CGImageDestinationCreateWithURL(
        url as CFURL,
        UTType.png.identifier as CFString,
        1,
        nil
    ) else {
        throw CaptureError.runtime("could not create PNG destination")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        throw CaptureError.runtime("could not finalize PNG")
    }
}

private func command(_ executable: String, _ arguments: [String], cwd: URL) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.currentDirectoryURL = cwd
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    try process.run()
    process.waitUntilExit()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    let output = String(decoding: data, as: UTF8.self)
    guard process.terminationStatus == 0 else {
        throw CaptureError.runtime(
            "\(executable) \(arguments.joined(separator: " ")) failed: \(output)"
        )
    }
    return output
}

private func metalDeviceName() -> String {
    let output = try? command(
        "/usr/sbin/system_profiler",
        ["SPDisplaysDataType", "-json"],
        cwd: URL(fileURLWithPath: "/")
    )
    guard let data = output?.data(using: .utf8),
          let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let displays = root["SPDisplaysDataType"] as? [[String: Any]],
          let name = displays.first?["sppci_model"] as? String
    else {
        return "unknown"
    }
    return name
}
