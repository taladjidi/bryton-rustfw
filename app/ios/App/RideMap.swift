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

    /// Metropolitan France, roughly: where IGN's tiles exist.
    static func inFrance(_ c: CLLocationCoordinate2D) -> Bool {
        (41.0...51.5).contains(c.latitude) && (-5.5...9.9).contains(c.longitude)
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
        if Self.inFrance(first) {
            let t = MKTileOverlay(urlTemplate: Self.ign)
            t.canReplaceMapContent = true
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
        if let old = c.marker { m.removeAnnotation(old) }
        c.marker = nil
        if let s = selected {
            let a = MKPointAnnotation()
            a.coordinate = s
            a.title = "Here"
            m.addAnnotation(a)
            c.marker = a
        }
    }
}
