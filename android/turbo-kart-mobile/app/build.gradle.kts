plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.holochain.turbokart.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.holochain.turbokart.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0-smoke"
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    // Bundles and native .so are copied into app/src/main/ by the build script.
}
