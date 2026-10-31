plugins {
    id("com.android.application")
}

android {
    namespace = "wordcraft.android"
    compileSdk = 36

    defaultConfig {
        applicationId = "wordcraft.android"
        // Tablets and phones from Android 8.0 on. The Rust library is built for arm64-v8a only.
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
