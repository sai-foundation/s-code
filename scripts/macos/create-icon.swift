import AppKit
import Foundation
let directory = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let image = NSImage(size: NSSize(width: pixels, height: pixels))
        image.lockFocus()
        let ctx = NSGraphicsContext.current!.cgContext
        ctx.scaleBy(x: CGFloat(pixels) / 80, y: CGFloat(pixels) / 80)
        ctx.translateBy(x: 0, y: 80); ctx.scaleBy(x: 1, y: -1)
        ctx.setFillColor(NSColor(calibratedRed: 0.063, green: 0.106, blue: 0.09, alpha: 1).cgColor)
        ctx.addPath(CGPath(roundedRect: CGRect(x: 4, y: 4, width: 72, height: 72), cornerWidth: 19, cornerHeight: 19, transform: nil)); ctx.fillPath()
        ctx.setStrokeColor(NSColor(calibratedRed: 0.57, green: 0.9, blue: 0.71, alpha: 1).cgColor)
        ctx.setLineWidth(6); ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 55, y: 24))
        ctx.addCurve(to: CGPoint(x: 25, y: 27), control1: CGPoint(x: 48, y: 19), control2: CGPoint(x: 31, y: 19))
        ctx.addCurve(to: CGPoint(x: 40, y: 41), control1: CGPoint(x: 16, y: 39), control2: CGPoint(x: 29, y: 41))
        ctx.addCurve(to: CGPoint(x: 55, y: 57), control1: CGPoint(x: 52, y: 41), control2: CGPoint(x: 63, y: 47))
        ctx.addCurve(to: CGPoint(x: 24, y: 58), control1: CGPoint(x: 49, y: 65), control2: CGPoint(x: 31, y: 63))
        ctx.strokePath(); image.unlockFocus()
        let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
        let suffix = scale == 2 ? "@2x" : ""
        try bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent("icon_\(size)x\(size)\(suffix).png"))
    }
}
