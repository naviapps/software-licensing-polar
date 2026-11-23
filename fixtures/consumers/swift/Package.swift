// swift-tools-version: 6.3
import PackageDescription

let package = Package(
  name: "PolarConsumer",
  platforms: [.macOS(.v26)],
  dependencies: [
    .package(name: "software-licensing-polar", path: "../sdk"),
    .package(
      url: "https://github.com/naviapps/software-licensing.git",
      revision: "b69902197a294598901aa3c595f8cfb3b11c7404"),
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
