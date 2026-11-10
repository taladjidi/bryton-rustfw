// The file service over CoreBluetooth on the already-connected, bonded unit (src/link/filesvc.rs): service
// aa3c0001, Control aa3c0002 (write with response), Data aa3c0003 (notify). Both characteristics need the bond.
// The unit is found as UnitLink does: retrieveConnectedPeripherals first, a scan as the fallback.
// After a drop it asks the system to connect again (a pending connect never times out) and calls onReady.

import CoreBluetooth
import Foundation

final class BleFileTransport: NSObject, FileTransport, CBCentralManagerDelegate, CBPeripheralDelegate {
    var onData: ((Data) -> Void)?
    var onReady: (() -> Void)?
    var onLost: (() -> Void)?
    var onWriteDone: (() -> Void)?
    var onError: ((String) -> Void)?
    var onDebug: ((String) -> Void)?

    private static let service = CBUUID(string: "AA3C0001-98D8-4772-99ED-DFB117C6746F")
    private static let controlChar = CBUUID(string: "AA3C0002-98D8-4772-99ED-DFB117C6746F")
    private static let dataChar = CBUUID(string: "AA3C0003-98D8-4772-99ED-DFB117C6746F")
    private static let battery = CBUUID(string: "180F")
    private static let info = CBUUID(string: "180A")
    /// A missing service means iOS holds an old copy of the unit's table (no Service Changed: docs/app.md).
    private static let staleTable =
        "The iPhone has an old copy of the unit's services. In Settings > Bluetooth, tap (i) next to S510, Forget This Device, then on the unit open PHONE > PAIR and connect again."
    private static let unitName = "S510"

    private var central: CBCentralManager!
    private var peripheral: CBPeripheral?
    private var control: CBCharacteristic?
    private var dataCharacteristic: CBCharacteristic?
    private var subscribeTries = 0

    /// CoreBluetooth runs on its own serial queue, so a notification is copied the moment it arrives and never waits
    /// for the main thread (SwiftUI redraws made it late); the callbacks hop to main in the order they came.
    private let queue = DispatchQueue(label: "ble-file", qos: .userInitiated)

    private func onMain(_ f: @escaping () -> Void) { DispatchQueue.main.async(execute: f) }

    func start() { central = CBCentralManager(delegate: self, queue: queue) }

    /// The unit has no long-write memory: a Control write must fit one ATT packet, MTU - 3 bytes. CoreBluetooth's
    /// `.withResponse` length is 512 whatever the MTU, so the `.withoutResponse` length (MTU - 3) is the one to use.
    var writeLength: Int { queue.sync { peripheral?.maximumWriteValueLength(for: .withoutResponse) ?? 20 } }

    func write(_ d: Data) {
        queue.async {
            guard let p = self.peripheral, let c = self.control else { return }
            p.writeValue(d, for: c, type: .withResponse)
        }
    }

    func centralManagerDidUpdateState(_ c: CBCentralManager) {
        guard c.state == .poweredOn else { return }
        let known = c.retrieveConnectedPeripherals(withServices: [Self.battery, Self.info])
        if let p = known.first(where: { $0.name?.hasPrefix(Self.unitName) == true }) {
            connect(p)
        } else {
            c.scanForPeripherals(withServices: [Self.battery])
        }
    }

    private func connect(_ p: CBPeripheral) {
        central.stopScan()
        peripheral = p  // CoreBluetooth does not keep it alive
        p.delegate = self
        central.connect(p)
    }

    func centralManager(
        _ c: CBCentralManager, didDiscover p: CBPeripheral, advertisementData: [String: Any], rssi: NSNumber
    ) {
        let n = (advertisementData[CBAdvertisementDataLocalNameKey] as? String) ?? p.name ?? ""
        if n.hasPrefix(Self.unitName), peripheral == nil { connect(p) }
    }

    func centralManager(_ c: CBCentralManager, didConnect p: CBPeripheral) {
        p.discoverServices([Self.service])
    }

    func centralManager(_ c: CBCentralManager, didFailToConnect p: CBPeripheral, error: Error?) {
        let m = error?.localizedDescription ?? "Could not connect"
        onMain { self.onError?(m) }
        c.connect(p)
    }

    func centralManager(_ c: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        control = nil
        onMain { self.onLost?() }
        c.connect(p)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        guard let s = p.services?.first(where: { $0.uuid == Self.service }) else {
            onMain { self.onError?(Self.staleTable) }
            return
        }
        p.discoverCharacteristics([Self.controlChar, Self.dataChar], for: s)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor s: CBService, error: Error?) {
        let chars = s.characteristics ?? []
        guard let c = chars.first(where: { $0.uuid == Self.controlChar }),
            let d = chars.first(where: { $0.uuid == Self.dataChar })
        else {
            onMain { self.onError?(Self.staleTable) }
            return
        }
        control = c
        dataCharacteristic = d
        subscribeTries = 0
        p.setNotifyValue(true, for: d)  // needs the bond: the first use asks iOS to encrypt
    }

    func peripheral(_ p: CBPeripheral, didUpdateNotificationStateFor ch: CBCharacteristic, error: Error?) {
        if let e = error {
            // the first use of a bonded characteristic can fail while iOS encrypts the link: try again shortly
            if Self.needsEncryption(e), subscribeTries < 3 {
                subscribeTries += 1
                queue.asyncAfter(deadline: .now() + 1) { [weak self] in
                    guard let self, p === self.peripheral, let d = self.dataCharacteristic else { return }
                    p.setNotifyValue(true, for: d)
                }
            } else {
                let m = "Could not subscribe: \(e.localizedDescription)"
                onMain { self.onError?(m) }
            }
            return
        }
        guard ch.uuid == Self.dataChar, ch.isNotifying else { return }
        let w = p.maximumWriteValueLength(for: .withResponse)
        let m = "Write length \(w) B with response (\(p.maximumWriteValueLength(for: .withoutResponse)) B without)"
        onMain {
            self.onDebug?(m)
            self.onReady?()
        }
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor ch: CBCharacteristic, error: Error?) {
        if ch.uuid == Self.dataChar, let v = ch.value {
            let copy = Data(v)
            onMain { self.onData?(copy) }
        }
    }

    func peripheral(_ p: CBPeripheral, didWriteValueFor ch: CBCharacteristic, error: Error?) {
        guard let e = error else {
            onMain { self.onWriteDone?() }
            return
        }
        if Self.needsEncryption(e) {
            onMain { self.onLost?() }  // not ready: the silence timer sends the command again, once encrypted
            queue.asyncAfter(deadline: .now() + 1) { [weak self] in self?.onMain { self?.onReady?() } }
        } else {
            let m = "Write failed: \(e.localizedDescription)"
            onMain { self.onError?(m) }
        }
    }

    private static func needsEncryption(_ e: Error) -> Bool {
        let c = (e as? CBATTError)?.code
        return c == .insufficientAuthentication || c == .insufficientEncryption
    }
}
