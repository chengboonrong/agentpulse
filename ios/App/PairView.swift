import AgentPulseLink
import AgentPulsePhone
import SwiftUI
import UIKit
import VisionKit

/// Pairing (docs/mobile.md, "Pairing"): scan or paste the link, confirm the Mac and relay,
/// show the six-digit code, wait for the Mac's first state.
struct PairView: View {
    enum Step {
        case scan
        case confirm(PairingLink)
        case code(String, Pairing)
    }

    let request: PairRequest
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var step = Step.scan
    @State private var pasted = ""
    @State private var error: String?
    @State private var working = false
    @ScaledMetric(relativeTo: .largeTitle) private var codeSize = 56.0

    var body: some View {
        NavigationStack {
            Group {
                switch step {
                case .scan: scan
                case .confirm(let link): confirm(link)
                case .code(let code, let pairing): codeView(code, pairing)
                }
            }
            .navigationTitle("Pair a Mac")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .onAppear {
            if let s = request.step {
                step = s
            } else if let link = request.link {
                take(link)
            }
        }
    }

    // MARK: Scan or paste

    @ViewBuilder private var scan: some View {
        if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
            Scanner { take($0) }
                .ignoresSafeArea(edges: .bottom)
                .overlay(alignment: .bottom) {
                    Text(error ?? "Point the camera at the code on your Mac.")
                        .font(.callout)
                        .padding()
                        .background(.regularMaterial, in: .rect(cornerRadius: 12))
                        .padding()
                }
        } else {
            Form {
                Section {
                    TextField("agentpulse://pair?…", text: $pasted, axis: .vertical)
                        .lineLimit(3...6)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(.callout.monospaced())
                    PasteButton(payloadType: String.self) { strings in
                        if let s = strings.first { pasted = s }
                    }
                } header: {
                    Text("Pairing link")
                } footer: {
                    Text("The camera isn't available. On your Mac, copy the link under the pairing code and paste it here.")
                }
                if let error {
                    Section { Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.red) }
                }
                Section {
                    Button("Continue") { take(pasted) }
                        .disabled(pasted.isEmpty)
                }
            }
        }
    }

    private func take(_ string: String) {
        do {
            let link = try PairingLink(string.trimmingCharacters(in: .whitespacesAndNewlines))
            error = nil
            step = .confirm(link)
            #if DEBUG
            // scripts/e2e-ios.sh: no taps in the simulator.
            if UserDefaults.standard.bool(forKey: "AgentPulseE2EAutoConfirm") { pair(link) }
            #endif
        } catch {
            self.error = "That isn't an AgentPulse pairing link."
        }
    }

    // MARK: Confirm

    private func confirm(_ link: PairingLink) -> some View {
        ScrollView {
            VStack(spacing: 16) {
                Image(systemName: "laptopcomputer")
                    .font(.system(size: 56))
                    .foregroundStyle(.tint)
                    .accessibilityHidden(true)
                Text("Pair with \(link.macName)?")
                    .font(.title2.bold())
                    .multilineTextAlignment(.center)
                LabeledContent("Relay", value: link.relay.host() ?? link.relay.absoluteString)
                    .padding()
                    .background(.fill.tertiary, in: .rect(cornerRadius: 12))
                Text("Pair only with a Mac you own. It will send its agents' status to this iPhone, end-to-end encrypted.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                }
                Button {
                    pair(link)
                } label: {
                    Text("Pair").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(working)
            }
            .padding()
        }
    }

    private func pair(_ link: PairingLink) {
        guard !working, !model.offline else { return }
        working = true
        Task {
            defer { working = false }
            do {
                let (code, pairing) = try await model.client.pair(link: link.string, phoneName: UIDevice.current.name)
                model.reload()
                step = .code(code, pairing)
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    // MARK: Code

    private func codeView(_ code: String, _ pairing: Pairing) -> some View {
        let spaced = "\(code.prefix(3)) \(code.suffix(3))"
        return ScrollView {
            VStack(spacing: 20) {
                Text("Check that your Mac shows")
                    .font(.title3)
                Text(spaced)
                    .font(.system(size: codeSize, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .accessibilityLabel(code.map(String.init).joined(separator: " "))
                Text("Then confirm on your Mac.")
                    .foregroundStyle(.secondary)
                if let error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.red)
                } else {
                    ProgressView("Waiting for \(pairing.macName)")
                }
            }
            .padding()
            .frame(maxWidth: .infinity)
        }
        .task { await waitForState(pairing) }
    }

    /// `GET down` every 2 s until the Mac confirms and its first state arrives.
    private func waitForState(_ pairing: Pairing) async {
        guard !model.offline else { return }
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(2))
            do {
                let p = try await model.client.fetchDown(pairing)
                guard p.lastState != nil else { continue }
                model.added(p)
                dismiss()
                model.path = [p.id]
                return
            } catch PhoneError.removed {
                error = "Your Mac declined or removed the pairing."
                return
            } catch {
                // Offline for a moment; keep waiting.
            }
        }
    }
}

/// VisionKit's QR scanner; calls `found` once with the first AgentPulse link.
private struct Scanner: UIViewControllerRepresentable {
    let found: @MainActor (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])], isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {
        if !scanner.isScanning { try? scanner.startScanning() }
    }

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: @MainActor (String) -> Void
        init(found: @escaping @MainActor (String) -> Void) { self.found = found }

        func dataScanner(
            _ scanner: DataScannerViewController, didAdd added: [RecognizedItem], allItems: [RecognizedItem]
        ) {
            for case .barcode(let code) in added {
                if let s = code.payloadStringValue, s.lowercased().hasPrefix("agentpulse://pair") {
                    scanner.stopScanning()
                    found(s)
                    return
                }
            }
        }
    }
}
