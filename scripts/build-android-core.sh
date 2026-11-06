#!/usr/bin/env bash
# Android용 코어(.so)와 Kotlin 바인딩을 만든다. macOS/Linux용 (Windows는 build-android-core.ps1).
#   사용: ./scripts/build-android-core.sh [--debug]
# 필요: Android NDK(ANDROID_NDK_HOME), rustup 타깃 aarch64/x86_64-linux-android, cargo-ndk
set -euo pipefail
cd "$(dirname "$0")/.."

: "${ANDROID_NDK_HOME:?ANDROID_NDK_HOME이 설정되어 있지 않습니다 (docs/02-architecture.md의 Android 빌드 환경 참고)}"
profile_dir=release
profile_flag=(--release)
if [[ "${1:-}" == "--debug" ]]; then profile_dir=debug; profile_flag=(); fi

jni=apps/android/app/src/main/jniLibs
kotlin=apps/android/app/src/main/java

cargo ndk -t arm64-v8a -t x86_64 -P 26 -o "$jni" build -p clip-ferry-ffi "${profile_flag[@]}"

# 바인딩은 .so 안의 메타데이터에서 만든다 (어느 ABI든 같다).
cargo run -q -p uniffi-bindgen -- generate \
  --library "target/aarch64-linux-android/$profile_dir/libclip_ferry_ffi.so" \
  --language kotlin --out-dir "$kotlin" --no-format
echo "완료: $jni, $kotlin"
