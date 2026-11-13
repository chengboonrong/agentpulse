// Lets the Rust app ask WidgetKit to reload timelines; WidgetCenter is Swift-only.
// Loaded with dlopen from Contents/Frameworks by crates/agentpulse-app/src/widget_export.rs.

import WidgetKit

@_cdecl("agentpulse_reload_widgets")
public func agentpulseReloadWidgets() {
    WidgetCenter.shared.reloadAllTimelines()
}
