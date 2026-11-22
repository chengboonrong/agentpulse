import Foundation

/// Which payload format versions this phone reads. Keep in step with
/// `WidgetPayload.decode` (macos/Shared/Payload.swift).
public enum PayloadFormat {
    public static let supported: ClosedRange<Int> = 1...1

    public enum Check: Equatable, Sendable {
        case current, macTooOld, phoneTooOld, unreadable
    }

    public static func check(_ data: Data) -> Check {
        struct Header: Decodable { let version: Int }
        guard let header = try? JSONDecoder().decode(Header.self, from: data) else { return .unreadable }
        if header.version < supported.lowerBound { return .macTooOld }
        if header.version > supported.upperBound { return .phoneTooOld }
        return .current
    }
}
