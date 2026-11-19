package io.github.artcraftandroid.capcraft

import android.app.PictureInPictureParams
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Rational
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.windowsizeclass.WindowWidthSizeClass
import androidx.compose.material3.windowsizeclass.calculateWindowSizeClass
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalActivity
import androidx.core.content.IntentCompat
import io.github.artcraftandroid.capcraft.engine.userMessage
import io.github.artcraftandroid.capcraft.ui.CapCraftTheme
import io.github.artcraftandroid.capcraft.ui.Palette
import io.github.artcraftandroid.capcraft.ui.Type
import io.github.artcraftandroid.capcraft.ui.editor.EditorScreen
import io.github.artcraftandroid.capcraft.ui.home.HomeScreen

class MainActivity : ComponentActivity() {
    /** Files shared to the app while it runs, for the home screen to turn into a project. */
    private val shared = mutableStateOf<List<Uri>>(emptyList())

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.dark(android.graphics.Color.BLACK),
            navigationBarStyle = SystemBarStyle.dark(android.graphics.Color.BLACK),
        )
        super.onCreate(savedInstanceState)
        shared.value = sharedUris(intent)
        val app = application as CapCraftApplication
        setContent {
            CapCraftTheme {
                var ready by remember { mutableStateOf<Result<Unit>?>(null) }
                LaunchedEffect(Unit) { ready = app.engine.await() }
                when (val r = ready) {
                    null -> Loading()
                    else -> r.fold(
                        onSuccess = { App(shared.value) { shared.value = emptyList() } },
                        onFailure = { Failed(it.userMessage) },
                    )
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        shared.value = sharedUris(intent)
    }

    /**
     * Enter picture-in-picture mode so the user can keep watching the preview while using
     * another app. Only offered when the editor is open and there is something to show.
     */
    fun enterPictureInPicture() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            val params = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                PictureInPictureParams.Builder()
                    .setAspectRatio(Rational(16, 9))
                    .build()
            } else {
                PictureInPictureParams.Builder().build()
            }
            enterPictureInPictureMode(params)
        }
    }

    override fun onPictureInPictureModeChanged(isInPictureInPictureMode: Boolean, newConfig: android.content.res.Configuration?) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        // When leaving PiP, resume any paused playback; when entering, the preview continues
        // in the small window and the engine keeps rendering.
    }

    private fun sharedUris(intent: Intent?): List<Uri> {
        intent ?: return emptyList()
        return when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE ->
                IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            // a video or photo opened from Files, Gallery or another app's share sheet
            Intent.ACTION_VIEW -> listOfNotNull(intent.data)
            else -> emptyList()
        }
    }
}

@Composable
private fun App(shared: List<Uri>, consumeShared: () -> Unit) {
    // Which project is open survives the phone taking the process back: the path is all it takes to
    // reopen one, and the editor has already saved the work. Held in rememberSaveable, not remember,
    // because the app coming back from a call is the same app, at the same place.
    var openPath by rememberSaveable { mutableStateOf<String?>(null) }
    var openThen by rememberSaveable { mutableStateOf<String?>(null) }
    // Files that start this session's project. Deliberately not saved: they are imported once, when
    // the editor first opens, and a restored editor already holds what they brought in.
    var opening by remember { mutableStateOf<List<Uri>>(emptyList()) }

    val windowSizeClass = calculateWindowSizeClass(
        activity = LocalActivity.current,
        modifier = Modifier.fillMaxSize(),
    )
    val expanded = windowSizeClass.widthSizeClass == WindowWidthSizeClass.Expanded

    Box(Modifier.fillMaxSize()) {
        val path = openPath
        if (expanded) {
            // foldable / tablet: two-pane, editor and home side by side
            Row(Modifier.fillMaxSize()) {
                HomeScreen(
                    shared = shared,
                    consumeShared = consumeShared,
                    open = { project, incoming, then ->
                        opening = incoming
                        openThen = then
                        openPath = project
                    },
                    modifier = Modifier.weight(1f),
                )
                if (path != null) {
                    EditorScreen(
                        path = path,
                        incoming = opening,
                        then = openThen,
                        shared = shared,
                        consumeShared = consumeShared,
                        close = {
                            openPath = null
                            openThen = null
                            opening = emptyList()
                        },
                        modifier = Modifier.weight(1f),
                    )
                } else {
                    Box(Modifier.fillMaxSize())
                }
            }
        } else {
            // phone: single pane, one screen at a time
            if (path == null) {
                HomeScreen(
                    shared = shared,
                    consumeShared = consumeShared,
                    open = { project, incoming, then ->
                        opening = incoming
                        openThen = then
                        openPath = project
                    },
                )
            } else {
                EditorScreen(
                    path = path,
                    incoming = opening,
                    then = openThen,
                    shared = shared,
                    consumeShared = consumeShared,
                    close = {
                        openPath = null
                        openThen = null
                        opening = emptyList()
                    },
                )
            }
        }
        io.github.artcraftandroid.capcraft.ui.common.MediaPickerHost()
    }
}

@Composable
private fun Loading() {
    Box(Modifier.fillMaxSize().background(Palette.Black), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = Palette.Accent)
    }
}

@Composable
private fun Failed(message: String) {
    Box(Modifier.fillMaxSize().background(Palette.Black), contentAlignment = Alignment.Center) {
        Text("The editor could not start: $message", style = Type.Body)
    }
}
