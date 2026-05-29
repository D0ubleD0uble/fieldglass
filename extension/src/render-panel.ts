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
          // The azimuthal targets read a centre / hemisphere preset; the
          // lat/lon-box targets ignore it.
          if (projection === 'orthographic') {
            options.projectionPreset = (document.getElementById('picker-preset-ortho') || {}).value;
          } else if (projection === 'polar_stereographic') {
            options.projectionPreset = (document.getElementById('picker-preset-polar') || {}).value;
          }
          if (mode && mode.value === 'manual') {
            const min = Number((document.getElementById('range-min') || {}).value);
            const max = Number((document.getElementById('range-max') || {}).value);
            if (Number.isFinite(min) && Number.isFinite(max) && max > min) {
              options.rangeMin = min;
              options.rangeMax = max;
            }
          }
          // Manual extent applies to the warped lat/lon targets
          // (equirectangular + Web Mercator), which both render a lat/lon
          // window. Send all four edges or none; the Rust side validates and
          // falls back to the computed bounds for a partial/inverted box.
          const bmode = document.querySelector('input[name="bounds-mode"]:checked');
          const warpsLatLon = projection === 'equirectangular' || projection === 'web_mercator';
          if (warpsLatLon && bmode && bmode.value === 'manual') {
            const laMin = Number((document.getElementById('bounds-lat-min') || {}).value);
            const laMax = Number((document.getElementById('bounds-lat-max') || {}).value);
            const loMin = Number((document.getElementById('bounds-lon-min') || {}).value);
            const loMax = Number((document.getElementById('bounds-lon-max') || {}).value);
            if (
              Number.isFinite(laMin) && Number.isFinite(laMax) && laMax > laMin &&
              Number.isFinite(loMin) && Number.isFinite(loMax) && loMax > loMin
            ) {
              options.boundsLatMin = laMin;
              options.boundsLatMax = laMax;
              options.boundsLonMin = loMin;
              options.boundsLonMax = loMax;
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
          // Pre-fill the manual-bounds inputs from the extent Rust actually
          // used (present for the warped lat/lon targets — equirectangular and
          // Web Mercator). The empty guard means we never clobber a value the
          // user has typed.
          if (payload.usedLatMin !== undefined && payload.usedLatMin !== null) {
            const fillBound = (id, v) => {
              const el = document.getElementById(id);
              if (el && !el.value) el.value = Number(v).toFixed(3);
            };
            fillBound('bounds-lat-min', payload.usedLatMin);
            fillBound('bounds-lat-max', payload.usedLatMax);
            fillBound('bounds-lon-min', payload.usedLonMin);
            fillBound('bounds-lon-max', payload.usedLonMax);
          }
        }

        function handleGridReady(msg) {
          lastPayload = msg;
          blit(msg);
          // Reproject the overlay only when the raster *geometry* changed
          // (projection / preset / flip-y / bounds). A range- or resampling-
          // only render leaves the geometry — and the existing overlay — valid,
          // so we skip the round-trip.
          if (overlayKey() !== lastOverlayKey) requestOverlay();
        }

        function handleGridError(msg) {
          setStatus('Error: ' + (msg.error || 'render failed'));
        }

        // --- Overlay layer (coastlines / graticule) --------------------------
        // The most-recently-received projected overlay geometry, kept so we can
        // redraw on resize without another round-trip. Cleared when all layers
        // are toggled off.
        let lastOverlay = null;
        // Monotonic request id: a reply for an older projection is ignored so
        // it can't be drawn against a newer raster (transient misalignment when
        // switching projections quickly).
        let overlaySeq = 0;
        // The geometry-affecting options behind the current overlay. A render
        // that changes only range/resampling leaves this unchanged, so we skip
        // a redundant reprojection round-trip.
        let lastOverlayKey = null;

        function overlayState() {
          return {
            coastlines: !!(document.getElementById('overlay-coastlines') || {}).checked,
            graticule: !!(document.getElementById('overlay-graticule') || {}).checked,
          };
        }

        // Only projection / preset / flip-y / bounds move the overlay's pixel
        // geometry; resampling and range do not.
        function overlayKey() {
          const o = currentOptions();
          return JSON.stringify([
            o.projection, o.projectionPreset, !!o.flipY,
            o.boundsLatMin, o.boundsLatMax, o.boundsLonMin, o.boundsLonMax,
          ]);
        }

        function clearOverlay() {
          lastOverlay = null;
          const o = document.getElementById('overlay');
          const ctx = o && o.getContext('2d');
          if (ctx) ctx.clearRect(0, 0, o.width, o.height);
        }

        // Ask the provider to project the enabled layers for the current
        // options. Geometry-only on the Rust side — never re-decodes.
        function requestOverlay() {
          if (!lastPayload) return;
          lastOverlayKey = overlayKey();
          const state = overlayState();
          if (!state.coastlines && !state.graticule) {
            clearOverlay();
            return;
          }
          overlaySeq += 1;
          vscode.postMessage({
            type: 'overlayRequest',
            seq: overlaySeq,
            options: currentOptions(),
            coastlines: state.coastlines,
            graticule: state.graticule,
            graticuleSpacing: Number((document.getElementById('graticule-spacing') || {}).value) || 30,
          });
        }

        function handleOverlayReady(msg) {
          // Drop a stale reply (an earlier projection) so it can't be drawn
          // against the current raster.
          if (msg.seq !== overlaySeq) return;
          lastOverlay = msg;
          drawOverlay();
        }

        // Stroke the projected runs onto the overlay canvas. The overlay's
        // backing store is sized to the image's *displayed* pixels (× DPR) so
        // lines stay crisp instead of inheriting the image's pixelated upscale;
        // raster-space coordinates from Rust are scaled to that size here.
        function drawOverlay() {
          const img = document.getElementById('canvas');
          const o = document.getElementById('overlay');
          const ctx = o && o.getContext('2d');
          if (!img || !o || !ctx || !lastPayload) return;
          const dpr = window.devicePixelRatio || 1;
          // Size the backing store to the overlay's *content* box (the canvas
          // bitmap fills the content box, inside its border) so a raster pixel
          // maps exactly onto the same spot as the image's content box — no
          // ~1px border offset. clientWidth/Height exclude the border.
          o.width = Math.max(1, Math.round(o.clientWidth * dpr));
          o.height = Math.max(1, Math.round(o.clientHeight * dpr));
          ctx.clearRect(0, 0, o.width, o.height);
          if (!lastOverlay || !lastOverlay.layers || !lastPayload.width) return;
          const sx = o.width / lastPayload.width;
          const sy = o.height / lastPayload.height;
          const fg = (getComputedStyle(document.documentElement)
            .getPropertyValue('--vscode-foreground') || '#ffffff').trim() || '#ffffff';
          const styles = {
            coastline: { color: fg, width: 1.1 },
            graticule: { color: fg, width: 0.6, alpha: 0.35 },
          };
          for (const layer of lastOverlay.layers) {
            const style = styles[layer.name] || styles.coastline;
            ctx.save();
            ctx.globalAlpha = style.alpha || 1;
            ctx.strokeStyle = style.color;
            ctx.lineWidth = style.width * dpr;
            ctx.beginPath();
            // Tolerate either typed arrays or plain arrays from the serializer.
            const xy = layer.xy || [];
            const segs = layer.segLengths || [];
            let p = 0;
            for (let s = 0; s < segs.length; s++) {
              const n = segs[s];
              for (let v = 0; v < n; v++) {
                const x = xy[p++] * sx;
                const y = xy[p++] * sy;
                if (v === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
              }
            }
            ctx.stroke();
            ctx.restore();
          }
        }

        // Show the centre/hemisphere preset selector that matches the active
        // projection, and hide the others.
        function syncPresetVisibility() {
          const projection = (document.getElementById('picker-projection') || {}).value || 'source';
          const ortho = document.getElementById('preset-ortho');
          const polar = document.getElementById('preset-polar');
          if (ortho) ortho.toggleAttribute('hidden', projection !== 'orthographic');
          if (polar) polar.toggleAttribute('hidden', projection !== 'polar_stereographic');
        }

        function attachControls() {
          const projPick = document.getElementById('picker-projection');
          if (projPick) projPick.addEventListener('change', () => { syncPresetVisibility(); requestRender(); });
          ['picker-preset-ortho', 'picker-preset-polar'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', requestRender);
          });
          syncPresetVisibility();
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
          document.querySelectorAll('input[name="bounds-mode"]').forEach((el) => {
            el.addEventListener('change', () => {
              const manual = document.getElementById('bounds-manual-fields');
              const isManual = el.value === 'manual' && el.checked;
              if (manual) manual.toggleAttribute('hidden', !isManual);
              requestRender();
            });
          });
          ['bounds-lat-min', 'bounds-lat-max', 'bounds-lon-min', 'bounds-lon-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', requestRender);
          });
          // Overlay toggles never re-render the image — they only reproject the
          // vector layer, so the image paint is untouched when toggling.
          const coast = document.getElementById('overlay-coastlines');
          if (coast) coast.addEventListener('change', requestOverlay);
          const grat = document.getElementById('overlay-graticule');
          if (grat) {
            grat.addEventListener('change', () => {
              const label = document.getElementById('graticule-spacing-label');
              if (label) label.toggleAttribute('hidden', !grat.checked);
              requestOverlay();
            });
          }
          const spacing = document.getElementById('graticule-spacing');
          if (spacing) spacing.addEventListener('change', requestOverlay);
          // Keep the overlay aligned + crisp as the panel (and the displayed
          // image size) resizes.
          window.addEventListener('resize', drawOverlay);
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          if (msg.type === 'gridReady') handleGridReady(msg);
          else if (msg.type === 'gridError') handleGridError(msg);
          else if (msg.type === 'overlayReady') handleOverlayReady(msg);
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
    /* The image canvas and the vector overlay share one positioned box so the
       overlay sits exactly on top. The image is pixel-upscaled; the overlay is
       a crisp vector layer drawn at display resolution (see drawOverlay). */
    .canvas-wrap { position: relative; flex: 1 1 auto; display: flex; }
    canvas#canvas {
      max-width: 100%;
      height: auto;
      image-rendering: pixelated;
      background: var(--vscode-editor-background);
      border: 1px solid var(--vscode-panel-border);
      flex: 1 1 auto;
    }
    canvas#overlay {
      position: absolute;
      left: 0;
      top: 0;
      width: 100%;
      height: 100%;
      box-sizing: border-box;
      border: 1px solid transparent;
      pointer-events: none;
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
    /* The rule above sets display, out-specifying the UA stylesheet's hidden
       rule (display:none); without this the preset selectors never hide when
       syncPresetVisibility toggles them off. */
    .toolbar label[hidden] { display: none; }
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
        <option value="web_mercator">Web Mercator</option>
        <option value="orthographic">Orthographic</option>
        <option value="polar_stereographic">Polar stereographic</option>
      </select>
    </label>
    <label id="preset-ortho" hidden>Center
      <select id="picker-preset-ortho">
        <option value="atlantic" selected>Atlantic (0°N 0°E)</option>
        <option value="pacific">Pacific (0°N 180°E)</option>
        <option value="north_pole">North pole</option>
        <option value="south_pole">South pole</option>
      </select>
    </label>
    <label id="preset-polar" hidden>Hemisphere
      <select id="picker-preset-polar">
        <option value="north" selected>North</option>
        <option value="south">South</option>
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
    <fieldset>
      <legend>Bounds:</legend>
      <label><input type="radio" name="bounds-mode" value="auto" checked> Auto</label>
      <label><input type="radio" name="bounds-mode" value="manual"> Manual</label>
      <span id="bounds-manual-fields" hidden>
        <label>lat min <input type="number" id="bounds-lat-min" step="any"></label>
        <label>lat max <input type="number" id="bounds-lat-max" step="any"></label>
        <label>lon min <input type="number" id="bounds-lon-min" step="any"></label>
        <label>lon max <input type="number" id="bounds-lon-max" step="any"></label>
      </span>
    </fieldset>
    <fieldset>
      <legend>Overlay:</legend>
      <label><input type="checkbox" id="overlay-coastlines"> Coastlines</label>
      <label><input type="checkbox" id="overlay-graticule"> Graticule</label>
      <label id="graticule-spacing-label" hidden>spacing
        <input type="number" id="graticule-spacing" value="30" min="5" max="90" step="5"></label>
    </fieldset>
  </div>
  <div id="status">Rendering…</div>
  <div class="render-area">
    <div class="canvas-wrap">
      <canvas id="canvas" width="320" height="320"></canvas>
      <canvas id="overlay"></canvas>
    </div>
    <div class="colorbar-wrap">
      <div class="cb" aria-label="viridis colormap"></div>
      <div class="colorbar-labels">
        <div id="cb-max">—</div>
        <div id="cb-min">—</div>
      </div>
    </div>
  </div>
  ${script}
</body>
</html>`;
}
