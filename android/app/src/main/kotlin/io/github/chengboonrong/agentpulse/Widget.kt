package io.github.chengboonrong.agentpulse

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.text.format.DateFormat
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalSize
import androidx.glance.action.actionStartActivity
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.background
import androidx.glance.color.ColorProvider
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column
import androidx.glance.layout.ContentScale
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxHeight
import androidx.glance.layout.fillMaxSize
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.github.chengboonrong.agentpulse.link.Pairing
import java.util.Date
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** What the widget shows for the first paired Mac (ios/Widget, macos/Shared's `WidgetState`). */
sealed interface WidgetState {
    data object Unpaired : WidgetState
    /** Paired, but no state from the Mac yet. */
    data class Waiting(val mac: String) : WidgetState
    /** An unreadable or unknown-version state, or the Mac app marked it stopped. */
    data object NotRunning : WidgetState
    /** Nothing newer than [AGE_LIMIT_MS]: the Mac may be asleep, off or offline; claim neither. */
    data class Stale(val sinceMs: Long) : WidgetState
    data class Live(val payload: WidgetPayload) : WidgetState

    companion object {
        /** As the Mac and iPhone widgets; well past the 15-minute refresh. */
        const val AGE_LIMIT_MS = 75 * 60_000L

        fun of(pairing: Pairing?, nowMs: Long): WidgetState {
            val state = pairing?.lastState ?: return pairing?.let { Waiting(it.macName) } ?: Unpaired
            val p = WidgetPayload.decode(state)
            return when {
                p == null || p.stopped -> NotRunning
                nowMs - p.generatedAtMs > AGE_LIMIT_MS -> Stale(p.generatedAtMs)
                else -> Live(p)
            }
        }
    }
}

/**
 * The home-screen widget: the first paired Mac's stored state, small (the cat and the
 * headline) or medium (and its sessions). [RefreshWorker] fetches through the relay and
 * redraws it; never direct.
 */
class AgentPulseWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Responsive(setOf(SMALL, MEDIUM))

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val pairing = withContext(Dispatchers.IO) {
            runCatching { phoneClient(context).store.pairings().firstOrNull() }.getOrNull()
        }
        val state = WidgetState.of(pairing, System.currentTimeMillis())
        val cat = (state as? WidgetState.Live)?.payload?.cat?.let(::catBitmap)
        provideContent { Content(state, cat, context) }
    }

    private companion object {
        val SMALL = DpSize(110.dp, 110.dp)
        val MEDIUM = DpSize(250.dp, 110.dp)
    }
}

class AgentPulseWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = AgentPulseWidget()

    override fun onUpdate(context: Context, appWidgetManager: android.appwidget.AppWidgetManager, appWidgetIds: IntArray) {
        super.onUpdate(context, appWidgetManager, appWidgetIds)
        RefreshWorker.schedule(context)
    }

    override fun onDisabled(context: Context) {
        super.onDisabled(context)
        WorkManager.getInstance(context).cancelUniqueWork(RefreshWorker.PERIODIC)
    }
}

/** `GET down` for the first paired Mac, then redraws the widgets; nothing while there are none. */
class RefreshWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val context = applicationContext
        if (GlanceAppWidgetManager(context).getGlanceIds(AgentPulseWidget::class.java).isEmpty()) return Result.success()
        withContext(Dispatchers.IO) {
            // Offline or removed: the stored state, which turns stale on its own.
            runCatching {
                val client = phoneClient(context)
                client.store.pairings().firstOrNull()?.let(client::fetchDown)
            }
        }
        AgentPulseWidget().updateAll(context)
        return Result.success()
    }

    companion object {
        const val PERIODIC = "widget-refresh"

        /** Every 15 minutes, WorkManager's shortest period; kept across calls. */
        fun schedule(context: Context) {
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(
                PERIODIC, ExistingPeriodicWorkPolicy.KEEP,
                PeriodicWorkRequestBuilder<RefreshWorker>(15, TimeUnit.MINUTES).build(),
            )
        }

        /** Once now: the app is going to the background with what it has seen. */
        fun now(context: Context) {
            WorkManager.getInstance(context).enqueueUniqueWork(
                "widget-now", ExistingWorkPolicy.REPLACE, OneTimeWorkRequestBuilder<RefreshWorker>().build(),
            )
        }
    }
}

/** macos/Shared's widget colours: cream ground, lilac stage. */
private object WidgetColors {
    val ground = ColorProvider(day = Color(0xFFFFF6E8), night = Color(0xFF1F1823))
    val stage = ColorProvider(day = Color(0xFFF1EAF3), night = Color(0xFF30263A))
    val primary = ColorProvider(day = Color(0xFF1F1823), night = Color(0xFFFFF6E8))
    val secondary = ColorProvider(day = Color(0x991F1823), night = Color(0x99FFF6E8))
}

@Composable
private fun Content(state: WidgetState, cat: Bitmap?, context: Context) {
    val medium = LocalSize.current.width >= 250.dp
    Box(
        GlanceModifier.fillMaxSize().background(WidgetColors.ground).padding(12.dp)
            .clickable(actionStartActivity<MainActivity>()),
        contentAlignment = Alignment.BottomStart,
    ) {
        when (state) {
            WidgetState.Unpaired -> Title("Pair a Mac in AgentPulse")
            is WidgetState.Waiting -> Title("Waiting for ${state.mac}")
            WidgetState.NotRunning -> Sleeping("AgentPulse isn't running", "Open the app to resume", medium)
            is WidgetState.Stale -> Sleeping("No update since ${time(context, state.sinceMs)}", "Open AgentPulse to refresh", medium)
            is WidgetState.Live -> if (medium) {
                Row(GlanceModifier.fillMaxSize()) {
                    Summary(state.payload, cat, context, GlanceModifier.width(132.dp).fillMaxHeight())
                    Spacer(GlanceModifier.width(14.dp))
                    Detail(state.payload, GlanceModifier.defaultWeight().fillMaxHeight())
                }
            } else {
                Summary(state.payload, cat, context, GlanceModifier.fillMaxSize())
            }
        }
    }
}

@Composable
private fun Title(text: String) = Text(text, style = TextStyle(WidgetColors.primary, 16.sp, FontWeight.Bold), maxLines = 3)

@Composable
private fun Sleeping(title: String, subtitle: String, medium: Boolean) = Column {
    Title(title)
    if (medium) Text(subtitle, style = TextStyle(WidgetColors.secondary, 14.sp))
}

@Composable
private fun Summary(p: WidgetPayload, cat: Bitmap?, context: Context, modifier: GlanceModifier) = Column(modifier) {
    if (cat != null) {
        Image(
            ImageProvider(cat), "Cat, ${p.cat.state.replace('_', ' ')}",
            GlanceModifier.defaultWeight().fillMaxWidth().background(WidgetColors.stage).cornerRadius(12.dp).padding(6.dp),
            contentScale = ContentScale.Fit,
        )
        Spacer(GlanceModifier.height(4.dp))
    } else {
        Spacer(GlanceModifier.defaultWeight())
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(p.agents.headline, style = TextStyle(WidgetColors.primary, 15.sp, FontWeight.Bold), maxLines = 1)
        lead(p.agents)?.let { (status, n) ->
            Spacer(GlanceModifier.width(6.dp))
            Ribbon(status, n)
        }
    }
    Text("${p.today.cost} today", style = TextStyle(WidgetColors.secondary, 13.sp), maxLines = 1)
    Text("As of ${time(context, p.generatedAtMs)}", style = TextStyle(WidgetColors.secondary, 11.sp), maxLines = 1)
}

@Composable
private fun Detail(p: WidgetPayload, modifier: GlanceModifier) = Column(modifier) {
    if (p.sessions.isEmpty()) {
        Text("No live sessions", style = TextStyle(WidgetColors.secondary, 12.sp))
    } else {
        // A column each: Glance drops a column's children past ten.
        p.sessions.take(3).forEach { s ->
            Column(GlanceModifier.padding(bottom = 5.dp)) {
                Ribbon(s.status)
                Text(s.project, style = TextStyle(WidgetColors.primary, 12.sp, FontWeight.Bold), maxLines = 1)
            }
        }
    }
    Spacer(GlanceModifier.defaultWeight())
    Text("Today ${p.today.tokens} tokens", style = TextStyle(WidgetColors.secondary, 12.sp), maxLines = 1)
}

/** A state's ribbon, white on its colour; with [count], as beside the headline. */
@Composable
private fun Ribbon(status: String, count: Int? = null) {
    val (color, label) = Palette.ribbon(status)
    val text = when {
        count == null -> label
        count > 1 && status == "Waiting" -> "$count need you"
        else -> "$count $label"
    }
    Text(
        text, GlanceModifier.background(color).cornerRadius(8.dp).padding(horizontal = 6.dp, vertical = 1.dp),
        style = TextStyle(ColorProvider(Color.White), 11.sp, FontWeight.Bold), maxLines = 1,
    )
}

/** Who needs you leads, as in the app's roster. */
private fun lead(a: WidgetPayload.Agents) = when {
    a.waiting > 0 -> "Waiting" to a.waiting
    a.active > 0 -> "Active" to a.active
    a.idle > 0 -> "Idle" to a.idle
    else -> null
}

private fun time(context: Context, ms: Long) = DateFormat.getTimeFormat(context).format(Date(ms))

/**
 * The cat sprite (32 cells wide) at [scale] pixels a cell, which the launcher fits. Capped,
 * as a widget update's bitmaps share a memory limit.
 */
private fun catBitmap(cat: WidgetPayload.Cat, scale: Int = 8): Bitmap {
    val w = cat.width.coerceIn(1, 64)
    val h = cat.height.coerceIn(1, 64)
    val bitmap = Bitmap.createBitmap(w * scale, h * scale, Bitmap.Config.ARGB_8888)
    val canvas = Canvas(bitmap)
    val paint = Paint()
    cat.rows.take(h).forEachIndexed { y, row ->
        row.take(w).forEachIndexed { x, ch ->
            val c = cat.palette[ch.toString()]?.takeIf { it.size == 4 } ?: return@forEachIndexed
            paint.color = android.graphics.Color.argb(c[3], c[0], c[1], c[2])
            canvas.drawRect((x * scale).toFloat(), (y * scale).toFloat(), ((x + 1) * scale).toFloat(), ((y + 1) * scale).toFloat(), paint)
        }
    }
    return bitmap
}
