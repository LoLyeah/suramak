// M13b shell spike — Gradle settings.
//
// One module (`:app`). Everything native is built by cargo-ndk beforehand
// (`scripts/android/build-apk.sh`); Gradle only compiles the Kotlin shim and
// packages `app/src/main/jniLibs/**` into the APK. There is deliberately no
// Gradle Rust plugin: the Rust build must stay reproducible outside Gradle.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "SuramakSpike"
include(":app")
