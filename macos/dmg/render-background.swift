// Renders the DMG window background: background.png (660x420) and background@2x.png.
// Run from the repo root: swift macos/dmg/render-background.swift
// Text is drawn with the system rounded face, so no font file ships. Colours are DESIGN.md's.
// If the window size or icon positions change, change them in make-ds-store.applescript too.
import AppKit

let size = CGSize(width: 660, height: 420)
let app = CGPoint(x: 170, y: 190)   // icon centres, top-left origin, as Finder stores them
let apps = CGPoint(x: 490, y: 190)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat(hex >> 16 & 0xFF) / 255, green: CGFloat(hex >> 8 & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}
let cream = color(0xFFF6E8), ink = color(0x2B2430), muted = color(0x66596C)
let peach = color(0xFFE9DC), periwinkle = color(0xE4E8F3)

func rounded(_ size: CGFloat, _ weight: NSFont.Weight) -> NSFont {
    let base = NSFont.systemFont(ofSize: size, weight: weight)
    return NSFont(descriptor: base.fontDescriptor.withDesign(.rounded) ?? base.fontDescriptor, size: size) ?? base
}

func text(_ s: String, _ font: NSFont, _ fill: CGColor, centreX: CGFloat, top: CGFloat) {
    let a = NSAttributedString(string: s, attributes: [.font: font, .foregroundColor: NSColor(cgColor: fill)!])
    let w = a.size().width
    a.draw(at: CGPoint(x: centreX - w / 2, y: top))
}

// First frame of the running-cat APNG, drawn with whole pixels.
let catRep = NSImage(contentsOfFile: "assets/cats/anim-running.png")!
    .representations.first as! NSBitmapImageRep
catRep.setProperty(.currentFrame, withValue: 0)
let cat = catRep.cgImage!

// The menu-bar template cat, tinted muted, as a corner mark.
let mark = NSImage(contentsOfFile: "assets/logo/menubar-template@2x.png")!
    .cgImage(forProposedRect: nil, context: nil, hints: nil)!

func paw(_ c: CGContext, at p: CGPoint, scale s: CGFloat) {
    // Toes point right, the way the cat walks.
    c.fillEllipse(in: CGRect(x: p.x - 6 * s, y: p.y - 5 * s, width: 8 * s, height: 10 * s))
    for (dx, dy) in [(6.5, -6.0), (9.5, -2.2), (9.5, 2.2), (6.5, 6.0)] {
        c.fillEllipse(in: CGRect(x: p.x + dx * s - 2 * s, y: p.y + dy * s - 2 * s, width: 4 * s, height: 4 * s))
    }
}

func render(scale: CGFloat, to path: String) {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale),
                               pixelsHigh: Int(size.height * scale), bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = size
    let g = NSGraphicsContext(bitmapImageRep: rep)!
    NSGraphicsContext.current = NSGraphicsContext(cgContext: g.cgContext, flipped: true)
    let c = g.cgContext
    c.translateBy(x: 0, y: size.height)   // top-left origin, y down
    c.scaleBy(x: 1, y: -1)

    c.setFillColor(cream)
    c.fill(CGRect(origin: .zero, size: size))

    // A pastel stage under each icon, as each cat stands on one in the app.
    for (p, fill) in [(app, peach), (apps, periwinkle)] {
        c.setFillColor(fill)
        c.addPath(CGPath(roundedRect: CGRect(x: p.x - 92, y: p.y - 92, width: 184, height: 184),
                         cornerWidth: 14, cornerHeight: 14, transform: nil))
        c.fillPath()
    }

    // Paw prints walking right, smaller as they near Applications, with the cat ahead of them.
    c.setFillColor(muted)
    for i in 0..<4 {
        let x = 284 + CGFloat(i) * 28
        paw(c, at: CGPoint(x: x, y: app.y + 18 + (i % 2 == 0 ? 7 : -7)), scale: 1 - CGFloat(i) * 0.12)
    }
    c.saveGState()
    c.interpolationQuality = .none
    c.translateBy(x: 0, y: app.y - 42)    // un-flip for the bitmap
    c.scaleBy(x: 1, y: -1)
    c.draw(cat, in: CGRect(x: 298, y: -32, width: 64, height: 32))
    c.restoreGState()

    text("Drag AgentPulse to Applications", rounded(20, .semibold), ink, centreX: size.width / 2, top: 318)
    text("macOS 14+ · Apple Silicon · signed and notarized", rounded(11, .regular), muted,
         centreX: size.width / 2, top: 384)

    c.saveGState()
    c.interpolationQuality = .none
    let markRect = CGRect(x: 20, y: 382, width: 16, height: 14)
    c.translateBy(x: 0, y: markRect.midY * 2)
    c.scaleBy(x: 1, y: -1)
    c.clip(to: markRect, mask: mark)
    c.setFillColor(muted)
    c.fill(markRect)
    c.restoreGState()

    NSGraphicsContext.current = nil
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
}

render(scale: 1, to: "macos/dmg/background.png")
render(scale: 2, to: "macos/dmg/background@2x.png")
