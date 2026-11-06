plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.clipferry.android"
    compileSdk = 37

    defaultConfig {
        applicationId = "app.clipferry.android"
        minSdk = 26
        targetSdk = 36
        // 배포 워크플로가 태그(v0.1.0 → 0.1.0)와 실행 번호로 덮어쓴다.
        versionCode = (System.getenv("CLIPFERRY_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("CLIPFERRY_VERSION_NAME") ?: "0.1.0"
        // 코어(.so)는 이 두 ABI만 만든다 (scripts/build-android-core).
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    // 배포용 서명 키 (scripts/release 참고). 환경 변수가 없으면 디버그 키로 서명한다(로컬 시험용).
    // 업데이트 설치는 같은 키로 서명된 APK끼리만 되므로, 배포본은 항상 같은 키를 쓴다.
    val releaseKeystore = System.getenv("CLIPFERRY_KEYSTORE")?.takeIf { it.isNotBlank() }
    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("CLIPFERRY_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("CLIPFERRY_KEY_ALIAS") ?: "clip-ferry"
                keyPassword = System.getenv("CLIPFERRY_KEY_PASSWORD")
                    ?: System.getenv("CLIPFERRY_KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        // 개발용 로그인 칸은 디버그 빌드에서만 보인다 (BuildConfig.DEBUG)
        buildConfig = true
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.09.00")
    implementation(composeBom)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    // UniFFI가 만든 Kotlin 바인딩이 쓰는 JNA (안드로이드는 aar로 네이티브 조각이 들어 있다).
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    // 페어링 QR 스캔 (F16). ML Kit은 모델을 앱에 넣는 번들판이라 Play 서비스 없이 기기 안에서만 인식한다.
    val camerax = "1.6.2"
    implementation("androidx.camera:camera-camera2:$camerax")
    implementation("androidx.camera:camera-lifecycle:$camerax")
    implementation("androidx.camera:camera-view:$camerax")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
    // 구글 로그인 (F35). Credential Manager + Google ID. 구글 Play 서비스가 계정 선택 창을 띄운다.
    implementation("androidx.credentials:credentials:1.5.0")
    implementation("androidx.credentials:credentials-play-services-auth:1.5.0")
    implementation("com.google.android.libraries.identity.googleid:googleid:1.1.1")
}
