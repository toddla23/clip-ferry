# Android용 코어(.so)와 Kotlin 바인딩을 만든다. Windows PowerShell용.
#   사용: .\scripts\build-android-core.ps1            (release, 기본)
#         .\scripts\build-android-core.ps1 -Debug
# 필요: Android NDK(ANDROID_NDK_HOME), rustup 타깃 aarch64/x86_64-linux-android, cargo-ndk
param([switch]$Debug)
$ErrorActionPreference = "Continue"  # cargo가 stderr로 진행 상황을 내므로 종료 코드로만 판단한다
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $env:ANDROID_NDK_HOME) { throw "ANDROID_NDK_HOME이 설정되어 있지 않습니다 (docs/02-architecture.md의 Android 빌드 환경 참고)" }
$profileDir = if ($Debug) { "debug" } else { "release" }
$cargoArgs = @("-t", "arm64-v8a", "-t", "x86_64", "-P", "26", "-o", "apps/android/app/src/main/jniLibs", "build", "-p", "clip-ferry-ffi")
if (-not $Debug) { $cargoArgs += "--release" }

$jni = "apps/android/app/src/main/jniLibs"
$kotlin = "apps/android/app/src/main/java"

cargo ndk @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo ndk 실패" }

# 바인딩은 .so 안의 메타데이터에서 만든다 (어느 ABI든 같다).
$lib = "target/aarch64-linux-android/$profileDir/libclip_ferry_ffi.so"
cargo run -q -p uniffi-bindgen -- generate --library $lib --language kotlin --out-dir $kotlin --no-format
if ($LASTEXITCODE -ne 0) { throw "Kotlin 바인딩 생성 실패" }
Write-Host "완료: $jni, $kotlin"
