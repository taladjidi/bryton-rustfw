// The ride's track on a map. SwiftUI's Map takes no tile overlay, so this is an MKMapView. Inside France the base is
// IGN's Plan IGN v2 (the owner's choice), a WMTS layer served without a key by the Geoplateforme:
//   https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2
//   &STYLE=normal&TILEMATRIXSET=PM&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&FORMAT=image/png
// PM is the web-Mercator pyramid, so MKTileOverlay's {z}/{x}/{y} fill TILEMATRIX/TILECOL/TILEROW as they are (the URL
// was checked with curl on 2026-11-07: 200 PNG inside France, 404 outside). Outside France Apple's map is shown.

import MapKit
import SwiftUI

struct RideMap: UIViewRepresentable {
    let points: [CLLocationCoordinate2D]
    /// The point the chart selection is on.
    let selected: CLLocationCoordinate2D?
    /// The climbs, drawn over the track in their grade colour.
    var climbs: [ClimbLine.Spec] = []
    @Environment(\.colorScheme) private var scheme

    /// The credit's link: IGN's geoservices page.
    static let creditURL = URL(string: "https://geoservices.ign.fr/") ?? URL(fileURLWithPath: "/")

    static let ign =
        "https://data.geopf.fr/wmts?SERVICE=WMTS&REQUEST=GetTile&VERSION=1.0.0"
        + "&LAYER=GEOGRAPHICALGRIDSYSTEMS.PLANIGNV2&STYLE=normal&TILEMATRIXSET=PM"
        + "&TILEMATRIX={z}&TILEROW={y}&TILECOL={x}&FORMAT=image/png"

    /// Where IGN's tiles exist, roughly: mainland France and Corsica (boxes, so a bit of the neighbours is in too;
    /// the tiles are drawn over Apple's map, so a 404 there only leaves Apple's map showing).
    static func inFrance(_ c: CLLocationCoordinate2D) -> Bool {
        let mainland = (42.3...51.1).contains(c.latitude) && (-5.2...8.3).contains(c.longitude)
        let corsica = (41.3...43.1).contains(c.latitude) && (8.5...9.6).contains(c.longitude)
        return mainland || corsica
    }

    /// The whole track decides, by the middle of its bounding box (not its first point).
    static func usesIGN(_ points: [CLLocationCoordinate2D]) -> Bool {
        guard let la = points.map(\.latitude).minMax(), let lo = points.map(\.longitude).minMax() else { return false }
        return inFrance(CLLocationCoordinate2D(latitude: (la.0 + la.1) / 2, longitude: (lo.0 + lo.1) / 2))
    }

    final class Coordinator: NSObject, MKMapViewDelegate {
        var marker: MKPointAnnotation?
        /// The climbs now on the map (redrawn only when they change: the insight arrives after the map).
        var shownClimbs: [Int] = []

        func mapView(_ map: MKMapView, rendererFor overlay: MKOverlay) -> MKOverlayRenderer {
            if let t = overlay as? IGNOverlay { return IGNRenderer(overlay: t) }
            if let c = overlay as? ClimbLine {
                let r = MKPolylineRenderer(polyline: c)
                (r.strokeColor, r.lineWidth, r.lineJoin) = (c.color, 6, .round)
                return r
            }
            let r = MKPolylineRenderer(overlay: overlay)
            r.strokeColor = Tok.ui("AccentColor")
            r.lineWidth = 4
            r.lineJoin = .round
            return r
        }

        func mapView(_ map: MKMapView, viewFor a: MKAnnotation) -> MKAnnotationView? {
            guard let a = a as? MKPointAnnotation else { return nil }
            let v = MKMarkerAnnotationView(annotation: a, reuseIdentifier: nil)
            // the marker's own label is dark text on the dimmed dark map; the glyph (S, E) says it already
            v.titleVisibility = .hidden
            switch a.title {
            case "Start": v.markerTintColor = Tok.ui("ok"); v.glyphText = "S"
            case "End": v.markerTintColor = Tok.uiZone(7); v.glyphText = "E"
            default: v.markerTintColor = Tok.uiZone(1); v.glyphText = nil
            }
            return v
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> MKMapView {
        let m = MKMapView()
        m.delegate = context.coordinator
        m.pointOfInterestFilter = .excludingAll
        guard let first = points.first, let last = points.last else { return m }
        if Self.usesIGN(points) {
            let t = IGNOverlay(urlTemplate: Self.ign)
            // no Apple map under it: it showed through while a zoom level's tiles loaded (the whole track is in France)
            t.canReplaceMapContent = true
            t.maximumZ = 18
            t.dark = scheme == .dark
            m.addOverlay(t, level: .aboveLabels)
        }
        let line = MKPolyline(coordinates: points, count: points.count)
        m.addOverlay(line, level: .aboveLabels)
        for (c, name) in [(first, "Start"), (last, "End")] {
            let a = MKPointAnnotation()
            a.coordinate = c
            a.title = name
            m.addAnnotation(a)
        }
        m.setVisibleMapRect(
            line.boundingMapRect, edgePadding: UIEdgeInsets(top: 30, left: 30, bottom: 30, right: 30), animated: false)
        return m
    }

    func updateUIView(_ m: MKMapView, context: Context) {
        let c = context.coordinator
        if let t = m.overlays.compactMap({ $0 as? IGNOverlay }).first, t.dark != (scheme == .dark) {
            t.dark = scheme == .dark
            m.renderer(for: t)?.setNeedsDisplay()
        }
        let ids = climbs.map(\.coords.count)
        if ids != c.shownClimbs {
            m.removeOverlays(m.overlays.filter { $0 is ClimbLine })
            for k in climbs { m.addOverlay(ClimbLine(spec: k), level: .aboveLabels) }
            c.shownClimbs = ids
        }
        guard let s = selected else {
            if let old = c.marker { m.removeAnnotation(old) }
            c.marker = nil
            return
        }
        if let a = c.marker {
            a.coordinate = s  // moved, not re-added, on every drag tick
        } else {
            let a = MKPointAnnotation()
            a.coordinate = s
            a.title = "Here"
            m.addAnnotation(a)
            c.marker = a
        }
    }
}

extension Array where Element == Double {
    func minMax() -> (Double, Double)? {
        guard let lo = self.min(), let hi = self.max() else { return nil }
        return (lo, hi)
    }
}

/// The IGN tiles, decoded and kept in memory, the downloads in a disk cache.
final class IGNOverlay: MKTileOverlay {
    private static let images: NSCache<NSString, UIImage> = {
        let c = NSCache<NSString, UIImage>()
        c.countLimit = 600  // about 150 MB of decoded 256 px tiles at most
        return c
    }()
    private static let session: URLSession = {
        let c = URLSessionConfiguration.default
        c.urlCache = URLCache(memoryCapacity: 0, diskCapacity: 200 << 20, directory: nil)
        c.requestCachePolicy = .returnCacheDataElseLoad
        return URLSession(configuration: c)
    }()
    /// Dark mode: the white map is dimmed (drawn by the renderer; the track lines are other overlays, above).
    var dark = false
    private let lock = NSLock()
    private var loading = Set<String>()

    static func key(_ z: Int, _ x: Int, _ y: Int) -> String { "\(z)/\(x)/\(y)" }

    func image(_ z: Int, _ x: Int, _ y: Int) -> UIImage? { Self.images.object(forKey: Self.key(z, x, y) as NSString) }

    /// Fetches a tile once (a second ask while it loads is dropped); `done` runs when it is in memory.
    func fetch(_ z: Int, _ x: Int, _ y: Int, done: @escaping () -> Void) {
        let k = Self.key(z, x, y)
        lock.lock()
        let new = loading.insert(k).inserted
        lock.unlock()
        guard new else { return }
        let u = url(forTilePath: MKTileOverlayPath(x: x, y: y, z: z, contentScaleFactor: 1))
        Self.session.dataTask(with: u) { [weak self] d, r, _ in
            if let d, (r as? HTTPURLResponse)?.statusCode == 200, let i = UIImage(data: d) {
                Self.images.setObject(i, forKey: k as NSString)
                done()
            }
            self?.lock.lock()
            self?.loading.remove(k)
            self?.lock.unlock()
        }.resume()
    }
}

/// Draws the IGN tiles itself: while a tile loads, the nearest coarser tile already in memory is drawn scaled in its
/// place (as map apps do), so a pinch zoom never shows an empty square; past IGN's last level the last level's tiles
/// are scaled up. MKTileOverlayRenderer left a tile empty until it loaded.
final class IGNRenderer: MKOverlayRenderer {
    private var ign: IGNOverlay { overlay as! IGNOverlay }  // only made for one (rendererFor)

    override func canDraw(_ mapRect: MKMapRect, zoomScale: MKZoomScale) -> Bool { true }

    override func draw(_ mapRect: MKMapRect, zoomScale: MKZoomScale, in ctx: CGContext) {
        let world = MKMapSize.world.width
        let z = max(0, Int(log2(world * Double(zoomScale) / 256).rounded()))
        let side = world / Double(1 << z)
        let x0 = Int(mapRect.minX / side), y0 = Int(mapRect.minY / side)
        let x1 = Int((mapRect.maxX - 1) / side), y1 = Int((mapRect.maxY - 1) / side)
        UIGraphicsPushContext(ctx)
        defer { UIGraphicsPopContext() }
        for x in x0...max(x0, x1) {
            for y in y0...max(y0, y1) {
                let tile = MKMapRect(x: Double(x) * side, y: Double(y) * side, width: side, height: side)
                draw(z, x, y, in: rect(for: tile), tile: tile, zoomScale: zoomScale)
            }
        }
    }

    private func dim(_ r: CGRect) {
        guard ign.dark else { return }
        UIColor.black.withAlphaComponent(0.55).setFill()
        UIRectFillUsingBlendMode(r, .normal)
    }

    private func draw(_ z: Int, _ x: Int, _ y: Int, in r: CGRect, tile: MKMapRect, zoomScale: MKZoomScale) {
        let top = ign.maximumZ
        if z <= top, let i = ign.image(z, x, y) {
            i.draw(in: r)
            dim(r)
            return
        }
        if z <= top {
            ign.fetch(z, x, y) { [weak self] in self?.setNeedsDisplay(tile, zoomScale: zoomScale) }
        }
        // the nearest coarser tile in memory, its part over this one scaled up
        let lo = max(1, z - top)
        guard lo <= z else { return }  // level 0: nothing coarser
        for dz in lo...min(z, lo + 8) {
            guard let p = ign.image(z - dz, x >> dz, y >> dz), let cg = p.cgImage else { continue }
            let n = 1 << dz
            let w = CGFloat(cg.width) / CGFloat(n), h = CGFloat(cg.height) / CGFloat(n)
            let part = CGRect(x: CGFloat(x % n) * w, y: CGFloat(y % n) * h, width: w, height: h)
            if let c = cg.cropping(to: part) { UIImage(cgImage: c).draw(in: r) }
            dim(r)
            return
        }
    }
}

/// A climb's stretch of the track, with its colour.
final class ClimbLine: MKPolyline {
    struct Spec {
        let coords: [CLLocationCoordinate2D]
        let color: UIColor
    }
    var color = Tok.uiZone(5)

    convenience init(spec: Spec) {
        self.init(coordinates: spec.coords, count: spec.coords.count)
        color = spec.color
    }
}
