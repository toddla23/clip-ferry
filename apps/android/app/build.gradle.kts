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
        // 기본 서버 (F43). 디버그는 에뮬레이터에서 본 PC의 개발 서버, 배포는 공개 서버.
        // 배포 값은 빌드 때 환경 변수 CF_DEFAULT_ACCOUNT_URL·CF_DEFAULT_RELAY_URL로 바꿀 수 있다.
        // 릴레이 주소는 로그인할 때 계정 서버(/v1/config)에서 받아 바꾸므로 여기 값은 처음 한 번만 쓴다.
        debug {
            buildConfigField("String", "DEFAULT_ACCOUNT_URL", "\"http://10.0.2.2:8788\"")
            buildConfigField("String", "DEFAULT_RELAY_URL", "\"ws://10.0.2.2:8787\"")
        }
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
            val account = System.getenv("CF_DEFAULT_ACCOUNT_URL")?.takeIf { it.isNotBlank() }
                ?: "https://clipferry.japaneast.cloudapp.azure.com:8443"
            val relay = System.getenv("CF_DEFAULT_RELAY_URL")?.takeIf { it.isNotBlank() }
                ?: "wss://clipferry.japaneast.cloudapp.azure.com"
            buildConfigField("String", "DEFAULT_ACCOUNT_URL", "\"$account\"")
            buildConfigField("String", "DEFAULT_RELAY_URL", "\"$relay\"")
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
