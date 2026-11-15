import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
    id("com.chaquo.python")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

android {
    // Pinned to 36 on purpose: targeting 37 adds the Android 17
    // ACCESS_LOCAL_NETWORK runtime grant plus new JNI/`static final` rules,
    // neither of which v1 needs or can absorb.
    compileSdk = 36
    namespace = "network.rchat.mobile"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "network.rchat.mobile"
        minSdk = 26
        targetSdk = 36
        ndk {
            // Phones only. One ABI keeps the APK and the Chaquopy runtime small.
            abiFilters += listOf("arm64-v8a")
        }
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

// Python 3.13 runtime for the Reticulum/LXMF stack. RNS is installed without
// its `cryptography` dependency on purpose: the app uses RNS's internal
// pure-Python crypto provider, which avoids a native wheel and keeps the build
// reproducible. `pyserial` stays because RNS imports it unconditionally.
chaquopy {
    defaultConfig {
        version = "3.13"
        buildPython("python3.13")
        pip {
            options("--no-deps")
            install("rns==1.6.0")
            install("lxmf==1.3.0")
            install("pyserial==3.5")
        }
    }
    sourceSets {
        getByName("main") {
            // One bridge.py for desktop and Android. Six levels up from this
            // module (gen/android/app) is the repository root.
            srcDir("../../../../../../sidecar")
        }
    }
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
