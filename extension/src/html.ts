// Shared HTML utilities for the extension's webview panels. Both the
// main metadata view (`provider.ts::renderHtml`) and the render-panel
// pop-out (`render-panel.ts::renderImagePanelHtml`) need the same
// escaping + CSP-nonce helpers; keep them here so neither has to
// re-implement them.

import { randomBytes } from "crypto";

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** CSPRNG-derived nonce string suitable for the CSP `script-src
 *  'nonce-…'` directive — the boundary that makes inline scripts safe
 *  inside a `default-src 'none'` policy. */
export function nonce(): string {
  return randomBytes(16).toString("base64").replace(/[^A-Za-z0-9]/g, "");
}
