plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "app.suramak.spike"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.suramak.spike"
        // minSdk 29 is the M13 floor (PLAN-M13-ANDROID §4); targetSdk is the
        // platform installed in the SDK (android-35) — keeping it equal to
        // compileSdk avoids the "targetSdk > compileSdk" AGP refusal.
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.3.0-m13b"

        // Only the phone ABI: this APK exists to be installed by the owner.
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            // Never built in M13b (no signing, no packaging work — M13h).
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        buildConfig = false
    }

    packaging {
        jniLibs {
            // Store the .so uncompressed and page-aligned (16 KB devices): the
            // loader maps it straight from the APK; zipalign handles alignment.
            useLegacyPackaging = false
        }
    }
}

dependencies {
    // GameActivity AAR. 3.0.5 is the version whose Java native-method table
    // matches the C++ glue vendored by android-activity 0.6.1 exactly — see
    // docs/reports/M13b.md §"Version pair" for the evidence (all 24 JNI
    // methods, including the 16-argument onTouchEventNative and the
    // keyboard/editor-action callbacks; 2.0.x lacks two of them and has a
    // 2-argument onTouchEventNative, so RegisterNatives aborts at startup).
    implementation("androidx.games:games-activity:3.0.5")
    // GameActivity extends AppCompatActivity, so an AppCompat theme (and the
    // Insets class its native code looks up) must be on the classpath.
    implementation("androidx.appcompat:appcompat:1.7.0")
}
