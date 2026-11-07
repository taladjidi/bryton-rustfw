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
    var onError: ((String) -> Void)?
    var onDebug: ((String) -> Void)?

    private static let service = CBUUID(string: "AA3C0001-98D8-4772-99ED-DFB117C6746F")
    private static let controlChar = CBUUID(string: "AA3C0002-98D8-4772-99ED-DFB117C6746F")
    private static let dataChar = CBUUID(string: "AA3C0003-98D8-4772-99ED-DFB117C6746F")
    private static let battery = CBUUID(string: "180F")
    private static let info = CBUUID(string: "180A")
    private static let unitName = "S510"

    private var central: CBCentralManager!
    private var peripheral: CBPeripheral?
    private var control: CBCharacteristic?
    private var dataCharacteristic: CBCharacteristic?
    private var subscribeTries = 0

    func start() { central = CBCentralManager(delegate: self, queue: .main) }

    func write(_ d: Data) {
        guard let p = peripheral, let c = control else { return }
        p.writeValue(d, for: c, type: .withResponse)
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
        onError?(error?.localizedDescription ?? "Could not connect")
        c.connect(p)
    }

    func centralManager(_ c: CBCentralManager, didDisconnectPeripheral p: CBPeripheral, error: Error?) {
        control = nil
        onLost?()
        c.connect(p)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverServices error: Error?) {
        guard let s = p.services?.first(where: { $0.uuid == Self.service }) else {
            onError?(error?.localizedDescription ?? "The unit has no file service (update its firmware).")
            return
        }
        p.discoverCharacteristics([Self.controlChar, Self.dataChar], for: s)
    }

    func peripheral(_ p: CBPeripheral, didDiscoverCharacteristicsFor s: CBService, error: Error?) {
        let chars = s.characteristics ?? []
        guard let c = chars.first(where: { $0.uuid == Self.controlChar }),
            let d = chars.first(where: { $0.uuid == Self.dataChar })
        else {
            onError?(error?.localizedDescription ?? "The file service is incomplete.")
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
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                    guard let self, p === self.peripheral, let d = self.dataCharacteristic else { return }
                    p.setNotifyValue(true, for: d)
                }
            } else {
                onError?("Could not subscribe: \(e.localizedDescription)")
            }
            return
        }
        guard ch.uuid == Self.dataChar, ch.isNotifying else { return }
        let w = p.maximumWriteValueLength(for: .withResponse)
        onDebug?("Write length \(w) B with response (\(p.maximumWriteValueLength(for: .withoutResponse)) B without)")
        onReady?()
    }

    func peripheral(_ p: CBPeripheral, didUpdateValueFor ch: CBCharacteristic, error: Error?) {
        if ch.uuid == Self.dataChar, let v = ch.value { onData?(v) }
    }

    func peripheral(_ p: CBPeripheral, didWriteValueFor ch: CBCharacteristic, error: Error?) {
        guard let e = error else { return }
        if Self.needsEncryption(e) {
            onLost?()  // not ready: the silence timer sends the command again, once the link is encrypted
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in self?.onReady?() }
        } else {
            onError?("Write failed: \(e.localizedDescription)")
        }
    }

    private static func needsEncryption(_ e: Error) -> Bool {
        let c = (e as? CBATTError)?.code
        return c == .insufficientAuthentication || c == .insufficientEncryption
    }
}
