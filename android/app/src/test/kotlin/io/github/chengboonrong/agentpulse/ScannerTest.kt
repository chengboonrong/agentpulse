package io.github.chengboonrong.agentpulse

import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ScannerTest {
    private val link = "agentpulse://pair?v=1&r=https://relay.example.com&id=AQEBAQEBAQEBAQEBAQEBAQ" +
        "&k=CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk&s=AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI&n=Studio"

    /**
     * A camera frame's Y plane: the code black on white, as the Mac draws it, rows padded to
     * [stride] but the last, as YUV_420_888 buffers often end.
     */
    private fun frame(text: String, width: Int, height: Int, stride: Int): ByteArray {
        val side = minOf(width, height) * 3 / 4
        val qr = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, side, side)
        val (left, top) = (width - side) / 2 to (height - side) / 2
        return ByteArray(stride * (height - 1) + width) { i ->
            val (x, y) = i % stride to i / stride
            val dark = x in left until left + side && y in top until top + side && qr[x - left, y - top]
            (if (dark) 20 else 230).toByte()
        }
    }

    @Test
    fun readsTheMacsCodeFromAPaddedFrame() {
        assertEquals(link, readQr(frame(link, 640, 480, stride = 704), 704, 640, 480))
    }

    @Test
    fun aFrameWithoutACodeReadsNothing() {
        assertNull(readQr(ByteArray(640 * 480) { 128.toByte() }, 640, 640, 480))
    }

    @Test
    fun onlyPairLinksCount() {
        assertTrue(isPairLink(link))
        assertTrue(isPairLink(" AgentPulse://pair?v=1"))
        assertFalse(isPairLink("https://example.com"))
    }
}
