// The widgets' data and views (flag 433-macos-widgets). The app writes
// snapshot.json into this extension's container; the widget holds no token
// and shows the last snapshot (crates/corvene-core/src/widgets.rs).
import SwiftUI
import WidgetKit

struct WidgetSnapshot: Decodable {
    struct PullRequest: Decodable, Identifiable {
        let title: String
        let repository: String
        let number: UInt64
        let url: String
        let reason: String
        var id: String { "\(repository)#\(number)" }
    }
    struct Repository: Decodable, Identifiable {
        let name: String
        let branch: String?
        let state: String
        let summary: String
        let url: String
        var id: String { name }
    }
    let generatedAt: UInt64
    let signedIn: Bool
    let reviewCount: Int
    let assignedCount: Int
    let pullRequests: [PullRequest]
    let repositories: [Repository]

    enum CodingKeys: String, CodingKey {
        case generatedAt = "generated_at"
        case signedIn = "signed_in"
        case reviewCount = "review_count"
        case assignedCount = "assigned_count"
        case pullRequests = "pull_requests"
        case repositories
    }

    /// NSHomeDirectory() is the container: the sandbox has no other way in.
    static func load() -> WidgetSnapshot? {
        let path = (NSHomeDirectory() as NSString).appendingPathComponent("snapshot.json")
        guard let data = FileManager.default.contents(atPath: path) else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }
}

private struct Message: View {
    let text: String
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Corvene").font(.caption).foregroundStyle(.secondary)
            Text(text).font(.footnote)
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct PullRequestsView: View {
    let snapshot: WidgetSnapshot?
    let family: WidgetFamily

    private var rows: Int {
        switch family {
        case .systemSmall: return 1
        case .systemMedium: return 2
        default: return 6
        }
    }

    private func count(_ value: Int, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(verbatim: "\(value)").font(.title2.weight(.semibold))
            Text(label).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        }
    }

    var body: some View {
        if let snapshot, snapshot.signedIn {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 14) {
                    count(snapshot.reviewCount, "To review")
                    count(snapshot.assignedCount, "Assigned")
                    Spacer(minLength: 0)
                }
                if rows > 0, snapshot.pullRequests.isEmpty {
                    Text("Nothing needs you.").font(.footnote).foregroundStyle(.secondary)
                }
                ForEach(snapshot.pullRequests.prefix(rows)) { pr in
                    Link(destination: URL(string: pr.url) ?? URL(string: "x-corvene://")!) {
                        VStack(alignment: .leading, spacing: 0) {
                            Text(pr.title).font(.footnote).lineLimit(1)
                            Text(verbatim: "\(pr.repository) #\(pr.number)")
                                .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else if snapshot != nil {
            Message(text: "Sign in to GitHub in Corvene to see your pull requests.")
        } else {
            Message(text: "Open Corvene to load your pull requests.")
        }
    }
}

struct ChecksView: View {
    let snapshot: WidgetSnapshot?
    let family: WidgetFamily

    private var rows: Int {
        switch family {
        case .systemSmall: return 3
        case .systemMedium: return 3
        default: return 8
        }
    }

    private func dot(_ state: String) -> Color {
        switch state {
        case "success": return .green
        case "failure": return .red
        case "pending": return .orange
        default: return .secondary
        }
    }

    var body: some View {
        if let snapshot, !snapshot.repositories.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(snapshot.repositories.prefix(rows)) { repo in
                    Link(destination: URL(string: repo.url) ?? URL(string: "x-corvene://")!) {
                        HStack(spacing: 6) {
                            Circle().fill(dot(repo.state)).frame(width: 8, height: 8)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(repo.name).font(.footnote).lineLimit(1)
                                Text(family == .systemSmall
                                    ? (repo.branch ?? repo.summary)
                                    : [repo.branch, repo.summary].compactMap { $0 }
                                        .joined(separator: " · "))
                                    .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else if snapshot != nil {
            Message(text: "Choose repositories in Corvene: Settings, Advanced, Menu bar.")
        } else {
            Message(text: "Open Corvene to load your checks.")
        }
    }
}
