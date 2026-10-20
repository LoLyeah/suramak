// M13b shell spike — root build file. Versions are pinned here only; the
// compatibility chain is recorded in docs/reports/M13b.md:
//   Gradle 8.9 (wrapper) ←→ AGP 8.7.3 ←→ compileSdk 35 (build-tools 35.0.0)
//   Kotlin 2.0.21 (AGP 8.7 supports the 2.0 line)
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
}
