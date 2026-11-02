// The widget extension's entry point (flag 433-macos-widgets). Built by
// packaging/widgets/build.sh and linked with -e _NSExtensionMain.
import SwiftUI
import WidgetKit

struct SnapshotEntry: TimelineEntry {
    let date: Date
    let snapshot: WidgetSnapshot?
}

struct SnapshotProvider: TimelineProvider {
    func placeholder(in context: Context) -> SnapshotEntry {
        SnapshotEntry(date: Date(), snapshot: nil)
    }

    func getSnapshot(in context: Context, completion: @escaping (SnapshotEntry) -> Void) {
        completion(SnapshotEntry(date: Date(), snapshot: WidgetSnapshot.load()))
    }

    // The app asks for a reload when it writes a new snapshot; this is for
    // when it is not running.
    func getTimeline(in context: Context, completion: @escaping (Timeline<SnapshotEntry>) -> Void) {
        let entry = SnapshotEntry(date: Date(), snapshot: WidgetSnapshot.load())
        completion(Timeline(entries: [entry], policy: .after(Date().addingTimeInterval(1800))))
    }
}

struct PullRequestsEntryView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry
    var body: some View {
        PullRequestsView(snapshot: entry.snapshot, family: family)
            .containerBackground(.fill.tertiary, for: .widget)
    }
}

struct ChecksEntryView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry
    var body: some View {
        ChecksView(snapshot: entry.snapshot, family: family)
            .containerBackground(.fill.tertiary, for: .widget)
    }
}

struct PullRequestsWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "pull-requests", provider: SnapshotProvider()) {
            PullRequestsEntryView(entry: $0)
        }
        .configurationDisplayName("Pull Requests")
        .description("Pull requests waiting for your review or assigned to you.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

struct ChecksWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "checks", provider: SnapshotProvider()) {
            ChecksEntryView(entry: $0)
        }
        .configurationDisplayName("Checks")
        .description("The checks of the current branch of chosen repositories.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
    }
}

@main
struct CorveneWidgets: WidgetBundle {
    var body: some Widget {
        PullRequestsWidget()
        ChecksWidget()
    }
}
