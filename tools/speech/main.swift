// lightair-speech: what the microphone hears, as text, for voice-phrase automations
// (crates/lightair-app/src/triggers.rs). Apple's speech recognition, on this Mac when the
// language allows (never sent anywhere then). One line per update on stdout:
//   TEXT <what's been said so far in this utterance>
//   DONE            (the utterance ended; the next TEXT starts a new one)
//   ERROR <why>
// Runs until its stdin closes (Lightair quitting or no automation listening any more).

import AVFoundation
import Foundation
import Speech

setvbuf(stdout, nil, _IOLBF, 0)

let locale = CommandLine.arguments.count > 1 ? Locale(identifier: CommandLine.arguments[1]) : Locale.current
var recognizer: SFSpeechRecognizer?
let engine = AVAudioEngine()
var request: SFSpeechAudioBufferRecognitionRequest?
var task: SFSpeechRecognitionTask?
var started = Date()

func say(_ line: String) {
    print(line.replacingOccurrences(of: "\n", with: " "))
}

func restart() {
    task?.cancel()
    task = nil
    request?.endAudio()
    let req = SFSpeechAudioBufferRecognitionRequest()
    req.shouldReportPartialResults = true
    if recognizer?.supportsOnDeviceRecognition == true {
        req.requiresOnDeviceRecognition = true
    }
    request = req
    started = Date()
    task = recognizer?.recognitionTask(with: req) { result, error in
        if let r = result {
            say("TEXT " + r.bestTranscription.formattedString)
            if r.isFinal {
                say("DONE")
                DispatchQueue.main.async { restart() }
            }
        } else if error != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { restart() }
        }
    }
}

func start() {
    guard let r = SFSpeechRecognizer(locale: locale), r.isAvailable else {
        say("ERROR speech recognition isn't available for \(locale.identifier)")
        exit(3)
    }
    recognizer = r
    let input = engine.inputNode
    let format = input.outputFormat(forBus: 0)
    input.removeTap(onBus: 0)
    input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
        request?.append(buffer)
    }
    engine.prepare()
    do {
        try engine.start()
    } catch {
        say("ERROR the microphone didn't start: \(error.localizedDescription)")
        exit(4)
    }
    restart()
    // a recognition task lasts about a minute: start a new one before then
    Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
        if Date().timeIntervalSince(started) > 50 {
            say("DONE")
            restart()
        }
    }
}

// stdin closing: Lightair doesn't want us any more
FileHandle.standardInput.readabilityHandler = { h in
    if h.availableData.isEmpty { exit(0) }
}

SFSpeechRecognizer.requestAuthorization { status in
    guard status == .authorized else {
        say("ERROR Lightair may not use speech recognition (System Settings › Privacy & Security › Speech Recognition)")
        exit(2)
    }
    DispatchQueue.main.async { start() }
}

RunLoop.main.run()
