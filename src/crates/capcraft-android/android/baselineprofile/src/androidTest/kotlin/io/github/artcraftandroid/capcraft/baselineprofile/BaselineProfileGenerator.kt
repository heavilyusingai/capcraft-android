package io.github.artcraftandroid.capcraft.baselineprofile

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.Until
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Generates Baseline Profiles for CapCraft's critical user journeys (CUJs).
 *
 * Baseline Profiles tell the Android Runtime (ART) which code paths to
 * Ahead-Of-Time (AOT) compile at install time, improving cold-start speed
 * by ~30% and reducing jank on the frames they cover.
 *
 * The generated profile is written to baselineprofile/build/outputs/ and
 * copied into src/main/baseline-prof.txt by the baselineProfile Gradle
 * plugin (see app/build.gradle.kts `saveInSrc = true`, `mergeIntoMain = true`).
 *
 * Run with: ./gradlew :baselineprofile:generateReleaseBaselineProfile
 *
 * Official guidance: https://developer.android.com/topic/performance/baselineprofiles/create-baselineprofile
 */
@RunWith(AndroidJUnit4::class)
class BaselineProfileGenerator {

    private val packageName =
        InstrumentationRegistry.getArguments().getString("packageName") ?: "io.github.artcraftandroid.capcraft"

    @get:Rule
    val baselineProfileRule = BaselineProfileRule()

    /** Journey 1: cold app start - home screen opens, recent projects list renders. */
    @Test
    fun appStartup() = baselineProfileRule.collect(
        packageName = packageName,
        includeInStartupProfile = true,
    ) {
        startActivityAndWait()
        device.waitForIdle()
    }

    /** Journey 2: open export sheet - resolution/fps/bitrate sliders render. */
    @Test
    fun openExportSheet() = baselineProfileRule.collect(packageName = packageName) {
        startActivityAndWait()
        device.waitForIdle()
        device.findObject(By.desc("Export")).clickAndWait(Until.newWindow(), 5_000)
        device.waitForIdle()
    }

    /** Journey 3: scroll the media picker grid - thumbnails load and recycle. */
    @Test
    fun scrollMediaPicker() = baselineProfileRule.collect(packageName = packageName) {
        startActivityAndWait()
        device.waitForIdle()
        device.findObject(By.desc("Media picker")).clickAndWait(Until.newWindow(), 5_000)
        device.waitForIdle()
        val grid = device.findObject(By.res("media_picker_grid"))
        grid.setGestureMargin(device.displayWidth / 5)
        grid.fling(Direction.DOWN)
        grid.fling(Direction.UP)
        device.waitForIdle()
    }

    /** Journey 4: scrub the timeline - playhead moves, preview redraws. */
    @Test
    fun scrubTimeline() = baselineProfileRule.collect(packageName = packageName) {
        startActivityAndWait()
        device.waitForIdle()
        device.findObject(By.desc("Editor")).clickAndWait(Until.newWindow(), 5_000)
        device.waitForIdle()
        val timeline = device.findObject(By.res("timeline"))
        timeline.setGestureMargin(device.displayWidth / 5)
        timeline.fling(Direction.LEFT)
        timeline.fling(Direction.RIGHT)
        device.waitForIdle()
    }
}
