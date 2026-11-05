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
 *  inside a `default-src 'none'` policy. Hex-encoded so all 128 bits of
 *  entropy survive (stripping non-alphanumerics from base64 would discard
 *  the `+`/`/`/`=` characters and weaken the nonce). */
export function nonce(): string {
  return randomBytes(16).toString("hex");
}

/** The keyboard focus ring of every button and disclosure toggle on the
 *  extension's pages, the file page and the render panel alike (#1717): a
 *  2px line in the theme's focus color, 1px clear of the control so it does
 *  not merge with the button's own border. VS Code's workbench draws 1px, and
 *  in a long table of identical Render buttons that is easy to lose; 2px is
 *  the minimum WCAG 2.4.13 asks for. The ring reaches 3px past the control,
 *  which the file page's scroll padding allows for. */
export const FOCUS_RING_CSS = "outline: 2px solid var(--vscode-focusBorder); outline-offset: 1px;";
