plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.holochain.turbokart.mobile"
    compileSdk = 35
    buildFeatures { buildConfig = true }

    defaultConfig {
        applicationId = "dev.holochain.turbokart.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0-smoke"
        ndk { abiFilters += (project.findProperty("androidAbis") as String? ?: "arm64-v8a,x86_64").split(",") }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            versionNameSuffix = "-combined"
        }
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // Bundles and native .so are copied into app/src/main/ by the build script.
}

// Kotlin 2.3+ requires the typed compilerOptions DSL.
kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
