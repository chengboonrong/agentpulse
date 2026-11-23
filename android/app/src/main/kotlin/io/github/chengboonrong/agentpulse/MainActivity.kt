package io.github.chengboonrong.agentpulse

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import io.github.chengboonrong.agentpulse.link.PhoneClient
import io.github.chengboonrong.agentpulse.link.PhoneStore
import java.io.File

class MainActivity : ComponentActivity() {
    private val model: AppModel by viewModels {
        viewModelFactory {
            initializer {
                DebugHooks.fixture(this@MainActivity, intent) ?: AppModel(
                    phoneClient(applicationContext),
                    finder = MacFinder(applicationContext),
                    context = applicationContext,
                ).also { DebugHooks.e2e(it, intent) }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) open(intent)
        setContent {
            val dark = isSystemInDarkTheme()
            val colors = when {
                Build.VERSION.SDK_INT >= 31 -> if (dark) dynamicDarkColorScheme(LocalContext.current) else dynamicLightColorScheme(LocalContext.current)
                dark -> darkColorScheme()
                else -> lightColorScheme()
            }
            MaterialTheme(colorScheme = colors) { AppScreen(model) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        open(intent)
        DebugHooks.e2e(model, intent)
    }

    /** An agentpulse://pair link opened from a browser, a scan or `adb`. */
    private fun open(intent: Intent) {
        intent.dataString?.let { model.open(it) }
    }

    override fun onStart() {
        super.onStart()
        model.start()
    }

    override fun onStop() {
        // Not for a rotation: start() would send `seen` to every Mac again.
        if (!isChangingConfigurations) {
            model.stop()
            RefreshWorker.now(applicationContext)
        }
        super.onStop()
    }
}

/** The pairings and their secrets, shared by the app, its widget and [RefreshWorker] in one process. */
fun phoneClient(context: Context) =
    PhoneClient(PhoneStore(File(context.noBackupFilesDir, "link"), KeystoreSecrets(context.applicationContext)))
