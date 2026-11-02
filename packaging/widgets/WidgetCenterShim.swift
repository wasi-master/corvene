// The app's way to WidgetKit (flag 433-macos-widgets): corvene-core opens this
// library at run time and calls the one function, so the binary keeps its
// macOS 10.15 minimum and links no Swift.
import WidgetKit

@_cdecl("corvene_reload_widgets")
public func corveneReloadWidgets() {
    WidgetCenter.shared.reloadAllTimelines()
}
