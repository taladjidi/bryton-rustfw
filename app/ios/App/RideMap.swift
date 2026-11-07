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

        func mapView(_ map: MKMapView, rendererFor overlay: MKOverlay) -> MKOverlayRenderer {
            if let t = overlay as? MKTileOverlay { return MKTileOverlayRenderer(tileOverlay: t) }
            let r = MKPolylineRenderer(overlay: overlay)
            r.strokeColor = .systemOrange
            r.lineWidth = 4
            r.lineJoin = .round
            return r
        }

        func mapView(_ map: MKMapView, viewFor a: MKAnnotation) -> MKAnnotationView? {
            guard let a = a as? MKPointAnnotation else { return nil }
            let v = MKMarkerAnnotationView(annotation: a, reuseIdentifier: nil)
            switch a.title {
            case "Start": v.markerTintColor = .systemGreen; v.glyphText = "S"
            case "End": v.markerTintColor = .systemRed; v.glyphText = "E"
            default: v.markerTintColor = .systemBlue; v.glyphText = nil
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
            let t = MKTileOverlay(urlTemplate: Self.ign)
            t.canReplaceMapContent = false  // Apple's map stays under the tiles where IGN has none
            t.maximumZ = 18
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
