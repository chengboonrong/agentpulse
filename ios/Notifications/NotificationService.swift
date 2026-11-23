import Foundation
import AgentPulsePhone
import UserNotifications

final class NotificationService: UNNotificationServiceExtension {
    private let lock = NSLock()
    private var handler: ((UNNotificationContent) -> Void)?
    private var original: UNNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        lock.lock()
        original = request.content
        handler = contentHandler
        lock.unlock()
        guard let message = request.content.userInfo["m"] as? String,
            let store = PhoneStore.appGroup("group.io.github.chengboonrong.AgentPulse"),
            let alert = PhoneClient(store: store).openAlert(message),
            let content = request.content.mutableCopy() as? UNMutableNotificationContent
        else {
            deliver(request.content)
            return
        }
        content.title = alert.title
        content.subtitle = alert.subtitle
        content.body = alert.body
        content.threadIdentifier = alert.thread
        deliver(content)
    }

    override func serviceExtensionTimeWillExpire() {
        lock.lock()
        let fallback = original
        lock.unlock()
        if let fallback { deliver(fallback) }
    }

    private func deliver(_ content: UNNotificationContent) {
        lock.lock()
        let callback = handler
        handler = nil
        lock.unlock()
        callback?(content)
    }
}
