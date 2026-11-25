import java.time.LocalDate
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// The key releases are signed with, where keystore.properties tells it,
// out of git: password, keyAlias and storeFile. Without it, releases come
// out unsigned.
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// meshctl's version, as the workspace's Cargo.toml has it.
val meshctlVersion = rootProject.file("../../../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }
    .substringAfter('"')
    .substringBefore('"')

// What tells Android one build is newer than another: the day it is built
// on, in UTC, and a last digit for a second one the same day, 0 unless
// MESHCTL_VERSION_CODE says otherwise, whole, as 202611251: the release
// workflow counts the releases tagged that day.
val meshctlVersionCode = System.getenv("MESHCTL_VERSION_CODE")?.toInt()
    ?: (LocalDate.now(ZoneOffset.UTC)
        .format(DateTimeFormatter.BASIC_ISO_DATE) + "0").toInt()

android {
    compileSdk = 37
    if (!keystoreProperties.isEmpty) {
        signingConfigs {
            create("release") {
                keyAlias = keystoreProperties["keyAlias"] as String
                keyPassword = keystoreProperties["password"] as String
                storeFile = file(keystoreProperties["storeFile"] as String)
                storePassword = keystoreProperties["password"] as String
            }
        }
    }
    namespace = "me.ogarcia.meshctl"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "me.ogarcia.meshctl"
        minSdk = 24
        targetSdk = 37
        versionCode = meshctlVersionCode
        versionName = meshctlVersion
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
            if (!keystoreProperties.isEmpty) {
                signingConfig = signingConfigs.getByName("release")
            }
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

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    // Radios by USB, without root: the serial chips' drivers, over
    // Android's own USB, as the Meshtastic app has them.
    implementation("com.github.mik3y:usb-serial-for-android:3.12.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
