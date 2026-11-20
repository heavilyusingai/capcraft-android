plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.baselineProfile)
}

android {
    namespace = "io.github.artcraftandroid.capcraft.baselineprofile"
    compileSdk = 37

    defaultConfig {
        minSdk = 29
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    targetProjectPath = ":app"
}

dependencies {
    implementation(libs.benchmarkMacroJunit4)
    implementation(libs.uiAutomator)
    testImplementation(libs.junit)
}
