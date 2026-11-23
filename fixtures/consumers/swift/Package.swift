// swift-tools-version: 6.3
import PackageDescription

let package = Package(
  name: "PolarConsumer",
  platforms: [.macOS(.v26)],
  dependencies: [
    .package(name: "software-licensing-polar", path: "../sdk"),
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "7eca335326b61a67cff1679975194a0babd780ed"),
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
