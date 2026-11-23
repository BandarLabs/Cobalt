import Foundation

/// An app as the store shows it: what the store says it is, laid over with
/// what is true of it on the reader that is plugged in.
public struct StoreEntry: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var summary: String
    public var description: String
    public var needs: [String]
    public var access: [String]
    public var setup: [SetupStep]
    public var shots: [Shot]
    public var releaseNotes: String
    /// The version on offer: the catalog's where there is one, since the
    /// catalog is newer than the kobo command can be.
    public var version: String
    public var installedVersion: String?
    public var updateAvailable = false
    public var runsOnThisReader = true
    public var minimumCobaltVersion: String?
    public var bytes: Int64?
    public var fits: Bool?
    /// Whether the catalog for this reader lists it, which is what makes it
    /// something that can be installed from here.
    public var offered = false

    public var isInstalled: Bool { installedVersion != nil }
}

public struct StoreSection: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var entries: [StoreEntry]
}

/// The store as the Apps place lays it out.
public struct Storefront: Equatable, Sendable {
    /// What the reader holds, by title.
    public var onReader: [StoreEntry]
    /// Everything else, in the store's groups and the store's order.
    public var sections: [StoreSection]
    /// The ids that can be updated on this reader as it stands.
    public var updates: [String]
    /// How many apps there are before the search narrows them.
    public var total: Int

    public var isEmpty: Bool { onReader.isEmpty && sections.allSatisfy(\.entries.isEmpty) }

    public func entry(_ id: String) -> StoreEntry? {
        onReader.first { $0.id == id } ?? sections.lazy.flatMap(\.entries).first { $0.id == id }
    }

    /// Lays the store's description of each app over what the reader and
    /// its catalog say about it.
    ///
    /// The store is compiled into the kobo command and knows what every app
    /// is, with pictures, and nothing about any reader. The catalog is
    /// fetched for one reader and knows what can be installed on it, at
    /// what version and size, and nothing about pictures. They are joined
    /// by id. An app the catalog offers that the store has not heard of,
    /// published after this kobo was built, is listed under a heading of
    /// its own with the catalog's words and no pictures. With no reader
    /// there is no catalog, and the store is still all there to read.
    public static func arrange(store: Store?, installed: [AppEntry]?, catalog: [AppEntry]?, query: String?) -> Storefront {
        let shelf = Shelf.arrange(installed: installed, catalog: catalog, query: "")
        let wanted = (query ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        func matches(_ entry: StoreEntry) -> Bool {
            wanted.isEmpty || [entry.title, entry.id, entry.summary, entry.description].contains { $0.lowercased().contains(wanted) }
        }
        func entry(for listed: ListedApp?, known: StoreApp?) -> StoreEntry? {
            guard listed != nil || known != nil else { return nil }
            let app = listed?.app
            return StoreEntry(
                id: known?.id ?? app?.id ?? "",
                title: known?.title ?? app?.title ?? "",
                // The catalog's line is the app's own, from its signed
                // manifest, and is at least as new as the store's.
                summary: [app?.summary, known?.summary].compactMap { $0 }.first { !$0.isEmpty } ?? "",
                description: known?.description ?? "",
                needs: known?.needs ?? [],
                access: known?.access ?? app?.capabilities ?? [],
                setup: known?.setup ?? [],
                shots: known?.shots ?? [],
                releaseNotes: known?.releaseNotes ?? "",
                version: [app?.version, known?.version].compactMap { $0 }.first { !$0.isEmpty } ?? "",
                installedVersion: app?.installedVersion,
                updateAvailable: app?.updateAvailable ?? false,
                runsOnThisReader: app?.runsOnThisReader ?? true,
                minimumCobaltVersion: app?.minimumCobaltVersion ?? known?.minimumCobaltVersion,
                bytes: app?.bytes,
                fits: app?.fits,
                offered: listed?.inCatalog ?? false
            )
        }

        let onReader = shelf.installed.compactMap { entry(for: $0, known: store?.apps[$0.id]) }
        let held = Set(onReader.map(\.id))
        var available: [String: ListedApp] = [:]
        for listed in shelf.available { available[listed.id] = listed }

        var sections: [StoreSection] = []
        var placed = held
        for group in store?.groups ?? [] {
            let entries = group.apps.filter { !placed.contains($0) }.compactMap { id in
                entry(for: available[id], known: store?.apps[id])
            }
            placed.formUnion(entries.map(\.id))
            sections.append(StoreSection(id: group.id, title: group.title, entries: entries))
        }
        // Known to the store and in none of its groups: still an app.
        let ungrouped = (store?.apps.values.filter { !placed.contains($0.id) } ?? []).sorted { $0.title < $1.title }
        let strays = ungrouped.compactMap { entry(for: available[$0.id], known: $0) }
        placed.formUnion(strays.map(\.id))
        let unheard = shelf.available.filter { !placed.contains($0.id) }.compactMap { entry(for: $0, known: nil) }
        if !strays.isEmpty || !unheard.isEmpty {
            sections.append(StoreSection(id: "more", title: store == nil ? "Available" : "More", entries: strays + unheard))
        }

        let total = onReader.count + sections.reduce(0) { $0 + $1.entries.count }
        return Storefront(
            onReader: onReader.filter(matches),
            sections: sections.map { StoreSection(id: $0.id, title: $0.title, entries: $0.entries.filter(matches)) }.filter { !$0.entries.isEmpty },
            updates: shelf.updates,
            total: total
        )
    }
}

public enum StoreWords {
    /// Which picture a step from one in a row lands on. It stops at the
    /// ends and does not go round. The page steps the same way.
    public static func galleryStep(from index: Int, by: Int, count: Int) -> Int {
        min(max(index + by, 0), max(count - 1, 0))
    }

    /// Why an app cannot be installed from here as things stand, or nothing
    /// when it can. One sentence, and always a reason.
    public static func obstacle(to entry: StoreEntry, reader: Reader?, hasCatalog: Bool) -> String {
        guard let reader else { return "Installing needs a reader plugged in." }
        guard reader.cobalt != nil else { return "Installing needs Cobalt on the reader." }
        if entry.isInstalled && !entry.updateAvailable { return "" }
        if !entry.runsOnThisReader {
            let wanted = entry.minimumCobaltVersion ?? ""
            return entry.isInstalled ? "Version \(entry.version) needs Cobalt \(wanted) or newer." : "Needs Cobalt \(wanted) or newer."
        }
        if entry.fits == false {
            let free = reader.freeBytes.map { "; the reader has \(Advice.fileSize($0)) free" } ?? ""
            return "Needs \(Advice.fileSize(entry.bytes))\(free)."
        }
        if !entry.offered, !entry.isInstalled {
            return hasCatalog ? "The catalog for this reader does not offer it." : ""
        }
        return ""
    }
}
