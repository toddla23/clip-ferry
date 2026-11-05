package app.clipferry.android

import android.util.Log
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** clip-ferry 초대 QR의 앞부분 (core/src/pairing/invite.rs `qr_string`). */
const val INVITE_QR_PREFIX = "clipferry://pair/"

/**
 * 카메라 미리보기와 QR 인식. clip-ferry 초대 QR을 처음 읽으면 [onInvite]를 한 번 부른다.
 * 다른 QR이 보이면 [onOtherQr]를 부른다(안내용). 카메라 권한은 부르는 쪽이 미리 받아 둔다.
 * 인식은 기기 안(ML Kit 번들 모델)에서만 하고, 영상은 어디에도 보내지 않는다.
 */
@Composable
fun QrScanner(
    modifier: Modifier = Modifier,
    onInvite: (String) -> Unit,
    onOtherQr: () -> Unit = {},
    onCameraError: (String) -> Unit = {},
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val currentOnInvite = rememberUpdatedState(onInvite)
    val currentOnOther = rememberUpdatedState(onOtherQr)
    val currentOnError = rememberUpdatedState(onCameraError)
    val previewView = remember { PreviewView(context) }

    DisposableEffect(lifecycleOwner) {
        val done = AtomicBoolean(false)
        val executor = Executors.newSingleThreadExecutor()
        val scanner = BarcodeScanning.getClient(
            BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
        )
        val mainExecutor = ContextCompat.getMainExecutor(context)
        val providerFuture = ProcessCameraProvider.getInstance(context)
        var provider: ProcessCameraProvider? = null

        val analysis = ImageAnalysis.Builder()
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
        analysis.setAnalyzer(executor) { proxy: ImageProxy ->
            val media = proxy.image
            if (media == null || done.get()) {
                proxy.close()
                return@setAnalyzer
            }
            val image = InputImage.fromMediaImage(media, proxy.imageInfo.rotationDegrees)
            scanner.process(image)
                .addOnSuccessListener(mainExecutor) { codes ->
                    val values = codes.mapNotNull { it.rawValue }
                    val invite = values.firstOrNull { it.startsWith(INVITE_QR_PREFIX) }
                    if (invite != null) {
                        if (done.compareAndSet(false, true)) currentOnInvite.value(invite)
                    } else if (values.isNotEmpty()) {
                        currentOnOther.value()
                    }
                }
                .addOnCompleteListener { proxy.close() }
        }

        providerFuture.addListener({
            try {
                val p = providerFuture.get()
                provider = p
                val preview = Preview.Builder().build().also {
                    it.surfaceProvider = previewView.surfaceProvider
                }
                p.unbindAll()
                p.bindToLifecycle(
                    lifecycleOwner,
                    CameraSelector.DEFAULT_BACK_CAMERA,
                    preview,
                    analysis,
                )
            } catch (e: Exception) {
                Log.w("clip-ferry", "카메라를 열지 못함: ${e.javaClass.simpleName}")
                currentOnError.value("카메라를 열 수 없습니다. 코드 입력으로 참여하세요")
            }
        }, mainExecutor)

        onDispose {
            done.set(true)
            provider?.unbindAll()
            analysis.clearAnalyzer()
            scanner.close()
            executor.shutdown()
        }
    }

    AndroidView(factory = { previewView }, modifier = modifier)
}
