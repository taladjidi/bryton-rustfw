# The phone app

Walking skeleton (roadmap section 9, A2 and A3 started): an iPhone app that finds the bike computer, reads its
Battery Level (0x2A19) and Manufacturer Name (0x2A29) and shows them. The bytes are decoded by a Rust core
(`app/core`, UniFFI), so the app and the unit share code; Swift only moves bytes from CoreBluetooth. The firmware
is unchanged: it already serves Battery 0x180F and Device Information 0x180A openly (HID is hidden from apps by iOS).

The unit advertises only while pairing, or bonded and not connected. When the phone already holds the link (the
notifications bond), the app asks the system for it with `retrieveConnectedPeripherals(withServices:)`, and scans
otherwise. It looks for the name "S510". That the system's connected list returns a bonded unit is reported by other
apps, not documented by Apple: **confirm it on the phone**. Not yet seen on a real iPhone.

## Build

Needs Xcode 16 or later, Rust (the pinned toolchain, with `aarch64-apple-ios` and `aarch64-apple-ios-sim`).

    app/ios/build-core.sh          # core for both iOS targets, Swift bindings, XCFramework (app/ios/Generated)
    cd app/core && cargo test      # the core's tests, on the Mac
    xcodebuild -project app/ios/Bryton.xcodeproj -scheme Bryton -sdk iphonesimulator \
        -destination 'generic/platform=iOS Simulator' CODE_SIGNING_ALLOWED=NO build

The simulator has no Bluetooth: the app shows "This device has no Bluetooth". check.sh runs the core's clippy and
tests, and the simulator build when something under `app/` changed on a branch.

## Install on an iPhone with a free Apple ID

1. Xcode > Settings > Accounts: add your Apple ID (a personal team appears).
2. `cp app/ios/Local.xcconfig.example app/ios/Local.xcconfig`; set `DEVELOPMENT_TEAM` (your team id) and a
   `BUNDLE_ID_PREFIX` of your own. The file is gitignored.
3. On the phone: Settings > Privacy & Security > Developer Mode on (restart).
4. Run `app/ios/build-core.sh`, open `app/ios/Bryton.xcodeproj`, pick the phone, Run. On first launch trust the
   developer under Settings > General > VPN & Device Management.

The install lasts 7 days; Run again renews it. After one cable pairing, Xcode's "Connect via network" lets Run work
over Wi-Fi.

## Distribution

Build from source for now. TestFlight and the App Store need the paid Apple Developer account, and the App Store's
terms are to be checked against this project's GPLv3. The free route for others is an Android APK (step A4).
