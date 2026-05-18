// HTML + embedded script for the render-panel webview. The pop-out tab
// users see beside the metadata table after clicking "Render": one large
// canvas, a viridis colorbar on the right, and a toolbar with the
// projection / resampling / flip-y / range pickers.
//
// The script lives as a template string inside this file rather than a
// separate `.ts` bundle because the webview's CSP nonce has to inject at
// render time, and we don't yet have a webview-bundle build step.
// Splitting the script into its own file (and adding a Rollup/esbuild
// pipeline that emits a CSP-friendly bundle) is captured as a
// follow-up — see PR #73's deferred-items list.

import * as vscode from "vscode";

import { escapeHtml, nonce } from "./html";
import type { MessageMeta } from "./native";

/** Returns the full HTML for the render-panel webview. The DOM mounts,
 *  the script posts `ready`, the provider responds with `gridReady`
 *  carrying paint-ready RGBA, and the canvas blits it. Picker changes
 *  flow back as `rerenderRequest`. */
export function renderImagePanelHtml(
  webview: vscode.Webview,
  meta: MessageMeta,
  projectionSummary: string
): string {
  const cspNonce = nonce();
  const csp = [
    `default-src 'none'`,
    `script-src 'nonce-${cspNonce}'`,
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `img-src ${webview.cspSource} blob: data:`,
  ].join("; ");
  const titleLine = `Message ${meta.messageIndex}`
    + (meta.parameterName ? ` — ${meta.parameterName}` : "")
    + (meta.parameterUnits ? ` (${meta.parameterUnits})` : "");
  // `level` is the bare value ("300", "—", "100 – 85"); `levelType` carries
  // the unit and surface name ("(hPa) Isobaric level", "Cloud base level").
  // Together they read naturally as "300 (hPa) Isobaric level". For surface
  // types whose value is meaningless (level === "—") only the levelType is
  // informative, so drop the placeholder.
  const levelDescription = meta.level && meta.level !== "—"
    ? [meta.level, meta.levelType].filter((s) => !!s).join(" ")
    : meta.levelType;
  const subLine = [levelDescription, meta.referenceTime, meta.forecastDisplay]
    .filter((s) => !!s).join(" · ");

  const script = `
    <script nonce="${cspNonce}">
      (function () {
        const vscode = acquireVsCodeApi();

        // The most-recently-received rendered grid. We only re-render
        // when the user changes a picker that affects the Rust output
        // (projection / resampling / flip-y / range). The cached payload
        // lets us redraw after a tab hide/show without a round-trip.
        let lastPayload = null;

        function setStatus(text) {
          const el = document.getElementById('status');
          if (el) el.textContent = text;
        }

        function currentOptions() {
          const projection = (document.getElementById('picker-projection') || {}).value || 'source';
          const resampling = (document.getElementById('picker-resampling') || {}).value || 'nearest';
          const flipY = !!(document.getElementById('flip-y') && document.getElementById('flip-y').checked);
          const mode = document.querySelector('input[name="range-mode"]:checked');
          const options = { projection, resampling, flipY };
          if (mode && mode.value === 'manual') {
            const min = Number((document.getElementById('range-min') || {}).value);
            const max = Number((document.getElementById('range-max') || {}).value);
            if (Number.isFinite(min) && Number.isFinite(max) && max > min) {
              options.rangeMin = min;
              options.rangeMax = max;
            }
          }
          return options;
        }

        function requestRender() {
          if (!lastPayload) {
            // Initial mount: provider posts ready-options-default automatically.
            return;
          }
          vscode.postMessage(Object.assign({ type: 'rerenderRequest' }, currentOptions()));
          setStatus('Rendering…');
        }

        function blit(payload) {
          const canvas = document.getElementById('canvas');
          if (!canvas) return;
          const ctx = canvas.getContext('2d');
          if (!ctx) return;
          canvas.width = payload.width;
          canvas.height = payload.height;
          // payload.rgba arrives as a Node Buffer; copy into a Uint8ClampedArray
          // so ImageData accepts it.
          const rgba = new Uint8ClampedArray(
            payload.rgba.buffer ? payload.rgba.buffer : payload.rgba,
            payload.rgba.byteOffset || 0,
            payload.rgba.byteLength || payload.rgba.length
          );
          const img = new ImageData(rgba, payload.width, payload.height);
          ctx.putImageData(img, 0, 0);
          const cbMin = document.getElementById('cb-min');
          const cbMax = document.getElementById('cb-max');
          if (cbMin) cbMin.textContent = payload.usedMin.toPrecision(4);
          if (cbMax) cbMax.textContent = payload.usedMax.toPrecision(4);
          const proj = document.getElementById('projection-summary');
          if (proj) proj.textContent = payload.projectionSummary || '';
          setStatus(
            payload.width + '×' + payload.height + ' · range ' +
            payload.usedMin.toPrecision(4) + ' … ' + payload.usedMax.toPrecision(4),
          );
          // Pre-fill the manual-range inputs once so the user can switch
          // to Manual mode without typing.
          const minIn = document.getElementById('range-min');
          const maxIn = document.getElementById('range-max');
          if (minIn && !minIn.value) minIn.value = payload.usedMin.toPrecision(6);
          if (maxIn && !maxIn.value) maxIn.value = payload.usedMax.toPrecision(6);
        }

        function handleGridReady(msg) {
          lastPayload = msg;
          blit(msg);
        }

        function handleGridError(msg) {
          setStatus('Error: ' + (msg.error || 'render failed'));
        }

        function attachControls() {
          const projPick = document.getElementById('picker-projection');
          if (projPick) projPick.addEventListener('change', requestRender);
          const sampPick = document.getElementById('picker-resampling');
          if (sampPick) sampPick.addEventListener('change', requestRender);
          const flip = document.getElementById('flip-y');
          if (flip) flip.addEventListener('change', requestRender);
          document.querySelectorAll('input[name="range-mode"]').forEach((el) => {
            el.addEventListener('change', () => {
              const manual = document.getElementById('range-manual-fields');
              const isManual = el.value === 'manual' && el.checked;
              if (manual) manual.toggleAttribute('hidden', !isManual);
              requestRender();
            });
          });
          ['range-min', 'range-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', requestRender);
          });
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          if (msg.type === 'gridReady') handleGridReady(msg);
          else if (msg.type === 'gridError') handleGridError(msg);
        });

        attachControls();
        vscode.postMessage({ type: 'ready' });
      })();
    </script>
  `;

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta http-equiv="Content-Security-Policy" content="${csp}" />
  <title>Fieldglass render</title>
  <style>
    body {
      font-family: var(--vscode-font-family);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 1.5rem;
      margin: 0;
    }
    h1 { font-size: 1.1rem; margin: 0 0 0.2rem 0; }
    .subtitle { color: var(--vscode-descriptionForeground); font-size: 0.85rem; margin-bottom: 0.5rem; }
    .projection { color: var(--vscode-descriptionForeground); font-size: 0.8rem; margin-bottom: 0.75rem; }
    #status { font-size: 0.85rem; margin-bottom: 0.75rem; min-height: 1.1em; }
    .render-area {
      display: flex;
      align-items: flex-start;
      gap: 0.75rem;
    }
    canvas#canvas {
      max-width: 100%;
      height: auto;
      image-rendering: pixelated;
      background: var(--vscode-editor-background);
      border: 1px solid var(--vscode-panel-border);
      flex: 1 1 auto;
    }
    .colorbar-wrap {
      display: flex;
      align-items: stretch;
      gap: 0.4rem;
      height: 320px;
      flex: 0 0 auto;
    }
    /* Static viridis gradient — 11 anchor stops matched against the
       Rust LUT in fieldglass-core::colormap (top = max, bottom = min). */
    .cb {
      width: 24px;
      height: 320px;
      border: 1px solid var(--vscode-panel-border);
      background: linear-gradient(
        to top,
        rgb(68, 1, 84) 0%,
        rgb(72, 36, 117) 10%,
        rgb(65, 68, 135) 20%,
        rgb(53, 95, 141) 30%,
        rgb(42, 120, 142) 40%,
        rgb(33, 145, 140) 50%,
        rgb(34, 168, 132) 60%,
        rgb(68, 191, 112) 70%,
        rgb(122, 209, 81) 80%,
        rgb(189, 223, 38) 90%,
        rgb(253, 231, 37) 100%
      );
    }
    .colorbar-labels {
      display: flex;
      flex-direction: column;
      justify-content: space-between;
      font-size: 0.75rem;
      color: var(--vscode-descriptionForeground);
    }
    .legend {
      margin-top: 0.75rem;
      font-size: 0.75rem;
      color: var(--vscode-descriptionForeground);
    }
    .toolbar {
      display: flex;
      align-items: center;
      flex-wrap: wrap;
      gap: 0.75rem 1.25rem;
      padding: 0.5rem 0.75rem;
      margin-bottom: 0.75rem;
      border: 1px solid var(--vscode-panel-border);
      border-radius: 3px;
      background: var(--vscode-editorWidget-background, transparent);
      font-size: 0.85rem;
    }
    .toolbar fieldset {
      display: flex; align-items: center; gap: 0.5rem;
      border: none; padding: 0; margin: 0;
    }
    .toolbar legend {
      padding: 0;
      font-size: 0.8rem;
      color: var(--vscode-descriptionForeground);
    }
    .toolbar label { display: inline-flex; align-items: center; gap: 0.25rem; }
    .toolbar input[type="number"] {
      width: 7rem;
      background: var(--vscode-input-background);
      color: var(--vscode-input-foreground);
      border: 1px solid var(--vscode-input-border, transparent);
      padding: 0.1rem 0.3rem;
      font-family: inherit;
      font-size: inherit;
    }
    .toolbar input[type="number"]:focus {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: -1px;
    }
  </style>
</head>
<body>
  <h1>${escapeHtml(titleLine)}</h1>
  <div class="subtitle">${escapeHtml(subLine)}</div>
  <div class="projection" id="projection-summary">${escapeHtml(projectionSummary)}</div>
  <div class="toolbar" role="toolbar" aria-label="Render settings">
    <label>Projection
      <select id="picker-projection">
        <option value="source" selected>Source projection</option>
        <option value="equirectangular">Equirectangular</option>
      </select>
    </label>
    <label>Resampling
      <select id="picker-resampling">
        <option value="nearest" selected>Nearest</option>
        <option value="bilinear">Bilinear</option>
      </select>
    </label>
    <label><input type="checkbox" id="flip-y"> Flip Y axis</label>
    <fieldset>
      <legend>Range:</legend>
      <label><input type="radio" name="range-mode" value="auto" checked> Auto</label>
      <label><input type="radio" name="range-mode" value="manual"> Manual</label>
      <span id="range-manual-fields" hidden>
        <label>min <input type="number" id="range-min" step="any"></label>
        <label>max <input type="number" id="range-max" step="any"></label>
      </span>
    </fieldset>
  </div>
  <div id="status">Rendering…</div>
  <div class="render-area">
    <canvas id="canvas" width="320" height="320"></canvas>
    <div class="colorbar-wrap">
      <div class="cb" aria-label="viridis colormap"></div>
      <div class="colorbar-labels">
        <div id="cb-max">—</div>
        <div id="cb-min">—</div>
      </div>
    </div>
  </div>
  <div class="legend">Rendered server-side (Rust). Bitmap-masked points are transparent. Source / equirectangular projections supported today; other targets tracked under #71.</div>
  ${script}
</body>
</html>`;
}
