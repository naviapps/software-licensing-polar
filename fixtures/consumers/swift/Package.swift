// swift-tools-version: 6.3
import PackageDescription

let package = Package(
  name: "PolarConsumer",
  platforms: [.macOS(.v26)],
  dependencies: [
    .package(name: "software-licensing-polar", path: "../sdk"),
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "b69391d690fd7d197a50970279b9274da5d54295"),
  ],
  targets: [
    .executableTarget(
      name: "PolarConsumer",
      dependencies: [
        .product(
          name: "SoftwareLicensingPolar", package: "software-licensing-polar"),
        .product(name: "SoftwareLicensing", package: "software-licensing"),
      ])
  ]
)
