import Foundation
import WebKit
import AppKit

@MainActor
enum PDFGenerator {
    static func run(_ input: [String: Any]) async throws {
        guard let html = input["html"] as? String else {
            writeError("Missing 'html' field")
            exit(1)
        }
        guard let outputPath = input["output_path"] as? String else {
            writeError("Missing 'output_path' field")
            exit(1)
        }

        // Initialize NSApplication for headless WebKit usage
        NSApplication.shared.setActivationPolicy(.prohibited)

        // A4 page size in points (72 dpi): 595 x 842
        let pageWidth: CGFloat = 595
        let pageHeight: CGFloat = 842

        let config = WKWebViewConfiguration()
        let webView = WKWebView(
            frame: NSRect(x: 0, y: 0, width: pageWidth, height: pageHeight),
            configuration: config
        )

        // Load HTML and wait for navigation to complete
        let waiter = NavigationWaiter()
        try await waiter.loadAndWait(webView: webView, html: html)

        // Generate PDF (page size derived from web view frame)
        let pdfConfig = WKPDFConfiguration()
        let pdfData = try await webView.pdf(configuration: pdfConfig)

        // Write PDF data to output file
        let url = URL(fileURLWithPath: outputPath)
        try pdfData.write(to: url)

        writeOutput([
            "success": true,
            "path": outputPath,
            "size": pdfData.count,
        ])
    }
}

/// Delegate that waits for WKWebView navigation to complete using async/await.
final class NavigationWaiter: NSObject, WKNavigationDelegate {
    private var continuation: CheckedContinuation<Void, Error>?

    @MainActor
    func loadAndWait(webView: WKWebView, html: String, timeout: TimeInterval = 10) async throws {
        webView.navigationDelegate = self
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            self.continuation = cont
            webView.loadHTMLString(html, baseURL: nil)

            // Timeout to avoid hanging on bad HTML
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { [weak self] in
                guard let self = self, let pending = self.continuation else { return }
                self.continuation = nil
                pending.resume(throwing: NSError(
                    domain: "PDFHelper",
                    code: 1,
                    userInfo: [NSLocalizedDescriptionKey: "Timeout: HTML failed to load within \(Int(timeout)) seconds"]
                ))
            }
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard let cont = continuation else { return }
        continuation = nil
        cont.resume()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        guard let cont = continuation else { return }
        continuation = nil
        cont.resume(throwing: error)
    }
}
