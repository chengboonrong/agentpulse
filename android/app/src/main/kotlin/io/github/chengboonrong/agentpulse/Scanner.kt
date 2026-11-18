package io.github.chengboonrong.agentpulse

import android.util.Size
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.view.CameraController
import androidx.camera.view.LifecycleCameraController
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BinaryBitmap
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicReference

/**
 * The camera, with ZXing reading each frame (no Play services); calls [found], on the
 * main thread, with each new AgentPulse link it reads (a good one ends the scan; a bad
 * one is reported once). The Mac draws its code black on white.
 */
@Composable
fun Scanner(found: (String) -> Unit, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val latest by rememberUpdatedState(found)
    val controller = remember {
        LifecycleCameraController(context).apply {
            setEnabledUseCases(CameraController.IMAGE_ANALYSIS)
            // Not the 640x480 default: the link's code is ~55 modules, too few pixels each.
            imageAnalysisResolutionSelector = ResolutionSelector.Builder()
                .setResolutionStrategy(ResolutionStrategy(Size(1280, 720), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER))
                .build()
        }
    }
    DisposableEffect(owner) {
        val frames = Executors.newSingleThreadExecutor()
        val last = AtomicReference<String?>(null)
        controller.setImageAnalysisAnalyzer(frames) { image ->
            val text = image.use {
                val y = it.planes[0]
                val bytes = ByteArray(y.buffer.remaining()).also { b -> y.buffer.get(b) }
                readQr(bytes, y.rowStride, it.width, it.height)
            }
            if (text != null && isPairLink(text) && last.getAndSet(text) != text) {
                ContextCompat.getMainExecutor(context).execute { latest(text) }
            }
        }
        controller.bindToLifecycle(owner)
        onDispose {
            controller.unbind()
            controller.clearImageAnalysisAnalyzer()
            frames.shutdown()
        }
    }
    // A TextureView, so the rounded corners clip it.
    AndroidView({
        PreviewView(it).apply {
            implementationMode = PreviewView.ImplementationMode.COMPATIBLE
            this.controller = controller
        }
    }, modifier)
}

fun isPairLink(text: String) = text.trim().lowercase().startsWith("agentpulse://pair")

/** A QR code's text in a frame's luminance (Y) plane, [rowStride] bytes a row; else null. */
fun readQr(y: ByteArray, rowStride: Int, width: Int, height: Int): String? = try {
    val source = PlanarYUVLuminanceSource(y, rowStride, height, 0, 0, width, height, false)
    QRCodeReader().decode(BinaryBitmap(HybridBinarizer(source))).text
} catch (_: Exception) {
    // Not only ReaderException: anything thrown here would kill the analysis thread's process.
    null
}
