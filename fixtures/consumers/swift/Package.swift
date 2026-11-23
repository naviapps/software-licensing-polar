// swift-tools-version: 6.3
import PackageDescription

let package = Package(
  name: "PolarConsumer",
  platforms: [.macOS(.v26)],
  dependencies: [
    .package(name: "software-licensing-polar", path: "../sdk"),
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "0d601499646fb9209d547de699b98ef0022f4d8e"),
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
