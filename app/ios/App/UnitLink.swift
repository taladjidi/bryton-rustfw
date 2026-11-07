// Finds the bike computer over CoreBluetooth and reads its Battery Level and Manufacturer Name.
// The bytes are decoded by the Rust core (decodeStatus, through UniFFI); this file only moves them.
//
// The unit advertises only while pairing, or bonded and not connected. Once the phone holds the link (the
// notifications bond), the unit is invisible to a scan, so retrieveConnectedPeripherals(withServices:) comes first
// (Apple: CBCentralManager.retrieveConnectedPeripherals(withServices:)); the scan is the fallback. Only the open
// services 0x180F and 0x180A are used (iOS hides HID from apps).

import CoreBluetooth
import Foundation

/// What the screen shows.
enum LinkState: Equatable {
    case bluetoothOff, unauthorized, unsupported
    case searching
    case notFound
    case connected
    case failed(String)
}

final class UnitLink: NSObject, ObservableObject, CBCentralManagerDelegate, CBPeripheralDelegate {
    @Published var state: LinkState = .searching
    @Published var name = ""
    @Published var status = UnitStatus(maker: nil, batteryPercent: nil)

    private static let battery = CBUUID(string: "180F")
    private static let info = CBUUID(string: "180A")
    private static let levelChar = CBUUID(string: "2A19")
    private static let makerChar = CBUUID(string: "2A29")
    /// The unit's GAP name (src/app/phone.rs: device_name_set); other connected gadgets also have a battery.
    private static let unitName = "S510"

    private var central: CBCentralManager!
    private var peripheral: CBPeripheral?
    private var timeout: DispatchWorkItem?
    private var makerRaw: Data?
    private var levelRaw: Data?
    private var pending = 0

    override init() {
        super.init()
        central = CBCentralManager(delegate: self, queue: .main)
    }

    /// Starts a search again (the refresh button).
    func refresh() {
        guard central.state == .poweredOn else { return centralManagerDidUpdateState(central) }
        timeout?.cancel()
        central.stopScan()
        if let p = peripheral { central.cancelPeripheralConnection(p) }
        peripheral = nil
        makerRaw = nil
        levelRaw = nil
        state = .searching
        let known = central.retrieveConnectedPeripherals(withServices: [Self.battery, Self.info])
        if let p = known.first(where: { $0.name?.hasPrefix(Self.unitName) == true }) {
            connect(p)
        } else {
            central.scanForPeripherals(withServices: [Self.battery])
            arm(seconds: 8)
        }
    }

    /// Gives up with `.notFound` (or the failure) when nothing came back in time.
    private func arm(seconds: Double) {
        timeout?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self, self.state == .searching else { return }
            self.central.stopScan()
            if let p = self.peripheral { self.central.cancelPeripheralConnection(p) }
            self.state = self.peripheral == nil ? .notFound : .failed("The unit did not answer")
        }
        timeout = item
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: item)
    }

    private func connect(_ p: CBPeripheral) {
        central.stopScan()
        peripheral = p  // CoreBluetooth does not keep it alive
        name = p.name ?? Self.unitName
        p.delegate = self
        central.connect(p)
        arm(seconds: 10)
    }

    func centralManagerDidUpdateState(_ c: CBCentralManager) {
        switch c.state {
        case .poweredOn: refresh()
        case .poweredOff: state = .bluetoothOff
        case .unauthorized: state = .unauthorized
        case .unsupported: state = .unsupported
        default: state = .searching  // resetting, unknown: another update follows
        }
    }

    func centralManager(
        _ c: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber
    ) {
        let n = (advertisementData[CBAdvertisementDataLocalNameKey] as? String) ?? p.name ?? ""
        if n.hasPrefix(Self.unitName), peripheral == nil { connect(p) }
    }

    func centralManager(_ c: CBCentralManager, didConnect p: CBPeripheral) {
        p.discoverServices([Self.battery, Self.info])
    }

    func centralManager(_ c: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        timeout?.cancel()
        state = .failed(error?.localizedDescription ?? "Could not connect")
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        pending = 0
        for s in p.services ?? [] {
            pending += 1
            p.discoverCharacteristics([Self.levelChar, Self.makerChar], for: s)
        }
        if pending == 0 { finish(error: "The unit shows no battery or device information") }
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor s: CBService, error: Error?) {
        pending -= 1
        for ch in s.characteristics ?? [] { p.readValue(for: ch); pending += 1 }
        if pending == 0 { finish(error: nil) }
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor ch: CBCharacteristic, error: Error?) {
        if ch.uuid == Self.levelChar { levelRaw = ch.value }
        if ch.uuid == Self.makerChar { makerRaw = ch.value }
        pending -= 1
        if pending == 0 { finish(error: error?.localizedDescription) }
    }

    private func finish(error: String?) {
        timeout?.cancel()
        status = decodeStatus(makerRaw: makerRaw, batteryRaw: levelRaw)
        state = (status.maker == nil && status.batteryPercent == nil) ? .failed(error ?? "No readable values") : .connected
        if let p = peripheral { central.cancelPeripheralConnection(p) }  // the phone's own link stays
    }
}
