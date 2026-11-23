// swift-tools-version: 6.3
import PackageDescription

// A host developing both packages may use one explicit local core checkout.
// Normal installation and isolated CI resolve the immutable remote revision.
let core: Package.Dependency =
  if let path = Context.environment["SOFTWARE_LICENSING_PACKAGE_PATH"], !path.isEmpty {
    .package(name: "software-licensing", path: path)
  } else {
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "e979b3a2150b8b59c294043aff3ae4d83e50ed35")
  }

let package = Package(
  name: "SoftwareLicensingPolar",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "SoftwareLicensingPolar", targets: ["SoftwareLicensingPolar"])
  ],
  dependencies: [
    core
  ],
  targets: [
    .binaryTarget(
      name: "CSoftwareLicensingPolar",
      path: "Artifacts/CSoftwareLicensingPolar.xcframework"),
    .target(
      name: "SoftwareLicensingPolar",
      dependencies: [
        .product(name: "SoftwareLicensing", package: "software-licensing"),
        "CSoftwareLicensingPolar",
      ]),
    .testTarget(
      name: "SoftwareLicensingPolarTests", dependencies: ["SoftwareLicensingPolar"],
      path: "SwiftTests/SoftwareLicensingPolarTests"),
  ]
)
