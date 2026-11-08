// Intervals.icu: the rider's FTP, max HR, LTHR, weight and W' for the app's loads and the unit's profile.
//
// API (looked up 2026-11-08; the parsing and the merge are in app/core/src/intervals.rs, which cites the same pages):
// - https://intervals.icu/api/v1/docs (OpenAPI): GET /api/v1/athlete/{id} returns the athlete with its sportSettings;
//   HTTP basic auth, user "API_KEY", password the API key from Intervals.icu's Settings > Developer Settings.
// - https://forum.intervals.icu/t/api-access-to-intervals-icu/609 : the athlete id is 0 for the key's own athlete,
//   or i<number>.
// The key lives in the Keychain only (kSecClassGenericPassword): never in UserDefaults, a log or the repository.

import Foundation
import Security

enum Keychain {
    private static func query(_ account: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: "intervals.icu", kSecAttrAccount as String: account]
    }

    static func get(_ account: String) -> String? {
        var q = query(account)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let d = out as? Data else { return nil }
        return String(data: d, encoding: .utf8)
    }

    @discardableResult
    static func set(_ account: String, _ value: String?) -> Bool {
        guard let value, !value.isEmpty else {
            SecItemDelete(query(account) as CFDictionary)
            return true
        }
        let data = Data(value.utf8)
        let st = SecItemUpdate(query(account) as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        guard st == errSecItemNotFound else { return st == errSecSuccess }
        var q = query(account)
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        return SecItemAdd(q as CFDictionary, nil) == errSecSuccess
    }
}

struct IntervalsFailure: Error {
    let text: String
    /// The key was refused: retrying without a new key is pointless.
    var auth = false
    /// Worth trying again later (network, 429, 5xx); false for a file or request Intervals.icu will always refuse.
    var retry = true
}

enum IntervalsClient {
    /// No cache or cookies on disk: the athlete's data is not kept by the system.
    static let session = URLSession(configuration: .ephemeral)


    /// The athlete with its Ride settings, or why not, in plain words. The key is never put in a message.
    static func fetch(key: String, athlete: String) async throws -> IntervalsProfile {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { return try demoProfile() }
        #endif
        let key = key.trimmingCharacters(in: .whitespacesAndNewlines)
        let athlete = athlete.trimmingCharacters(in: .whitespacesAndNewlines)
        let id = athlete.isEmpty ? "0" : athlete
        guard id.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }),
            let url = URL(string: "https://intervals.icu/api/v1/athlete/\(id)")
        else { throw IntervalsFailure(text: "The athlete id should be 0 or like i12345.") }
        var req = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
        req.setValue("Basic " + Data("API_KEY:\(key)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
        let (data, resp): (Data, URLResponse)
        do {
            (data, resp) = try await session.data(for: req)
        } catch {
            throw IntervalsFailure(text: "Could not reach Intervals.icu. Check the connection.")
        }
        switch (resp as? HTTPURLResponse)?.statusCode ?? 0 {
        case 200: break
        case 401, 403: throw IntervalsFailure(text: "Intervals.icu refused the API key. Check the key and the athlete id.")
        case 404: throw IntervalsFailure(text: "Intervals.icu does not know that athlete id.")
        case let c: throw IntervalsFailure(text: "Intervals.icu answered with an error (\(c)). Try again later.")
        }
        do {
            return try intervalsParse(json: data)
        } catch let e as IntervalsParseError {
            switch e {
            case .NotAthlete: throw IntervalsFailure(text: "Intervals.icu answered with something that is not an athlete.")
            case .NoRideSettings:
                throw IntervalsFailure(text: "This Intervals.icu account has no sport settings for Ride. Add them in its Settings.")
            }
        } catch {
            throw IntervalsFailure(text: "Intervals.icu answered with something unreadable.")
        }
    }

    #if DEBUG
    /// The Simulator's screenshots: a made-up answer, no network.
    private static func demoProfile() throws -> IntervalsProfile {
        let json = #"{"name":"Demo Rider","weight":72.0,"sportSettings":[{"types":["Ride"],"ftp":265,"lthr":171,"max_hr":188,"w_prime":21500}]}"#
        return try intervalsParse(json: Data(json.utf8))
    }
    #endif
}

/// Fetches the profile and spreads it: the app's own values (so RideLibrary recomputes), then the unit.
@MainActor
final class IntervalsSync: ObservableObject {
    /// The last Test or sync, in words.
    @Published var status: String?
    @Published var busy = false
    private static let hour: TimeInterval = 3600
    static let athleteKey = "intervalsAthlete"
    private static let lastKey = "intervalsLastSync"

    /// A key is stored (read once: the Keychain is not asked on every redraw).
    @Published private(set) var hasKey = false

    init() { refreshKey() }

    private func refreshKey() {
        #if DEBUG
        if CommandLine.arguments.contains("-demo") { hasKey = true; return }
        #endif
        hasKey = Keychain.get("apikey") != nil
    }

    var athlete: String { UserDefaults.standard.string(forKey: Self.athleteKey) ?? "" }

    /// Tries a key (typed in the sheet): stores it only when Intervals.icu accepts it.
    func test(key: String, athlete: String) async {
        let k = key.isEmpty ? (Keychain.get("apikey") ?? "") : key
        guard !k.isEmpty else { status = "Enter the API key first."; return }
        busy = true
        defer { busy = false }
        do {
            let p = try await IntervalsClient.fetch(key: k, athlete: athlete)
            guard Keychain.set("apikey", k.trimmingCharacters(in: .whitespacesAndNewlines)) else {
                status = "Could not store the key in the Keychain."
                return
            }
            refreshKey()
            UserDefaults.standard.set(athlete, forKey: Self.athleteKey)
            status = "\(p.name ?? "Athlete"): FTP " + (p.ftp.map { "\($0) W" } ?? "not set")
        } catch let e as IntervalsFailure {
            status = e.text
        } catch {
            status = "Could not test the key."
        }
    }

    func forget() {
        Keychain.set("apikey", nil)
        refreshKey()
        status = "Key removed."
    }

    /// On launch and pull to refresh at most once an hour (`force` for the button). Without a key it does nothing.
    func sync(rides: RideBrowser, force: Bool) async {
        guard !busy else { return }
        #if DEBUG
        let demo = CommandLine.arguments.contains("-demo")
        #else
        let demo = false
        #endif
        guard let key = demo ? "demo" : Keychain.get("apikey") else { return }
        let d = UserDefaults.standard
        if !force, !demo, Date().timeIntervalSince1970 - d.double(forKey: Self.lastKey) < Self.hour { return }
        busy = true
        defer { busy = false }
        do {
            let p = try await IntervalsClient.fetch(key: key, athlete: athlete)
            d.set(Date().timeIntervalSince1970, forKey: Self.lastKey)
            // the app's values: the core's merge over an empty profile keeps exactly the fields in the menus' ranges (others 0)
            let m = mergeUnitProfile(
                unit: UnitProfile(ftp: 0, maxHr: 0, lthr: 0, weightKg: 0, wprimeJ: 0, zonesLthr: false, female: false), iv: p)
            for (k, v) in [("ftp", Int(m.profile.ftp)), ("maxHR", Int(m.profile.maxHr)), ("lthr", Int(m.profile.lthr)),
                ("weight", Int(m.profile.weightKg)), ("wPrime", Int(m.profile.wprimeJ))] where v > 0 {
                d.set(v, forKey: k)
            }
            status = "Synced from Intervals.icu" + (m.skipped.isEmpty ? "" : ". Not used: " + m.skipped.joined(separator: "; "))
            rides.pushProfile(p, source: "Intervals.icu")
        } catch let e as IntervalsFailure {
            status = e.text
        } catch {
            status = "Could not sync from Intervals.icu."
        }
    }
}
