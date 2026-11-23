// swift-tools-version: 6.3
import PackageDescription

let package = Package(
  name: "PolarConsumer",
  platforms: [.macOS(.v26)],
  dependencies: [
    .package(name: "software-licensing-polar", path: "../sdk"),
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "b5e41e21a737f3009da35d41bf0b786c91ede350"),
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
