#!/usr/bin/env bash
# iOS용 코어(정적 라이브러리 → xcframework)와 Swift 바인딩을 만든다. macOS 전용.
#   사용: ./scripts/build-ios-core.sh [--debug]
# 필요: Xcode, rustup 타깃 aarch64-apple-ios · aarch64-apple-ios-sim · x86_64-apple-ios
#
# 만드는 것 (둘 다 git에 넣지 않는다):
#   apps/ios/Core/ClipFerryCore.xcframework   기기(arm64) + 시뮬레이터(arm64·x86_64)
#   apps/ios/Core/Generated/clip_ferry_ffi.swift
set -euo pipefail
cd "$(dirname "$0")/.."

profile_dir=release
profile_flag=(--release)
if [[ "${1:-}" == "--debug" ]]; then profile_dir=debug; profile_flag=(); fi

out=apps/ios/Core
gen="$out/Generated"
headers="$out/.headers"
rm -rf "$out/ClipFerryCore.xcframework" "$gen" "$headers"
mkdir -p "$gen" "$headers"

export IPHONEOS_DEPLOYMENT_TARGET=17.0
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  cargo build -p clip-ferry-ffi --lib --target "$target" "${profile_flag[@]}"
done

lib=libclip_ferry_ffi.a
mkdir -p "target/ios-sim/$profile_dir"
lipo -create \
  "target/aarch64-apple-ios-sim/$profile_dir/$lib" \
  "target/x86_64-apple-ios/$profile_dir/$lib" \
  -output "target/ios-sim/$profile_dir/$lib"

# 바인딩은 라이브러리 안의 메타데이터에서 만든다 (어느 타깃이든 같다).
cargo run -q -p uniffi-bindgen -- generate \
  --library "target/aarch64-apple-ios/$profile_dir/$lib" \
  --language swift --out-dir "$gen"

# xcframework의 헤더 폴더: C 헤더 + module.modulemap (Swift에서 `import clip_ferry_ffiFFI`)
mv "$gen/clip_ferry_ffiFFI.h" "$headers/"
mv "$gen/clip_ferry_ffiFFI.modulemap" "$headers/module.modulemap"

xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/$profile_dir/$lib" -headers "$headers" \
  -library "target/ios-sim/$profile_dir/$lib" -headers "$headers" \
  -output "$out/ClipFerryCore.xcframework"
rm -rf "$headers"
echo "완료: $out/ClipFerryCore.xcframework, $gen/clip_ferry_ffi.swift"
