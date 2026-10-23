// Downloads macOS's on-demand PingFang SC font asset, which a fresh runner
// lacks. Without it CoreText answers Han fallback with the private
// `.PingFang UI` face, which fontique cannot use, and fontique falls back
// to Heiti SC, whose line metrics differ: the web-core e2e goldens that show
// Chinese were recorded with PingFang SC.
import CoreText
import Foundation

let descriptor = CTFontDescriptorCreateWithAttributes(
    [kCTFontFamilyNameAttribute: "PingFang SC"] as CFDictionary)
let done = DispatchSemaphore(value: 0)
var failed = false
CTFontDescriptorMatchFontDescriptorsWithProgressHandler(
    [descriptor] as CFArray, nil
) { state, progress in
    switch state {
    case .didFailWithError:
        failed = true
        print("PingFang SC download failed:", progress)
    case .didFinish:
        done.signal()
    default:
        break
    }
    return true
}
if done.wait(timeout: .now() + 600) == .timedOut {
    print("PingFang SC download timed out")
    exit(1)
}
let check = CTFontCreateForString(
    CTFontCreateWithName("Helvetica" as CFString, 16, nil), "简体中" as CFString,
    CFRange(location: 0, length: 3))
let family = CTFontCopyFamilyName(check) as String
print("Han fallback:", family)
exit(failed || family != "PingFang SC" ? 1 : 0)
