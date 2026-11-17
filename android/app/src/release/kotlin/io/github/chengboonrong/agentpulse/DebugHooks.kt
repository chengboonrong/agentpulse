package io.github.chengboonrong.agentpulse

import android.content.Context
import android.content.Intent

/** Release builds: no fixtures and no e2e hooks (the debug source set has them). */
object DebugHooks {
    fun fixture(context: Context, intent: Intent): AppModel? = null

    fun e2e(model: AppModel, intent: Intent) {}
}
