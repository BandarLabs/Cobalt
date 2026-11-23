// swift-tools-version: 6.0
import PackageDescription

// The Mac front end for `kobo ui`. CobaltKit is everything that decides
// something and can be held to an example without a window: the wire
// format, the client, the words, and the record of what the owner is in the
// middle of. Cobalt is the windows drawn from it. Nothing here depends on
// anything but the system, so the bundle is one executable and one list.
let package = Package(
    name: "Cobalt",
    platforms: [.macOS(.v13)],
    targets: [
        .target(name: "CobaltKit"),
        .executableTarget(name: "Cobalt", dependencies: ["CobaltKit"]),
        .testTarget(name: "CobaltKitTests", dependencies: ["CobaltKit"]),
    ]
)
