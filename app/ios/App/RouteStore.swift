// Routes on the phone: GPX files read by the Rust core (importRoute) and kept in Documents/Routes as the FIT course
// the unit follows (the same bytes that are sent), plus a small ledger of what was sent under which name, since the
// unit's list gives a course's check, not its name (docs/app.md, "Routes").

import SwiftUI

/// A route kept on the phone.
struct PhoneRoute: Identifiable, Equatable {
    let name: String
    let view: RouteView
    var id: String { name }
}

/// The navigation value of a route page.
struct RouteRoute: Hashable { let name: String }

/// A route sent to the unit: the name it got there, the size of the file, and the phone route it came from.
struct SentRoute: Codable, Equatable {
    let unitName: String
    let size: UInt32
    let route: String
}

/// A course in the unit's `\Courses` list: its key is its place in the folder, the check is of its name and size.
struct UnitRoute: Identifiable, Equatable {
    let key: String
    let size: UInt32
    let check: UInt16
    var id: String { key }
}

final class RouteStore: ObservableObject {
    @Published var items: [PhoneRoute] = []
    @Published var problem: String?
    @Published private(set) var sent: [SentRoute] = []

    private static var dir: URL {
        let d = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Routes")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private static var ledger: URL { dir.appendingPathComponent("sent.json") }

    init() {
        sent = (try? JSONDecoder().decode([SentRoute].self, from: Data(contentsOf: Self.ledger))) ?? []
        reload()
    }

    func reload() {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: Self.dir.path)) ?? []
        items = names.filter { $0.hasSuffix(".fit") }.sorted().compactMap { n in
            let stem = String(n.dropLast(4))
            guard let bytes = try? Data(contentsOf: Self.dir.appendingPathComponent(n)),
                let v = try? routeFromFit(bytes: bytes, name: stem)
            else { return nil }
            return PhoneRoute(name: stem, view: v)
        }
    }

    /// Reads a GPX file (a security-scoped URL from Files or the share sheet) and keeps it; returns the route.
    @discardableResult
    func importGpx(_ url: URL) -> PhoneRoute? {
        let ok = url.startAccessingSecurityScopedResource()
        defer { if ok { url.stopAccessingSecurityScopedResource() } }
        do {
            // the core refuses over 32 MB too; this keeps a huge file out of memory
            if let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size > 32 << 20 {
                problem = "That file is too big for a route (32 MB at most)."
                return nil
            }
            return try add(gpx: Data(contentsOf: url), fallback: url.deletingPathExtension().lastPathComponent)
        } catch {
            problem = message(error)
            return nil
        }
    }

    /// Converts and writes a GPX; a name already kept for other bytes gets " 2", " 3"...
    func add(gpx: Data, fallback: String) throws -> PhoneRoute {
        let v = try importRoute(bytes: gpx, fallbackName: fallback)
        var name = v.name
        var n = 2
        func file(_ n: String) -> URL { Self.dir.appendingPathComponent("\(n).fit") }
        while let old = try? Data(contentsOf: file(name)), old != v.fit {
            let tail = " \(n)"
            name = String(v.name.prefix(20 - tail.count)) + tail
            n += 1
        }
        try v.fit.write(to: file(name), options: .atomic)
        reload()
        problem = nil
        guard let r = items.first(where: { $0.name == name }) else { throw DraftError.Invalid(message: "Could not keep the route.") }
        return r
    }

    func delete(_ r: PhoneRoute) {
        try? FileManager.default.removeItem(at: Self.dir.appendingPathComponent("\(r.name).fit"))
        reload()
    }

    /// What the phone sent: remembered so the unit's list can be read.
    func remember(_ s: SentRoute) {
        sent.removeAll { $0.unitName.lowercased() == s.unitName.lowercased() }
        sent.append(s)
        if let d = try? JSONEncoder().encode(sent) { try? d.write(to: Self.ledger, options: .atomic) }
    }

    /// The name a unit course was sent under, if this phone sent it (its check is of that name and size).
    func unitName(_ r: UnitRoute) -> String? {
        sent.first { $0.size == r.size && courseCheck(name: $0.unitName, size: $0.size) == r.check }?.unitName
    }

    /// The phone route a unit course came from.
    func source(_ r: UnitRoute) -> PhoneRoute? {
        let n = sent.first { $0.size == r.size && courseCheck(name: $0.unitName, size: $0.size) == r.check }?.route
        return items.first { $0.name == n }
    }

    /// Whether the unit holds this route, sent under any name.
    func onUnit(_ p: PhoneRoute, _ list: [UnitRoute]) -> Bool {
        list.contains { u in source(u)?.name == p.name }
    }

    func message(_ e: Error) -> String {
        if case DraftError.Invalid(let m) = e { return m }
        return "\(e)"
    }
}
