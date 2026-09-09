// The build script classpath `gen/android/build.gradle.kts` carries, which
// `tauri android init` writes and a fresh checkout does not have. The two
// must name the same Android Gradle plugin and Kotlin plugin: this root
// compiles `:edet-keystore` and `:tauri-android` exactly as the app project
// does, or a green run here says nothing about the module the app links.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}
