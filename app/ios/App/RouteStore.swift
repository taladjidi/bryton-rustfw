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

/// A name the rider gave on the phone to a unit course it did not send (known by its size and check).
struct RouteAlias: Codable, Equatable {
    let size: UInt32
    let check: UInt16
    let name: String
}

/// A course in the unit's `\Courses` list: its key is its place in the folder, the check is of its name and size.
struct UnitRoute: Identifiable, Equatable {
    let key: String
    let size: UInt32
    let check: UInt16
    var id: String { key }
}

/// Main actor: it publishes state the views read; the file work runs in detached tasks and hops back explicitly.
@MainActor
final class RouteStore: ObservableObject {
    @Published var items: [PhoneRoute] = []
    @Published var problem: String?
    /// A GPX is being read (off the main thread).
    @Published var importing = false
    @Published private(set) var sent: [SentRoute] = []
    @Published private(set) var aliases: [RouteAlias] = []

    /// Decoded routes by file, as long as the file's size and date are the same: a change of the list parses nothing.
    private var cache: [String: (stamp: String, route: PhoneRoute)] = [:]
    /// Counts the keeps and deletes: a reload that started before the last one holds an older folder and is dropped
    /// (it would blank a route just imported).
    private var generation = 0

    private nonisolated static var dir: URL {
        let d = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("Routes")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private nonisolated static var ledger: URL { dir.appendingPathComponent("sent.json") }
    private nonisolated static var aliasFile: URL { dir.appendingPathComponent("aliases.json") }

    init() {
        sent = (try? JSONDecoder().decode([SentRoute].self, from: Data(contentsOf: Self.ledger))) ?? []
        aliases = (try? JSONDecoder().decode([RouteAlias].self, from: Data(contentsOf: Self.aliasFile))) ?? []
        reload()
    }

    private nonisolated static func file(_ n: String) -> URL { dir.appendingPathComponent("\(n).fit") }

    /// Reads the folder off the main thread; a file already decoded with the same size and date is not read again.
    func reload() {
        let known = cache.mapValues(\.stamp)
        let routes = cache
        let started = generation
        Task.detached {
            var fresh: [String: (stamp: String, route: PhoneRoute)] = [:]
            let names = (try? FileManager.default.contentsOfDirectory(atPath: Self.dir.path)) ?? []
            for n in names where n.hasSuffix(".fit") {
                let stem = String(n.dropLast(4))
                let at = try? FileManager.default.attributesOfItem(atPath: Self.dir.appendingPathComponent(n).path)
                let stamp = "\(at?[.size] as? Int ?? 0) \(at?[.modificationDate] as? Date ?? .distantPast)"
                if known[stem] == stamp, let r = routes[stem] {
                    fresh[stem] = r
                } else if let bytes = try? Data(contentsOf: Self.dir.appendingPathComponent(n)),
                    let v = try? routeFromFit(bytes: bytes, name: stem)
                {
                    fresh[stem] = (stamp, PhoneRoute(name: stem, view: v))
                }
            }
            await MainActor.run {
                guard self.generation == started else { return }
                self.cache = fresh
                self.items = fresh.values.map(\.route).sorted { $0.name < $1.name }
            }
        }
    }

    /// Reads a GPX file (a security-scoped URL from Files or the share sheet) and keeps it, off the main thread;
    /// `open` gets the route when it is kept.
    func importGpx(_ url: URL, then open: @escaping (PhoneRoute) -> Void = { _ in }) {
        importing = true
        problem = nil
        Task {
            let result = await Task.detached { () -> Result<RouteView, Error> in
                let ok = url.startAccessingSecurityScopedResource()
                defer { if ok { url.stopAccessingSecurityScopedResource() } }
                return Result {
                    // the core refuses over 8 MB too; this keeps a huge file out of memory
                    if let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size > 8 << 20 {
                        throw DraftError.Invalid(message: "That file is too big for a route (8 MB at most).")
                    }
                    return try importRoute(bytes: Data(contentsOf: url), fallbackName: url.deletingPathExtension().lastPathComponent)
                }
            }.value
            switch result {
            case .success(let v):
                do { open(try keep(v)) } catch { problem = message(error) }
            case .failure(let e): problem = message(error: e)
            }
            importing = false
        }
    }

    /// Converts a GPX synchronously (the demo's).
    func add(gpx: Data, fallback: String) throws -> PhoneRoute {
        try keep(try importRoute(bytes: gpx, fallbackName: fallback))
    }

    /// Writes the course; a name already kept for other bytes gets " 2", " 3"...
    private func keep(_ v: RouteView) throws -> PhoneRoute {
        var name = v.name
        var n = 2
        while let old = try? Data(contentsOf: Self.file(name)), old != v.fit {
            let tail = " \(n)"
            name = String(v.name.prefix(20 - tail.count)) + tail
            n += 1
        }
        try v.fit.write(to: Self.file(name), options: .atomic)
        // the decoded route is the one in hand (named as kept): no second parse
        let kept = try routeFromFit(bytes: v.fit, name: name)
        let r = PhoneRoute(name: name, view: kept)
        cache[name] = ("new", r)
        generation += 1
        items = (items.filter { $0.name != name } + [r]).sorted { $0.name < $1.name }
        problem = nil
        return r
    }

    func delete(_ r: PhoneRoute) {
        try? FileManager.default.removeItem(at: Self.file(r.name))
        cache[r.name] = nil
        generation += 1
        items.removeAll { $0.name == r.name }
    }

    /// What the phone sent: remembered so the unit's list can be read.
    func remember(_ s: SentRoute) {
        sent.removeAll { routeClash(a: $0.unitName, b: s.unitName) }
        sent.append(s)
        if let d = try? JSONEncoder().encode(sent) { try? d.write(to: Self.ledger, options: .atomic) }
    }

    /// A name for a unit course that was not sent from here (only on this phone; the unit keeps its own).
    func rename(_ u: UnitRoute, to name: String) {
        aliases.removeAll { $0.size == u.size && $0.check == u.check }
        if let n = routeName(text: name) { aliases.append(RouteAlias(size: u.size, check: u.check, name: n)) }
        if let d = try? JSONEncoder().encode(aliases) { try? d.write(to: Self.aliasFile, options: .atomic) }
    }

    private func sentEntry(_ r: UnitRoute) -> SentRoute? {
        sent.first { $0.size == r.size && courseCheck(name: $0.unitName, size: $0.size) == r.check }
    }

    /// The name a unit course was sent under, if this phone sent it (its check is of that name and size).
    func unitName(_ r: UnitRoute) -> String? { sentEntry(r)?.unitName }

    /// What to call a unit course: the name it was sent under, else the rider's own name for it.
    func label(_ r: UnitRoute) -> (name: String, own: Bool)? {
        if let n = unitName(r) { return (n, false) }
        return aliases.first { $0.size == r.size && $0.check == r.check }.map { ($0.name, true) }
    }

    /// The phone route a unit course came from.
    func source(_ r: UnitRoute) -> PhoneRoute? {
        let n = sentEntry(r)?.route
        return items.first { $0.name == n }
    }

    /// Whether the unit holds this route, sent under any name.
    func onUnit(_ p: PhoneRoute, _ list: [UnitRoute]) -> Bool {
        list.contains { u in source(u)?.name == p.name }
    }

    /// The names the unit's courses are known by here (sent from this phone, or named by the rider).
    func unitNames(_ list: [UnitRoute]) -> [String] { list.compactMap { label($0)?.name } }

    /// A send of `route` as `name` would meet this unit course (any case, `_` as a space), by the name it has there;
    /// `same`: it is this very route already (sending it again changes nothing).
    func clash(_ name: String, route: PhoneRoute, in list: [UnitRoute]) -> (name: String, same: Bool)? {
        for u in list {
            guard let l = label(u), routeClash(a: l.name, b: name) else { continue }
            let e = sentEntry(u)
            return (l.name, e?.route == route.name && e?.size == UInt32(clamping: route.view.fit.count))
        }
        return nil
    }

    func message(_ e: Error) -> String { message(error: e) }

    func message(error e: Error) -> String {
        if case DraftError.Invalid(let m) = e { return m }
        return "\(e)"
    }
}
