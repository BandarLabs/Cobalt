import Foundation

/// An app as the Apps place lists it: the entry, and whether the catalog
/// knows of it.
public struct ListedApp: Equatable, Sendable, Identifiable {
    public var app: AppEntry
    /// False for an app the reader has and the catalog does not list, or
    /// when there is no catalog to ask.
    public var inCatalog: Bool

    public var id: String { app.id }
}

/// The apps in the order and grouping the Apps place shows them.
public struct Shelf: Equatable, Sendable {
    public var installed: [ListedApp]
    public var available: [ListedApp]
    /// The ids that can be updated on this reader as it stands.
    public var updates: [String]
    /// How many apps there are before the search narrows them.
    public var total: Int

    /// What is on the reader first, then what the catalog offers that is
    /// not, each narrowed to what was typed in the search field.
    ///
    /// `installed` is what the reader holds and is always known. `catalog`
    /// is what was last fetched and may be nil, when it has not arrived or
    /// could not be reached; the reader's own apps are still listed then,
    /// with no updates offered, because nothing is known about any.
    public static func arrange(installed: [AppEntry]?, catalog: [AppEntry]?, query: String?) -> Shelf {
        let wanted = (query ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        func matches(_ listed: ListedApp) -> Bool {
            wanted.isEmpty || [listed.app.title, listed.app.id, listed.app.summary].contains { $0.lowercased().contains(wanted) }
        }
        let english = Locale(identifier: "en")
        func byTitle(_ left: ListedApp, _ right: ListedApp) -> Bool {
            let order = left.app.title.compare(right.app.title, options: [.caseInsensitive, .diacriticInsensitive], range: nil, locale: english)
            return order == .orderedSame ? left.app.id < right.app.id : order == .orderedAscending
        }

        var offered: [String: AppEntry] = [:]
        for app in catalog ?? [] { offered[app.id] = app }
        let onReader = (installed ?? []).map { held -> ListedApp in
            guard let entry = offered[held.id] else {
                var alone = held
                alone.updateAvailable = false
                alone.runsOnThisReader = true
                return ListedApp(app: alone, inCatalog: false)
            }
            // The catalog's entry knows the version on offer; the reader's
            // own record knows only what it has.
            var merged = held
            // What an app is for is said in the catalog's words where the
            // catalog has them: the reader's own record of a built-in app
            // describes it for a different audience.
            if !entry.summary.isEmpty { merged.summary = entry.summary }
            merged.version = entry.version
            merged.updateAvailable = entry.updateAvailable
            merged.runsOnThisReader = entry.runsOnThisReader
            merged.minimumCobaltVersion = entry.minimumCobaltVersion
            merged.bytes = entry.bytes ?? held.bytes
            merged.fits = entry.fits ?? held.fits
            return ListedApp(app: merged, inCatalog: true)
        }
        let held = Set(onReader.map(\.id))
        let available = (catalog ?? []).filter { !held.contains($0.id) }.map { ListedApp(app: $0, inCatalog: true) }
        return Shelf(
            installed: onReader.filter(matches).sorted(by: byTitle),
            available: available.filter(matches).sorted(by: byTitle),
            updates: onReader.filter { $0.app.updateAvailable && $0.app.runsOnThisReader }.map(\.id),
            total: onReader.count + available.count
        )
    }
}
