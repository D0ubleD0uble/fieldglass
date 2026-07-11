// HTML + embedded script for the render-panel webview. The pop-out tab
// users see beside the metadata table after clicking "Render": one large
// canvas, a colorbar on the right, and a toolbar with the
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
import type { ColormapInfo, MessageMeta, NetcdfVariableMeta } from "./native";

/** Which 2-D plane of an N-D NetCDF variable to draw: the variable, the two
 *  image axes (positions into the variable's dimensions), and the held index
 *  for every dimension (the `yDim` / `xDim` entries are ignored). */
export interface SliceSpec {
  variableIndex: number;
  yDim: number;
  xDim: number;
  sliceIndices: number[];
}

/** Drives the NetCDF two-tier slice picker injected into the render panel: the
 *  renderable variables (each with their dimensions and detected horizontal
 *  axes) and the slice the panel opens on. */
export interface SlicePanelData {
  variables: NetcdfVariableMeta[];
  initial: SliceSpec;
}

/** Returns the full HTML for the render-panel webview. The DOM mounts,
 *  the script posts `ready`, the provider responds with `gridReady`
 *  carrying paint-ready RGBA, and the canvas blits it. Picker changes
 *  flow back as `rerenderRequest`.
 *
 *  When `slice` is supplied (NetCDF, #122) the panel also shows the two-tier
 *  slice picker — a variable selector, X / Y axis selectors, and one index
 *  control per non-horizontal dimension — and every `rerenderRequest` /
 *  `overlayRequest` carries the chosen {@link SliceSpec}. */
/** Serialise the slice data for embedding in the inline `<script>`. NetCDF
 *  variable names are decoded from an untrusted file, so escape `<` to its
 *  `\\u003c` form — that prevents a `</script>` substring in a name from closing
 *  the script element early (the JSON stays valid; the escape parses back to
 *  `<`). The CSP nonce already blocks any injected script from executing, but
 *  this keeps the panel's own script intact. */
function sliceJson(slice: SlicePanelData): string {
  return JSON.stringify(slice).replace(/</g, "\\u003c");
}

/** The colormap registry, inlined into the panel script. Same `<` escaping as
 *  {@link sliceJson}. The panel needs it to build its picker and to paint the
 *  legend gradient in the colours Rust will actually paint the grid with. */
function colormapsJson(colormaps: ColormapInfo[]): string {
  return JSON.stringify(colormaps).replace(/</g, "\\u003c");
}

/** The Colors toolbar row: the colormap dropdown (grouped sequential /
 *  diverging) plus the reverse toggle. Built from the registry, so a colormap
 *  added in Rust shows up here with no edit. With no registry — the native
 *  binding failed to load — the row is omitted entirely rather than offering a
 *  picker the renderer can't honour. */
function colormapFieldsetHtml(colormaps: ColormapInfo[]): string {
  if (colormaps.length === 0) {
    return "";
  }
  const group = (kind: string, label: string): string => {
    const entries = colormaps.filter((c) => c.kind === kind);
    if (entries.length === 0) {
      return "";
    }
    const opts = entries
      .map(
        (c) =>
          // Pre-select the renderer's default (the registry's first entry),
          // wherever it sits among the groups — the picker must agree with what
          // Rust paints when the panel opens untouched.
          `            <option value="${escapeHtml(c.name)}"${
            c.name === colormaps[0].name ? " selected" : ""
          }>${escapeHtml(c.label)}</option>`,
      )
      .join("\n");
    return `          <optgroup label="${escapeHtml(label)}">\n${opts}\n          </optgroup>`;
  };
  const groups = [group("sequential", "Sequential"), group("diverging", "Diverging")]
    .filter(Boolean)
    .join("\n");
  return `    <fieldset>
      <legend>Colors:</legend>
      <label>Colormap
        <select id="picker-colormap">
${groups}
        </select>
      </label>
      <label><input type="checkbox" id="reverse-colormap"> Reverse</label>
    </fieldset>
`;
}

export function renderImagePanelHtml(
  webview: vscode.Webview,
  meta: MessageMeta,
  projectionSummary: string,
  colormaps: ColormapInfo[],
  slice?: SlicePanelData
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

        // --- NetCDF slice picker (#122) -------------------------------------
        // SLICE is null for the GRIB panels; for NetCDF it carries the
        // renderable variables and the slice the panel opened on. sliceState is
        // the live {variableIndex, yDim, xDim, sliceIndices} the controls drive,
        // attached to every rerender / overlay request so the provider knows
        // which 2-D plane to draw.
        // The colormap registry, straight from the Rust side: [{name, label,
        // kind, stops}]. Both the picker and the legend gradient are built from
        // it, so the colours in the strip are the colours in the image.
        const COLORMAPS = ${colormapsJson(colormaps)};
        const SLICE = ${slice ? sliceJson(slice) : "null"};
        let sliceState = SLICE ? Object.assign({}, SLICE.initial, {
          sliceIndices: SLICE.initial.sliceIndices.slice(),
        }) : null;

        function sliceVariable(idx) {
          return SLICE ? SLICE.variables.find((v) => v.variableIndex === idx) : undefined;
        }

        // Extra fields merged into a rerender / overlay request: the slice spec
        // when this is a NetCDF panel, nothing otherwise.
        function sliceFields() {
          return sliceState ? { slice: sliceState } : {};
        }

        // Rebuild the per-variable axis controls: the X / Y dimension selectors
        // and one index slider per non-horizontal dimension. Called on mount and
        // whenever the variable changes (its dimensions differ).
        function buildSliceAxes() {
          const wrap = document.getElementById('slice-dims');
          const ySel = document.getElementById('slice-y');
          const xSel = document.getElementById('slice-x');
          if (!wrap || !ySel || !xSel || !sliceState) return;
          const v = sliceVariable(sliceState.variableIndex);
          if (!v) return;
          const dimOptions = v.dims
            .map((d, i) => '<option value="' + i + '">' + escapeAttr(d.name) + '</option>')
            .join('');
          ySel.innerHTML = dimOptions;
          xSel.innerHTML = dimOptions;
          ySel.value = String(sliceState.yDim);
          xSel.value = String(sliceState.xDim);
          // One slider + number per dimension that isn't an image axis.
          let html = '';
          for (let i = 0; i < v.dims.length; i++) {
            if (i === sliceState.yDim || i === sliceState.xDim) continue;
            const len = v.dims[i].length;
            const max = Math.max(0, len - 1);
            const val = sliceState.sliceIndices[i] || 0;
            html += '<label class="slice-index">' + escapeAttr(v.dims[i].name) +
              ' <input type="range" class="slice-slider" data-dim="' + i + '" min="0" max="' + max +
              '" value="' + val + '">' +
              '<input type="number" class="slice-number" data-dim="' + i + '" min="0" max="' + max +
              '" value="' + val + '"><span class="slice-len">/' + max + '</span></label>';
          }
          wrap.innerHTML = html;
        }

        // Minimal attribute escaper for the option/label text we build in JS
        // (variable + dimension names come from the decoded file).
        function escapeAttr(s) {
          return String(s).replace(/[&<>"']/g, (c) => ({
            '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
          })[c]);
        }

        function syncSliceIndex(dim, value) {
          if (!sliceState) return;
          const v = sliceVariable(sliceState.variableIndex);
          const max = v ? Math.max(0, v.dims[dim].length - 1) : 0;
          const clamped = Math.min(max, Math.max(0, Math.floor(Number(value) || 0)));
          sliceState.sliceIndices[dim] = clamped;
          // Keep the paired slider + number input in lockstep.
          document.querySelectorAll('[data-dim="' + dim + '"]').forEach((el) => {
            if (el.value !== String(clamped)) el.value = String(clamped);
          });
        }

        function setupSlice() {
          if (!sliceState) return;
          const varSel = document.getElementById('slice-variable');
          if (varSel) {
            varSel.innerHTML = SLICE.variables
              .map((v) => '<option value="' + v.variableIndex + '">' + escapeAttr(v.name) + '</option>')
              .join('');
            varSel.value = String(sliceState.variableIndex);
            varSel.addEventListener('change', () => {
              const v = sliceVariable(Number(varSel.value));
              if (!v) return;
              // New variable → reset axes to its detected horizontals (falling
              // back to the first two dims) and zero the held indices.
              const yDim = v.detectedYDim != null ? v.detectedYDim : 0;
              let xDim = v.detectedXDim != null ? v.detectedXDim : 1;
              if (xDim === yDim) xDim = yDim === 0 ? 1 : 0;
              sliceState = {
                variableIndex: v.variableIndex,
                yDim,
                xDim,
                sliceIndices: v.dims.map(() => 0),
              };
              buildSliceAxes();
              requestRender();
            });
          }
          const ySel = document.getElementById('slice-y');
          const xSel = document.getElementById('slice-x');
          const onAxisChange = () => {
            if (!ySel || !xSel) return;
            const y = Number(ySel.value);
            const x = Number(xSel.value);
            if (y === x) {
              setStatus('The X and Y axes must be different dimensions.');
              // Revert the just-changed selector to the stored value.
              ySel.value = String(sliceState.yDim);
              xSel.value = String(sliceState.xDim);
              return;
            }
            sliceState.yDim = y;
            sliceState.xDim = x;
            buildSliceAxes();
            requestRender();
          };
          if (ySel) ySel.addEventListener('change', onAxisChange);
          if (xSel) xSel.addEventListener('change', onAxisChange);
          buildSliceAxes();
          // Delegate the dynamically-built index controls.
          // 'change' (slider release / number commit) rather than 'input' so a
          // slider drag doesn't fire a render per tick. 'input' still keeps the
          // paired slider + number box in lockstep for live visual feedback.
          const wrap = document.getElementById('slice-dims');
          if (wrap) {
            wrap.addEventListener('input', (ev) => {
              const t = ev.target;
              if (!t || t.dataset == null || t.dataset.dim == null) return;
              syncSliceIndex(Number(t.dataset.dim), t.value);
            });
            wrap.addEventListener('change', (ev) => {
              const t = ev.target;
              if (!t || t.dataset == null || t.dataset.dim == null) return;
              syncSliceIndex(Number(t.dataset.dim), t.value);
              requestRender();
            });
          }
        }

        function setStatus(text) {
          const el = document.getElementById('status');
          if (el) el.textContent = text;
        }

        // The warped lat/lon targets (equirectangular + Web Mercator) both
        // render a lat/lon window: they accept a manual bounds box and show the
        // Bounds control. Keep the rule in one place so the two gates can't
        // drift.
        function warpsLatLon(projection) {
          return projection === 'equirectangular' || projection === 'web_mercator';
        }

        function currentOptions() {
          const projection = (document.getElementById('picker-projection') || {}).value || 'source';
          const resampling = (document.getElementById('picker-resampling') || {}).value || 'nearest';
          const flipY = !!(document.getElementById('flip-y') && document.getElementById('flip-y').checked);
          const mode = document.querySelector('input[name="range-mode"]:checked');
          const options = { projection, resampling, flipY };
          // Colours. The name is a registry name the Rust side knows; reverse
          // flips the ramp end-for-end.
          const cmapEl = document.getElementById('picker-colormap');
          if (cmapEl && cmapEl.value) options.colormap = cmapEl.value;
          options.reverseColormap = !!(document.getElementById('reverse-colormap')
            && document.getElementById('reverse-colormap').checked);
          // The azimuthal targets read a free-form centre; the lat/lon-box
          // targets ignore it. Orthographic takes a centre lat + lon; polar
          // stereographic takes a hemisphere (its pole) plus a central
          // meridian. A blank/non-numeric field is omitted so the Rust side
          // falls back to its default for that component.
          const num = (id) => {
            const v = Number((document.getElementById(id) || {}).value);
            return Number.isFinite(v) ? v : undefined;
          };
          if (projection === 'orthographic') {
            options.centerLat = num('picker-center-lat');
            options.centerLon = num('picker-center-lon');
          } else if (projection === 'polar_stereographic') {
            options.projectionPreset = (document.getElementById('picker-preset-polar') || {}).value;
            options.centerLon = num('picker-central-meridian');
          } else if (isWorldProjection(projection)) {
            // The world targets have no preset — only a central meridian.
            options.centerLon = num('picker-world-meridian');
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
          if (warpsLatLon(projection) && bmode && bmode.value === 'manual') {
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
          vscode.postMessage(Object.assign({ type: 'rerenderRequest' }, currentOptions(), sliceFields()));
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
          if (overlayKey() !== lastOverlayKey) {
            // Wipe the stale strokes immediately so the previous projection's
            // lines don't linger over the new raster while the async reproject
            // is in flight.
            clearOverlay();
            requestOverlay();
          }
        }

        function handleGridError(msg) {
          setStatus('Error: ' + (msg.error || 'render failed'));
        }

        // An overlay projection failed on the provider side. Resolve the
        // in-flight request and re-arm the overlay key so the next render
        // retries the overlay instead of leaving it permanently blank.
        function handleOverlayError(msg) {
          if (msg.seq !== overlaySeq) return; // stale: a newer request superseded it
          lastOverlayKey = null;
          clearOverlay();
          setStatus('Overlay error: ' + (msg.error || 'projection failed'));
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
        // The themed foreground colour for overlay strokes, read from CSS once
        // and cached — drawOverlay fires on every resize tick, so we avoid the
        // repeated getComputedStyle reflow. (A theme switch mid-session keeps
        // the first-read colour; acceptable for a thin vector overlay.)
        let overlayFg = null;
        let overlayStyles = null;

        // Lazily compute + cache the overlay stroke styles from the themed
        // foreground colour.
        function overlayStrokeStyles() {
          if (!overlayStyles) {
            overlayFg = (getComputedStyle(document.documentElement)
              .getPropertyValue('--vscode-foreground') || '#ffffff').trim() || '#ffffff';
            overlayStyles = {
              coastline: { color: overlayFg, width: 1.1 },
              graticule: { color: overlayFg, width: 0.6, alpha: 0.35 },
            };
          }
          return overlayStyles;
        }

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
            o.projection, o.projectionPreset, o.centerLat, o.centerLon, !!o.flipY,
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
          vscode.postMessage(Object.assign({
            type: 'overlayRequest',
            seq: overlaySeq,
            options: currentOptions(),
            coastlines: state.coastlines,
            graticule: state.graticule,
            graticuleSpacing: Math.min(90, Math.max(5,
              Number((document.getElementById('graticule-spacing') || {}).value) || 30)),
          }, sliceFields()));
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
          const o = document.getElementById('overlay');
          const ctx = o && o.getContext('2d');
          if (!o || !ctx || !lastPayload) return;
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
          const styles = overlayStrokeStyles();
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

        // Show only the controls the active projection actually uses: the
        // matching centre/hemisphere preset selector, and the Bounds (manual
        // lat/lon window) control — which applies to the lat/lon-box targets
        // only. The azimuthal targets frame themselves by preset and ignore a
        // bounds rectangle, and the source projection has no window either, so
        // showing Bounds there would be an inert, misleading control.
        // The whole-world targets share one control: a central meridian. They
        // take no preset (they always show the entire globe) and no bounds.
        function isWorldProjection(projection) {
          return projection === 'mollweide' || projection === 'robinson'
            || projection === 'equal_earth';
        }

        // Repaint the legend strip in the selected colormap. The stops come from
        // the Rust registry — the same lookup table that paints the grid — so
        // the strip can't drift from the image the way a hardcoded CSS gradient
        // could (and did). Bottom = min, top = max, hence 'to top'.
        function syncColorbar() {
          const strip = document.querySelector('.cb');
          if (!strip || !COLORMAPS.length) return;
          const name = (document.getElementById('picker-colormap') || {}).value;
          const entry = COLORMAPS.find((c) => c.name === name) || COLORMAPS[0];
          const reversed = !!(document.getElementById('reverse-colormap')
            && document.getElementById('reverse-colormap').checked);
          const stops = reversed ? entry.stops.slice().reverse() : entry.stops;
          const parts = stops.map((c, i) =>
            c + ' ' + (stops.length > 1 ? (i / (stops.length - 1)) * 100 : 0) + '%');
          strip.style.background = 'linear-gradient(to top, ' + parts.join(', ') + ')';
          strip.setAttribute('aria-label', entry.label + (reversed ? ' (reversed)' : '') + ' colormap');
        }

        function syncProjectionControls() {
          const projection = (document.getElementById('picker-projection') || {}).value || 'source';
          const ortho = document.getElementById('preset-ortho');
          const polar = document.getElementById('preset-polar');
          const world = document.getElementById('preset-world');
          if (ortho) ortho.toggleAttribute('hidden', projection !== 'orthographic');
          if (polar) polar.toggleAttribute('hidden', projection !== 'polar_stereographic');
          if (world) world.toggleAttribute('hidden', !isWorldProjection(projection));
          const bounds = document.getElementById('bounds-fieldset');
          if (bounds) bounds.toggleAttribute('hidden', !warpsLatLon(projection));
        }

        // --- Selection persistence across tab hide/show ----------------------
        // The panel is created with retainContextWhenHidden: false (kept off
        // deliberately — a retained hidden panel pins its full canvas in
        // memory), so VS Code tears this webview down whenever its tab hides
        // and re-runs the whole script on re-show. vscode.setState survives
        // that teardown: every control change snapshots the selections,
        // restoreState() writes them back on remount, and the 'ready' message
        // carries the restored options so the provider's first paint honours
        // them instead of the baked-in defaults.
        function snapshotState() {
          const val = (id) => {
            const el = document.getElementById(id);
            return el ? el.value : undefined;
          };
          const chk = (id) => {
            const el = document.getElementById(id);
            return !!(el && el.checked);
          };
          const radio = (name) => {
            const el = document.querySelector('input[name="' + name + '"]:checked');
            return el ? el.value : undefined;
          };
          vscode.setState({
            projection: val('picker-projection'),
            centerLon: val('picker-center-lon'),
            centerLat: val('picker-center-lat'),
            polarPreset: val('picker-preset-polar'),
            centralMeridian: val('picker-central-meridian'),
            worldMeridian: val('picker-world-meridian'),
            resampling: val('picker-resampling'),
            colormap: val('picker-colormap'),
            reverseColormap: chk('reverse-colormap'),
            flipY: chk('flip-y'),
            rangeMode: radio('range-mode'),
            rangeMin: val('range-min'),
            rangeMax: val('range-max'),
            boundsMode: radio('bounds-mode'),
            boundsLatMin: val('bounds-lat-min'),
            boundsLatMax: val('bounds-lat-max'),
            boundsLonMin: val('bounds-lon-min'),
            boundsLonMax: val('bounds-lon-max'),
            coastlines: chk('overlay-coastlines'),
            graticule: chk('overlay-graticule'),
            graticuleSpacing: val('graticule-spacing'),
            slice: sliceState,
          });
        }

        // Write a saved snapshot back into the DOM (and sliceState). Runs
        // before attachControls so setupSlice builds the restored variable's
        // axis controls and syncProjectionControls shows the restored
        // projection's preset/bounds groups.
        function restoreState() {
          const s = vscode.getState();
          if (!s) return;
          const setVal = (id, v) => {
            if (v === undefined || v === null) return;
            const el = document.getElementById(id);
            if (!el) return;
            // A <select> silently blanks on a value with no matching option
            // (e.g. a projection this grid doesn't offer); keep its default.
            if (el.options && !Array.from(el.options).some((o) => o.value === String(v))) return;
            el.value = String(v);
          };
          const setChk = (id, v) => {
            const el = document.getElementById(id);
            if (el) el.checked = !!v;
          };
          const setRadio = (name, v) => {
            if (!v) return;
            const el = document.querySelector('input[name="' + name + '"][value="' + v + '"]');
            if (el) el.checked = true;
          };
          setVal('picker-projection', s.projection);
          setVal('picker-center-lon', s.centerLon);
          setVal('picker-center-lat', s.centerLat);
          setVal('picker-preset-polar', s.polarPreset);
          setVal('picker-central-meridian', s.centralMeridian);
          setVal('picker-world-meridian', s.worldMeridian);
          setVal('picker-resampling', s.resampling);
          setVal('picker-colormap', s.colormap);
          setChk('reverse-colormap', s.reverseColormap);
          setChk('flip-y', s.flipY);
          setRadio('range-mode', s.rangeMode);
          setVal('range-min', s.rangeMin);
          setVal('range-max', s.rangeMax);
          setRadio('bounds-mode', s.boundsMode);
          setVal('bounds-lat-min', s.boundsLatMin);
          setVal('bounds-lat-max', s.boundsLatMax);
          setVal('bounds-lon-min', s.boundsLonMin);
          setVal('bounds-lon-max', s.boundsLonMax);
          setChk('overlay-coastlines', s.coastlines);
          setChk('overlay-graticule', s.graticule);
          setVal('graticule-spacing', s.graticuleSpacing);
          // Dependent visibility the change handlers would normally toggle.
          // (The projection-dependent groups are syncProjectionControls' job.)
          const rm = document.getElementById('range-manual-fields');
          if (rm) rm.toggleAttribute('hidden', s.rangeMode !== 'manual');
          const bm = document.getElementById('bounds-manual-fields');
          if (bm) bm.toggleAttribute('hidden', s.boundsMode !== 'manual');
          const gl = document.getElementById('graticule-spacing-label');
          if (gl) gl.classList.toggle('spacing-hidden', !s.graticule);
          // NetCDF slice: adopt the saved spec only if it still describes a
          // variable in this panel with sane axes; indices clamp per dimension.
          const v = SLICE && s.slice ? sliceVariable(s.slice.variableIndex) : undefined;
          if (v) {
            const nd = v.dims.length;
            const okAxis = (a) => Number.isInteger(a) && a >= 0 && a < nd;
            if (okAxis(s.slice.yDim) && okAxis(s.slice.xDim) && s.slice.yDim !== s.slice.xDim &&
                Array.isArray(s.slice.sliceIndices)) {
              sliceState = {
                variableIndex: s.slice.variableIndex,
                yDim: s.slice.yDim,
                xDim: s.slice.xDim,
                sliceIndices: v.dims.map((d, i) => {
                  const raw = Math.floor(Number(s.slice.sliceIndices[i]) || 0);
                  return Math.min(Math.max(0, raw), Math.max(0, d.length - 1));
                }),
              };
            }
          }
        }

        function attachControls() {
          setupSlice();
          const projPick = document.getElementById('picker-projection');
          if (projPick) projPick.addEventListener('change', () => { syncProjectionControls(); requestRender(); });
          ['picker-center-lat', 'picker-center-lon', 'picker-preset-polar',
           'picker-central-meridian', 'picker-world-meridian'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', requestRender);
          });
          syncProjectionControls();
          // Changing the colormap repaints the legend strip locally and asks
          // the Rust side for a re-render of the grid in the new colours.
          ['picker-colormap', 'reverse-colormap'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', () => { syncColorbar(); requestRender(); });
          });
          syncColorbar();
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
              // visibility toggle (not display) so the field's box stays
              // reserved — showing it never grows the toolbar row.
              if (label) label.classList.toggle('spacing-hidden', !grat.checked);
              requestOverlay();
            });
          }
          const spacing = document.getElementById('graticule-spacing');
          if (spacing) spacing.addEventListener('change', requestOverlay);
          // Keep the overlay aligned + crisp as the panel (and the displayed
          // image size) resizes.
          window.addEventListener('resize', drawOverlay);
          // Snapshot the selections on every control change. One delegated
          // pair on the toolbar covers every control — including the
          // dynamically-built slice inputs — and bubbles fire *after* the
          // per-control target-phase handlers, so sliceState is already
          // synced when the snapshot reads it. 'input' additionally catches
          // slider drags and typed-but-uncommitted numbers.
          const toolbar = document.querySelector('.toolbar');
          if (toolbar) {
            toolbar.addEventListener('change', snapshotState);
            toolbar.addEventListener('input', snapshotState);
          }
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          if (msg.type === 'gridReady') handleGridReady(msg);
          else if (msg.type === 'gridError') handleGridError(msg);
          else if (msg.type === 'overlayReady') handleOverlayReady(msg);
          else if (msg.type === 'overlayError') handleOverlayError(msg);
        });

        restoreState();
        attachControls();
        // Attach the (possibly restored) selections so the provider's first
        // paint honours them; a fresh panel just sends its defaults.
        vscode.postMessage(Object.assign({ type: 'ready' }, currentOptions(), sliceFields()));
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
    .picker-note { display: block; color: var(--vscode-descriptionForeground); font-size: 0.8rem; margin-top: 0.25rem; }
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
    /* The gradient is painted by syncColorbar() from the Rust colormap
       registry (top = max, bottom = min) — never hardcoded here, so the strip
       cannot drift from the colours the grid is actually painted with. */
    .cb {
      width: 24px;
      height: 320px;
      border: 1px solid var(--vscode-panel-border);
    }
    .colorbar-labels {
      display: flex;
      flex-direction: column;
      justify-content: space-between;
      font-size: 0.75rem;
      color: var(--vscode-descriptionForeground);
    }
    /* The toolbar stacks its control groups vertically: the projection pickers
       on the first row, then one row each for Color Range, Bounds, and Overlay.
       Giving every group its own row is what keeps the layout stable — a group's
       Manual inputs (the Bounds row alone has four lat/lon fields) expand into
       that row's own free width and at worst wrap within it, so they never
       reshuffle the other groups the way a single shared wrap-row did. */
    .toolbar {
      display: flex;
      flex-direction: column;
      align-items: flex-start;
      gap: 0.5rem;
      padding: 0.5rem 0.75rem;
      margin-bottom: 0.75rem;
      border: 1px solid var(--vscode-panel-border);
      border-radius: 3px;
      background: var(--vscode-editorWidget-background, transparent);
      font-size: 0.85rem;
    }
    /* One row of the toolbar: lay its controls out horizontally, wrapping
       within the row only when the window is too narrow to hold them. */
    .toolbar-row {
      display: flex; align-items: center; flex-wrap: wrap;
      gap: 0.5rem 1.25rem;
    }
    .toolbar fieldset {
      display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem;
      border: none; padding: 0; margin: 0;
    }
    /* The fieldset display:flex rule above out-specifies the UA hidden rule, so
       restore it explicitly — the Bounds row hides for projections without a
       manual lat/lon window (see syncProjectionControls). */
    .toolbar fieldset[hidden] { display: none; }
    .toolbar legend {
      padding: 0;
      font-size: 0.8rem;
      color: var(--vscode-descriptionForeground);
    }
    .toolbar label { display: inline-flex; align-items: center; gap: 0.25rem; }
    /* The rule above sets display, out-specifying the UA stylesheet's hidden
       rule (display:none); without this the preset selectors never hide when
       syncProjectionControls toggles them off. */
    .toolbar label[hidden] { display: none; }
    /* The azimuthal centre groups (orthographic centre lon/lat, polar
       hemisphere + central meridian) bundle their fields in a span so
       syncProjectionControls can toggle the whole group; inline-flex spaces the
       fields like the other groups, and the explicit hidden rule restores the
       toggle the display above would otherwise out-specify. */
    .toolbar-row > span { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.5rem 1rem; }
    .toolbar-row > span[hidden] { display: none; }
    /* The graticule-spacing field hides without reflowing: visibility:hidden
       keeps its box in the toolbar's flow, so toggling Graticule on/off can't
       grow the toolbar and shove the canvas down (it also drops the field out
       of the tab order while hidden). */
    #graticule-spacing-label.spacing-hidden { visibility: hidden; }
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
    /* NetCDF slice picker: the per-dimension index controls lay out inline and
       wrap within their row like the other groups. */
    #slice-dims { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.5rem 1rem; }
    .slice-index { display: inline-flex; align-items: center; gap: 0.3rem; }
    .slice-index input[type="range"] { vertical-align: middle; }
    .slice-index input[type="number"] { width: 4.5rem; }
    .slice-len { color: var(--vscode-descriptionForeground); font-size: 0.8rem; }
  </style>
</head>
<body>
  <h1>${escapeHtml(titleLine)}</h1>
  <div class="subtitle">${escapeHtml(subLine)}</div>
  <div class="projection" id="projection-summary">${escapeHtml(projectionSummary)}</div>
  <div class="toolbar" role="toolbar" aria-label="Render settings">
${slice
    ? `    <div class="toolbar-row" id="slice-row">
      <label>Variable <select id="slice-variable"></select></label>
      <label>Y axis (rows) <select id="slice-y"></select></label>
      <label>X axis (cols) <select id="slice-x"></select></label>
      <span id="slice-dims"></span>
    </div>`
    : ""}
    <div class="toolbar-row">
      <label>Projection
        <select id="picker-projection">
          <option value="source" selected>Source projection</option>
${meta.reprojectable
          ? `          <option value="equirectangular">Equirectangular</option>
          <option value="web_mercator">Web Mercator</option>
          <option value="orthographic">Orthographic</option>
          <option value="polar_stereographic">Polar stereographic</option>
          <option value="mollweide">Mollweide</option>
          <option value="robinson">Robinson</option>
          <option value="equal_earth">Equal Earth</option>`
          : ""}
        </select>
${meta.reprojectable
        ? ""
        : `        <span class="picker-note">Reprojection isn't available for ${escapeHtml(meta.gridType ?? "this")} grids yet.</span>`}
      </label>
      <span id="preset-ortho" hidden>
        <label>Center lon
          <input type="number" id="picker-center-lon" value="0" min="-360" max="360" step="any"></label>
        <label>Center lat
          <input type="number" id="picker-center-lat" value="0" min="-90" max="90" step="any"></label>
      </span>
      <span id="preset-polar" hidden>
        <label>Hemisphere
          <select id="picker-preset-polar">
            <option value="north" selected>North</option>
            <option value="south">South</option>
          </select>
        </label>
        <label>Central meridian
          <input type="number" id="picker-central-meridian" value="0" min="-360" max="360" step="any"></label>
      </span>
      <span id="preset-world" hidden>
        <label>Central meridian
          <input type="number" id="picker-world-meridian" value="0" min="-360" max="360" step="any"></label>
      </span>
      <label>Resampling
        <select id="picker-resampling">
          <option value="nearest" selected>Nearest</option>
          <option value="bilinear">Bilinear</option>
        </select>
      </label>
      <label><input type="checkbox" id="flip-y"> Flip Y axis</label>
    </div>
${colormapFieldsetHtml(colormaps)}
    <fieldset>
      <legend>Color Range:</legend>
      <label><input type="radio" name="range-mode" value="auto" checked> Auto</label>
      <label><input type="radio" name="range-mode" value="manual"> Manual</label>
      <span id="range-manual-fields" hidden>
        <label>min <input type="number" id="range-min" step="any"></label>
        <label>max <input type="number" id="range-max" step="any"></label>
      </span>
    </fieldset>
    <fieldset id="bounds-fieldset" hidden>
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
      <label id="graticule-spacing-label" class="spacing-hidden">spacing
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
      <div class="cb" aria-label="colormap"></div>
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
