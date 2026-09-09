import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// edet: release signing comes from the ENVIRONMENT, never from the tree.
//
// A release APK or AAB is only installable signed, and the key cannot live
// here: `.github/workflows/android-release.yaml` decodes it from a repository
// secret into a file outside the checkout and names it through these four
// variables. With any of them unset the config is not created at all and
// `release` carries no `signingConfig` — so a local `cargo tauri android
// build` still produces an unsigned release rather than failing on a key
// nobody has, and the workflow's own `apksigner verify` is what refuses to
// publish one.
val edetStoreFile: String? = System.getenv("ANDROID_STORE_FILE")
val edetStorePassword: String? = System.getenv("ANDROID_STORE_PASSWORD")
val edetKeyAlias: String? = System.getenv("ANDROID_KEY_ALIAS")
val edetKeyPassword: String? = System.getenv("ANDROID_KEY_PASSWORD")
val edetSigningReady = !edetStoreFile.isNullOrBlank() &&
    !edetStorePassword.isNullOrBlank() &&
    !edetKeyAlias.isNullOrBlank() &&
    !edetKeyPassword.isNullOrBlank()

android {
    compileSdk = 36
    namespace = "org.edet.client"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "org.edet.client"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (edetSigningReady) {
            create("release") {
                storeFile = file(edetStoreFile!!)
                storePassword = edetStorePassword
                keyAlias = edetKeyAlias
                keyPassword = edetKeyPassword
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (edetSigningReady) {
                signingConfig = signingConfigs.getByName("release")
            }
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    // edet: Java 17 rather than the template's 8 — see
    // `edet-keystore/build.gradle.kts`, which this module consumes and which
    // therefore cannot be raised without raising this one too. `compileOptions`
    // is set explicitly because AGP's default is still Java 8 and the template
    // leaves it unset, so the javac half of the deprecation warning came from
    // here even though only `jvmTarget` was written down.
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation(project(":edet-keystore"))
    // `MainActivity` reads `BackgroundMode.armed` from this one, so it is a
    // compile dependency of the app module and not only a runtime one.
    implementation(project(":edet-background"))
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")