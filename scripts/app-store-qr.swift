import CoreImage
import Foundation

let url = "https://apps.apple.com/app/agentpulse/id6835137307"

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    exit(1)
}

guard let filter = CIFilter(name: "CIQRCodeGenerator") else { fail("No QR generator") }
filter.setValue(Data(url.utf8), forKey: "inputMessage")
filter.setValue("M", forKey: "inputCorrectionLevel")
guard let image = filter.outputImage else { fail("No QR image") }
let w = Int(image.extent.width), h = Int(image.extent.height)
var modules = [UInt8](repeating: 0, count: w * h * 4)
let context = CIContext()
let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
context.render(
    image, toBitmap: &modules, rowBytes: w * 4, bounds: image.extent, format: .RGBA8,
    colorSpace: colorSpace)

// Decode the modules we read back, enlarged with nearest neighbours.
let scale = 10
var bitmap = [UInt8](repeating: 255, count: w * h * 4 * scale * scale)
for y in 0..<(h * scale) {
    for x in 0..<(w * scale) {
        if modules[(y / scale * w + x / scale) * 4] < 128 {
            let offset = (y * w * scale + x) * 4
            bitmap[offset] = 0
            bitmap[offset + 1] = 0
            bitmap[offset + 2] = 0
        }
    }
}
let enlarged = CIImage(
    bitmapData: Data(bitmap), bytesPerRow: w * scale * 4,
    size: CGSize(width: w * scale, height: h * scale), format: .RGBA8,
    colorSpace: colorSpace)
guard let detector = CIDetector(
    ofType: CIDetectorTypeQRCode, context: context,
    options: [CIDetectorAccuracy: CIDetectorAccuracyHigh]) else { fail("No QR detector") }
let decoded = detector.features(in: enlarged).compactMap { ($0 as? CIQRCodeFeature)?.messageString }
guard decoded == [url] else { fail("QR did not decode to \(url): \(decoded)") }

print("const APP_STORE_QR: &str = \"")
for y in 0..<h {
    print((0..<w).map { modules[(y * w + $0) * 4] < 128 ? "#" : "." }.joined())
}
print("\";")
