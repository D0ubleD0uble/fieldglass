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
import type { PickerColormap } from "./color-tables";
import type {
  AxisValues,
  CombineOpInfo,
  Georef,
  GridSupport,
  MessageInfo,
  VariableInfo,
  Placement,
  ProbeResult,
} from "./native";

/** What the render panel reads of the field it draws (#574).
 *
 *  A GRIB message's `MessageInfo` is one as it stands. A NetCDF or Zarr slice
 *  builds one from its variable and the handle's `sliceGrid`, so its
 *  `reprojectable`, `placement` and grid label are the slice's own answers rather than
 *  stand-ins. `grid` is `null` for a message that declares no grid, and
 *  `truncation` for anything but a band-limited spectral message (#637). */
export type PanelField = Pick<
  MessageInfo,
  | "index"
  | "parameter"
  | "units"
  | "level"
  | "levelType"
  | "forecast"
  | "uvRelativeToGrid"
  | "reprojectable"
  | "placement"
  | "supports"
  | "truncation"
  | "ensembleMember"
> & {
  grid: Pick<Georef, "label"> | null;
  /** Every GRIB message states one; a NetCDF or Zarr slice has none. */
  referenceTime: string | null;
};

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
  variables: VariableInfo[];
  initial: SliceSpec;
  /** Set while the variable the panel showed is gone from its changed file
   *  and none is picked yet (#839): the page is written with no variable's
   *  heading, as a GRIB panel is with no message's (#1230). */
  picking?: boolean;
}

/** The heading of a GRIB panel whose message is gone from its changed file,
 *  while it waits for one to be picked (#959). */
export const NO_MESSAGE_HEADING = "No message drawn";

/** The heading of a panel whose file can no longer be read, GRIB or slice (#1350). */
export const NO_FIELD_HEADING = "Nothing drawn";

/** The same for a slice panel whose variable is gone (#1230). */
export const NO_VARIABLE_HEADING = "No variable drawn";

/** One selectable "field B" for the compare / difference-map picker (#239): a
 *  GRIB message in the same file, with a human label. Field A is the message
 *  the panel opened on. */
export interface CompareFieldOption {
  index: number;
  label: string;
  /** The message's identity (`messageKey`), so a page written again for a
   *  file that changed finds its Field B by it rather than by an index the
   *  new file may give another field (#959). */
  key?: string;
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

/** A string from the file (a variable name, its units, the file's own name),
 *  inlined into the panel script as a literal. Same `<` escaping as
 *  {@link sliceJson}: a file named `</script>…` or a variable whose units
 *  hold one must not close the script (#1038). */
function scriptString(text: string): string {
  return JSON.stringify(text).replace(/</g, "\\u003c");
}

/** The colormap registry, inlined into the panel script. Same `<` escaping as
 *  {@link sliceJson}. The panel needs it to build its picker and to paint the
 *  legend gradient in the colours Rust will actually paint the grid with. */
function colormapsJson(colormaps: PickerColormap[]): string {
  return JSON.stringify(colormaps).replace(/</g, "\\u003c");
}

/** The Colors toolbar row: the colormap dropdown (grouped sequential /
 *  diverging / imported) plus the reverse toggle. Built from the registry, so a colormap
 *  added in Rust shows up here with no edit. With no registry — the native
 *  binding failed to load — the row is omitted entirely rather than offering a
 *  picker the renderer can't honour. */
function colormapFieldsetHtml(colormaps: PickerColormap[]): string {
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
  const groups = [
    group("sequential", "Sequential"),
    group("diverging", "Diverging"),
    // Colour tables the user imported (#236), after the built-in maps.
    group("imported", "Imported"),
  ]
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
      <label><input type="checkbox" id="scale-log"> Log₁₀</label>
    </fieldset>
`;
}

/** The shared operation selector for the Compare row (#239). Field A is the
 *  field the panel opened on; choosing an operation combines it element-wise
 *  with Field B. `(off)` (empty value) is the default — a plain single render. */
function compareOpSelectHtml(combineOps: CombineOpInfo[]): string {
  const options = combineOps
    .map((o) => `          <option value="${escapeHtml(o.value)}">${escapeHtml(o.label)}</option>`)
    .join("\n");
  return `      <label>Operation
        <select id="compare-op">
          <option value="" selected>(off)</option>
${options}
        </select>
      </label>`;
}

/** One `<option>` per message, its value the index and its `data-key` the
 *  message's identity (#959). Both come from a decoded file, so both are
 *  escaped. */
function messageOptionsHtml(fields: CompareFieldOption[]): string {
  return fields
    .map((f) => {
      const key = f.key ? ` data-key="${escapeHtml(f.key)}"` : "";
      return `          <option value="${f.index}"${key}>${escapeHtml(f.label)}</option>`;
    })
    .join("\n");
}

/** The GRIB Compare row: the operation selector plus a Field B message picker.
 *  Omitted (empty string) when there is nothing to compare — fewer than two
 *  messages — so a single-message file shows no Compare row. `hidden` while
 *  the page is written waiting for a pick (#1237). */
function gribCompareFieldsetHtml(
  fields: CompareFieldOption[],
  combineOps: CombineOpInfo[],
  hidden = false,
): string {
  if (fields.length < 2) {
    return "";
  }
  const options = messageOptionsHtml(fields);
  return `    <fieldset id="compare-fieldset"${hidden ? " hidden" : ""}>
      <legend>Compare:</legend>
${compareOpSelectHtml(combineOps)}
      <label>Field B
        <select id="compare-field-b">
${options}
        </select>
      </label>
    </fieldset>
`;
}

/** The Vectors row (#241): the second component of a u/v pair, the arrow
 *  spacing, and the usual colour and weight.
 *
 *  GRIB carries wind as two messages, so the pair is picked the way Compare
 *  picks its second field — and like Compare, the row only appears when the
 *  file holds two messages to pair. `uvRelativeToGrid` decides whether the
 *  components are read along the grid's axes; it comes from the message rather
 *  than from the user, because the file states it. */
function vectorFieldsetHtml(
  fields: CompareFieldOption[],
  gridRelative: boolean,
  hidden: boolean,
): string {
  if (fields.length < 2) {
    return "";
  }
  const options = messageOptionsHtml(fields);
  // Arrows are drawn along the grid's own geometry, so a message nothing
  // places takes none (#840), and neither does a placed one the library
  // cannot yet map to the Earth point by point (#945, #979). The row is then
  // `hidden` and the "Not available" line says why (#1036). A GRIB panel
  // draws one message, so this is settled when the panel is written.
  return `    <fieldset id="vector-fieldset"${hidden ? " hidden" : ""}>
      <legend>Vectors:</legend>
      <label><input type="checkbox" id="overlay-vectors"> Arrows</label>
      <label>V component
        <select id="vector-field-v">
${options}
        </select>
      </label>
      <label>every
        <input type="number" id="vector-spacing" class="layer-width" min="1" max="64" step="1" placeholder="auto" title="Draw an arrow every N grid cells; blank fits about 24 across">
        cells</label>
      <input type="color" id="color-vectors" class="layer-color" aria-label="Arrow colour">
      <input type="number" id="width-vectors" class="layer-width" value="1" min="0.2" max="5" step="0.1" aria-label="Arrow line weight">
      <span class="slice-len" id="vector-scale" aria-live="polite"></span>
      <span class="picker-note" id="vector-note">${
        // Said only of arrows that can be drawn.
        gridRelative && !hidden ? "This file states its components along the grid's axes; they are rotated to true north." : ""
      }</span>
      <span class="picker-note" id="vector-pair-note" aria-live="polite"></span>
      <span class="picker-note" id="vector-key-note" aria-live="polite"></span>
    </fieldset>
`;
}

/** The NetCDF Compare row: the operation selector plus a second set of index
 *  steppers for Field B, which is the same variable at a different slice (the
 *  "difference of two time steps / levels" case). The steppers are built
 *  dynamically into `#compare-dims` by the panel script, mirroring Field A's,
 *  each named as Field B's and set apart from Field A's (#1041). `hidden`
 *  while the page is written waiting for a pick (#1237). */
function netcdfCompareFieldsetHtml(combineOps: CombineOpInfo[], hidden = false): string {
  return `    <fieldset id="compare-fieldset"${hidden ? " hidden" : ""}>
      <legend>Compare:</legend>
${compareOpSelectHtml(combineOps)}
      <span class="compare-b slice-steppers" id="compare-dims" role="group" aria-label="Field B"></span>
    </fieldset>
`;
}

/** Longest stem an export's file name has, before its extension. Comfortably
 *  inside every filesystem's 255-byte component limit, and short enough that
 *  the save dialog shows the whole name. */
const MAX_EXPORT_NAME_STEM = 96;

/** Export CSV's tooltip (#1039). The CSV holds one field's values; with
 *  Compare on, the PNG shows the combined map but the CSV is Field A's. */
export const EXPORT_CSV_TITLE = "Export the values as CSV. With Compare on, this exports Field A, not the difference.";

/** What the CSV layout prompt adds with Compare on (#1039). */
export const CSV_FIELD_A_NOTE = "Compare is on: the CSV holds Field A, not the difference.";

/** The file types the render panel exports. */
export type ExportExtension = "png" | "csv";

/** Reduce an export filename to a safe basename ending in `.<ext>`: strip any
 *  path, keep only `[A-Za-z0-9._-]`, and fall back to `render` when nothing
 *  usable remains. Shared by the panel (default name) and the provider (final
 *  name before writing), so a hostile webview string can't steer the write.
 *  No `:` or `/` survives, so the name is valid on Windows, macOS and Linux. */
export function sanitizeExportName(name: string, ext: ExportExtension): string {
  const last = name.split(/[\\/]/).pop() ?? "";
  const base = last.toLowerCase().endsWith(`.${ext}`) ? last.slice(0, -(ext.length + 1)) : last;
  const cleaned = base
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^[-.]+|[-._]+$/g, "")
    // A last resort: `composeExportName` already fits its name under the cap
    // by shortening the parameter, the stem and the slice parts' units and
    // names, so this cut lands only on a name from elsewhere or one whose
    // values alone pass the cap. Trim any separator it leaves dangling.
    .slice(0, MAX_EXPORT_NAME_STEM)
    .replace(/[-._]+$/g, "")
    .toLowerCase()
    // Windows reserves its device names as a file's name, with any extension
    // or none (`aux.csv` is the AUX device), so one gets a `-` after it.
    .replace(/^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?=\.|$)/, "$1-");
  return `${cleaned || "render"}.${ext}`;
}

/** {@link sanitizeExportName} for a PNG. */
export function sanitizePngName(name: string): string {
  return sanitizeExportName(name, "png");
}

/** Width of the exported PNG (#243).
 *
 *  The image has two things to fit: the map block (raster + colorbar + its
 *  labels) and the title/subtitle header, which is drawn as one unwrapped
 *  line. Sizing on the map block alone clipped the header on any field whose
 *  raster is narrower than its own title — a synthesized spectral grid is
 *  256 px wide, so its reference time ran off the right edge, while a
 *  1440-wide GFS raster hid the defect entirely.
 *
 *  Serialized into the panel script (`exportCanvasWidth.toString()`) so the
 *  webview runs this exact function rather than a copy of the rule. Keep it
 *  free of closure references for that reason. */
export function exportCanvasWidth(
  mapBlockW: number,
  headerW: number,
  margin: number,
): number {
  return Math.max(mapBlockW, margin + headerW + margin);
}

/** How much longer one side of a lat/lon map's raster may be than the other
 *  before it is held (#1251). A map keeps its own shape, so this is far past
 *  the eight a view with no geographic meaning is held to; it only stops a
 *  1 × 720 raster being drawn under a pixel wide. At 48 the tall image is
 *  still a column the page's height over 48 wide, about a dozen pixels.
 *  Serialized into the panel script. */
export const MAP_MOST_ASPECT = 48;

/** The CSS `aspect-ratio` the image is drawn at for a raster `width × height`
 *  (#1226): empty, which keeps the raster's own aspect, unless one side is
 *  more than `most` (default eight) times the other; then `most` to one,
 *  the long way.
 *
 *  The image fills the panel's width, so a raster 2 wide and 366 tall was
 *  drawn about 775 × 141,000 px, past what the page will paint, and one 366
 *  wide and 2 tall was a strip of rows a pixel or two high. Held to eight to
 *  one, each cell across the short side is a band, and the cells stretch
 *  along it instead of being square. The click maps by fractions of the
 *  image's box, and the overlay scales each axis on its own, so both still
 *  land on the cell drawn. Export PNG writes the raster at its own size.
 *
 *  The panel does not apply the default eight to a lat/lon map
 *  (Equirectangular, Web Mercator), whose raster has its grid's or its Bounds
 *  window's shape: it passes {@link MAP_MOST_ASPECT} (48) as `most` instead,
 *  so only an extreme raster (about 1 × 720) is held, to a width a pixel can
 *  show (#1251). So does a view with arrows drawn: they are laid out in
 *  square raster pixels, which a stretch would bend, and up to that limit
 *  there is none (#1251). Whether the
 *  image is then drawn to the page's height rather than across the panel's
 *  width follows from the aspect drawn and the page, not from this (#1252).
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function displayAspect(width: number, height: number, most = 8): string {
  if (!(width > 0) || !(height > 0)) return "";
  if (height > most * width) return "1 / " + most;
  if (width > most * height) return most + " / 1";
  return "";
}

/** Whether the slice on axes `(yDim, xDim)` is a map — the file's own
 *  horizontal pair — as opposed to a cross-section through any two other axes
 *  (#171).
 *
 *  A file whose horizontal axes are projected (WRF, a satellite swath) detects
 *  neither, and its axes are whatever the picker opened on; those stay maps,
 *  because only the Rust side can say where such a grid sits. A file that names
 *  its latitude and longitude is a map on exactly that pair, in that order: the
 *  transposed pick draws latitude across the columns, which is a plot, not a
 *  map.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function isMapSlice(
  yDim: number,
  xDim: number,
  detectedYDim: number | null | undefined,
  detectedXDim: number | null | undefined,
): boolean {
  if (detectedYDim == null || detectedXDim == null) return true;
  return yDim === detectedYDim && xDim === detectedXDim;
}

/** The map targets the projection picker offers beside the source view, in
 *  picker order. The provider's `PROJECTIONS` must list every one. */
export const MAP_PROJECTIONS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "equirectangular", label: "Equirectangular" },
  { value: "web_mercator", label: "Web Mercator" },
  { value: "orthographic", label: "Orthographic" },
  { value: "polar_stereographic", label: "Polar stereographic" },
  { value: "mollweide", label: "Mollweide" },
  { value: "robinson", label: "Robinson" },
  { value: "equal_earth", label: "Equal Earth" },
];

/** The projection picker's options: the source view always, and the map
 *  targets only for a field that can be reprojected.
 *
 *  Built here for the panel's first HTML and again by the panel script each
 *  time a slice render says the answer changed (#822), so the two cannot offer
 *  different lists. `targets` is {@link MAP_PROJECTIONS}, whose labels are
 *  constants. Serialized into the panel script, so it must not reference
 *  anything outside itself. */
export function projectionOptionsHtml(
  targets: ReadonlyArray<{ value: string; label: string }>,
  reprojectable: boolean,
): string {
  let html = '<option value="source">Source projection</option>';
  if (reprojectable) {
    for (const t of targets) html += '<option value="' + t.value + '">' + t.label + "</option>";
  }
  return html;
}

/** The projection `<select>`, moved to a slice render's answer to whether it
 *  can be reprojected (#822). `offered` is whether the select currently lists
 *  the map targets; the options are rebuilt only when that changes, keeping the
 *  selection where it is still offered and going back to `source` where it is
 *  not — which is what the provider drew it in. Where it is not, the panel
 *  hides the picker and gives the reason in its "Not available for this grid"
 *  line (#1036), and holds the target it moved off for the next slice that
 *  takes it (#1099).
 *
 *  Returns the new `offered`, and whether the selection moved. Takes its
 *  element as an argument, so a test can run it against a stand-in. Serialized
 *  into the panel script, so it must not reference anything outside itself but
 *  {@link projectionOptionsHtml}, which is serialized beside it. */
export function applyReprojectable(
  projection: { value: string; innerHTML: string; options: ArrayLike<{ value: string }> } | null,
  sliceGrid: { reprojectable: boolean },
  offered: boolean,
  targets: ReadonlyArray<{ value: string; label: string }>,
): { offered: boolean; moved: boolean } {
  const reprojectable = sliceGrid.reprojectable === true;
  let moved = false;
  if (projection && reprojectable !== offered) {
    const keep = projection.value;
    projection.innerHTML = projectionOptionsHtml(targets, reprojectable);
    const still = Array.from(projection.options).some((o) => o.value === keep);
    projection.value = still ? keep : "source";
    moved = !still;
  }
  return { offered: reprojectable, moved };
}

/** Whether the render panel offers its overlays for a field: the map layers,
 *  the graticule, contours and arrows all project through the field's
 *  geometry, so a field that is not placed on the Earth takes none of them
 *  (#840). The source view of an unplaceable field still draws; it is only
 *  the map that is missing. */
export function offersOverlays(placement: Placement | null | undefined): boolean {
  return placement === "placed";
}

/** What a field takes when nothing answered for it: none of the point
 *  operations. A slice the handle could not place, or a message with no grid. */
export const NO_SUPPORT: GridSupport = { contours: false, vectors: false, longCsv: false, zonalMean: false, bilinear: false };

/** Said in the CSV layout prompt when a placed field's grid does not take the
 *  long layout (#945). Today that is every satellite view; see
 *  {@link SATELLITE_NOT_YET_REASON}. */
export const NO_LONG_CSV_NOTE = "The long layout isn't available for satellite-view grids yet.";

/** {@link NO_LONG_CSV_NOTE} for a field nothing places on the Earth, where no
 *  point has a position. */
export const UNPLACED_LONG_CSV_NOTE = "The long layout needs a grid placed on the Earth, and this one isn't.";

/** A panel control a grid can't use (#1036). The panel hides it, rather than
 *  disabling it, and names it in the "Not available for this grid" line under
 *  the grid caption. `longCsv` is the long layout of a slice panel's CSV
 *  export, which the layout prompt leaves out; the rest are on the page.
 *  Resampling goes with the projection: the source view paints one pixel per
 *  cell and never resamples (`TargetKind::Source` in the `fieldglass` crate's
 *  `render.rs`), so a field shown only in it has nothing to resample. It also
 *  goes on a grid whose warp draws nearest whatever is asked (#1225). */
export type UnavailableKey = "projection" | "resampling" | "overlay" | "contours" | "arrows" | "zonal" | "longCsv";

/** Each control's label as the panel shows it, which is how the line names it. */
export const UNAVAILABLE_LABELS: Readonly<Record<UnavailableKey, string>> = {
  projection: "Projection",
  resampling: "Resampling",
  overlay: "Overlay",
  contours: "Contours",
  arrows: "Vectors",
  zonal: "Zonal average",
  longCsv: "Long layout (Export CSV)",
};

/** Controls a grid can't use for one reason (#1036).
 *
 *  `structural` says the grid itself rules them out. `notYet` says Fieldglass
 *  does not support them for this kind of grid yet. The two tell a user
 *  different things, so the line keeps them apart, structural reasons first. */
export interface UnavailableGroup {
  kind: "structural" | "notYet";
  reason: string;
  controls: Array<{ key: UnavailableKey; label: string }>;
}

/** Why a control needs a grid on the Earth and this one has none (#840,
 *  #960): overlays, arrows, the zonal average, the long CSV and reprojection
 *  all give it for a grid Rust does not place. */
export const UNPLACED_REASON = "this grid has no known position on the Earth.";

/** Why a placed grid takes no zonal average: a rotated or projected grid (#945). */
export const NO_ZONAL_REASON = "this grid's rows aren't circles of latitude.";

/** Why a grid that reprojects takes no Resampling choice (#1225): it is a list
 *  of cell centres, such as a satellite swath or a tripolar ocean grid, and the
 *  warp draws the nearest cell whatever is asked, since two cells side by side
 *  in the file can be far apart on the Earth. Which grids those are is Rust's
 *  answer (`GridSupport.bilinear`), not a list kept here. */
export const NO_BILINEAR_REASON = "this grid places each cell by its own coordinates, so the map always shows the nearest cell.";

/** Why a placed grid takes no contours, arrows or long CSV (#945). Which grids
 *  those are is Rust's answer (`GridSupport`), not a list kept here. Today it
 *  is every satellite view (a geostationary grid), whatever its extent: the
 *  library has no map from such a grid to the Earth yet (#979). */
export const SATELLITE_NOT_YET_REASON = "not yet supported for satellite-view grids.";

/** Why a field has no map projection but the source view, or `null` for one
 *  that can be reprojected (#960).
 *
 *  Keyed on the placement Rust reports, never on the family: a grid of a
 *  family this build reprojects can still be declined because its own numbers
 *  place nothing (a Lambert grid on an Earth of radius zero is `unplaceable`
 *  under the label `lambert`). Only an `unsupported` template is a family this
 *  build does not support yet. A placed grid that does not reproject (a
 *  lat/lon grid scanned east to west) is that one grid's layout, not its
 *  family, and a limit of this build rather than of the grid: Rust's answer
 *  gives no reason beyond `reprojectable`, so the line says only that.
 *  Everything else is not placed on the Earth.
 *
 *  `label` is the grid's family, `null` when there is no grid. It is the
 *  file's text, which {@link unavailableHtml} escapes. */
export function projectionReason(
  reprojectable: boolean,
  placement: Placement | null | undefined,
  label: string | null,
): { kind: UnavailableGroup["kind"]; reason: string } | null {
  if (reprojectable) return null;
  if (placement === "unsupported") {
    return { kind: "notYet", reason: "not yet supported for " + (label === null ? "this grid" : label + " grids") + "." };
  }
  if (placement === "placed") {
    return { kind: "notYet", reason: "not yet supported for this grid's layout." };
  }
  return { kind: "structural", reason: UNPLACED_REASON };
}

/** The controls a field's grid can't use, grouped by reason, structural
 *  reasons first (#1036). Rust decides what each grid takes (`placement`,
 *  `reprojectable`, `supports`, Resampling included through
 *  `supports.bilinear`); this only names the controls and the reason.
 *
 *  `offers` says which of these the panel has at all: the Vectors row is
 *  written only for a GRIB file of two or more messages, and only a slice
 *  panel exports a CSV. A control the panel never had is not listed. Contours
 *  sit in the Overlay row, so a grid that takes no overlay lists the row, not
 *  its contours. */
export function unavailableControls(
  field: {
    reprojectable: boolean;
    placement: Placement | null | undefined;
    label: string | null;
    supports: GridSupport;
  },
  offers: { arrows: boolean; csv: boolean },
): UnavailableGroup[] {
  const placed = offersOverlays(field.placement);
  const notPlaced = { kind: "structural", reason: UNPLACED_REASON } as const;
  const notYet = { kind: "notYet", reason: SATELLITE_NOT_YET_REASON } as const;
  const found: Array<{ key: UnavailableKey; kind: UnavailableGroup["kind"]; reason: string }> = [];
  const projection = projectionReason(field.reprojectable, field.placement, field.label);
  if (projection) {
    found.push({ key: "projection", ...projection });
    // Only the source view is left, which never resamples.
    found.push({ key: "resampling", ...projection });
  } else if (!field.supports.bilinear) {
    // The warp draws nearest whatever is asked, so Bilinear would change
    // nothing (#1225).
    found.push({ key: "resampling", kind: "structural", reason: NO_BILINEAR_REASON });
  }
  if (!placed) found.push({ key: "overlay", ...notPlaced });
  else if (!field.supports.contours) found.push({ key: "contours", ...notYet });
  if (offers.arrows && !(placed && field.supports.vectors)) {
    found.push({ key: "arrows", ...(placed ? notYet : notPlaced) });
  }
  if (!field.supports.zonalMean) {
    found.push({ key: "zonal", ...(placed ? ({ kind: "structural", reason: NO_ZONAL_REASON } as const) : notPlaced) });
  }
  if (offers.csv && !(placed && field.supports.longCsv)) {
    found.push({ key: "longCsv", ...(placed ? notYet : notPlaced) });
  }
  const groups: UnavailableGroup[] = [];
  for (const kind of ["structural", "notYet"] as const) {
    for (const f of found) {
      if (f.kind !== kind) continue;
      let group = groups.find((g) => g.kind === kind && g.reason === f.reason);
      if (!group) {
        group = { kind, reason: f.reason, controls: [] };
        groups.push(group);
      }
      group.controls.push({ key: f.key, label: UNAVAILABLE_LABELS[f.key] });
    }
  }
  return groups;
}

/** The "Not available for this grid" line's summary (#1036): every hidden
 *  control by its label, as plain text. Empty when nothing is hidden.
 *
 *  Serialized into the panel script, which sets it as the summary's
 *  `textContent` from each slice render's answer, so it must not reference
 *  anything outside itself. */
export function unavailableSummary(groups: ReadonlyArray<UnavailableGroup> | null | undefined): string {
  if (!groups || groups.length === 0) return "";
  const labels: string[] = [];
  for (const g of groups) for (const c of g.controls) labels.push(c.label);
  return "Not available for this grid: " + labels.join(", ");
}

/** The line's list (#1036): one `<li>` per reason, naming the controls that
 *  share it, `data-kind` saying which kind of reason it is.
 *
 *  Serialized into the panel script like {@link unavailableSummary}. A reason
 *  can carry the file's grid family, so every string is escaped. */
export function unavailableItemsHtml(groups: ReadonlyArray<UnavailableGroup> | null | undefined): string {
  if (!groups) return "";
  const esc = (s: string) =>
    String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  let items = "";
  for (const g of groups) {
    const names = g.controls.map((c) => esc(c.label));
    items += '<li data-kind="' + esc(g.kind) + '">' + names.join(", ") + ": " + esc(g.reason) + "</li>";
  }
  return items;
}

/** The inside of the "Not available for this grid" disclosure (#1036), as the
 *  panel's first HTML writes it. The summary and the list are always there,
 *  empty when nothing is hidden (the panel then hides the disclosure), so the
 *  script can rewrite each in place and keep the summary node a user may be
 *  focused on. */
export function unavailableHtml(groups: ReadonlyArray<UnavailableGroup> | null | undefined): string {
  return "<summary>" + escapeHtml(unavailableSummary(groups)) + "</summary><ul>" + unavailableItemsHtml(groups) + "</ul>";
}

/** Up to `count` evenly spaced indices along an axis of `length` points, always
 *  including the first and last. Where a cross-section puts its tick labels.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function axisTickIndices(length: number, count: number): number[] {
  if (length <= 0) return [];
  if (length === 1) return [0];
  const wanted = Math.max(2, Math.min(count, length));
  const ticks: number[] = [];
  for (let i = 0; i < wanted; i++) {
    ticks.push(Math.round((i * (length - 1)) / (wanted - 1)));
  }
  // Rounding can land two ticks on one index on a short axis.
  return ticks.filter((v, i) => i === 0 || v !== ticks[i - 1]);
}

/** A CF time's origin, read from `units` of the form `<unit> since
 *  <reference>`: `epoch`, the reference as UTC milliseconds since 1970, and
 *  `unit`, the milliseconds in one of its units. `null` for units that are
 *  not a time, or a reference it cannot read.
 *
 *  The reference is read in UDUNITS' spelling here rather than by Date.parse,
 *  which refuses most of it (#1028): one-digit fields (`1900-1-1`),
 *  fractional or one-digit seconds (`00:00:0.0`, which every NCEP/NCAR
 *  reanalysis file writes), a space or `T` before the time, no time at all,
 *  and a zone as `Z`, `UTC`, `GMT` or an offset (`+05:30`, `-6`). A field out
 *  of range (month 13, minute 60) is refused rather than rolled over.
 *
 *  The calendar is not consulted: no calendar attribute reaches the panel, so
 *  a `noleap` or `360_day` axis is counted on the Gregorian calendar and its
 *  dates drift from the model's (#1033).
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function cfTimeOrigin(units: string | null): { epoch: number; unit: number } | null {
  // `(.+)` and a trim after, not `(.+?)\s*$`: the lazy form backtracks over
  // trailing whitespace, quadratic in a units attribute a file controls.
  const since = /^\s*(\w+)\s+since\s+(.+)$/i.exec(units || "");
  const perUnit: Record<string, number> = {
    second: 1000, seconds: 1000, sec: 1000, secs: 1000, s: 1000,
    minute: 60000, minutes: 60000, min: 60000, mins: 60000,
    hour: 3600000, hours: 3600000, hr: 3600000, hrs: 3600000, h: 3600000,
    day: 86400000, days: 86400000, d: 86400000,
    week: 604800000, weeks: 604800000,
  };
  const unit = since ? perUnit[since[1].toLowerCase()] : undefined;
  if (!since || !unit) return null;
  let rest = since[2].trim();
  const take = (re: RegExp) => {
    const m = re.exec(rest);
    if (m) rest = rest.slice(m[0].length);
    return m;
  };
  const day = take(/^(\d{1,4})-(\d{1,2})-(\d{1,2})/);
  const clock = day ? take(/^[T ]+(\d{1,2}):(\d{1,2})/) : null;
  const seconds = clock ? take(/^:(\d{1,2}\.?\d*)/) : null;
  rest = rest.trim();
  // `+5`, `+05`, `+0530` or `+05:30`, and nothing looser: `+123` is refused,
  // not read as one hour and 23 minutes.
  const offset = /^([+-])(\d\d?)$/.exec(rest) || /^([+-])(\d\d):?(\d\d)$/.exec(rest);
  const inRange = !!day && Number(day[2]) >= 1 && Number(day[2]) <= 12 &&
    Number(day[3]) >= 1 && Number(day[3]) <= 31 &&
    (!clock || (Number(clock[1]) <= 24 && Number(clock[2]) <= 59)) &&
    (!seconds || Number(seconds[1]) < 61) &&
    (!offset || (Number(offset[2]) <= 14 && Number(offset[3] || 0) <= 59));
  if (!day || !inRange || !(rest === "" || /^(Z|UTC|GMT)$/i.test(rest) || offset)) return null;
  const at = new Date(0);
  // setUTCFullYear, since Date.UTC reads a year below 100 as 19xx.
  at.setUTCFullYear(Number(day[1]), Number(day[2]) - 1, Number(day[3]));
  at.setUTCHours(clock ? Number(clock[1]) : 0, clock ? Number(clock[2]) : 0, 0, 0);
  let epoch = at.getTime() + (seconds ? Number(seconds[1]) : 0) * 1000;
  if (offset) {
    // `+05:30` is 5 h 30 min ahead of UTC, so its midnight is earlier.
    const minutes = Number(offset[2]) * 60 + Number(offset[3] || 0);
    epoch -= (offset[1] === "-" ? -1 : 1) * minutes * 60000;
  }
  return { epoch: epoch, unit: unit };
}

/** One axis value as a tick label. A CF time (`units` of the form
 *  `<unit> since <reference>`, read by {@link cfTimeOrigin}) becomes a date,
 *  since `481140 minutes` is not a label anyone can read; everything else,
 *  and a time whose reference cannot be read, is the number, trimmed.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself but the helpers injected beside it (it calls {@link cfTimeOrigin}
 *  and {@link typesetMinus}). */
export function formatAxisValue(value: number, units: string | null): string {
  if (!Number.isFinite(value)) return "";
  const origin = cfTimeOrigin(units);
  if (origin) {
    const at = new Date(origin.epoch + value * origin.unit);
    // An instant past the Date range is invalid, and toISOString would throw;
    // one outside years 0 to 9999 has no YYYY-MM-DD form to show.
    const year = at.getUTCFullYear();
    if (Number.isFinite(at.getTime()) && year >= 0 && year <= 9999) {
      const date = at.toISOString().slice(0, 10);
      const time = at.toISOString().slice(11, 16);
      return time === "00:00" ? date : date + " " + time;
    }
  }
  // Six significant digits, then back through `Number` to drop the padding
  // `toPrecision` adds: a coordinate array carries float noise
  // (`0.30000000004`), and a tick label should show neither that, nor `0.000`
  // for zero, nor `1013` for a pressure level of `1013.25`. Signed the way the
  // line plot's caption is (#1043).
  return typesetMinus(String(Number(value.toPrecision(6))));
}

/** Units as a reader sees them after a value: trimmed, and empty for none. CF's
 *  dimensionless unit `"1"` (sigma and s coordinates, ratios) reads as none,
 *  since `0.995 1` is a number and a stray digit. Only exactly `1` after
 *  trimming is dropped; `1e-3` and `100 1` are real units and stay.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function displayUnits(units: string | null | undefined): string {
  const text = (units || "").trim();
  return text === "1" ? "" : text;
}

/** One coordinate value as the slice controls and the subtitle state it
 *  (#1041): the axis label {@link formatAxisValue} writes, with the axis's
 *  units after it, `500 millibar`. A CF time reads as its date alone,
 *  `2024-01-15`; a time whose reference cannot be read stays a number and
 *  keeps its units, `1963872 hours since …`, so it is never a bare count.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself but the helpers injected beside it. */
export function formatCoordinate(value: number, units: string | null): string {
  const text = formatAxisValue(value, units);
  const shown = displayUnits(units);
  // A date starts `YYYY-MM-DD`; a number's minus signs are U+2212, so no
  // number reads that way.
  const date = /^\d{4}-\d\d-\d\d/.test(text);
  return text !== "" && !date && shown ? text + " " + shown : text;
}

/** One dimension a slice holds still, as {@link composeSliceNote} takes it:
 *  its name, the index held, and the coordinate there as
 *  {@link formatCoordinate} writes it, `null` for an axis with no
 *  coordinates. */
export interface SliceNotePart {
  name: string;
  index: number;
  coordinate: string | null;
}

/** The slice a map shows, for the panel subtitle and so the exported PNG
 *  (#1041): each held dimension by name and coordinate, `time 2024-01-15 ·
 *  level 500 millibar`, or by its index where the file gives the axis no
 *  coordinates, `phony_dim_0 index 3`. A Compare map adds where Field B
 *  differs from it, `Field B at level 850 millibar`. Empty when nothing is
 *  held.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function composeSliceNote(
  parts: ReadonlyArray<SliceNotePart>,
  partsB: ReadonlyArray<SliceNotePart> | null,
): string {
  const one = (p: SliceNotePart) => p.name + " " + (p.coordinate !== null ? p.coordinate : "index " + p.index);
  const note = parts.map(one);
  if (partsB) {
    const differ = partsB.filter((b, i) => {
      const [a] = parts.slice(i, i + 1);
      return !a || a.index !== b.index;
    });
    if (differ.length > 0) note.push("Field B at " + differ.map(one).join(", "));
  }
  return note.join(" · ");
}

/** The dimensions a slice holds still, as {@link composeSliceNote} and the
 *  export name ({@link composeExportName}) state them: every dimension but
 *  the two image axes, by `dims` name, at its index in `sliceIndices`, with
 *  the coordinate there as {@link formatCoordinate} writes it, or `null` where
 *  the axis has none. `axisOf(dim)` gives an axis's values, `null` for none,
 *  or `undefined` while they are not yet known, which makes the answer `null`
 *  so a slice is never named by index and then by value.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself but the helpers injected beside it. */
export function heldSliceParts(
  dims: ReadonlyArray<string>,
  slice: { yDim: number; xDim: number; sliceIndices: ReadonlyArray<number> },
  axisOf: (dim: number) => Pick<AxisValues, "coordinates" | "units"> | null | undefined,
): SliceNotePart[] | null {
  const out: SliceNotePart[] = [];
  let known = true;
  dims.forEach((name, i) => {
    if (i === slice.yDim || i === slice.xDim) return;
    const axis = axisOf(i);
    if (axis === undefined) known = false;
    const [held] = slice.sliceIndices.slice(i, i + 1);
    const index = held || 0;
    const [value] = axis && axis.coordinates ? axis.coordinates.slice(index, index + 1) : [];
    const text = axis && value !== undefined ? formatCoordinate(value, axis.units) : "";
    out.push({ name: name, index: index, coordinate: text || null });
  });
  return known ? out : null;
}

/** A number's text with a true minus sign (U+2212) for each hyphen, as a tick
 *  label and the line plot's caption write it (#1037, #1043): `−90`, not
 *  `-90`, and `1.5e−7`. A value that reads as zero is not negative, so `-0`
 *  and `-0.0` lose the sign. For a number's text only: a date's hyphens are
 *  not minus signs.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function typesetMinus(text: string): string {
  return (Number(text) === 0 ? text.replace(/^-/, "") : text).replace(/-/g, "−");
}

/** Round numbers to label a value axis from `min` to `max` with about `count`
 *  ticks: steps of 1, 2, 2.5 or 5 times a power of ten, every tick inside the
 *  range (#1043). Empty for a range with no span.
 *
 *  An axis in `degrees` (a latitude or longitude) steps the way a map's
 *  graticule does once a step is a degree or more: 1, 2, 5, 10, 15, 30, 45, 90
 *  or 180, so a globe is ticked `−60, −30, 0, 30, 60` rather than every 50
 *  (#1229).
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function niceTicks(min: number, max: number, count: number, degrees = false): number[] {
  if (!(Number.isFinite(min) && Number.isFinite(max)) || !(max > min) || !(count >= 1)) return [];
  const raw = (max - min) / count;
  const magnitude = Math.pow(10, Math.floor(Math.log10(raw)));
  const angular = degrees && raw >= 1 ? [1, 2, 5, 10, 15, 30, 45, 90, 180].find((s) => s >= raw) : undefined;
  const step = angular || [1, 2, 2.5, 5, 10].map((f) => f * magnitude).find((s) => s >= raw) || 10 * magnitude;
  const first = Math.ceil(min / step - 1e-9);
  const ticks: number[] = [];
  for (let k = first; k * step <= max + step * 1e-9 && ticks.length < 100; k++) {
    // Through toPrecision, so 0.1 + 0.2 steps print as 0.3.
    ticks.push(Number((k * step).toPrecision(12)));
  }
  return ticks;
}

/** Round dates to label a CF time axis from `min` to `max` (values in its
 *  `units`, `<unit> since <reference>`) with about `count` ticks (#1392):
 *  each tick's `value` in those units and its label. The step is the first of
 *  these that gives no more than `count`: 1, 2, 5, 10, 15 or 30 minutes; 1,
 *  3, 6 or 12 hours; 1 or 2 days; a week, on Mondays; half a month, on the
 *  1st and 16th; 1, 2, 3 or 6 months, on the 1st; or 1, 2 or 5 times a power
 *  of ten years, on 1 January. Minutes, hours and days count from midnight.
 *  The step is never finer than `spacing`, the closest two points in the same
 *  units, so a daily axis is not labelled at noon.
 *
 *  One format for every label on the axis: `YYYY-MM-DD HH:MM` for a step under
 *  a day, `YYYY-MM-DD` otherwise. All in UTC.
 *
 *  Dates are counted on the proleptic Gregorian calendar, as
 *  {@link formatAxisValue} writes them (#1033 has the calendar).
 *
 *  `null` for units that are not a readable time, an end outside years 0 to
 *  9999, or a span too short for two ticks a minute or more apart.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself but the helpers injected beside it (it calls {@link cfTimeOrigin}). */
export function calendarTicks(
  min: number,
  max: number,
  units: string | null,
  count: number,
  spacing = 0,
): { value: number; text: string }[] | null {
  const origin = cfTimeOrigin(units);
  if (!origin || !(Number.isFinite(min) && Number.isFinite(max)) || !(max > min) || !(count >= 1)) return null;
  const from = origin.epoch + min * origin.unit;
  const to = origin.epoch + max * origin.unit;
  const datable = (t: number) => {
    const year = new Date(t).getUTCFullYear();
    return Number.isFinite(year) && year >= 0 && year <= 9999;
  };
  if (!datable(from) || !datable(to)) return null;
  const minute = 60000;
  const hour = 60 * minute;
  const day = 24 * hour;
  // Each step as [milliseconds, months]: a fixed length, or whole months,
  // where 0.5 is the 1st and 16th.
  const steps: [number, number][] = [
    [minute, 0], [2 * minute, 0], [5 * minute, 0], [10 * minute, 0], [15 * minute, 0], [30 * minute, 0],
    [hour, 0], [3 * hour, 0], [6 * hour, 0], [12 * hour, 0], [day, 0], [2 * day, 0], [7 * day, 0],
    [0, 0.5], [0, 1], [0, 2], [0, 3], [0, 6],
  ];
  for (let years = 1; years <= 10000; years *= 10) {
    steps.push([0, 12 * years], [0, 24 * years], [0, 60 * years]);
  }
  // A step's rough length, to choose by: a month is 1/12 of 365.2425 days.
  const length = (s: [number, number]) => s[0] || (s[1] * 365.2425 * day) / 12;
  const finest = Math.max(0, spacing) * origin.unit * (1 - 1e-9);
  const usable = steps.filter((s) => length(s) >= finest);
  const at = (year: number, month: number, date: number) => {
    const t = new Date(0);
    // setUTCFullYear, since Date.UTC reads a year below 100 as 19xx.
    t.setUTCFullYear(year, month, date);
    return t.getTime();
  };
  const instants = (s: [number, number]) => {
    const out: number[] = [];
    if (s[0]) {
      // A week starts on Monday; 1970-01-05 was one.
      const anchor = s[0] === 7 * day ? 4 * day : 0;
      for (let k = Math.ceil((from - anchor) / s[0] - 1e-9); anchor + k * s[0] <= to && out.length < 1000; k++) {
        out.push(anchor + k * s[0]);
      }
      return out;
    }
    const start = new Date(from);
    const months = Math.max(1, s[1]);
    // Months counted from January of year 0, so a step of months or years
    // lands on the same months every year.
    let index = Math.ceil((start.getUTCFullYear() * 12 + start.getUTCMonth()) / months) * months;
    if (s[1] < 1) index = start.getUTCFullYear() * 12 + start.getUTCMonth();
    for (; out.length < 1000; index += months) {
      const year = Math.floor(index / 12);
      const days = s[1] < 1 ? [1, 16] : [1];
      const next = days.map((d) => at(year, index - year * 12, d));
      for (const t of next) if (t >= from && t <= to) out.push(t);
      if (next[0] > to) break;
    }
    return out;
  };
  const fits = usable.findIndex((s) => (to - from) / length(s) <= count);
  // A coarse step can miss all but one round date in the span; the steps
  // below it have more.
  let found: number[] = [];
  let step: [number, number] = [0, 0];
  for (let k = fits < 0 ? usable.length - 1 : fits; k >= 0 && found.length < 2; k--) {
    [step] = usable.slice(k, k + 1);
    found = instants(step);
  }
  if (found.length < 2) return null;
  const withTime = step[0] > 0 && step[0] < day;
  return found.map((t) => {
    const iso = new Date(t).toISOString();
    return { value: (t - origin.epoch) / origin.unit, text: withTime ? iso.slice(0, 10) + " " + iso.slice(11, 16) : iso.slice(0, 10) };
  });
}

/** One tick on a coordinate axis (#1229): the round `value` it labels, the
 *  `index` along the axis where that value falls, fractional between two
 *  points, and its label. */
export interface CoordinateTick {
  value: number;
  index: number;
  text: string;
}

/** Round-number ticks for a numeric coordinate axis, about `count` of them
 *  (#1229): {@link niceTicks} over the span of the axis's values, each placed
 *  at the index where its value falls, read off the coordinates between the
 *  two points either side of it. So an uneven axis, such as Gaussian
 *  latitudes, or one stored north first, is ticked at `60, 30, 0, −30, −60`
 *  in the right places, not at the values its points happen to have. An axis
 *  in degrees takes {@link niceTicks}'s graticule steps.
 *
 *  Evenly spaced in value, these suit a line plot, which is drawn linear in
 *  value. A cross-section rail draws one row per point, so it takes them
 *  only where the points are about evenly spaced, and keeps its own points on
 *  an uneven axis such as ocean depth ({@link railTicks}, #1258).
 *
 *  A CF time is ticked at round dates instead, by {@link calendarTicks}
 *  (#1392), no finer than the axis's own points are spaced.
 *
 *  `null` where the axis keeps its own values as ticks: one with no
 *  coordinates or fewer than two, any of them not finite, values that do not
 *  run one way (nothing to interpolate along), an atmospheric pressure (its
 *  ticks are the levels themselves, drawn on log pressure, #1043), or a span
 *  too short for two round values or two round dates.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself but the helpers injected beside it (it calls
 *  {@link niceTicks}, {@link calendarTicks} and {@link formatAxisValue}). */
export function coordinateTicks(
  axis: { coordinates: number[] | null; units: string | null; vertical?: { logPressure: boolean } | null } | null | undefined,
  count: number,
): CoordinateTick[] | null {
  const values = axis && axis.coordinates;
  if (!axis || !values || values.length < 2 || !values.every((v) => Number.isFinite(v))) return null;
  if (axis.vertical && axis.vertical.logPressure) return null;
  const rising = values[1] > values[0];
  // Every step the same way, so each value falls between one pair of points.
  // The closest two points, for a time's finest step.
  let previous: number | null = null;
  let closest = Infinity;
  for (const v of values) {
    if (previous !== null && (rising ? !(v > previous) : !(v < previous))) return null;
    if (previous !== null) closest = Math.min(closest, Math.abs(v - previous));
    previous = v;
  }
  const first = values[0];
  const last = values[values.length - 1];
  const low = Math.min(first, last);
  const high = Math.max(first, last);
  // CF's latitude and longitude units (§4.1, §4.2), and plain degrees.
  const units = (axis.units || "").trim().toLowerCase().replace(/_/g, "");
  const degrees = /^(degrees?|deg)(north|east|south|west|n|e|s|w)?$/.test(units);
  // A date starts `YYYY-MM-DD`; a number's minus signs are U+2212, so no
  // number reads that way.
  const marks = /^\d{4}-\d\d-\d\d/.test(formatAxisValue(first, axis.units))
    ? calendarTicks(low, high, axis.units, count, closest)
    : niceTicks(low, high, count, degrees).map((v) => ({ value: v, text: formatAxisValue(v, axis.units) }));
  if (!marks) return null;
  const ticks: CoordinateTick[] = [];
  for (const { value: v, text } of marks) {
    // The pair either side of the value: the first point at or past it, and
    // the one before. A round value a rounding error past the last point
    // takes the last pair.
    const past = values.findIndex((c) => (rising ? c >= v : c <= v));
    const i = past < 0 ? values.length - 2 : Math.max(0, past - 1);
    const [a, b] = values.slice(i, i + 2);
    const index = Math.min(values.length - 1, Math.max(0, i + (v - a) / (b - a)));
    ticks.push({ value: v, index: index, text: text });
  }
  return ticks.length >= 2 ? ticks : null;
}

/** About `count` ticks for a cross-section rail of `length` rows or columns,
 *  each at the `index` its label falls at, fractional between two, in order
 *  of index (#1258).
 *
 *  A rail draws one row per point, evenly spaced by index whatever the
 *  values. So it takes {@link coordinateTicks}'s round values only where they
 *  land about evenly along it: no stretch between two of them, or from an end
 *  of the rail to the nearest, more than twice the shortest stretch between
 *  two. That holds on an evenly spaced axis, where the stretches between
 *  round values are equal and an end's is shorter, and on Gaussian
 *  latitudes, which are nearly even. On an uneven axis, such as ocean depths
 *  close together near the surface and far apart at depth, round values
 *  evenly spaced in depth crowd the deep end and leave the rest bare, so the
 *  rail labels evenly spaced points with their own values instead, as it does
 *  for a time's dates, a pressure's levels, and the index of an axis whose
 *  coordinates are missing or do not match the rail.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself but the helpers injected beside it (it calls
 *  {@link coordinateTicks}, {@link axisTickIndices} and
 *  {@link formatAxisValue}). */
export function railTicks(
  axis: { coordinates: number[] | null; units: string | null; vertical?: { logPressure: boolean } | null } | null | undefined,
  length: number,
  count: number,
): { index: number; text: string }[] {
  const values = axis && axis.coordinates;
  const nice = axis && values && values.length === length ? coordinateTicks(axis, count) : null;
  if (nice) {
    const ticks = nice.map((t) => ({ index: t.index, text: t.text })).sort((a, b) => a.index - b.index);
    const between: number[] = [];
    ticks.reduce((previous, t) => {
      between.push(t.index - previous.index);
      return t;
    });
    const ends = [ticks[0].index, length - 1 - ticks[ticks.length - 1].index];
    if (Math.max(...between, ...ends) <= 2 * Math.min(...between)) return ticks;
  }
  return axisTickIndices(length, count).map((i) => ({
    index: i,
    // `i` is an index axisTickIndices made, below `length`.
    // eslint-disable-next-line security/detect-object-injection
    text: axis && values && i < values.length ? formatAxisValue(values[i], axis.units) : String(i),
  }));
}

/** Where an axis's tick labels go so that none overlaps another or leaves the
 *  axis's own label area (#1043). One rule for the line plot's two axes and
 *  the cross-section's two rails.
 *
 *  Each label has a `centre`, where it belongs along the axis, and a `size`,
 *  its length the same way; the area runs from `lo` to `hi`. Each is centred
 *  on its tick, then slid inward as far as it must be to stay inside the area,
 *  which is what keeps an end label off the corner, the colour bar and the
 *  note above the image. The first and last are placed first, then the rest
 *  in order, and a label that would come within `gap` of one already placed
 *  is dropped rather than drawn over it. Returns each label's start, in the
 *  order given, or `null` for one dropped.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function layoutAxisLabels(
  labels: ReadonlyArray<{ centre: number; size: number }>,
  lo: number,
  hi: number,
  gap: number,
): Array<number | null> {
  const placed = labels.map((l) => ({ centre: l.centre, size: l.size, start: null as number | null }));
  // The ends first, then the rest in order.
  const order = placed.length > 1 ? [placed[0], placed[placed.length - 1]].concat(placed.slice(1, -1)) : placed.slice();
  const kept: Array<[number, number]> = [];
  for (const label of order) {
    const { centre, size } = label;
    if (!(size >= 0) || !Number.isFinite(centre) || size > hi - lo) continue;
    const start = Math.min(Math.max(centre - size / 2, lo), hi - size);
    const end = start + size;
    if (kept.some(([a, b]) => start < b + gap && end > a - gap)) continue;
    kept.push([start, end]);
    label.start = start;
  }
  return placed.map((label) => label.start);
}

/** Whether a cross-section draws its rows bottom-up to put a vertical Y axis
 *  the right way up (#1043): the panel's default, which Flip Y axis then
 *  inverts.
 *
 *  Rows are drawn in storage order, first at the top. CF's `positive` (§4.3)
 *  says which way the axis's values increase: `down` for a pressure or a depth,
 *  so the values should grow down the page, which puts 1000 hPa at the bottom
 *  and a depth of 0 m at the top; `up` for a height, so they should grow up
 *  it. The rows are turned over when storage order says otherwise. An axis
 *  that is not vertical, a Z axis nothing orients (`positive` null), or one
 *  with no coordinates to read the order from, stays in storage order.
 *
 *  Serialized into the panel script, so it must not reference anything
 *  outside itself. */
export function verticalAxisFlips(
  axis: { coordinates: number[] | null; vertical: { positive: string | null } | null } | null | undefined,
): boolean {
  const values = axis && axis.coordinates;
  if (!axis || !axis.vertical || !values || values.length < 2) return false;
  if (axis.vertical.positive !== "up" && axis.vertical.positive !== "down") return false;
  const growsDownThePage = values[values.length - 1] > values[0];
  return growsDownThePage !== (axis.vertical.positive === "down");
}

/** The value range in a line plot's caption, `"−72.8 to 31.5"` (#1037).
 *
 *  Written with "to" and a true minus sign (U+2212): an en dash between two
 *  signed numbers, `-72.769–31.484`, reads as a range from −72.769 to
 *  −31.484. Both ends take the same number of decimals, enough for four
 *  significant figures of the span, less any trailing zeros both share, and
 *  more where that would print two different ends alike. A flat line is its
 *  one value, to seven significant figures, so `101325` reads as itself. A
 *  value of a billion or more, or below 0.0001, is written in exponent form,
 *  with the same rule for keeping the ends apart.
 *
 *  Serialized into the panel script, so it may use only typesetMinus, which
 *  is serialized beside it. */
export function composeLineRange(min: number, max: number): string {
  // A value that rounds to zero is not negative, as on the axes' ticks.
  const signed = typesetMinus;
  if (min === max) return signed(String(Number(min.toPrecision(7))));
  const largest = Math.max(Math.abs(min), Math.abs(max));
  if (!(largest < 1e9) || largest < 1e-4) {
    let digits = 4;
    while (digits < 17 && min.toPrecision(digits) === max.toPrecision(digits)) digits++;
    return signed(min.toPrecision(digits)) + " to " + signed(max.toPrecision(digits));
  }
  const span = Math.abs(max - min);
  let decimals = Math.min(15, Math.max(0, 3 - Math.floor(Math.log10(span))));
  const write = (v: number) => signed(v.toFixed(decimals));
  while (decimals < 15 && write(min) === write(max)) decimals++;
  // Zeros both ends close on say nothing: `101325 to 101330`, not
  // `101325.000 to 101330.000`.
  while (decimals > 0 && /0$/.test(min.toFixed(decimals)) && /0$/.test(max.toFixed(decimals))) decimals--;
  return write(min) + " to " + write(max);
}

/** The frame an animation steps to from `index` along an axis of `length`
 *  steps, moving by `step` (±1): the next index, wrapped to the other end when
 *  `loop` is set, or `null` when an unlooped animation has run off the end.
 *
 *  Serialized into the panel script (`nextFrame.toString()`), so it must not
 *  reference anything outside itself. */
export function nextFrame(index: number, length: number, step: number, loop: boolean): number | null {
  if (length <= 1) return null;
  const next = index + step;
  if (next >= 0 && next < length) return next;
  if (!loop) return null;
  return ((next % length) + length) % length;
}

/** The Animate row's transport icons (#1042), drawn on one 16 × 16 grid so
 *  they come out the same size and centred whatever font the panel has. Every
 *  shape spans y 3 to 13 and each icon is centred on x = 8; forward and last
 *  are back and first mirrored about the middle. One `d` per shape. */
export const TRANSPORT_ICONS = {
  first: ["M1.5 3h2v10h-2z", "M9.5 3 4.5 8 9.5 13z", "M14.5 3 9.5 8 14.5 13z"],
  back: ["M3 3h2v10H3z", "M13 3 6 8 13 13z"],
  play: ["M4 3 12 8 4 13z"],
  pause: ["M4 3h3v10H4z", "M9 3h3v10H9z"],
  forward: ["M3 3 10 8 3 13z", "M11 3h2v10h-2z"],
  last: ["M1.5 3 6.5 8 1.5 13z", "M6.5 3 11.5 8 6.5 13z", "M12.5 3h2v10h-2z"],
} as const;

/** The name of one transport icon. */
export type TransportIcon = keyof typeof TRANSPORT_ICONS;

/** One transport icon as inline SVG, in the button's text colour. It is
 *  decoration: the button's `aria-label` and `title` name it. */
export function transportIconSvg(name: TransportIcon): string {
  // `name` is one of the record's own keys, checked by its type.
  // eslint-disable-next-line security/detect-object-injection
  const shapes = TRANSPORT_ICONS[name].map((d) => `<path d="${d}"/>`).join("");
  return (
    `<svg class="transport-icon icon-${name}" viewBox="0 0 16 16" fill="currentColor" ` +
    `aria-hidden="true" focusable="false">${shapes}</svg>`
  );
}

/** Which axis a slice panel offers to animate along first: the variable's
 *  detected time axis when it can be stepped, otherwise the first other axis
 *  that can, or `null` when none can. `dims` are the variable's axis lengths;
 *  the two image axes are never offered (#170).
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function defaultAnimationDim(
  dims: number[],
  yDim: number,
  xDim: number,
  timeDim: number | null | undefined,
): number | null {
  const steppable = dims.map((len, i) => i !== yDim && i !== xDim && len > 1);
  if (steppable.some((ok, i) => ok && i === timeDim)) return timeDim ?? null;
  const first = steppable.indexOf(true);
  return first < 0 ? null : first;
}

/** A geographic window in degrees: what the lat/lon targets render (#245). */
export interface MapWindow {
  latMin: number;
  latMax: number;
  lonMin: number;
  lonMax: number;
}

/** Zoom `window` by `factor` about the point `(fx, fy)` of the image, so the
 *  place under the pointer stays under it.
 *
 *  `fx` and `fy` run 0 → 1 from the west and north edges. `factor` scales the
 *  spans: below 1 zooms in, above 1 out. On Web Mercator (`mercator`) the
 *  vertical axis is linear in Mercator Y, as the image rows are, so the anchor
 *  holds there too; on the equirectangular target it is linear in latitude.
 *
 *  The spans stay between a hundredth of a degree and the whole world (360° of
 *  longitude; 180° of latitude, or Mercator's ±85.0511° band), and a window that
 *  would run past a pole is slid back rather than squashed. Longitude is not
 *  clamped: a window may cross the antimeridian, which the renderer accepts.
 *
 *  Serialized into the panel script (`zoomMapWindow.toString()`), so it must not
 *  reference anything outside itself. */
export function zoomMapWindow(
  window: MapWindow,
  mercator: boolean,
  fx: number,
  fy: number,
  factor: number,
): MapWindow {
  const MIN_SPAN = 0.01;
  const MERCATOR_MAX_LAT = 85.05112877980659;
  const toY = (lat: number) =>
    mercator ? Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)) : lat;
  const toLat = (y: number) =>
    mercator ? ((2 * Math.atan(Math.exp(y)) - Math.PI / 2) * 180) / Math.PI : y;
  const limit = mercator ? MERCATOR_MAX_LAT : 90;

  const lonSpan = Math.min(360, Math.max(MIN_SPAN, (window.lonMax - window.lonMin) * factor));
  const lonAnchor = window.lonMin + fx * (window.lonMax - window.lonMin);
  let lonMin = lonAnchor - fx * lonSpan;
  lonMin = ((((lonMin + 180) % 360) + 360) % 360) - 180;

  const yTop = toY(window.latMax);
  const yBottom = toY(window.latMin);
  const yLimit = toY(limit);
  const minYSpan = toY(Math.min(limit, MIN_SPAN / 2)) - toY(-Math.min(limit, MIN_SPAN / 2));
  const ySpan = Math.min(2 * yLimit, Math.max(minYSpan, (yTop - yBottom) * factor));
  const yAnchor = yTop - fy * (yTop - yBottom);
  let top = yAnchor + fy * ySpan;
  top = Math.min(yLimit, Math.max(-yLimit + ySpan, top));

  return {
    latMin: toLat(top - ySpan),
    latMax: toLat(top),
    lonMin,
    lonMax: lonMin + lonSpan,
  };
}

/** Pan `window` by a drag of `(dfx, dfy)`, as fractions of the image's width
 *  and height, positive right and down: the map follows the pointer, so the
 *  window moves the other way.
 *
 *  The spans are kept. Latitude is slid back inside the world (Mercator's band
 *  on `mercator`) rather than past a pole; longitude runs freely across the
 *  antimeridian, and the west edge is kept within [-180, 180) so repeated laps
 *  do not grow the numbers.
 *
 *  Serialized into the panel script, so it must not reference anything outside
 *  itself. */
export function panMapWindow(
  window: MapWindow,
  mercator: boolean,
  dfx: number,
  dfy: number,
): MapWindow {
  const MERCATOR_MAX_LAT = 85.05112877980659;
  const toY = (lat: number) =>
    mercator ? Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)) : lat;
  const toLat = (y: number) =>
    mercator ? ((2 * Math.atan(Math.exp(y)) - Math.PI / 2) * 180) / Math.PI : y;
  const yLimit = toY(mercator ? MERCATOR_MAX_LAT : 90);

  const lonSpan = window.lonMax - window.lonMin;
  let lonMin = window.lonMin - dfx * lonSpan;
  lonMin = ((((lonMin + 180) % 360) + 360) % 360) - 180;

  const yTop = toY(window.latMax);
  const ySpan = yTop - toY(window.latMin);
  let top = yTop + dfy * ySpan;
  top = Math.min(yLimit, Math.max(-yLimit + ySpan, top));

  return {
    latMin: toLat(top - ySpan),
    latMax: toLat(top),
    lonMin,
    lonMax: lonMin + lonSpan,
  };
}

/** What a panel draws: a GRIB message, or a variable of a NetCDF file or Zarr
 *  store. "Message" is a GRIB word, so only a GRIB panel uses it (#1038). */
export type FieldKind = "message" | "variable";

/** The file or store a panel draws from (#1038): its kind of field, and its
 *  name as the user sees it in the explorer (`gfs.grib2`, `air_v2.zarr`). */
export interface PanelSource {
  kind: FieldKind;
  name: string;
}

/** The file extensions Fieldglass opens by, which a default export name drops
 *  from the source's name. Anything else stays: a GRIB file named
 *  `gfs.t00z.pgrb2.0p25.f000` keeps its forecast hour. */
const SOURCE_EXTENSIONS = /\.(grb|grib|grib1|grb1|grb2|grib2|nc|nc4|netcdf|zarr)$/i;

/** A source's name without the extension Fieldglass opened it by. */
export function sourceStem(name: string): string {
  return name.replace(SOURCE_EXTENSIONS, "");
}

/** One part of an export name: anything outside `[A-Za-z0-9._-]` becomes `-`,
 *  so a space, a `/` or a `:` inside a name stays inside its part, then the
 *  part is trimmed and lowercased. A `/` used to cut the name at it, the
 *  source's stem with it ("Covariance of geopotential/temperature"). */
function exportNamePart(s: string | null | undefined): string {
  return (s ?? "")
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^[-._]+|[-._]+$/g, "")
    .toLowerCase();
}

/** Longest a slice part of an export name gets, its units and then its
 *  dimension's name shortened to fit; its value is never cut. A time whose
 *  reference cannot be read keeps its units (`1963872 hours since …`). */
const MAX_SLICE_NAME_PART = 40;

/** The parameter, then the stem, is shortened to fit the cap, but neither
 *  below this. */
const MIN_CUT_NAME_PART = 16;

/** A slice part's dimension name is shortened, but not below this. */
const MIN_CUT_DIM_NAME = 3;

/** One held dimension of an export name, in three pieces: the dimension's
 *  name and the coordinate's units, which may be shortened, and the value or
 *  `index-N` between them, which tells two slices apart and is never cut. */
interface SliceNamePieces {
  name: string;
  value: string;
  units: string;
}

/** One held dimension as an export name states it (#1112): the subtitle's
 *  words for it (see {@link composeSliceNote}) as filename pieces,
 *  `time-2024-01-15`, `level-500-millibar`, `phony_dim_0-index-3`. A time of
 *  day loses its colon, `2020-01-01-0600`, and a minus sign reads `minus`
 *  rather than becoming a separator and losing the sign. */
function sliceNamePieces(p: SliceNotePart): SliceNamePieces {
  if (p.coordinate === null) return { name: exportNamePart(p.name), value: `index-${p.index}`, units: "" };
  const text = p.coordinate
    .replace(/(\d{4}-\d\d-\d\d) (\d\d):(\d\d)/, "$1-$2$3")
    // formatCoordinate writes a number's minus as U+2212; a date's hyphens
    // are ASCII.
    .replace(/−/g, "minus")
    .trim();
  // formatCoordinate writes the value, then a space and its units.
  const space = text.search(/\s/);
  return {
    name: exportNamePart(p.name),
    value: exportNamePart(space < 0 ? text : text.slice(0, space)),
    units: space < 0 ? "" : exportNamePart(text.slice(space)),
  };
}

const slicePieceText = (q: SliceNamePieces) => [q.name, q.value, q.units].filter((s) => !!s).join("-");

/** `s` shortened by `over` characters, but not below `floor`, with no
 *  separator left dangling. */
function cutNamePart(s: string, over: number, floor: number): string {
  if (over <= 0 || s.length <= floor) return s;
  return s.slice(0, Math.max(floor, s.length - over)).replace(/[-._]+$/, "");
}

/** Shorten `pieces` until `length()` is at most `limit`: every units first,
 *  then every name, never a value. */
function fitSlicePieces(pieces: SliceNamePieces[], length: () => number, limit: number): void {
  for (const q of pieces) q.units = cutNamePart(q.units, length() - limit, 0);
  for (const q of pieces) q.name = cutNamePart(q.name, length() - limit, MIN_CUT_DIM_NAME);
}

/** An export's default filename, one rule for the PNG and the CSV (#243,
 *  #1038, #1039, #1112): the source's stem, the parameter, then what tells two
 *  exports of one source apart, joined by `_`:
 *
 *  - a GRIB message's index, `gfs_temperature_message-4.png`;
 *  - each dimension a slice holds, by its coordinate,
 *    `ncep_air_daily_2024_air_time-2024-01-15.csv`, or by its index where the
 *    axis has no coordinates, `k_k_phony_dim_0-index-3.png`;
 *  - a variable's index, when its name keeps no letter or digit.
 *
 *  Inside a part a space or a symbol becomes `-`. The name is capped at
 *  `MAX_EXPORT_NAME_STEM` characters by shortening the parameter, then the
 *  stem, then each slice part's units and dimension name. The message index
 *  and the slice values are not cut, so two messages with one long name, or
 *  two ensemble members of a 5-D variable, still get two names; only a name
 *  whose values alone pass the cap reaches `sanitizeExportName`'s last-resort
 *  cut. The provider sanitises it again before writing. */
export function composeExportName(
  meta: Pick<PanelField, "index" | "parameter">,
  source: PanelSource,
  ext: ExportExtension,
  slice: ReadonlyArray<SliceNotePart> = [],
): string {
  // A name with no ASCII letter or digit leaves nothing once cleaned, and two
  // such variables would share one name; the variable's index tells them
  // apart. A GRIB name already carries its message index.
  const cleaned = exportNamePart(meta.parameter);
  const named = /[a-z0-9]/.test(cleaned);
  let parameter = named ? cleaned : "";
  let stem = exportNamePart(sourceStem(source.name));
  const index = source.kind === "message" ? `message-${meta.index}` : named ? "" : String(meta.index);
  const pieces = slice.map(sliceNamePieces);
  // Each slice part fits its own cap first, by its units and then its name.
  for (const q of pieces) fitSlicePieces([q], () => slicePieceText(q).length, MAX_SLICE_NAME_PART);
  const join = () => [stem, parameter, index, ...pieces.map(slicePieceText)].filter((s) => !!s).join("_");
  const over = () => join().length - MAX_EXPORT_NAME_STEM;
  // Then the whole name fits the cap: the parameter, the stem, then the slice
  // parts' units and names. The message index and every value are kept, so
  // what tells two exports apart is never what is cut.
  parameter = cutNamePart(parameter, over(), MIN_CUT_NAME_PART);
  stem = cutNamePart(stem, over(), MIN_CUT_NAME_PART);
  fitSlicePieces(pieces, () => join().length, MAX_EXPORT_NAME_STEM);
  return sanitizeExportName(`${join()}.${ext}`, ext);
}

/** {@link composeExportName} for a PNG, which every render sends for the
 *  reason {@link composeTitleLine} is. */
export function composeDefaultPngName(
  meta: Pick<PanelField, "index" | "parameter">,
  source: PanelSource,
  slice: ReadonlyArray<SliceNotePart> = [],
): string {
  return composeExportName(meta, source, "png", slice);
}

/** The panel's heading: which message or variable is drawn, and in what
 *  units. A GRIB panel reads `Message 0 — Temperature (K)`; a NetCDF or Zarr
 *  panel names its variable alone, `air (degK)`, since "message" is a GRIB
 *  word (#1038). The file or store is named on the line under it.
 *
 *  Exported because the heading has to be rebuilt whenever the drawn field
 *  changes — a NetCDF panel keeps one webview and switches variables inside it
 *  — so `buildGridReadyMessage` sends a fresh one with every render. Composing
 *  it in one place keeps the live update from drifting out of step with the
 *  HTML the panel was first built with. */
export function composeTitleLine(
  meta: Pick<PanelField, "index" | "parameter" | "units">,
  kind: FieldKind,
): string {
  const shownUnits = displayUnits(meta.units);
  const units = shownUnits ? ` (${shownUnits})` : "";
  if (kind === "variable") return (meta.parameter || `Variable ${meta.index}`) + units;
  return `Message ${meta.index}` + (meta.parameter ? ` — ${meta.parameter}` : "") + units;
}

/** The ensemble member a GRIB message is, as a message list and the panel's
 *  heading name it (`member 7`), or `null` for a message that is no member.
 *  Two members of one field can differ in nothing else (#1395). The number is
 *  Rust's (`MessageInfo.ensembleMember`); the check is nullish, as napi sends
 *  `None` as `undefined`. */
export function memberLabel(m: Pick<MessageInfo, "ensembleMember">): string | null {
  return m.ensembleMember != null ? `member ${m.ensembleMember}` : null;
}

/** Longest a part of a tab title may run, in code points, so a hostile name
 *  can't fill it. */
const MAX_TAB_PART = 64;

/** What a tab title drops from a name, as code point ranges: control
 *  characters, the line and paragraph separators, and the bidi embeddings,
 *  overrides and isolates, which could make `a · b.nc` read backwards. Kept
 *  as numbers so the source holds none of the characters it names. */
const TAB_TITLE_STRIP: ReadonlyArray<readonly [number, number]> = [
  [0x0000, 0x001f],
  [0x007f, 0x007f],
  [0x2028, 0x2029],
  [0x202a, 0x202e],
  [0x2066, 0x2069],
];

/** One name from the file made safe for a tab title: the `TAB_TITLE_STRIP`
 *  characters dropped, then capped at `MAX_TAB_PART` code points. */
export function cleanTitlePart(s: string | null | undefined): string {
  return Array.from(s ?? "")
    .filter((ch) => {
      const c = ch.codePointAt(0) ?? 0;
      return !TAB_TITLE_STRIP.some(([lo, hi]) => c >= lo && c <= hi);
    })
    .slice(0, MAX_TAB_PART)
    .join("");
}

/** A render panel's tab title (#1038): the field, then the file or store it
 *  comes from, `msg 0 TMP · gfs.grib2` or `air · air_v2.zarr`. The field goes
 *  first because VS Code cuts a narrow tab's title at its end. Both parts come
 *  from the file (a decoded name, a file name), so each goes through
 *  {@link cleanTitlePart}, which caps by code point so a character outside
 *  the BMP is never cut in half. VS Code shows a title as plain text, so there is
 *  nothing to escape. */
export function composeTabTitle(field: string, sourceName: string): string {
  return [cleanTitlePart(field), cleanTitlePart(sourceName)].filter((s) => !!s).join(" · ");
}

/** The band-limit note for a spectral map drawn below the truncation its
 *  message declares (#637): `"shown at T359 of T7999"`, or `null` when the map
 *  carries every wavenumber the file holds. Goes in the subtitle, which the PNG
 *  export also draws, so the note travels with the picture. The numbers are
 *  Rust's: `MessageInfo.truncation` for the panel a field opens on, and the
 *  rendered map's own `RenderedGrid.truncation` for every render after. */
export function composeTruncationNote(meta: Pick<PanelField, "truncation">): string | null {
  const t = meta.truncation;
  return t != null ? `shown at T${t.truncatedTo} of T${t.declared}` : null;
}

/** The panel subtitle: the field's level, times and forecast (`base`), then
 *  the band-limit note of the map on screen. The note moves with every render,
 *  since a Compare map takes the label of whichever operand is band-limited
 *  (#814), while the base stays field A's. The PNG export draws the same line.
 *  Serialized into the panel script (`composeSubtitle.toString()`), so it must
 *  not reference anything outside itself. */
export function composeSubtitle(base: string, truncationNote: string | null): string {
  return [base, truncationNote].filter((s) => !!s).join(" · ");
}

/** The value part of the point-probe readout (#172): the value under the
 *  cursor, and for a band-limited spectral map the full-detail value at the
 *  same cell beside it (#637), `"325.60 K shown · 318.40 K at full detail
 *  (T7999)"`. Both numbers are Rust's (`ProbeResult.value`,
 *  `fullDetailValue`). Serialized into the panel script
 *  (`composeProbeValue.toString()`), so it must not reference anything outside
 *  itself. */
export function composeProbeValue(
  r: Pick<ProbeResult, "value" | "fullDetailValue" | "fullDetailTruncation">,
  units: string,
): string {
  const shownUnits = displayUnits(units);
  const format = (v: number) => Number(v).toPrecision(5) + (shownUnits ? " " + shownUnits : "");
  const shown = r.value == null ? "no data" : format(r.value);
  if (r.fullDetailValue == null) return shown;
  const truncation = r.fullDetailTruncation != null ? " (T" + r.fullDetailTruncation + ")" : "";
  return shown + " shown · " + format(r.fullDetailValue) + " at full detail" + truncation;
}

/** The status line's range part: the range the field was painted over, or
 *  `"no values"` when Rust reported none (`RenderedGrid.usedMin` is `null`
 *  when every cell is masked, #871). Serialized into the panel script
 *  (`composeRangeText.toString()`), so it must not reference anything outside
 *  itself. */
export function composeRangeText(usedMin: number | null, usedMax: number | null): string {
  if (usedMin == null || usedMax == null) return "no values";
  return "range " + usedMin.toPrecision(4) + " … " + usedMax.toPrecision(4);
}

/** The arrow key's label: the speed a full-length arrow stands for, with its
 *  units, or nothing when no arrow was drawn (`ProjectedVectors.referenceSpeed`
 *  is `null`, #871). The key itself is an arrow drawn on the map at that
 *  length (#1046); the Vectors row repeats the label in words. Serialized into
 *  the panel script like `composeRangeText`. */
export function composeVectorScale(referenceSpeed: number | null | undefined, units: string): string {
  if (referenceSpeed == null) return "";
  const shown = displayUnits(units);
  return Number(referenceSpeed).toPrecision(3) + (shown ? " " + shown : "");
}

/** The Vectors row's text for the key: the label, said in words, for a reader
 *  who does not see the map. Serialized into the panel script. */
export function composeVectorScaleNote(label: string): string {
  return label ? "Full-length arrow: " + label : "";
}

/** What the panel says when the map is too small for the arrow key and its
 *  label (#1168): in the Vectors row for the map on screen, and after the
 *  export result for a PNG. Serialized into the panel script. */
export const VECTOR_KEY_LEFT_OFF = "No arrow key: the map is too small to fit one.";

/** What the panel says instead of the arrow key when the map is drawn
 *  stretched, so one raster pixel is not square on screen (#1251): the key is
 *  scaled by the horizontal axis only, so its length would be wrong for every
 *  arrow not pointing east or west. Serialized into the panel script. */
export const VECTOR_KEY_STRETCHED = "No arrow key: the map is stretched, so an arrow's length does not stand for one speed.";

/** The Vectors row's note when the arrows' two messages are not a u/v pair,
 *  or are one in the wrong order (#1049). `pairing` is the addon's
 *  `ProjectedVectors.pairing`, which Rust reads from the parameter tables;
 *  `u` is this panel's message and `v` the V component pick. The arrows are
 *  drawn either way. `"unknown"`, a pair whose parameters no table names, gets
 *  no note: there is nothing to say either way (#1142). Serialized into the
 *  panel script. */
export function composeVectorPairingNote(pairing: string | null | undefined, u: string, v: string): string {
  if (pairing === "not_a_pair") {
    return "These messages aren't a u/v pair (" + u + ", " + v + "). The arrows treat them as the u and v components.";
  }
  if (pairing === "swapped") {
    return (
      "These messages are a u/v pair the wrong way round: " + u + " is the v component and " + v +
      " the u. For arrows that point the right way, open " + v + " and pick " + u + " as V."
    );
  }
  return "";
}

export function renderImagePanelHtml(
  webview: vscode.Webview,
  meta: PanelField,
  projectionSummary: string,
  colormaps: PickerColormap[],
  combineOps: CombineOpInfo[],
  slice?: SlicePanelData,
  compareFields?: CompareFieldOption[],
  pickMessages?: CompareFieldOption[],
  // The file or store's name, which the line under the heading opens with
  // (#1038). A slice panel draws a variable; any other draws a GRIB message.
  sourceName = "",
  // The number the page sends back on every message, so the provider can tell
  // it from a page it has since replaced (#1202). See `PanelHandleGate`.
  page = 0,
): string {
  const source: PanelSource = { kind: slice ? "variable" : "message", name: sourceName };
  const cspNonce = nonce();
  const csp = [
    `default-src 'none'`,
    `script-src 'nonce-${cspNonce}'`,
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `img-src ${webview.cspSource} blob: data:`,
  ].join("; ");
  // A panel written while it waits for a message (#959) or a variable (#1230)
  // to be picked names none, in the heading and in what the PNG export would
  // draw.
  const picking = !!pickMessages || slice?.picking === true;
  const titleLine = pickMessages
    ? NO_MESSAGE_HEADING
    : slice?.picking === true
      ? NO_VARIABLE_HEADING
      : composeTitleLine(meta, source.kind);
  // `level` is the bare value ("300", "100 – 85"); `levelType` carries the
  // unit and surface name ("(hPa) Isobaric level", "Cloud base level").
  // Together they read naturally as "300 (hPa) Isobaric level". For surface
  // types whose value is meaningless `level` is `null` (#775) and only the
  // levelType is informative.
  // Some level types have no meaningful value, and the decoder then reports the
  // surface name in *both* fields (a spectral message reads level = levelType =
  // "Ground or water surface"). Joining them blind printed it twice, in the
  // panel header and the exported PNG alike.
  const levelDescription = meta.level && meta.level !== meta.levelType
    ? [meta.level, meta.levelType].filter((s) => !!s).join(" ")
    : meta.levelType;
  // The source first, in every format and while a message is being picked:
  // with several panels open on twins of the same data, it is what tells
  // them apart (#1038).
  const subBase = [source.name, levelDescription, meta.referenceTime, meta.forecast, memberLabel(meta)]
    .filter((s) => !!s).join(" · ");
  const subLine = picking ? source.name : composeSubtitle(subBase, composeTruncationNote(meta));
  const defaultPngName = composeDefaultPngName(meta, source);
  // Whether the overlays are offered for the field the panel is written for.
  // A slice panel then follows each slice it draws (#840).
  const placed = offersOverlays(meta.placement);
  // Which point operations the field takes, Rust's answer (#945). A slice
  // panel follows each slice it draws for these too.
  const supports: GridSupport = meta.supports ?? NO_SUPPORT;
  // The controls the field's grid can't use, which are hidden, and why
  // (#1036). A slice panel follows each slice it draws for these too. A GRIB
  // panel waiting for a message to be picked has no grid to speak for.
  const unavailable: UnavailableGroup[] = picking
    ? []
    : unavailableControls(
      {
        reprojectable: meta.reprojectable,
        placement: meta.placement,
        label: meta.grid ? meta.grid.label : null,
        supports,
      },
      // Every panel has Export CSV (#1039), so every panel can list its long
      // layout as unavailable.
      { arrows: !slice && (compareFields ?? []).length >= 2, csv: true },
    );
  const hides = (key: UnavailableKey) => unavailable.some((g) => g.controls.some((c) => c.key === key));

  const script = `
    <script nonce="${cspNonce}">
      (function () {
        // Every message carries the page's number. A page written again over a
        // changed file has a new one, and what the old page sent is dropped,
        // its \`ready\` included, however late it arrives (#1202).
        const api = acquireVsCodeApi();
        const PAGE = ${JSON.stringify(page)};
        const vscode = {
          postMessage: (m) => api.postMessage(Object.assign({}, m, { page: PAGE })),
          getState: () => api.getState(),
          setState: (state) => api.setState(state),
        };

        // The most-recently-received rendered grid. We only re-render
        // when the user changes a picker that affects the Rust output
        // (projection / resampling / flip-y / range). The cached payload
        // lets us redraw after a tab hide/show without a round-trip.
        let lastPayload = null;
        // Whether setDisplayAspect held the image to a shape other than the
        // raster's, so its pixels are not square on screen (#1321).
        let displayHeld = false;
        // Whether the provider has answered the mount's own render, with an
        // image or an error. Until it has, a control change waits for that
        // answer rather than asking again. After an error there is still no
        // payload, and keying on one left a panel whose first render failed
        // unable to ask for another (#839).
        let mountAnswered = false;
        // The map target the user chose, held while the picker is put away and
        // offers only the source view: the panel lost its handle (#839), the
        // axes are a cross-section (#171), or the slice can't be reprojected
        // (#822). The snapshot saves it, the render asks for it once the axes
        // are a map again, and the picker shows it again on the first answer
        // that offers the map targets (#1099). Like any hidden control, the
        // picker keeps the user's choice for the next field that takes it.
        let heldProjection = null;
        // Set while the picker waits for a variable to replace one gone from
        // the file; a render asked for meanwhile would draw a variable nobody
        // chose (#839).
        let awaitingPick = false;
        // Set while the heading says nothing is drawn (headNothingDrawn): a
        // pick is awaited, or the file cannot be read. Compare and the arrows
        // are put away then too, whatever the pickers hold (#1388): picking a
        // variable ends the wait, but not a missing file.
        let nothingDrawn = ${JSON.stringify(picking)};

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
        // The field's units, for the point-probe readout (#172). Not const:
        // a NetCDF panel switches variables inside one webview, and the units
        // move with the variable, so gridReady refreshes this. It used to be
        // frozen at the variable the panel opened on, which put that
        // variable's unit against another variable's numbers.
        let UNITS = ${scriptString(meta.units || "")};
        const VECTOR_KEY_LEFT_OFF = ${scriptString(VECTOR_KEY_LEFT_OFF)};
        const VECTOR_KEY_STRETCHED = ${scriptString(VECTOR_KEY_STRETCHED)};
        const MAP_MOST_ASPECT = ${MAP_MOST_ASPECT};
        // The thinnest stroke an exported PNG draws, in PNG pixels (#1366).
        const EXPORT_MIN_STROKE = 1.5;
        // Title / subtitle / default filename for the PNG export (#243).
        // TITLE_LINE tracks the drawn field for the same reason as UNITS.
        let TITLE_LINE = ${scriptString(titleLine)};
        // SUB_LINE's band-limit note follows the map on screen: a Compare map
        // is labelled by whichever operand is band-limited (#814), so every
        // gridReady recomposes it over SUB_BASE.
        const SUB_BASE = ${scriptString(subBase)};
        let SUB_LINE = ${scriptString(subLine)};
        // The file or store, which stays on the line under the heading even
        // while no message is drawn (#1038).
        const SOURCE_NAME = ${scriptString(source.name)};
        ${composeSubtitle.toString()}
        // Tracks the drawn field too: a slice panel's export was named after the
        // variable it opened on (#822).
        let DEFAULT_PNG_NAME = ${scriptString(defaultPngName)};
        // Whether this message's u/v run along the grid's axes (#241). The file
        // says so; the panel passes it on rather than asking the user.
        const UV_GRID_RELATIVE = ${JSON.stringify(meta.uvRelativeToGrid === true)};
        // Injected verbatim so the export sizes itself with the same function
        // the host-side tests pin, instead of a re-implementation that can drift.
        ${exportCanvasWidth.toString()}
        ${zoomMapWindow.toString()}
        ${panMapWindow.toString()}
        ${nextFrame.toString()}
        ${defaultAnimationDim.toString()}
        ${isMapSlice.toString()}
        ${projectionOptionsHtml.toString()}
        ${applyReprojectable.toString()}
        const MAP_PROJECTIONS = ${JSON.stringify(MAP_PROJECTIONS)};
        // Whether the picker offers the map targets. It starts as the answer
        // for the field the panel opened on; a slice panel replaces it with the
        // answer for each slice it draws (#822), since the picker can move onto
        // a variable or an axis pair with a different one.
        let mapsOffered = ${JSON.stringify(meta.reprojectable)};
        // Whether the field on screen is placed on the Earth, which every
        // overlay needs (#840). Like mapsOffered, it starts as the answer for
        // the field the panel was written for, and a slice panel takes each
        // slice render's.
        let mapPlaced = ${JSON.stringify(placed)};
        // Which point operations the field on screen takes (#945): Rust's
        // answer, taken like mapPlaced. Contours, arrows and the zonal average
        // are asked for only where it says so.
        let fieldSupports = ${JSON.stringify(supports)};
        // The controls the field on screen can't use, grouped by reason
        // (#1036): hidden, and named in the "Not available" line. Taken like
        // mapPlaced.
        // A reason can carry the file's grid family: same escaping as sliceJson.
        let unavailable = ${JSON.stringify(unavailable).replace(/</g, "\\u003c")};
        ${unavailableSummary.toString()}
        ${unavailableItemsHtml.toString()}
        ${axisTickIndices.toString()}
        ${displayAspect.toString()}
        ${cfTimeOrigin.toString()}
        ${formatAxisValue.toString()}
        ${displayUnits.toString()}
        ${formatCoordinate.toString()}
        ${composeSliceNote.toString()}
        ${heldSliceParts.toString()}
        ${typesetMinus.toString()}
        ${niceTicks.toString()}
        ${calendarTicks.toString()}
        ${coordinateTicks.toString()}
        ${railTicks.toString()}
        ${layoutAxisLabels.toString()}
        ${verticalAxisFlips.toString()}
        ${composeLineRange.toString()}
        ${composeProbeValue.toString()}
        ${composeRangeText.toString()}
        ${composeVectorScale.toString()}
        ${composeVectorScaleNote.toString()}
        ${composeVectorPairingNote.toString()}
        let sliceState = SLICE ? Object.assign({}, SLICE.initial, {
          sliceIndices: SLICE.initial.sliceIndices.slice(),
        }) : null;
        // Field B's held indices for the compare picker (#239). Starts equal to
        // Field A (a self-difference of zero) until the user steps it.
        let compareIndices = SLICE ? SLICE.initial.sliceIndices.slice() : [];

        function sliceVariable(idx) {
          return SLICE ? SLICE.variables.find((v) => v.index === idx) : undefined;
        }

        // Extra fields merged into a rerender / overlay request: the slice spec
        // when this is a NetCDF panel, nothing otherwise.
        function sliceFields() {
          return sliceState ? { slice: sliceState } : {};
        }

        // The difference-map request rider (#239): when an operation is chosen
        // and a Field B is selected, combine field A (this message) with B. Off
        // (empty operation) or no picker → a plain single-field render. The
        // provider validates the op and index and falls back to single render.
        function compareRequest() {
          const opEl = document.getElementById('compare-op');
          if (!opEl || !opEl.value) return {};
          if (SLICE) {
            // NetCDF: Field B is the same variable at its own slice indices.
            if (!sliceState) return {};
            return { compare: {
              op: opEl.value,
              variableIndexB: sliceState.variableIndex,
              sliceIndicesB: compareIndices.slice(),
            } };
          }
          // GRIB: Field B is another message in the file.
          const bEl = document.getElementById('compare-field-b');
          if (!bEl || bEl.value === '') return {};
          return { compare: { op: opEl.value, messageIndexB: Number(bEl.value) } };
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
          wrap.innerHTML = dimStepperHtml(v, sliceState.yDim, sliceState.xDim, sliceState.sliceIndices);
          requestStepperAxes();
          fillSliceCoordinates();
          buildAnimationControls();
          fillLineAxes();
          syncCrossSectionMode();
        }

        // One slider + number per dimension that isn't an image axis, reading
        // its value from the indices array. Shared by Field A (#slice-dims) and
        // the compare Field B (#compare-dims); a container-delegated listener
        // reads data-dim to drive the right index array, so both use data-dim.
        //
        // Each stepper names the coordinate at its index beside the number
        // (#1041), 'level 5 500 millibar', filled in by fillSliceCoordinates
        // once the axis's values are known; an axis with none shows the index
        // alone. Field B's are named as Field B's, since with the main
        // steppers scrolled away they were taken for the panel's own. A
        // separator stands between two steppers, which ran together as
        // '0–11level'.
        function dimStepperHtml(v, yDim, xDim, indices, fieldB) {
          let html = '';
          for (let i = 0; i < v.dims.length; i++) {
            if (i === yDim || i === xDim) continue;
            const len = v.dims[i].length;
            const max = Math.max(0, len - 1);
            const val = indices[i] || 0;
            // The index stays 0-based: it is the array index the probe
            // readout, the CSV export and the napi slice API all speak, and
            // what ncdump and every NetCDF binding show. Only the hint beside
            // it changes.
            //
            // It used to read '/max', which invites reading as "n of total"
            // and is wrong twice over: a 22-channel dimension showed '/21',
            // and a length-1 dimension showed '/0' — "zero of zero", as though
            // the dimension were empty, when it holds exactly one slice. So
            // the hint states the range you may type instead, and a dimension
            // with nothing to step says so and disables its controls rather
            // than offering a slider that cannot move.
            const single = len <= 1;
            const off = single ? ' disabled' : '';
            const hint = single ? '(only slice)' : '(0\u2013' + max + ')';
            html += '<label class="slice-index"><span class="slice-name">' + (fieldB ? 'Field B \u00b7 ' : '') +
              escapeAttr(v.dims[i].name) + '</span> <input type="range" class="slice-slider" data-dim="' + i + '" min="0" max="' + max +
              '" value="' + val + '"' + off + '>' +
              '<input type="number" class="slice-number" data-dim="' + i + '" min="0" max="' + max +
              '" value="' + val + '"' + off + '> <span class="slice-coord" data-coord-dim="' + i + '"></span>' +
              ' <span class="slice-len">' + hint + '</span></label>';
          }
          return html;
        }

        // The coordinate at index along dimension dim of the variable on
        // the picker, as formatCoordinate writes it: '' while its values are
        // being asked for, null when the axis has none there (no coordinate
        // array, one too short, or a value that is not a number).
        function coordinateAt(dim, index) {
          const axis = axisCache[axisKey(dim)];
          if (axis === undefined) return '';
          return coordinateText(axis, index);
        }

        // An axis's coordinate at index as formatCoordinate writes it, or null.
        function coordinateText(axis, index) {
          const values = axis && axis.coordinates;
          if (!values || !(index < values.length)) return null;
          return formatCoordinate(values[index], axis.units) || null;
        }

        // Ask for the values of every axis a stepper holds, so each can be
        // named by its coordinate (#1041). Each is asked for once per
        // variable and kept, as the cross-section rails' are.
        function requestStepperAxes() {
          const v = sliceState && sliceVariable(sliceState.variableIndex);
          if (!v) return;
          for (let i = 0; i < v.dims.length; i++) {
            if (i !== sliceState.yDim && i !== sliceState.xDim) requestAxis(i);
          }
        }

        // Write each stepper's coordinate beside it, Field A's and Field B's,
        // from the index it holds now: called as an index moves (a slider
        // drag included) and as an axis's values arrive.
        function fillSliceCoordinates() {
          const fill = (container, indices) => {
            document.querySelectorAll(container + ' .slice-coord').forEach((el) => {
              const dim = Number(el.dataset.coordDim);
              const text = coordinateAt(dim, indices[dim] || 0);
              el.textContent = text || '';
            });
          };
          if (!sliceState) return;
          fill('#slice-dims', sliceState.sliceIndices);
          fill('#compare-dims', compareIndices);
        }

        // Field B's index steppers (the compare picker), same variable + image
        // axes as Field A but its own held indices. Rebuilt alongside Field A's
        // whenever the variable or axes change.
        function buildCompareAxes() {
          const wrap = document.getElementById('compare-dims');
          if (!wrap || !sliceState) return;
          const v = sliceVariable(sliceState.variableIndex);
          if (!v) return;
          // A variable with no dimension to hold still has no steppers, and
          // the row still says which field is B.
          wrap.innerHTML = dimStepperHtml(v, sliceState.yDim, sliceState.xDim, compareIndices, true) || 'Field B';
          fillSliceCoordinates();
        }

        function syncCompareIndex(dim, value) {
          const v = sliceState ? sliceVariable(sliceState.variableIndex) : null;
          const max = v ? Math.max(0, v.dims[dim].length - 1) : 0;
          const clamped = Math.min(max, Math.max(0, Math.floor(Number(value) || 0)));
          compareIndices[dim] = clamped;
          document.querySelectorAll('#compare-dims [data-dim="' + dim + '"]').forEach((el) => {
            if (el.value !== String(clamped)) el.value = String(clamped);
          });
          fillSliceCoordinates();
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
          // Keep the paired slider + number input in lockstep. Scoped to Field
          // A's container so it never touches Field B's steppers (which share
          // the same data-dim values).
          document.querySelectorAll('#slice-dims [data-dim="' + dim + '"]').forEach((el) => {
            if (el.value !== String(clamped)) el.value = String(clamped);
          });
          fillSliceCoordinates();
        }

        // --- Cross-sections (#171) -------------------------------------------
        // Any two axes may be the image's. When they are not the file's own
        // latitude/longitude pair, the plane is not a map: nothing places it on
        // the Earth, so the projection picker and the overlays are put away and
        // the axes are labelled from their coordinate arrays instead.
        //
        // The axis values are asked for per (variable, axis) and kept, because
        // they change only when the picker does and reading one must not cost a
        // decode.
        let axisCache = {};
        let axisPending = {};

        // Whether the slice the pickers hold is a cross-section: what the
        // next render is, and what the controls are set up for. The rails
        // ask it of the slice on screen (#1145).
        function isCrossSection() {
          return isCrossSectionOf(sliceState);
        }

        function isCrossSectionOf(spec) {
          if (!spec) return false;
          const v = sliceVariable(spec.variableIndex);
          if (!v) return false;
          return !isMapSlice(spec.yDim, spec.xDim, v.detectedYDim, v.detectedXDim);
        }

        function axisKey(dim) {
          return axisKeyOf(sliceState, dim);
        }

        // An axis of the variable a slice spec is of, as the cache files it.
        function axisKeyOf(spec, dim) {
          return spec ? spec.variableIndex + ':' + dim : '';
        }

        // Ask the host for an axis's coordinates once, then redraw the rails.
        // The axis is of the slice the pickers hold, or of the slice spec
        // given: the rails ask for the axes of the slice on screen (#1145).
        function requestAxis(dim, spec) {
          const of = spec || sliceState;
          const key = axisKeyOf(of, dim);
          if (!key || axisCache[key] !== undefined || axisPending[key]) return;
          axisPending[key] = true;
          vscode.postMessage({ type: 'axisRequest', dim: dim, slice: of });
        }

        function handleAxisResult(msg) {
          // Kept under the variable it was asked for, which the answer names:
          // the picker can have moved on meanwhile, and an answer filed under
          // the variable on it now would label that one with another's
          // values (#1041).
          const key = typeof msg.variableIndex === 'number' ? msg.variableIndex + ':' + msg.dim : axisKey(msg.dim);
          if (key) {
            axisPending[key] = false;
            // With no handle (the document was read again into bytes that did
            // not open; #839, #1338) nothing is known about the axis, so it is
            // asked for again once the file opens, not kept as none.
            if (msg.handleGone !== true) axisCache[key] = msg.result || null;
          }
          // A render held for the Y axis goes now: on its values, or without
          // them, since a panel with no handle answers the render with why.
          if (awaitingYAxis && (msg.handleGone === true || yAxisKnown())) {
            const send = awaitingYAxis;
            awaitingYAxis = null;
            send();
          }
          drawAxisRails();
          fillSliceCoordinates();
          updateFrameLabel();
          // While a pick is awaited nothing is drawn, and the line names only
          // the file (headNothingDrawn): an axis answering then must not bring
          // back the slice of a variable nobody picked (#1230), nor while the
          // file cannot be read (#1388).
          if (!awaitingPick && !nothingDrawn) updateSubtitle();
        }

        // A render held until the Y axis's values arrive (#1043): which way up
        // a cross-section's rows go depends on them, and a render drawn the
        // wrong way up and then redrawn would flash. Only the latest is kept.
        let awaitingYAxis = null;

        function yAxisKnown() {
          return !!sliceState && axisCache[axisKey(sliceState.yDim)] !== undefined;
        }

        function whenYAxisKnown(send) {
          if (!isCrossSection() || yAxisKnown()) {
            awaitingYAxis = null;
            send();
            return;
          }
          awaitingYAxis = send;
          requestAxis(sliceState.yDim);
        }

        // Whether the next render should draw the rows bottom-up: Flip Y axis,
        // turned over once more for a cross-section whose vertical Y axis
        // storage order puts the wrong way up (#1043). So the box stays the
        // user's override of the default, on either orientation. This is what
        // a render asks for, read from the box and the Y axis as they are
        // now; the image on screen can be an older render (see drawnFlipY).
        function rowsFlipped() {
          const box = !!(document.getElementById('flip-y') && document.getElementById('flip-y').checked);
          if (!isCrossSection()) return box;
          return box !== verticalAxisFlips(axisCache[axisKey(sliceState.yDim)]);
        }

        // The options the image on screen was drawn with, as its render
        // echoes them (#1114), or null before the first render. Between a
        // change and the render it asks for, the controls already say the
        // new image while the old one is still shown, so what reads the
        // image on screen reads these.
        function drawnOptions() {
          return (lastPayload && lastPayload.options) || null;
        }

        // Whether the image on screen has its rows bottom-up (#1114). Between
        // a Flip Y toggle or a Y axis answer and the render that follows,
        // rowsFlipped() already says the new way while the old image is
        // still shown, so the rails, the probe and the overlays read this.
        // Before the first render there is no image, and the request is for
        // the one to come.
        function drawnFlipY() {
          const drawn = drawnOptions();
          return drawn && typeof drawn.flipY === 'boolean' ? drawn.flipY : rowsFlipped();
        }

        // The projection of the image on screen (#1114): the provider's own
        // answer, which is the source view when the slice could not take the
        // target asked for. The picker's value until the first render.
        function drawnProjection() {
          const drawn = drawnOptions();
          if (drawn && typeof drawn.projection === 'string') return drawn.projection;
          return (document.getElementById('picker-projection') || {}).value || 'source';
        }

        // The projection the next render asks for: the picker's, or the map
        // target held while a slice waits for its axes (#1099). What a render
        // request carries (currentOptions(true)), and what a gesture's window
        // is written for (#1147).
        function requestedProjection() {
          if (heldProjection !== null && !isCrossSection()) return heldProjection;
          return (document.getElementById('picker-projection') || {}).value || 'source';
        }

        // Place the tick labels down the left of the image and under it. A label
        // sits at the fraction of the image its index does — the centre of its
        // row or column, or between two centres for a round value that falls
        // between two points (#1229) — and reads that value, or the index
        // when the file gives the axis no coordinates. Then each rail's labels
        // are fitted inside it (#1043): an end label is slid inward rather
        // than run into the other rail, the colour bar or the note above the
        // image, and one that would overlap a neighbour is dropped.
        // The labels are placed and bounded by the image, not by the rail: the
        // rail's own box can run a few pixels past the image (#1043, seen in
        // CI, where a 37 px image sat in a 40 px row), and a label inside the
        // rail but past the image's edge runs into the corner or under the
        // note above. A label's fraction is of the image too.
        function fitRail(rail, vertical) {
          const box = rail.getBoundingClientRect();
          const canvas = document.getElementById('canvas');
          const image = canvas ? canvas.getBoundingClientRect() : box;
          const lo = vertical ? image.top - box.top : image.left - box.left;
          const extent = vertical ? image.height : image.width;
          // Not laid out yet: the percentages stand until the ResizeObserver
          // that sees it laid out fits them.
          if (!(extent > 0)) return;
          const spans = Array.from(rail.children);
          const starts = layoutAxisLabels(spans.map((s) => {
            const r = s.getBoundingClientRect();
            return { centre: lo + (Number(s.dataset.at) / 100) * extent, size: vertical ? r.height : r.width };
          }), lo, lo + extent, 4);
          spans.forEach((s, i) => {
            if (starts[i] === null) { s.remove(); return; }
            s.style.transform = 'none';
            if (vertical) s.style.top = starts[i] + 'px'; else s.style.left = starts[i] + 'px';
          });
        }

        // An element's laid-out size as "WxH", whole pixels.
        function sizeKey(box) {
          return Math.round(box.width) + 'x' + Math.round(box.height);
        }

        function drawAxisRails() {
          const railY = document.getElementById('axis-y');
          const railX = document.getElementById('axis-x');
          const caption = document.getElementById('axis-caption');
          if (!railY || !railX || !caption) return;
          // The rails label the image on screen: its slice (#1145) and its
          // orientation (#1114), not the ones a render on its way will have.
          // The pickers can be on another variable or axis pair meanwhile.
          const spec = drawnSpec();
          const cross = isCrossSectionOf(spec);
          railY.toggleAttribute('hidden', !cross);
          railX.toggleAttribute('hidden', !cross);
          caption.toggleAttribute('hidden', !cross);
          if (!cross || !spec || !lastPayload) {
            railY.innerHTML = '';
            railX.innerHTML = '';
            return;
          }
          const v = sliceVariable(spec.variableIndex);
          if (!v) return;
          const flip = drawnFlipY();
          const axisY = axisCache[axisKeyOf(spec, spec.yDim)];
          const axisX = axisCache[axisKeyOf(spec, spec.xDim)];
          const rows = lastPayload.height;
          const cols = lastPayload.width;

          // A rail's ticks (railTicks), each at the row or column its value
          // falls in, fractional between two, counted from the top or the
          // left and in that order. flipped: the renderer draws row 0 first,
          // and a render drawn flipped turns the plane over.
          const ticksOf = (axis, length, count, flipped) => railTicks(axis, length, count)
            .map((t) => ({ pos: flipped ? length - 1 - t.index : t.index, text: t.text }))
            .sort((a, b) => a.pos - b.pos);
          railY.innerHTML = ticksOf(axisY, rows, 6, flip)
            .map((t) => {
              const at = ((t.pos + 0.5) / rows) * 100;
              return '<span data-at="' + at + '" style="top:' + at + '%">' + escapeAttr(t.text) + '</span>';
            })
            .join('');
          railX.innerHTML = ticksOf(axisX, cols, 5, false)
            .map((t) => {
              const at = ((t.pos + 0.5) / cols) * 100;
              return '<span data-at="' + at + '" style="left:' + at + '%">' + escapeAttr(t.text) + '</span>';
            })
            .join('');
          fitRail(railY, true);
          fitRail(railX, false);
          // The image size the rails were last fitted to, so a reader (the
          // test driver) can tell fitted labels from ones still placed for
          // the size before (#1043).
          const wrap = document.getElementById('plot-wrap');
          const image = document.getElementById('canvas');
          if (wrap && image) wrap.dataset.fitted = sizeKey(image.getBoundingClientRect());
          const name = (axis, dim) => {
            const base = axis ? axis.dimension : v.dims[dim].name;
            const shown = axis ? displayUnits(axis.units) : '';
            const units = shown && !/ since /i.test(shown) ? ' (' + shown + ')' : '';
            return base + units;
          };
          caption.textContent = 'x: ' + name(axisX, spec.xDim) + ' · y: ' + name(axisY, spec.yDim);
        }

        // Take a slice render's own answer to whether it can be reprojected
        // (#822); see applyReprojectable. A map target the answer takes off the
        // picker is held, and a held one goes back on once the answer offers
        // it again (#1099). Returns whether the selection moved.
        function applySliceGrid(sliceGrid) {
          const picker = document.getElementById('picker-projection');
          const before = picker ? picker.value : 'source';
          const next = applyReprojectable(picker, sliceGrid, mapsOffered, MAP_PROJECTIONS);
          mapsOffered = next.offered;
          if (next.moved) holdProjection(before);
          return releaseHeldProjection() || next.moved;
        }

        // Hold a map target the picker is about to stop showing (#1099). The
        // first one held stands: what replaces it is the source view, which is
        // not a choice.
        function holdProjection(value) {
          if (heldProjection === null && typeof value === 'string' && value !== 'source') heldProjection = value;
        }

        // Put the held map target back on the picker where the field on screen
        // takes it: the axes are a map and the picker offers it. Set as a
        // value, not a change event, so it is not a new choice and asks for
        // nothing; the render that drew this answer already asked for it
        // (currentOptions). Returns whether the picker moved.
        function releaseHeldProjection() {
          const picker = document.getElementById('picker-projection');
          if (heldProjection === null || !picker || !mapsOffered || isCrossSection()) return false;
          if (!Array.from(picker.options).some((o) => o.value === heldProjection)) return false;
          picker.value = heldProjection;
          heldProjection = null;
          return true;
        }

        // Whether the overlays, contours and arrows apply to the field on
        // screen: it is placed on the Earth (#840), and its axes are a map. An
        // overlay that does not apply is not asked for and not drawn, and its
        // toggle, hidden meanwhile (#1036), keeps its state for the next field
        // that takes it.
        function overlaysOffered() {
          return mapPlaced && !isCrossSection();
        }

        // Contours apply where overlays do and the grid takes them (#945).
        function contoursOffered() {
          return overlaysOffered() && fieldSupports.contours === true;
        }

        // The element each control the grid can't use is hidden by (#1036).
        // The long CSV layout has none: the CSV prompt leaves it out. Compare
        // is never one the grid rules out; it is here to be put away while a
        // pick is awaited (PICK_HIDES).
        const UNAVAILABLE_ELEMENTS = {
          projection: 'projection-control',
          resampling: 'resampling-control',
          overlay: 'overlay-fieldset',
          contours: 'contour-layer',
          arrows: 'vector-fieldset',
          zonal: 'zonal-control',
          compare: 'compare-fieldset',
        };
        // What a cross-section puts away whatever its answer says (#171): its
        // axes are not a map, so there is nothing to project or overlay. Its
        // own note says so at once; the line follows with the slice's answer.
        const CROSS_SECTION_HIDES = ['projection', 'resampling', 'overlay'];
        // What is put away while the panel waits for a message or variable to
        // be picked (#1237), and while the heading says nothing is drawn for
        // another reason, a file that cannot be read (#1388). Nothing is
        // drawn, so a Compare row reading A − B, or Arrows on with a V
        // component, would describe a picture that is not on screen, from
        // pickers that list the old file's fields. Hidden, they
        // keep their values: a GRIB pick finds Field B and the V component
        // again by identity (#1230).
        const PICK_HIDES = ['compare', 'arrows'];

        // Whether the field on screen can't use a control, by its key.
        function isUnavailable(key) {
          return unavailable.some((g) => g.controls.some((c) => c.key === key));
        }

        // Whether a control is put away: the field on screen can't use it, a
        // cross-section has no map for it, a pick is awaited (#1237), or
        // (Resampling alone, #1091) the projection is Source, where nothing is
        // resampled. The last two are not limits of the grid, so they are not
        // in the "Not available" line. Hidden, the control keeps its value.
        function controlHidden(key) {
          if (isUnavailable(key) || (isCrossSection() && CROSS_SECTION_HIDES.includes(key))) return true;
          if ((awaitingPick || nothingDrawn) && PICK_HIDES.includes(key)) return true;
          if (key !== 'resampling') return false;
          const picker = document.getElementById('picker-projection');
          return !picker || picker.value === 'source';
        }

        // Hide every control the field on screen can't use, show the rest,
        // and rewrite the "Not available for this grid" line to match (#1036).
        // A hidden control keeps its value for the next field that takes it;
        // what it asks for is gated on the field's answers (overlaysOffered,
        // contoursOffered, vectorsEnabled, zonalOn), not on the box.
        //
        // The line keeps its own <summary> node: only the summary's text and
        // the list are rewritten, so a line a user opened stays open, and one
        // they are on keeps the keyboard focus, while they step through the
        // slices. Focus inside a control that goes moves to the summary.
        //
        // fileClosed: the last answer came while the panel had no handle,
        // because its document was read again into bytes that did not open
        // (#839, #1338). Nothing is known about the slice then, so the controls
        // stay put away but the line makes no claim about the grid; the status
        // says the file is gone.
        let fileClosed = false;
        function applyUnavailable(groups) {
          unavailable = Array.isArray(groups) ? groups : [];
          const line = document.getElementById('unavailable');
          const summary = line ? line.querySelector('summary') : null;
          let focusLost = false;
          for (const key of Object.keys(UNAVAILABLE_ELEMENTS)) {
            const el = document.getElementById(UNAVAILABLE_ELEMENTS[key]);
            if (!el) continue;
            // A cross-section puts the map's own controls away too.
            const hide = controlHidden(key);
            if (hide && el.contains(document.activeElement)) focusLost = true;
            el.toggleAttribute('hidden', hide);
          }
          if (!line || !summary) return;
          const list = line.querySelector('ul');
          const text = unavailableSummary(unavailable);
          if (summary.textContent !== text) summary.textContent = text;
          const items = unavailableItemsHtml(unavailable);
          if (list && list.innerHTML !== items) list.innerHTML = items;
          // Nor does it speak for a grid while a pick is awaited: nothing is
          // drawn, and a slice panel's answer is then for a variable nobody
          // picked (#1237).
          const shown = unavailable.length > 0 && !fileClosed && !awaitingPick && !nothingDrawn;
          line.toggleAttribute('hidden', !shown);
          if (focusLost && shown) summary.focus();
        }

        // Take a slice render's answer to whether it is placed (#840), which
        // point operations it takes (#945) and which controls it can't use
        // (#1036).
        function applySlicePlacement(placed, supports, groups, handleGone) {
          mapPlaced = placed === true;
          if (supports) fieldSupports = supports;
          fileClosed = handleGone === true;
          applyUnavailable(groups);
          applyContoursOnly();
        }

        // Take a slice panel's answers for the slice picked, which come with a
        // render and with a render's error alike (#839): the picker follows
        // them (#822) and so does the Overlay row (#840). A failed render would
        // otherwise leave both on the previous slice's answer. A GRIB panel's
        // messages carry none.
        //
        // handleGone says the answer is about the panel having no handle (its
        // file was read again into bytes that did not open), not about the slice. The map targets go, but the one
        // selected is held, so a panel restored once the file is open again
        // draws on it (heldProjection), as one that moves onto a slice that
        // can't be reprojected does (#1099).
        function applySliceAnswer(sliceGrid, handleGone) {
          if (!sliceGrid) return;
          if (applySliceGrid(sliceGrid)) {
            syncProjectionControls();
            snapshotState();
          }
          applySlicePlacement(sliceGrid.placed, sliceGrid.supports, sliceGrid.unavailable, handleGone);
        }

        // A cross-section has no projection and no coastlines to draw on it.
        function syncCrossSectionMode() {
          const cross = isCrossSection();
          const note = document.getElementById('cross-section-note');
          const projection = document.getElementById('picker-projection');
          if (note) {
            note.toggleAttribute('hidden', !cross);
            note.textContent = cross
              ? 'Cross-section: these axes are not a map, so it is drawn in index space.'
              : '';
          }
          // Held, not lost: the axes going back to a map draw it again (#1099).
          if (projection && cross && projection.value !== 'source') {
            holdProjection(projection.value);
            projection.value = 'source';
          }
          // The picker, Resampling and the Overlay row are hidden, not greyed
          // out, as for any grid that can't use them (#1036), and come back
          // with the map unless the slice's answer says otherwise.
          applyUnavailable(unavailable);
          applyContoursOnly();
          if (cross && sliceState) {
            requestAxis(sliceState.yDim);
            requestAxis(sliceState.xDim);
          }
          // And the axes of the slice on screen, which the rails label
          // (#1145). Asked for here, not as the rails are drawn: an axis
          // answer redraws them, and a closed file's answer caches nothing,
          // so asking from there would ask again on every answer.
          const drawn = drawnSpec();
          if (drawn !== sliceState && isCrossSectionOf(drawn)) {
            requestAxis(drawn.yDim, drawn);
            requestAxis(drawn.xDim, drawn);
          }
          syncProjectionControls();
          drawAxisRails();
        }

        // --- Animation along an axis (#170) ----------------------------------
        // Playing steps the chosen axis with the same slice-index change the
        // stepper makes, and waits for each frame to arrive before timing the
        // next one, so a slow decode lowers the frame rate rather than queueing
        // requests behind itself.
        let animation = null;
        // The animated axis and the line plot's axis the user chose, kept apart
        // from their selects: those are rebuilt with each slice, after
        // restoreState has run, so a saved choice is applied when they are
        // filled rather than when the snapshot is read (#1027).
        let preferredAnimateDim = null;
        let preferredLineAxis = null;
        // The range playback held, kept once it stops (#1227): the frame it
        // stopped on was drawn on it, and a hide and show or a page written
        // again draws that frame on it again rather than on the frame's own.
        // Saved with the other selections. The next render asked for (a step,
        // or any control that draws again) is on the frame's own range, as
        // #1155 has it. Automatic contours over the frame are spaced on it too.
        let pausedRange = null;

        function animationDim() {
          const sel = document.getElementById('animate-dim');
          return sel && sel.value !== '' ? Number(sel.value) : null;
        }

        function buildAnimationControls() {
          const row = document.getElementById('animate-row');
          const sel = document.getElementById('animate-dim');
          if (!row || !sel || !sliceState) return;
          stopAnimation();
          const v = sliceVariable(sliceState.variableIndex);
          const lengths = v ? v.dims.map((d) => d.length) : [];
          const choice = defaultAnimationDim(lengths, sliceState.yDim, sliceState.xDim, v ? v.detectedTimeDim : null);
          if (!v || choice == null) {
            row.toggleAttribute('hidden', true);
            sel.innerHTML = '';
            return;
          }
          sel.innerHTML = v.dims
            .map((d, i) => (i === sliceState.yDim || i === sliceState.xDim || d.length <= 1)
              ? ''
              : '<option value="' + i + '">' + escapeAttr(d.name) + '</option>')
            .join('');
          sel.value = String(choice);
          if (preferredAnimateDim !== null &&
              Array.from(sel.options).some((o) => o.value === preferredAnimateDim)) {
            sel.value = preferredAnimateDim;
          }
          syncSliceRows();
          updateFrameLabel();
        }

        function updateFrameLabel() {
          const label = document.getElementById('anim-frame');
          const dim = animationDim();
          const v = sliceState && sliceVariable(sliceState.variableIndex);
          if (!label || dim == null || !v) return;
          const index = sliceState.sliceIndices[dim];
          const step = 'step ' + index + ' of 0\u2013' + (v.dims[dim].length - 1);
          // The coordinate first, the step after it (#1041): '500 millibar
          // (step 5 of 0\u201316)'. An axis with no coordinates keeps the step.
          const at = coordinateAt(dim, index);
          label.textContent = at ? at + ' (' + step + ')' : step;
        }

        // Move the animated axis to the given index and render it.
        function showFrame(index) {
          const dim = animationDim();
          if (dim == null) return;
          syncSliceIndex(dim, index);
          updateFrameLabel();
          snapshotState();
          requestRender();
        }

        function stepAnimation(step) {
          const dim = animationDim();
          const v = sliceState && sliceVariable(sliceState.variableIndex);
          if (dim == null || !v) return null;
          const loop = !!(document.getElementById('anim-loop') || {}).checked;
          return nextFrame(sliceState.sliceIndices[dim], v.dims[dim].length, step, loop);
        }

        // Play is off while the panel waits for a variable to be picked: a frame
        // would not be asked for, and playback would wait on it for good.
        function setAwaitingPick(on) {
          awaitingPick = on;
          if (on) stopAnimation();
          // Whatever is picked next is drawn on its own range.
          if (on && pausedRange) {
            pausedRange = null;
            snapshotState();
          }
          const play = document.getElementById('anim-play');
          if (play) play.disabled = on;
          // Compare and the arrows go while nothing is drawn (PICK_HIDES).
          applyUnavailable(unavailable);
          syncSliceRows();
        }

        // A slice panel's axis pickers, steppers and Animate row are put away
        // while a variable is to be picked (#1388). They are a variable's
        // nobody picked: the old file's, when the file has no variable left,
        // or the new file's first. The Variable picker stays, to pick from.
        // The Animate row also needs an axis to step through.
        function syncSliceRows() {
          const ySel = document.getElementById('slice-y');
          const xSel = document.getElementById('slice-x');
          for (const el of [
            ySel && ySel.closest('label'),
            xSel && xSel.closest('label'),
            document.getElementById('slice-dims'),
          ]) {
            if (el) el.toggleAttribute('hidden', awaitingPick);
          }
          const row = document.getElementById('animate-row');
          const sel = document.getElementById('animate-dim');
          if (row && sel) row.toggleAttribute('hidden', awaitingPick || sel.options.length === 0);
        }

        function playAnimation() {
          if (animation || awaitingPick || animationDim() == null) return;
          const mode = document.querySelector('input[name="range-mode"]:checked');
          const auto = !mode || mode.value !== 'manual';
          animation = {
            // No range to hold when the last frame had no values (#871), or
            // a zero-width one: a constant frame's min == max, which Rust
            // refuses as a manual range (#1155), so each frame then ranges
            // itself.
            lock: auto && lastPayload && lastPayload.usedMin != null
              && lastPayload.usedMax > lastPayload.usedMin
              ? [lastPayload.usedMin, lastPayload.usedMax]
              : null,
            awaiting: false,
            timer: 0,
            requestedAt: 0,
          };
          const play = document.getElementById('anim-play');
          if (play) { play.classList.add('playing'); play.setAttribute('aria-label', 'Pause'); play.title = 'Pause'; }
          advanceAnimation();
        }

        function advanceAnimation() {
          if (!animation) return;
          const next = stepAnimation(1);
          if (next == null) { stopAnimation(); return; }
          animation.awaiting = true;
          animation.requestedAt = Date.now();
          showFrame(next);
        }

        // Called when a render lands (or fails) while playing.
        function animationFrameArrived() {
          if (!animation || !animation.awaiting) return;
          animation.awaiting = false;
          const speed = Number((document.getElementById('anim-speed') || {}).value) || 2;
          const wait = Math.max(0, 1000 / speed - (Date.now() - animation.requestedAt));
          animation.timer = setTimeout(advanceAnimation, wait);
        }

        function stopAnimation() {
          if (!animation) return;
          clearTimeout(animation.timer);
          const held = animation.lock;
          animation = null;
          if (held) {
            pausedRange = held;
            snapshotState();
          }
          const play = document.getElementById('anim-play');
          if (play) { play.classList.remove('playing'); play.setAttribute('aria-label', 'Play'); play.title = 'Play'; }
        }

        function setupAnimation() {
          const on = (id, handler) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('click', handler);
          };
          on('anim-play', () => (animation ? stopAnimation() : playAnimation()));
          const jump = (pick) => () => {
            stopAnimation();
            const dim = animationDim();
            const v = sliceState && sliceVariable(sliceState.variableIndex);
            if (dim == null || !v) return;
            const to = pick(v.dims[dim].length);
            if (to != null && to !== sliceState.sliceIndices[dim]) showFrame(to);
          };
          on('anim-first', jump(() => 0));
          on('anim-last', jump((len) => len - 1));
          on('anim-back', jump(() => stepAnimation(-1)));
          on('anim-forward', jump(() => stepAnimation(1)));
          const sel = document.getElementById('animate-dim');
          if (sel) sel.addEventListener('change', () => {
            preferredAnimateDim = sel.value;
            stopAnimation();
            updateFrameLabel();
          });
        }

        function setupSlice() {
          if (!sliceState) return;
          const varSel = document.getElementById('slice-variable');
          if (varSel) {
            varSel.innerHTML = SLICE.variables
              .map((v) => '<option value="' + v.index + '">' + escapeAttr(v.name) + '</option>')
              .join('');
            varSel.value = String(sliceState.variableIndex);
            // A page written while a pick is awaited opens on a blank picker,
            // with nothing asked for, until the refusal or a render comes
            // (#1237).
            if (SLICE.picking === true) {
              varSel.selectedIndex = -1;
              setAwaitingPick(true);
            }
            varSel.addEventListener('change', () => {
              const v = sliceVariable(Number(varSel.value));
              if (!v) return;
              setAwaitingPick(false);
              // New variable → reset axes to its detected horizontals (falling
              // back to the first two dims) and zero the held indices.
              const yDim = v.detectedYDim != null ? v.detectedYDim : 0;
              let xDim = v.detectedXDim != null ? v.detectedXDim : 1;
              if (xDim === yDim) xDim = yDim === 0 ? 1 : 0;
              sliceState = {
                variableIndex: v.index,
                yDim,
                xDim,
                sliceIndices: v.dims.map(() => 0),
              };
              // Field B follows the new variable's shape, reset alongside A.
              compareIndices = v.dims.map(() => 0);
              buildSliceAxes();
              buildCompareAxes();
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
            buildCompareAxes();
            requestRender();
          };
          if (ySel) ySel.addEventListener('change', onAxisChange);
          if (xSel) xSel.addEventListener('change', onAxisChange);
          buildSliceAxes();
          buildCompareAxes();
          setupAnimation();
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
              // A step taken by hand is the user taking over from playback.
              stopAnimation();
              syncSliceIndex(Number(t.dataset.dim), t.value);
              updateFrameLabel();
              requestRender();
            });
          }
          // Field B's compare steppers, wired the same way into their own
          // container so they drive compareIndices independently.
          const cwrap = document.getElementById('compare-dims');
          if (cwrap) {
            cwrap.addEventListener('input', (ev) => {
              const t = ev.target;
              if (!t || t.dataset == null || t.dataset.dim == null) return;
              syncCompareIndex(Number(t.dataset.dim), t.value);
            });
            cwrap.addEventListener('change', (ev) => {
              const t = ev.target;
              if (!t || t.dataset == null || t.dataset.dim == null) return;
              syncCompareIndex(Number(t.dataset.dim), t.value);
              requestRender();
            });
          }
        }

        function setStatus(text) {
          const el = document.getElementById('status');
          if (el) el.textContent = text;
        }

        // Contour projection failures show in their own line, not #status, so a
        // "not supported on this grid" message never clobbers the render summary
        // (dimensions + range). Empty string hides the line (CSS :empty). #338
        function setContourStatus(text) { setLine('contour-status', text); }

        // Every feature that can fail says so on a line of its own, so one
        // feature's success or request never clears another's message (#1051):
        // the arrows' refusal (#vector-status), the overlay layers'
        // (#overlay-status) and the PNG export's outcome (#export-status). Each
        // is cleared by the feature that wrote it, and none touches #status.
        //
        // A line cleared keeps the room it took (kept-room), so a render
        // that clears it, or clears it and says it again when the reply
        // lands, does not move the map under a pointer held still (#1255).
        // The room is given back by a render of a new picture, when the
        // drawn image goes, or when the feature is turned off, since no answer
        // is coming to say anything there (releaseLineRoom).
        const FEATURE_LINES = ['contour-status', 'vector-status', 'overlay-status', 'export-status'];
        function setLine(id, text) {
          const el = document.getElementById(id);
          if (!el) return;
          if (!text && el.textContent) {
            const height = el.getBoundingClientRect().height;
            const kept = parseFloat(el.style.minHeight) || 0;
            if (height > kept) el.style.minHeight = height + 'px';
            if (height > 0 || kept > 0) el.classList.add('kept-room');
          }
          el.textContent = text;
        }
        function releaseLineRoom(...only) {
          for (const id of only.length ? only : FEATURE_LINES) {
            const el = document.getElementById(id);
            if (!el) continue;
            el.style.minHeight = '';
            el.classList.remove('kept-room');
          }
        }
        function setVectorStatus(text) { setLine('vector-status', text); }
        function setOverlayStatus(text) { setLine('overlay-status', text); }
        function setExportStatus(text) {
          setLine('export-status', text);
          const el = document.getElementById('export-status');
          if (el) el.classList.toggle('failed', /^(Export failed|Render the field|Pick a )/.test(text));
        }

        // What either export says while a pick is awaited: nothing is drawn,
        // and the pickers hold a field nobody chose (#1230).
        function pickBeforeExport(format) {
          return 'Pick a ' + (SLICE ? 'variable' : 'message') + ' to draw before exporting ' + format + '.';
        }

        // The warped lat/lon targets (equirectangular + Web Mercator) both
        // render a lat/lon window: they accept a manual bounds box and show the
        // Bounds control. Keep the rule in one place so the two gates can't
        // drift.
        function warpsLatLon(projection) {
          return projection === 'equirectangular' || projection === 'web_mercator';
        }

        // Whether the displayed render uses log10 colour scaling: the toggle is
        // checked and enabled (disabled means the range has no positive floor).
        // Toggling it re-renders, so this always matches the on-screen image.
        // One source for both the render request and the PNG colorbar (#331).
        function isLog10Active() {
          const logEl = document.getElementById('scale-log');
          return !!(logEl && logEl.checked && !logEl.disabled);
        }

        // render: the options are for a render request, which asks for a held
        // map target (#1099) once the axes are a map again, though the picker
        // offers only the source view until the answer comes back. The
        // provider draws a slice that can't take it in the source view, and
        // says so. Every other request (overlays, a probe) is about the image
        // on screen, so its geometry (projection, centre, bounds, which way
        // up the rows are) is the one that image was drawn with (#1114), not
        // what the controls say a render on its way will have.
        function currentOptions(render) {
          const projection = render === true
            ? requestedProjection()
            : (document.getElementById('picker-projection') || {}).value || 'source';
          const resampling = (document.getElementById('picker-resampling') || {}).value || 'nearest';
          const flipY = rowsFlipped();
          const mode = document.querySelector('input[name="range-mode"]:checked');
          const options = { projection, resampling, flipY };
          // Colours. The name is a registry name the Rust side knows; reverse
          // flips the ramp end-for-end.
          const cmapEl = document.getElementById('picker-colormap');
          if (cmapEl && cmapEl.value) options.colormap = cmapEl.value;
          options.reverseColormap = !!(document.getElementById('reverse-colormap')
            && document.getElementById('reverse-colormap').checked);
          // Log₁₀ scaling. Only send it when the toggle is both checked and
          // enabled: it is disabled whenever the current range has no positive
          // floor (log10 needs one), so this never round-trips a request the
          // Rust side would reject. Otherwise the field maps linearly.
          if (isLog10Active()) options.scaleMode = 'log10';
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
            // Each end is sent as typed, and a field left empty is left to the
            // data (#1155). Rust decides whether the pair holds, and a refusal
            // is shown at these fields (showRangeRefusal), not dropped here.
            // Text the field cannot parse (1e999, a lone minus sign) reads as
            // '' but is not empty: it is sent as null, which the host reads as
            // a number Rust refuses as not finite, rather than drawing the
            // data's own end (#1341). A message carries no NaN, so null stands
            // for it.
            const end = (id) => {
              const el = document.getElementById(id);
              if (!el) return undefined;
              if (el.value === '') return el.validity && el.validity.badInput ? null : undefined;
              const v = Number(el.value);
              return Number.isFinite(v) ? v : null;
            };
            options.rangeMin = end('range-min');
            options.rangeMax = end('range-max');
          } else if (animation && animation.lock) {
            // Playing on an auto range: every frame is coloured on the range the
            // first one was, so the colours mean the same values throughout
            // rather than each frame stretching to its own extremes (#170).
            options.rangeMin = animation.lock[0];
            options.rangeMax = animation.lock[1];
          } else if (pausedRange) {
            // Stopped there: the frame on screen is the one drawn on it (#1227).
            options.rangeMin = pausedRange[0];
            options.rangeMax = pausedRange[1];
          }
          // Manual extent applies to the warped lat/lon targets
          // (equirectangular + Web Mercator), which both render a lat/lon
          // window. Send all four edges or none; the Rust side validates and
          // falls back to the computed bounds for a partial/inverted box.
          const typed = warpsLatLon(projection) ? typedMapWindow() : null;
          if (typed) {
            options.boundsLatMin = typed.latMin;
            options.boundsLatMax = typed.latMax;
            options.boundsLonMin = typed.lonMin;
            options.boundsLonMax = typed.lonMax;
          }
          // The options that place the image's pixels on the field, which a
          // request about the image on screen takes from its render.
          const drawn = render === true ? null : drawnOptions();
          if (drawn) {
            for (const key of [
              'projection', 'projectionPreset', 'centerLat', 'centerLon', 'flipY',
              'boundsLatMin', 'boundsLatMax', 'boundsLonMin', 'boundsLonMax',
            ]) {
              if (drawn[key] === undefined || drawn[key] === null) delete options[key];
              else options[key] = drawn[key];
            }
          }
          return options;
        }

        // The window the Bounds fields ask for, as a render request carries
        // it: on Manual, all four edges forming a box. Null on Auto, or for a
        // box Rust would not take, when the grid's own window is drawn.
        function typedMapWindow() {
          const bmode = document.querySelector('input[name="bounds-mode"]:checked');
          if (!bmode || bmode.value !== 'manual') return null;
          const laMin = Number((document.getElementById('bounds-lat-min') || {}).value);
          const laMax = Number((document.getElementById('bounds-lat-max') || {}).value);
          const loMin = Number((document.getElementById('bounds-lon-min') || {}).value);
          const loMax = Number((document.getElementById('bounds-lon-max') || {}).value);
          if (
            Number.isFinite(laMin) && Number.isFinite(laMax) && laMax > laMin &&
            Number.isFinite(loMin) && Number.isFinite(loMax) && loMax > loMin
          ) {
            return { latMin: laMin, latMax: laMax, lonMin: loMin, lonMax: loMax };
          }
          return null;
        }

        function requestRender() {
          // A render asked for is a change made to the picture, which ends a
          // range held from playback (#1227). Playback's own frames go on
          // holding theirs (animation.lock).
          if (pausedRange) {
            pausedRange = null;
            snapshotState();
          }
          if (!mountAnswered) {
            // Initial mount: provider posts ready-options-default automatically.
            return;
          }
          if (awaitingPick) return;
          // A cross-section's rows are oriented by its Y axis (#1043), so its
          // values are read before the first render that needs them.
          whenYAxisKnown(() => {
            // A render held for its Y axis is not sent once a pick is awaited
            // meanwhile: it would draw a variable nobody chose (#1237).
            if (awaitingPick) return;
            postRerender();
          });
          setStatus('Rendering…');
        }

        // How the image is drawn for the raster the payload describes. A view
        // with no geographic meaning (source, cross-section) is held to eight to
        // one when the raster is longer than that (#1226). A lat/lon map keeps
        // its own shape, held only past MAP_MOST_ASPECT so a 1 x 720 raster is
        // not a hairline (#1251). While arrows are asked for the view is held
        // like a map, at MAP_MOST_ASPECT: Rust lays the arrows out in square
        // raster pixels, so a stretch would turn them and make the key wrong,
        // and up to 48 to 1 there is none. Past it the stretch is real, and
        // drawVectorKey leaves the key off and says why. Any image that would be taller than most of the page
        // at the panel's width is drawn to that height instead, held or not
        // (#1252): the stylesheet caps the wrapper's width at the height times
        // --drawn-aspect, so a slightly less thin image is never drawn taller
        // than a thinner one. Called again when the arrows are turned on or off.
        function setDisplayAspect(payload) {
          const canvas = document.getElementById('canvas');
          if (!canvas || !payload) return;
          const held = displayAspect(
            payload.width,
            payload.height,
            vectorsEnabled() || warpsLatLon(drawnProjection()) ? MAP_MOST_ASPECT : undefined,
          );
          canvas.style.aspectRatio = held;
          displayHeld = held !== '';
          const wrap = canvas.closest('.canvas-wrap');
          if (wrap) {
            const parts = held ? held.split('/').map(Number) : [payload.width, payload.height];
            const aspect = parts[0] / parts[1];
            if (aspect > 0 && Number.isFinite(aspect)) wrap.style.setProperty('--drawn-aspect', String(aspect));
            else wrap.style.removeProperty('--drawn-aspect');
          }
        }

        // The colour bar is never taller than the image beside it (#1393): the
        // stylesheet takes the lesser of 320px and --image-h. Called when an
        // image is drawn and when the image changes size.
        function fitColorbar() {
          const bar = document.getElementById('colorbar');
          const img = document.getElementById('canvas');
          if (!bar || !img) return;
          // The layout height: a zoom preview's scale() must not resize the bar.
          const h = img.offsetHeight;
          if (h > 0) bar.style.setProperty('--image-h', h + 'px');
        }

        function blit(payload) {
          takeAskedKey(payload);
          settleGesturePreview(payload);
          const canvas = document.getElementById('canvas');
          if (!canvas) return;
          const ctx = canvas.getContext('2d');
          if (!ctx) return;
          canvas.width = payload.width;
          canvas.height = payload.height;
          setDisplayAspect(payload);
          // payload.rgba arrives as a Uint8Array; view it as a Uint8ClampedArray
          // so ImageData accepts it.
          const rgba = new Uint8ClampedArray(
            payload.rgba.buffer ? payload.rgba.buffer : payload.rgba,
            payload.rgba.byteOffset || 0,
            payload.rgba.byteLength || payload.rgba.length
          );
          const img = new ImageData(rgba, payload.width, payload.height);
          ctx.putImageData(img, 0, 0);
          fitColorbar();
          // A field with no values has no range, so the colorbar is hidden
          // rather than labelled with one (#871).
          const hasRange = payload.usedMin != null && payload.usedMax != null;
          const colorbar = document.getElementById('colorbar');
          if (colorbar) colorbar.hidden = !hasRange;
          const cbMin = document.getElementById('cb-min');
          const cbMax = document.getElementById('cb-max');
          if (cbMin) cbMin.textContent = hasRange ? payload.usedMin.toPrecision(4) : '—';
          if (cbMax) cbMax.textContent = hasRange ? payload.usedMax.toPrecision(4) : '—';
          const proj = document.getElementById('projection-summary');
          if (proj) proj.textContent = payload.projectionSummary || '';
          // When the image is a computed (e.g. difference) field, name the
          // operation up front. The one the render combined with, which the
          // provider echoes (#1145), not the Operation picker: that can read
          // A − B over a single field when Field B is unchosen (#1230).
          const opLabel = compareOpLabel(payload);
          const opText = opLabel ? opLabel + ' · ' : '';
          setStatus(
            opText + payload.width + '×' + payload.height + ' · ' +
            composeRangeText(payload.usedMin, payload.usedMax),
          );
          // The last export's outcome was said of the previous image.
          setExportStatus('');
          // The range this image was drawn on held, so no refusal stands.
          showRangeRefusal(null);
          // Pre-fill the manual-range inputs once so the user can switch
          // to Manual mode without typing. Nothing to offer without a range.
          // Only while the range is automatic: in Manual mode a field left
          // blank means "from the data" (#1155), and filling it would turn a
          // one-ended range into a two-ended one behind the user's back.
          const minIn = document.getElementById('range-min');
          const maxIn = document.getElementById('range-max');
          const rangeMode = document.querySelector('input[name="range-mode"]:checked');
          const autoRange = !rangeMode || rangeMode.value !== 'manual';
          if (autoRange && hasRange && minIn && !minIn.value) minIn.value = payload.usedMin.toPrecision(6);
          if (autoRange && hasRange && maxIn && !maxIn.value) maxIn.value = payload.usedMax.toPrecision(6);
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

        // Whether log₁₀ scaling has a positive lower bound to work from. In
        // manual mode that's a valid manual box whose minimum is > 0; in auto
        // mode it's the last render's data minimum (echoed as usedMin) being
        // > 0. Log10 has no logarithm for a non-positive floor, so the toggle
        // is only offered when this holds.
        function logHasPositiveFloor() {
          const mode = document.querySelector('input[name="range-mode"]:checked');
          if (mode && mode.value === 'manual') {
            // A typed minimum is the floor; a blank one comes from the data,
            // like the automatic range's (#1155).
            const raw = (document.getElementById('range-min') || {}).value;
            const min = raw === undefined || raw === '' ? NaN : Number(raw);
            if (Number.isFinite(min)) return min > 0;
          }
          return !!(lastPayload && typeof lastPayload.usedMin === 'number'
            && lastPayload.usedMin > 0);
        }

        // Enable/disable the log toggle to match logHasPositiveFloor(), with a
        // hint on why it's off, and drop the check when it becomes unavailable
        // so a stale ✓ can't send a request Rust would reject.
        function updateLogAvailability() {
          const logEl = document.getElementById('scale-log');
          if (!logEl) return;
          const ok = logHasPositiveFloor();
          logEl.disabled = !ok;
          const label = logEl.closest ? logEl.closest('label') : null;
          const hint = ok ? '' : 'Log₁₀ needs a positive range — set a manual minimum above 0';
          logEl.title = hint;
          if (label) label.title = hint;
          if (!ok && logEl.checked) logEl.checked = false;
        }

        // The slice the image on screen was drawn from, as its render named it
        // (#1041), and Field B's indices when it is a Compare map; null for a
        // GRIB panel. The subtitle states the slice drawn, not the one the
        // steppers have moved on to while that render is on its way.
        let drawnSlice = null;
        let drawnSliceB = null;
        let drawnTruncation = null;

        // The slice of the image on screen (#1145): what the rails label, and
        // what the probe, the line plot, the zonal average, the overlays and
        // the CSV export read. The pickers can have moved on while a render is
        // on its way, or after one failed or was refused, and then they
        // describe an image not drawn. Before the first render there is no
        // image, and a request is about the one to come.
        function drawnSpec() {
          return drawnSlice || sliceState;
        }

        // sliceFields() for the image on screen.
        function drawnSliceFields() {
          const spec = drawnSpec();
          return spec ? { slice: spec } : {};
        }

        // compareRequest() for the image on screen (#1145): the operation and
        // Field B its render combined, as the provider took them, or none for
        // a single field. The Compare controls' own until the first render.
        function drawnCompareRequest() {
          if (!lastPayload || !('compare' in lastPayload)) return compareRequest();
          return lastPayload.compare ? { compare: lastPayload.compare } : {};
        }

        // The subtitle's slice part: each held dimension of the drawn slice by
        // its coordinate. Left out until every one of its axes is known, so
        // the line never names a slice by index and then by value. The export
        // name is built host-side from the same parts (heldSliceParts).
        function drawnSliceNote() {
          const v = drawnSlice && sliceVariable(drawnSlice.variableIndex);
          if (!v) return '';
          const names = v.dims.map((d) => d.name);
          const axisOf = (i) => axisCache[drawnSlice.variableIndex + ':' + i];
          const parts = (indices) => heldSliceParts(
            names, { yDim: drawnSlice.yDim, xDim: drawnSlice.xDim, sliceIndices: indices }, axisOf);
          const a = parts(drawnSlice.sliceIndices);
          const b = drawnSliceB ? parts(drawnSliceB) : null;
          if (!a || (drawnSliceB && !b)) return '';
          return composeSliceNote(a, b);
        }

        // The line under the heading: the source and the field's level and
        // times, the slice drawn (#1041), then the band-limit note of the map
        // on screen. The PNG export draws the same line.
        function updateSubtitle() {
          SUB_LINE = composeSubtitle(composeSubtitle(SUB_BASE, drawnSliceNote()), drawnTruncation);
          const subtitle = document.querySelector('.subtitle');
          if (subtitle) subtitle.textContent = SUB_LINE;
        }

        // A slice page written while a pick was awaited opens waiting, and is
        // shown again from the same HTML after the pick. An answer to its
        // ready that is not the refusal (a render of the variable picked, or
        // an error such as the file being closed) says the pick was made: the
        // wait ends and the picker shows the slice held (#1237). The provider
        // answers nothing else while a pick is awaited.
        function endStaleWait() {
          if (!SLICE || !awaitingPick) return;
          setAwaitingPick(false);
          const varSel = document.getElementById('slice-variable');
          if (varSel && sliceState) varSel.value = String(sliceState.variableIndex);
        }

        function handleGridReady(msg) {
          lastPayload = msg;
          lastRenderFailed = false;
          // Read before blit takes the request's record.
          const scene = askedScene(msg);
          const sameScene = scene !== null && scene === drawnScene;
          drawnScene = scene;
          mountAnswered = true;
          endStaleWait();
          // Re-label for the field actually drawn. A NetCDF panel switches
          // variables inside one webview, and name/units travel with the
          // variable; a GRIB panel sends the same strings every time, so this
          // is a no-op there.
          if (typeof msg.titleLine === 'string') {
            TITLE_LINE = msg.titleLine;
            const h1 = document.getElementById('title-line');
            if (h1) h1.textContent = TITLE_LINE;
          }
          // Something is drawn, so Compare and the arrows are offered again
          // (#1388). Before blit, which asks for the arrows.
          setNothingDrawn(false);
          // Applied when null too: turning Compare off must drop a label that
          // only field B carried.
          drawnTruncation = msg.truncationNote ?? null;
          drawnSlice = msg.slice || null;
          drawnSliceB = Array.isArray(msg.compareSliceIndicesB) ? msg.compareSliceIndicesB : null;
          updateSubtitle();
          // The line plot offers the drawn variable's axes (#1145).
          fillLineAxes();
          if (typeof msg.parameterUnits === 'string') UNITS = msg.parameterUnits;
          if (typeof msg.defaultPngName === 'string') DEFAULT_PNG_NAME = msg.defaultPngName;
          // A slice panel's picker and Overlay row follow the slice on screen.
          // Before the overlay and contour keys below are read, so overlays are
          // asked for in the projection the image was drawn in, and only for a
          // slice that is placed.
          applySliceAnswer(msg.sliceGrid, false);
          blit(msg);
          updateLogAvailability();
          animationFrameArrived();
          syncCrossSectionMode();
          // A render that only moved the window or recoloured the picture
          // (#1255) leaves the readout, the probed cell and its line (or the
          // zonal average) as they are: they read the same values. Clearing
          // them would also take the plot's room above the map, which would
          // move under a pointer held still for the next wheel tick.
          if (!sameScene) {
            // The old probe readout referred to the previous field; clear it,
            // and drop a reply still on its way for it (#1146), as a line reply.
            probeSeq += 1;
            lineSeq += 1;
            const probeEl = document.getElementById('probe');
            if (probeEl) probeEl.textContent = '';
            // And the line through the probed cell, for the same reason (#172) —
            // unless a zonal mean is on, which describes the new field and is
            // asked for again (#240).
            // The readout's cell goes with the readout either way.
            probedCell = null;
            // A zonal average back on a field that takes it again, after a
            // cell's line drawn where its box was hidden (#1036), replaces that
            // line as a ticked box would (#1234).
            if (zonalOn()) { zonalReplacesLine(); requestZonal(); } else hideLine();
          }
          // Reproject the overlay only when the raster *geometry* changed
          // (projection / preset / flip-y / bounds). A range- or resampling-
          // only render leaves the geometry — and the existing overlay — valid,
          // so we skip the round-trip.
          if (overlayKey() !== lastOverlayKey) {
            // Wipe the stale strokes immediately so the previous projection's
            // lines don't linger over the new raster while the async reproject
            // is in flight. Contours are geometry-bound too, so drop them until
            // their re-fetch lands — before clearOverlay, which repaints and
            // would otherwise re-stroke the stale runs it was called to remove.
            lastContour = null;
            lastVectors = null;
            clearOverlay();
            requestOverlay();
            // The arrows are pixel-space too, so they follow the geometry.
            requestVectors();
          }
          // Contours track the field, used range, projection, and interval —
          // re-fetch only when one of those moved. A palette- or resampling-only
          // repaint leaves the projected runs valid, so skip the marching-
          // squares + projection round-trip (#336). contourKey returns null when
          // contours are off, matching lastContourKey so nothing fires.
          if (contourKey() !== lastContourKey) {
            requestContours();
          }
          // A new picture may move the map anyway: the feature lines give
          // back the room they kept. After the requests above, whose clears
          // would keep it again.
          if (!sameScene) releaseLineRoom();
        }

        function handleGridError(msg) {
          // A frame that fails ends playback rather than retrying it forever.
          stopAnimation();
          // As a drawn render: only the answer to the last request ends what
          // a gesture is waiting for.
          const latest = !msg || typeof msg.seq !== 'number' || msg.seq >= renderSeq;
          if (latest) {
            answeredSeq = renderSeq;
            clearGesturePreview();
            // A refused range has its own export message (rangeRefused), and does
            // not clear an earlier failure: the picture is still the old one.
            if (!msg.rangeRefusal) lastRenderFailed = true;
          }
          const err = msg.error || 'render failed';
          const askedKey = takeAskedKey(msg);
          mountAnswered = true;
          // A refused colour range is said once, at the fields it is about
          // (showRangeRefusal); the status line only points there (#1155).
          setStatus(msg.rangeRefusal
            ? 'Colour range not drawn: see the range fields'
            : 'Error: ' + err);
          applySliceAnswer(msg.sliceGrid, msg.handleGone === true);
          // A panel whose file cannot be read names no field either (#1350),
          // and shows none: a picture left on screen would be one the heading
          // no longer names (#1388).
          if (msg.handleGone === true) {
            clearDrawn();
            headNothingDrawn(${JSON.stringify(NO_FIELD_HEADING)});
          } else if (latest && typeof msg.titleLine === 'string') {
            headFailedField(msg);
          }
          if (msg.pickVariable !== true && msg.noVariables !== true) endStaleWait();
          // The variable the panel was showing is gone from its file, and the
          // picker is left with nothing chosen: whichever variable is picked
          // then is a change, and draws. Until then nothing is asked for.
          // A file left with no variable at all is the same: the old file's
          // picture and heading go (#1237), and a variable the picker still
          // lists has the panel look at the file again.
          if (msg.pickVariable === true || msg.noVariables === true) {
            const varSel = document.getElementById('slice-variable');
            if (varSel) varSel.selectedIndex = -1;
            setAwaitingPick(true);
            clearDrawn();
            // Nor does the heading name a variable, as a GRIB panel's names no
            // message (#1230).
            headNothingDrawn(${JSON.stringify(NO_VARIABLE_HEADING)});
          }
          // The message a GRIB panel was drawing is not exactly once in the
          // file that changed under it (#959).
          if (Array.isArray(msg.pickMessages)) offerMessages(msg.pickMessages);
          // The file holds no variable now, so a range refused of one read from
          // the old file is about nothing (#1194).
          if (msg.noVariables === true) rangeEdited();
          const refusal = msg.rangeRefusal || null;
          const manual = document.querySelector('input[name="range-mode"]:checked');
          const isManual = !!(manual && manual.value === 'manual');
          // Self-heal the one render error the log toggle can cause on its
          // own: switching to a field with no positive floor while log was on
          // and the range was automatic. Drop log and re-render linearly rather
          // than leaving the panel stuck on an error. Read from the refusal's
          // code, not its wording (#1155).
          const logEl = document.getElementById('scale-log');
          if (logEl && logEl.checked && refusal && refusal.reason === 'log10_not_positive'
            && !isManual) {
            logEl.checked = false;
            logEl.disabled = true;
            requestRender();
            return;
          }
          // A range the user typed that does not hold is shown at the fields
          // it is about, in Rust's words. An error that is not about the range
          // (a Compare on another grid fails before the range is read) says
          // nothing of it, so a refusal already standing is left up when it
          // does not depend on the data and the field is the same: the fields
          // still hold what was refused. It clears on a drawn image or when the
          // range fields or mode change (#1174), or the field changes under a
          // refusal read off the data (#1186).
          if (refusal) {
            showRangeRefusal(refusal, msg.rangeMessage, askedKey);
          } else if (rangeRefused && !(rangeRefusalFixed || refusedFieldKey === askedKey)) {
            // A refusal that was read off the data is about the field it was
            // read from. The field has changed and the new render failed before
            // reading any range, so the note would be about a field no longer
            // chosen (#1186). One that names only typed values stands for any
            // field.
            rangeEdited();
          }
        }

        // Each render request carries an id, and the field it asks for is kept
        // under it until the host's answer (which echoes the id) comes back. A
        // refusal is said of the field its request named, not of whatever is
        // chosen by the time it arrives (#1186). An answer with no id the page
        // knows (the host refusing on its own) is taken to be about the field
        // chosen now.
        let renderSeq = 0;
        // The id of the last request answered, drawn or refused: less than
        // renderSeq while the render last asked for is on its way.
        let answeredSeq = 0;
        //
        // The picture it asks for is kept with it too (sceneKey), so a drawn
        // answer can tell whether it changed anything but the window or the
        // colours.
        const askedKeys = new Map();
        function newAsk(options) {
          const seq = ++renderSeq;
          askedKeys.set(seq, { field: fieldKey(), scene: sceneKey(options) });
          if (askedKeys.size > 64) askedKeys.delete(askedKeys.keys().next().value);
          return seq;
        }
        function takeAskedKey(msg) {
          if (msg && typeof msg.seq === 'number' && askedKeys.has(msg.seq)) {
            const key = askedKeys.get(msg.seq).field;
            askedKeys.delete(msg.seq);
            return key;
          }
          return fieldKey();
        }
        // The scene asked for by the request msg answers, or null for an
        // answer to no request the page knows (the host drawing on its own).
        function askedScene(msg) {
          const asked = msg && typeof msg.seq === 'number' ? askedKeys.get(msg.seq) : undefined;
          return asked ? asked.scene : null;
        }
        function postRerender() {
          const options = currentOptions(true);
          vscode.postMessage(Object.assign({ type: 'rerenderRequest', seq: newAsk(options) }, options, sliceFields(), compareRequest()));
        }

        // The options a render can change and still draw the picture on
        // screen: its window (a wheel, a drag, the Bounds fields, Reset view)
        // and how its values are coloured (#1255). The values at a cell are
        // the same either way, so the readout, the probed cell and the line
        // through it stay. Anything else is a new picture: the field (the
        // message, the variable, a step of its slice, Compare), the
        // projection and its centre, Flip Y axis, the resampling.
        const VIEW_OPTIONS = [
          'boundsLatMin', 'boundsLatMax', 'boundsLonMin', 'boundsLonMax',
          'colormap', 'reverseColormap', 'scaleMode', 'rangeMin', 'rangeMax',
        ];
        function sceneKey(options) {
          const rest = Object.keys(options || {}).filter((k) => !VIEW_OPTIONS.includes(k)).sort();
          return JSON.stringify([fieldKey(), rest.map((k) => [k, options[k] === undefined ? null : options[k]])]);
        }
        // The scene of the image on screen, null before one is drawn or after
        // it is cleared.
        let drawnScene = null;

        // The field selection a refusal was said of: the message, the variable
        // and slice, and the compare operation with its Field B.
        function fieldKey() {
          const pick = document.getElementById('message-pick');
          return JSON.stringify([pick ? pick.value : '', sliceFields(), compareRequest()]);
        }

        // A refusal that holds whatever the data is: it names only what was
        // typed. The ones that took an end from the data (one end typed and the
        // other left to the field; a center mirrored about the field's ends)
        // go stale when the field does.
        function refusalIsDataFree(refusal) {
          switch (refusal.reason) {
            case 'center_with_both_ends':
            case 'center_outside':
              return true;
            case 'not_finite':
              // A typed value that is not a number names no given value. A center
              // that pushes an end past the largest number does, and that
              // reach comes from the data's ends.
              return refusal.given == null;
            case 'inverted':
              return refusal.end === 'both';
            case 'log10_not_positive':
              return refusal.given != null;
            default:
              return false;
          }
        }

        // The range fields or mode were edited: what was refused was said of
        // the old values, and the render this change asks for answers again.
        function rangeEdited() {
          if (rangeRefused) {
            setExportStatus('');
            showRangeRefusal(null);
          }
        }

        // Show a refused colour range at the min/max fields (#1155): the
        // message under them, and the field (or both) it names marked
        // invalid. A null refusal clears it.
        function showRangeRefusal(refusal, message, askedKey) {
          const note = document.getElementById('range-note');
          const ends = { min: ['range-min'], max: ['range-max'], both: ['range-min', 'range-max'] };
          const marked = refusal ? ends[refusal.end] || [] : [];
          ['range-min', 'range-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (!el) return;
            if (marked.includes(id)) el.setAttribute('aria-invalid', 'true');
            else el.removeAttribute('aria-invalid');
          });
          rangeRefused = !!refusal;
          rangeRefusalFixed = !!refusal && refusalIsDataFree(refusal);
          refusedFieldKey = refusal ? askedKey : null;
          // Contours share the range, so they would be refused in the same
          // words: drop them rather than say it a third time. The next drawn
          // image asks for them again (its contour key has moved).
          if (rangeRefused) {
            contourSeq += 1;
            lastContour = null;
            lastContourKey = null;
            setContourStatus('');
            drawOverlay();
          }
          if (!note) return;
          note.textContent = refusal ? (message || '') : '';
          note.hidden = !refusal;
        }

        // The heading and the line under it while nothing is drawn: no field's
        // name, only the file or store (#959, #1230).
        function headNothingDrawn(heading) {
          TITLE_LINE = heading;
          SUB_LINE = SOURCE_NAME;
          const h1 = document.getElementById('title-line');
          if (h1) h1.textContent = TITLE_LINE;
          const subtitle = document.querySelector('.subtitle');
          if (subtitle) subtitle.textContent = SUB_LINE;
          const proj = document.getElementById('projection-summary');
          if (proj) proj.textContent = '';
          setNothingDrawn(true);
        }

        // Whether the heading says nothing is drawn. Compare and the arrows
        // follow it (PICK_HIDES), as the "Not available" line does.
        function setNothingDrawn(on) {
          if (nothingDrawn === on) return;
          nothingDrawn = on;
          applyUnavailable(unavailable);
        }

        // A render failed, and the heading names the field it was asked for
        // (#1388), as a render that drew it would. The tab does the same. A
        // slice panel's picture of another variable goes, with the slice and
        // the projection said of it; a picture of this field stays, as after
        // any failed render, under the heading that already names it. A GRIB
        // panel draws one message, so its picture is always that one's.
        function headFailedField(msg) {
          const other = SLICE
            ? !lastPayload || !drawnSlice || drawnSlice.variableIndex !== msg.messageIndex
            : !lastPayload;
          if (other) {
            clearDrawn();
            updateSubtitle();
            const proj = document.getElementById('projection-summary');
            if (proj) proj.textContent = '';
          }
          TITLE_LINE = msg.titleLine;
          const h1 = document.getElementById('title-line');
          if (h1) h1.textContent = TITLE_LINE;
          setNothingDrawn(false);
        }

        // Nothing is drawn under the old heading: the image, its overlays and
        // the heading go, and the new file's messages are offered with none
        // chosen, so whichever is picked is a change and draws (#959). Until
        // then nothing is asked for.
        function offerMessages(list) {
          const row = document.getElementById('message-pick-row');
          const sel = document.getElementById('message-pick');
          if (!row || !sel) return;
          sel.textContent = '';
          for (const f of list) {
            const option = document.createElement('option');
            option.value = String(f.index);
            option.textContent = f.label;
            sel.appendChild(option);
          }
          sel.selectedIndex = -1;
          row.hidden = false;
          setAwaitingPick(true);
          clearDrawn();
          headNothingDrawn(${JSON.stringify(NO_MESSAGE_HEADING)});
        }

        // The picture on screen and everything said of it go, for a panel that
        // waits for a pick: a GRIB panel's message (#959) or a slice panel's
        // variable (#1237) is no longer in the file. The "Not available" line
        // goes with setAwaitingPick, which applyUnavailable reads.
        function clearDrawn() {
          lastPayload = null;
          lastContour = null;
          lastVectors = null;
          // Replies in flight were asked of the file this one replaces.
          vectorSeq += 1;
          overlaySeq += 1;
          contourSeq += 1;
          probeSeq += 1;
          lineSeq += 1;
          lastContourKey = null;
          // The overlay on screen went with the picture, so the key it was
          // drawn under is spent: a pick that lands on the same variable
          // number and axes asks for the coastlines, graticule and arrows
          // again (#1342). The arrows are requested under this key too; the
          // contour key is reset above, and the zonal mean keeps none.
          lastOverlayKey = null;
          clearVectorPairNote();
          setVectorStatus('');
          setContourStatus('');
          setOverlayStatus('');
          setExportStatus('');
          releaseLineRoom();
          drawnScene = null;
          // And what the line under the heading said of it: an axis answer
          // that rebuilds the line must not bring back the slice it held
          // (#1388).
          drawnSlice = null;
          drawnSliceB = null;
          drawnTruncation = null;
          // A refused range was said of the image that went; the next render
          // over these fields says it again if it still does not hold (#1155).
          showRangeRefusal(null);
          clearOverlay();
          const canvas = document.getElementById('canvas');
          const ctx = canvas && canvas.getContext('2d');
          if (ctx) ctx.clearRect(0, 0, canvas.width, canvas.height);
          const colorbar = document.getElementById('colorbar');
          if (colorbar) colorbar.hidden = true;
          const probeEl = document.getElementById('probe');
          if (probeEl) probeEl.textContent = '';
          hideLine();
        }

        {
          const sel = document.getElementById('message-pick');
          if (sel) sel.addEventListener('change', () => {
            if (sel.value === '') return;
            vscode.postMessage({ type: 'pickMessage', messageIndex: Number(sel.value) });
            setStatus('Rendering…');
          });
        }

        // An overlay projection failed on the provider side. Resolve the
        // in-flight request and re-arm the overlay key so the next render
        // retries the overlay instead of leaving it permanently blank.
        function handleOverlayError(msg) {
          if (msg.seq !== overlaySeq) return; // stale: a newer request superseded it
          overlayAsked = 0;
          lastOverlayKey = null;
          clearOverlay();
          setOverlayStatus('Overlay error: ' + (msg.error || 'projection failed'));
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
        // Contour isolines (#238) are value-derived, so they carry their own
        // projected runs and request counter, re-fetched on any render (the
        // field, range, projection, or interval can all move them).
        let lastContour = null;
        // The projected arrows of a u/v pair (#241), and the request that is in
        // flight, so a stale answer cannot overwrite a newer one.
        let lastVectors = null;
        let vectorSeq = 0;
        let contourSeq = 0;
        // The request each line layer is waiting on, 0 once it is answered.
        // A request that a newer one (or a clear) replaced no longer counts:
        // see linesPending (#1379).
        let overlayAsked = 0;
        let contourAsked = 0;
        let vectorAsked = 0;
        // The point probe's request id (#1146), bumped by each click and each
        // render, and the same for the line plot's two requests (a cell's line
        // and a zonal mean draw into one plot, so they share one id).
        let probeSeq = 0;
        let lineSeq = 0;
        // Whether the range on the fields now is one Rust refused (#1155):
        // contours are not asked for over it.
        let rangeRefused = false;
        // Whether the latest render failed for a reason other than the range:
        // lastPayload then still holds an older picture (#1187). Cleared by a
        // gridReady.
        let lastRenderFailed = false;
        // Whether that refusal is independent of the data, and the field it was
        // said of (#1186).
        let rangeRefusalFixed = false;
        let refusedFieldKey = null;
        // The state the cached contour runs correspond to (geometry + used range
        // + interval + field). A palette-only repaint leaves it unchanged, so we
        // skip re-running marching squares + projection (#336). null = no runs.
        let lastContourKey = null;
        // The geometry-affecting options behind the current overlay. A render
        // that changes only range/resampling leaves this unchanged, so we skip
        // a redundant reprojection round-trip.
        let lastOverlayKey = null;
        // The overlay layers, in draw order: the vector layers, then the
        // graticule on top. \`flag\` is the field the provider reads on an
        // overlayRequest; \`name\` is what it labels the projected runs with and
        // what the stroke styles key off. \`themed\` layers default to the
        // editor's foreground colour (so they read on light and dark alike);
        // the water layers default to the blue baked into their colour input.
        const OVERLAY_LAYERS = [
          { id: 'coastline', flag: 'coastlines', themed: true, alpha: 1 },
          { id: 'borders', flag: 'borders', themed: true, alpha: 0.75 },
          { id: 'lakes', flag: 'lakes', themed: false, alpha: 1 },
          { id: 'rivers', flag: 'rivers', themed: false, alpha: 1 },
          { id: 'graticule', flag: 'graticule', themed: true, alpha: 0.35 },
        ];

        // The themed foreground colour, read from CSS once and cached —
        // getComputedStyle reflows, and this is consulted on every resize tick.
        // (A theme switch mid-session keeps the first-read colour; acceptable
        // for a thin vector overlay.)
        let overlayFg = null;
        function themedForeground() {
          if (!overlayFg) {
            overlayFg = (getComputedStyle(document.documentElement)
              .getPropertyValue('--vscode-foreground') || '#ffffff').trim() || '#ffffff';
          }
          return overlayFg;
        }
        // The editor's background, read and cached the same way: the arrow
        // key's backing (#1046).
        let overlayBg = null;
        function themedBackground() {
          if (!overlayBg) {
            overlayBg = (getComputedStyle(document.documentElement)
              .getPropertyValue('--vscode-editor-background') || '#000000').trim() || '#000000';
          }
          return overlayBg;
        }
        // A theme change rewrites the body's class and the --vscode-* colours:
        // drop both cached colours and repaint, so the key's backing and label
        // follow the new theme. The colour inputs the user did not set are
        // re-seeded from the new foreground first (#1340); a picked colour
        // stays.
        if (typeof MutationObserver === 'function') {
          new MutationObserver(() => {
            overlayFg = null;
            overlayBg = null;
            initLayerColours();
            drawOverlay();
          }).observe(document.body, { attributes: true, attributeFilter: ['class', 'data-vscode-theme-kind'] });
        }

        // Seed each themed layer's colour input from the editor foreground. Only
        // touches an input the user (or restored state) hasn't already set, so a
        // saved colour survives a remount.
        function initLayerColours() {
          for (const layer of OVERLAY_LAYERS) {
            if (!layer.themed) continue;
            const el = document.getElementById('color-' + layer.id);
            if (el && !el.dataset.userSet) el.value = toHexColour(themedForeground());
          }
          // Contours default to the themed foreground too (#238).
          const contour = document.getElementById('color-contours');
          if (contour && !contour.dataset.userSet) contour.value = toHexColour(themedForeground());
          // And the arrows (#241).
          const vectors = document.getElementById('color-vectors');
          if (vectors && !vectors.dataset.userSet) vectors.value = toHexColour(themedForeground());
        }

        // An <input type="color"> only accepts #rrggbb. The themed foreground
        // arrives as whatever CSS holds — usually a hex, sometimes rgb(). Convert
        // what we can and fall back to white rather than letting the input reject
        // the value and silently snap to black.
        function toHexColour(css) {
          const s = String(css).trim();
          if (/^#[0-9a-f]{6}$/i.test(s)) return s;
          if (/^#[0-9a-f]{3}$/i.test(s)) {
            return '#' + s[1] + s[1] + s[2] + s[2] + s[3] + s[3];
          }
          const m = s.match(/^rgba?\\(\\s*(\\d+)\\s*,\\s*(\\d+)\\s*,\\s*(\\d+)/i);
          if (m) {
            const hex = (v) => Math.min(255, Number(v)).toString(16).padStart(2, '0');
            return '#' + hex(m[1]) + hex(m[2]) + hex(m[3]);
          }
          return '#ffffff';
        }

        // Current stroke style per layer, straight from its controls. Read fresh
        // on each draw: restyling is a repaint, never a reprojection.
        function overlayStrokeStyles() {
          const styles = {};
          for (const layer of OVERLAY_LAYERS) {
            const colour = (document.getElementById('color-' + layer.id) || {}).value;
            const width = Number((document.getElementById('width-' + layer.id) || {}).value);
            styles[layer.id] = {
              color: colour || themedForeground(),
              width: Number.isFinite(width) && width > 0 ? width : 1,
              alpha: layer.alpha,
            };
          }
          return styles;
        }

        // Which layers are switched on, keyed by the provider's request flags.
        // None of them for a field that takes no overlays (#840).
        function overlayState() {
          const state = {};
          const offered = overlaysOffered();
          for (const layer of OVERLAY_LAYERS) {
            state[layer.flag] = offered && !!(document.getElementById('overlay-' + layer.flag) || {}).checked;
          }
          return state;
        }

        // Only projection / preset / flip-y / bounds / output size move the
        // overlay's pixel geometry; resampling and range do not. The size (#465)
        // is in the list even though currentOptions() does not set one yet: this
        // cache fails *open*, so an option that moves the raster and is missing
        // here reuses the previous raster's runs over a differently-sized image.
        //
        // A slice's variable and axes are in it too: a slice panel moves onto
        // another grid with the projection unchanged, and the overlay drawn for
        // the last one does not fit it. So is whether overlays apply at all
        // (#840), so that moving onto a field that takes none drops them.
        function overlayKey() {
          const o = currentOptions();
          return JSON.stringify([
            o.projection, o.projectionPreset, o.centerLat, o.centerLon, !!o.flipY,
            o.boundsLatMin, o.boundsLatMax, o.boundsLonMin, o.boundsLonMax,
            o.width, o.height,
            drawnSpec() ? [drawnSpec().variableIndex, drawnSpec().yDim, drawnSpec().xDim] : null,
            overlaysOffered(),
          ]);
        }

        // Everything the projected contour runs depend on: the overlay geometry
        // (contours project through the same path), the used range the levels
        // track, the manual interval, and the field being sliced. Returns null
        // when contours are off or there is no field, so an off-to-off render
        // compares equal and skips. A palette/colormap change moves none of
        // this, so the cached runs stay valid (#336). It also holds the compare
        // rider, since a difference map is a different field (#1149).
        function contourKey() {
          if (!contoursEnabled() || !lastPayload) return null;
          const o = currentOptions();
          const iv = Number((document.getElementById('contour-interval') || {}).value);
          return JSON.stringify([
            o.projection, o.projectionPreset, o.centerLat, o.centerLon, !!o.flipY,
            o.boundsLatMin, o.boundsLatMax, o.boundsLonMin, o.boundsLonMax,
            o.width, o.height,
            lastPayload.usedMin, lastPayload.usedMax,
            Number.isFinite(iv) && iv > 0 ? iv : null,
            drawnSliceFields(),
            // The rider requestContours sends: contours follow the combined
            // field, and under a manual range nothing above moves when
            // Compare or Field B does (#1149).
            drawnCompareRequest(),
          ]);
        }

        // Drop the geographic layers and repaint. The overlay canvas is shared
        // with the contours, so this must go through drawOverlay rather than
        // clearing the bitmap directly: turning off the last geographic layer
        // used to wipe still-enabled contours off the screen, and nothing
        // repainted them until the next resize or control change.
        // drawOverlay clears first and re-strokes whatever survives, so the
        // all-layers-off case still ends with an empty canvas when contours
        // are off too.
        function clearOverlay() {
          lastOverlay = null;
          drawOverlay();
        }

        // Ask the provider to project the enabled layers for the current
        // options. Geometry-only on the Rust side — never re-decodes.
        function requestOverlay() {
          if (!lastPayload) return;
          setOverlayStatus('');
          lastOverlayKey = overlayKey();
          const state = overlayState();
          if (!OVERLAY_LAYERS.some((l) => state[l.flag])) {
            // A reply still in flight was asked for the field this one replaced.
            overlaySeq += 1;
            releaseLineRoom('overlay-status');
            clearOverlay();
            return;
          }
          overlaySeq += 1;
          overlayAsked = overlaySeq;
          vscode.postMessage(Object.assign({
            type: 'overlayRequest',
            seq: overlaySeq,
            options: currentOptions(),
            graticuleSpacing: Math.min(90, Math.max(5,
              Number((document.getElementById('graticule-spacing') || {}).value) || 30)),
          }, state, drawnSliceFields()));
        }

        function handleOverlayReady(msg) {
          // Drop a stale reply (an earlier projection) so it can't be drawn
          // against the current raster.
          if (msg.seq !== overlaySeq) return;
          overlayAsked = 0;
          lastOverlay = msg;
          drawOverlay();
        }

        // A V component left unchosen, as a restore leaves it when its message
        // is gone from a file that changed (#1027), draws no arrows: its value
        // is '', which Number() would read as message 0.
        function vectorsEnabled() {
          const v = document.getElementById('vector-field-v');
          return overlaysOffered() && fieldSupports.vectors === true &&
            !!(document.getElementById('overlay-vectors') || {}).checked &&
            !!v && v.value !== '';
        }

        // The Vectors row's note on the pair (#1049) belongs to the arrows it
        // was said of, so it goes whenever they do.
        function clearVectorPairNote() {
          const pairNote = document.getElementById('vector-pair-note');
          if (pairNote) pairNote.textContent = '';
        }

        // Ask the provider for the arrows of this message paired with the chosen
        // V component, projected onto the current raster. Re-fetched whenever the
        // raster geometry moves, like the contours.
        function requestVectors() {
          // Arrows asked for, or no longer, change whether the image may be
          // stretched (#1251).
          setDisplayAspect(lastPayload);
          if (!lastPayload || !vectorsEnabled()) {
            lastVectors = null;
            // A reply still in flight was asked for arrows no longer wanted.
            vectorSeq += 1;
            // No arrows, so no key for them (#871), and nothing to say about
            // their pair (#1049).
            const scale = document.getElementById('vector-scale');
            if (scale) scale.textContent = '';
            const keyNote = document.getElementById('vector-key-note');
            if (keyNote) keyNote.textContent = '';
            clearVectorPairNote();
            setVectorStatus('');
            releaseLineRoom('vector-status');
            drawOverlay();
            return;
          }
          const v = Number((document.getElementById('vector-field-v') || {}).value);
          if (!Number.isFinite(v)) {
            vectorSeq += 1;
            clearVectorPairNote();
            setVectorStatus('');
            releaseLineRoom('vector-status');
            return;
          }
          // A new pair or spacing is a new question: the last answer's refusal
          // is not about it. A refusal is re-reported when the reply lands.
          setVectorStatus('');
          const spacing = Number((document.getElementById('vector-spacing') || {}).value);
          vectorSeq += 1;
          vectorAsked = vectorSeq;
          vscode.postMessage({
            type: 'vectorRequest',
            seq: vectorSeq,
            messageIndexV: v,
            spacing: Number.isFinite(spacing) && spacing >= 1 ? Math.round(spacing) : undefined,
            gridRelative: UV_GRID_RELATIVE,
            options: currentOptions(),
          });
        }

        function handleVectorResult(msg) {
          // An answer to a request that has since been superseded is dropped.
          if (msg.seq !== vectorSeq) return;
          vectorAsked = 0;
          lastVectors = msg.error ? null : msg;
          // Its own line: a success clears a refusal, and contours can't touch it.
          setVectorStatus(msg.error ? 'Vectors: ' + msg.error : '');
          // What a full-length arrow stands for, in the field's own units: the
          // reference a reader needs to take a speed off the picture. The key
          // on the map draws it (drawVectorKey); the row says it in words.
          const scale = document.getElementById('vector-scale');
          if (scale) {
            scale.textContent = composeVectorScaleNote(
              composeVectorScale(lastVectors && lastVectors.referenceSpeed, UNITS));
          }
          // Whether the two messages are a u/v pair at all (#1049): the arrows
          // are drawn either way, and the row says when they mean nothing.
          const pairNote = document.getElementById('vector-pair-note');
          if (pairNote) {
            const names = (lastVectors && lastVectors.pairNames) || ['', ''];
            pairNote.textContent = lastVectors
              ? composeVectorPairingNote(lastVectors.pairing, names[0], names[1]) : '';
          }
          drawOverlay();
        }

        function contoursEnabled() {
          return contoursOffered() && !!(document.getElementById('overlay-contours') || {}).checked;
        }

        // Ask the provider for the field's contour isolines, projected onto the
        // current raster. Re-fetched on every render (the levels track the used
        // range) as well as on a contour-control change. Off → drop the runs.
        function requestContours() {
          // Clear any prior contour error up front: this covers both turning
          // contours off (below) and issuing a fresh request — a failure gets
          // re-reported by handleContourError when the response comes back.
          setContourStatus('');
          if (!lastPayload || !contoursEnabled() || rangeRefused) {
            // As for the overlay: a reply in flight is for the field replaced.
            contourSeq += 1;
            lastContour = null;
            lastContourKey = null;
            releaseLineRoom('contour-status');
            drawOverlay();
            return;
          }
          // Record the state these runs correspond to, so handleGridReady can
          // skip the re-fetch when a later render doesn't move any of it (#336).
          lastContourKey = contourKey();
          contourSeq += 1;
          contourAsked = contourSeq;
          const iv = Number((document.getElementById('contour-interval') || {}).value);
          vscode.postMessage(Object.assign({
            type: 'contourRequest',
            seq: contourSeq,
            options: currentOptions(),
            // A positive interval overrides the automatic levels; blank = auto.
            interval: Number.isFinite(iv) && iv > 0 ? iv : undefined,
            // Carry the compare rider so a difference map contours the combined
            // field, not field A (#329). The slice and rider of the image on
            // screen, which the contours are drawn over (#1145).
          }, drawnSliceFields(), drawnCompareRequest()));
        }

        function handleContourReady(msg) {
          if (msg.seq !== contourSeq) return; // stale: a newer request superseded it
          contourAsked = 0;
          lastContour = msg;
          drawOverlay();
        }

        // Contours can't be projected for every grid (geostationary scan-angle
        // grids aren't wired); surface why and drop them rather than
        // dead-ending.
        function handleContourError(msg) {
          if (msg.seq !== contourSeq) return;
          contourAsked = 0;
          lastContour = null;
          drawOverlay();
          // Its own line, so the render summary in #status survives (#338).
          setContourStatus('Contours: ' + (msg.error || 'unavailable for this grid'));
        }

        // Contours-only mode hides the colour raster so the isolines read on a
        // blank background; the overlay canvas (contours + coastlines) stays.
        // Only while contours are drawn (#840, #945, #1265): on a field that takes
        // none, or with Contours unticked, the panel would otherwise show nothing
        // at all.
        function contoursOnlyOn() {
          return contoursEnabled() && !!(document.getElementById('contours-only') || {}).checked;
        }
        function applyContoursOnly() {
          const canvas = document.getElementById('canvas');
          if (canvas) canvas.style.visibility = contoursOnlyOn() ? 'hidden' : 'visible';
        }

        // --- Point probe (#172) ---------------------------------------------
        function formatLatLon(lat, lon) {
          if (lat == null || lon == null) return '';
          return Math.abs(lat).toFixed(2) + '°' + (lat >= 0 ? 'N' : 'S') + ', ' +
            Math.abs(lon).toFixed(2) + '°' + (lon >= 0 ? 'E' : 'W');
        }

        // A click maps to an output-raster pixel; ask the provider for the field
        // there. currentOptions() carries the geometry the image was drawn
        // with (#1114), so the readout matches exactly what is on screen.
        // --- Zoom and pan (#245) ---------------------------------------------
        // Only the lat/lon targets render a window a gesture can move. A gesture
        // computes the new window with zoomMapWindow / panMapWindow, writes it
        // into the manual bounds fields, and re-renders through the same path
        // typing those fields takes, so the two cannot disagree. While a gesture
        // is under way, and until its render lands, the current image is moved
        // with a CSS transform, which costs nothing; the real warp happens once
        // it ends.
        //
        // A gesture starts from the window the last gesture asked for, not the
        // one on screen, while that render is on its way: a tick that landed
        // in that time used to zoom from the old window and undo the tick
        // before it (#1228). The preview puts the drawn image where it sits in
        // the window asked for, so the place under the pointer is the place
        // the next gesture anchors on.
        let gesture = null;
        let suppressNextClick = false;
        let pendingZoom = null;
        // The window the last gesture asked for, until the render for it (or a
        // later one) is drawn, and how its preview is placed: a drag's keeps
        // the frame it was dragged in (see previewWindow).
        let gestureWindow = null;
        let gestureView = null;
        // Set while the Bounds ask for a window not known until its render
        // lands: Reset view, or Auto. A gesture then waits for that render,
        // as over a target with no window (#1147), rather than moving the
        // window on screen and undoing the reset (#1259).
        let windowAwaited = false;

        // The window the image on screen was rendered for, when a gesture can
        // move it: Rust echoes it for the lat/lon targets. Read in the
        // projection that image was drawn in, not the picker's, which can
        // have moved on while its render is on its way (#1114). The render a
        // gesture asks for is the picker's, so that must take a window too:
        // a wheel over the old map while a Mollweide render is on its way
        // would tick Bounds Manual for a target that ignores them (#1147).
        function drawnMapWindow() {
          if (!warpsLatLon(drawnProjection()) || !warpsLatLon(requestedProjection())) return null;
          if (!lastPayload || lastPayload.usedLatMin == null) return null;
          return {
            latMin: lastPayload.usedLatMin,
            latMax: lastPayload.usedLatMax,
            lonMin: lastPayload.usedLonMin,
            lonMax: lastPayload.usedLonMax,
          };
        }

        // The window a new gesture moves: the one the last gesture or the
        // Bounds fields asked for while its render is on its way, else the
        // one on screen. None while that render's window is not known yet.
        function currentMapWindow() {
          const drawn = drawnMapWindow();
          if (!drawn || windowAwaited) return null;
          return gestureWindow || drawn;
        }

        function isMercator() {
          return drawnProjection() === 'web_mercator';
        }

        // A place fy of the way down the image, as zoomMapWindow takes it:
        // down from the north edge. With the rows drawn bottom-up (Flip Y
        // axis, flipped) the top of the image is its south, so a fraction
        // down the image is that much up from the south (#1147). The caller
        // passes the drawn flip, read with the window: the pointer is over
        // the image on screen.
        function northFraction(fy, flipped) {
          return flipped ? 1 - fy : fy;
        }

        // view: the preview to show until the render lands, as previewWindow
        // takes it ({ t, ref }); w itself unless given.
        function applyMapWindow(w, view) {
          const manual = document.querySelector('input[name="bounds-mode"][value="manual"]');
          if (manual) manual.checked = true;
          const fields = document.getElementById('bounds-manual-fields');
          if (fields) fields.classList.toggle('spacing-hidden', false);
          const put = (id, v) => {
            const el = document.getElementById(id);
            if (el) el.value = Number(v).toFixed(4);
          };
          put('bounds-lat-min', w.latMin);
          put('bounds-lat-max', w.latMax);
          put('bounds-lon-min', w.lonMin);
          put('bounds-lon-max', w.lonMax);
          gestureWindow = w;
          gestureView = view || { t: w, ref: null };
          previewWindow(gestureView.t, gestureView.ref);
          snapshotState();
          requestRender();
        }

        function resetMapWindow() {
          const auto = document.querySelector('input[name="bounds-mode"][value="auto"]');
          if (auto) auto.checked = true;
          const fields = document.getElementById('bounds-manual-fields');
          if (fields) fields.classList.toggle('spacing-hidden', true);
          // Cleared so the next render pre-fills them with the grid's own extent.
          ['bounds-lat-min', 'bounds-lat-max', 'bounds-lon-min', 'bounds-lon-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.value = '';
          });
          boundsAsked();
          snapshotState();
          requestRender();
        }

        // The Bounds controls now ask for another window. A window typed in
        // the fields is known, so a gesture carries on from it, as from a
        // gesture's own, and the image is moved to where it sits in it (#1259).
        // Auto's is the grid's, known only once its render lands. Web
        // Mercator draws a typed window inside its ±85.0511° band, as Rust
        // clamps it; a pole would put the window's edge at infinity.
        function boundsAsked() {
          clearGesturePreview();
          const target = requestedProjection();
          let typed = warpsLatLon(target) ? typedMapWindow() : null;
          if (typed && target === 'web_mercator') {
            const band = 85.05112877980659;
            const clamp = (lat) => Math.min(band, Math.max(-band, lat));
            typed = Object.assign({}, typed, { latMin: clamp(typed.latMin), latMax: clamp(typed.latMax) });
            if (!(typed.latMax > typed.latMin)) typed = null;
          }
          if (typed) {
            gestureWindow = typed;
            gestureView = { t: typed, ref: null };
            previewWindow(typed);
          } else {
            windowAwaited = true;
          }
        }

        function previewTransform(css, origin) {
          ['canvas', 'overlay'].forEach((id) => {
            const el = document.getElementById(id);
            if (!el) return;
            el.style.transformOrigin = origin || '';
            el.style.transform = css;
          });
        }

        // Move the image on screen to where it sits in window t, as t will be
        // drawn into the same box: scaled by the ratio of the spans, and
        // shifted by where its edges fall in t. Longitude and the image rows'
        // own vertical axis (latitude, or Mercator Y) are linear across the
        // image, so this is exact, slides back from a pole included. The
        // drawn image is taken by whole turns to the copy whose middle is
        // nearest the middle of ref (t unless given): t's west edge is
        // written in [-180, 180), so on a world-wide window edges can be a
        // turn apart when the places are not (#1228). A drag passes the
        // window it started from, with t unwrapped from it, so a drag of
        // more than half a turn does not jump to the other copy.
        function previewWindow(t, ref) {
          const d = drawnMapWindow();
          if (!d || !t) { previewTransform('', ''); return; }
          const merc = isMercator();
          const toY = (lat) => (merc ? Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360)) : lat);
          const tLon = t.lonMax - t.lonMin;
          const tN = toY(t.latMax);
          const tS = toY(t.latMin);
          const dN = toY(d.latMax);
          const dS = toY(d.latMin);
          if (!(tLon > 0) || !(tN > tS)) { previewTransform('', ''); return; }
          const r = ref || t;
          const turns = Math.round(((r.lonMin + r.lonMax) / 2 - (d.lonMin + d.lonMax) / 2) / 360);
          const dWest = d.lonMin + 360 * turns;
          let sx = (d.lonMax - d.lonMin) / tLon;
          let sy = (dN - dS) / (tN - tS);
          let tx = (dWest - t.lonMin) / tLon;
          // The top of the image is its north, or its south on bottom-up rows.
          let ty = drawnFlipY() ? (dS - tS) / (tN - tS) : (tN - dN) / (tN - tS);
          // Gestures chained over renders still on their way can ask for a
          // scale far past the wheel's own limit; past 64 either way the
          // image is held at that, about the middle of the box.
          const held = (sc, tr) => {
            const capped = Math.min(64, Math.max(1 / 64, sc));
            return [capped, 0.5 - (capped * (0.5 - tr)) / sc];
          };
          [sx, tx] = held(sx, tx);
          [sy, ty] = held(sy, ty);
          previewTransform(
            'translate(' + tx * 100 + '%, ' + ty * 100 + '%) scale(' + sx + ', ' + sy + ')',
            '0 0',
          );
        }

        // Preview what is under way: a wheel burst or a drag, else the last
        // gesture's window. False when there is nothing to preview.
        function refreshPreview() {
          if (pendingZoom) { previewWindow(pendingZoom.target()); return true; }
          if (gesture && gesture.moved && gesture.preview) { previewWindow(gesture.preview, gesture.from); return true; }
          if (gestureWindow && gestureView) { previewWindow(gestureView.t, gestureView.ref); return true; }
          return false;
        }

        // A wheel burst still settling is asked for now, so a gesture or a
        // button that starts meanwhile carries on from it rather than from
        // the window before it (#1228).
        function flushPendingZoom() {
          const z = pendingZoom;
          if (!z) return;
          clearTimeout(z.timer);
          pendingZoom = null;
          if (currentMapWindow()) applyMapWindow(z.target());
        }

        // A render was drawn. The answer to the last request ends the wait for
        // the gesture's window; an older one, with a gesture's render still on
        // its way, is moved to where it sits in that window.
        function settleGesturePreview(payload) {
          const latest = !payload || typeof payload.seq !== 'number' || payload.seq >= renderSeq;
          if (latest) {
            answeredSeq = renderSeq;
            gestureWindow = null;
            gestureView = null;
            windowAwaited = false;
          }
          // An image no gesture can move. Only the latest answer ends what
          // the page waits for: a stale one, such as a Mollweide render
          // landing ahead of a Reset view's, leaves the window asked for, or
          // the wait for one, to the renders still on their way (#1259).
          if (!drawnMapWindow()) {
            if (latest) clearGesturePreview();
            else previewTransform('', '');
            return;
          }
          if (!refreshPreview()) previewTransform('', '');
        }

        // A wheel burst or a drag ended with no window to write. Only it is
        // dropped: the window asked for before it, or the wait for one,
        // stays for the latest render to end, as a stale image with no
        // window can be on screen meanwhile (#1259).
        function dropGesture() {
          if (!refreshPreview()) previewTransform('', '');
        }

        function clearGesturePreview() {
          if (pendingZoom && pendingZoom.timer) clearTimeout(pendingZoom.timer);
          pendingZoom = null;
          gesture = null;
          gestureWindow = null;
          gestureView = null;
          windowAwaited = false;
          previewTransform('', '');
          const wrap = document.querySelector('.canvas-wrap');
          if (wrap) wrap.classList.remove('dragging');
        }

        // The image's box as laid out, without the preview's transform: the
        // box the window a gesture moves fills (#1228).
        function imageBox() {
          const canvas = document.getElementById('canvas');
          if (!canvas) return null;
          const moved = canvas.style.transform;
          canvas.style.transform = 'none';
          const rect = canvas.getBoundingClientRect();
          canvas.style.transform = moved;
          return rect;
        }

        // Where a client point sits on the image's box, as fractions from its
        // left and top edges, or null when there is no box.
        function imageFraction(clientX, clientY) {
          const rect = imageBox();
          if (!rect || rect.width <= 0 || rect.height <= 0) return null;
          return {
            fx: (clientX - rect.left) / rect.width,
            fy: (clientY - rect.top) / rect.height,
            width: rect.width,
            height: rect.height,
          };
        }

        function onWheel(ev) {
          // A wheel turned mid-drag is not taken: the drag's window is not
          // asked for yet, so a zoom could only start from the one before
          // it, and the drag would then undo the zoom (#1228).
          if (gesture && gesture.moved) { ev.preventDefault(); return; }
          // Over a map no wheel zooms, the page scrolls: a target with no
          // window drawn, or asked for while a lat/lon image is still up
          // (#1147), as it will once that render lands. A lat/lon map picked
          // and still on its way over one with no window is zoomed once it
          // lands, so the wheel over the image is held meanwhile (#1272). A
          // target held while the picker is put away (#1099) never holds it:
          // no lat/lon map is coming from that.
          const drawn = drawnMapWindow();
          const coming = !drawn && heldProjection === null && answeredSeq < renderSeq &&
            warpsLatLon(requestedProjection()) && !warpsLatLon(drawnProjection());
          if (!drawn && !coming) return;
          const at = imageFraction(ev.clientX, ev.clientY);
          if (!at || at.fx < 0 || at.fx > 1 || at.fy < 0 || at.fy > 1) return;
          ev.preventDefault();
          if (!drawn) return;
          // The window asked for is not known until its render lands (Reset
          // view, or Auto): the wheel is held back, not let through to
          // scroll the page out from under the pointer (#1269).
          const w = currentMapWindow();
          if (!w) return;
          if (!pendingZoom) {
            const z = { from: w, flipped: drawnFlipY(), mercator: isMercator(), factor: 1, fx: at.fx, fy: at.fy, timer: 0 };
            z.target = () => zoomMapWindow(z.from, z.mercator, z.fx, northFraction(z.fy, z.flipped), z.factor);
            pendingZoom = z;
          }
          // A notch of a mouse wheel is about 100 units: roughly 20% per notch,
          // and trackpads' smaller deltas scale smoothly.
          pendingZoom.factor = Math.min(64, Math.max(1 / 64, pendingZoom.factor * Math.pow(1.002, ev.deltaY)));
          previewWindow(pendingZoom.target());
          clearTimeout(pendingZoom.timer);
          pendingZoom.timer = setTimeout(() => {
            const z = pendingZoom;
            pendingZoom = null;
            if (!z) return;
            // The picker can have left the lat/lon targets while the wheel
            // settled, or a stale image with no window be on screen; then
            // there is no window to write.
            if (!currentMapWindow()) { dropGesture(); return; }
            applyMapWindow(z.target());
          }, 250);
        }

        function onMouseDown(ev) {
          // A press starts a new click. A drag released outside the image
          // fires no click to use up the suppression its release set, which
          // would otherwise swallow this press's click and its probe.
          suppressNextClick = false;
          if (ev.button !== 0 || !currentMapWindow()) return;
          const at = imageFraction(ev.clientX, ev.clientY);
          if (!at) return;
          flushPendingZoom();
          const w = currentMapWindow();
          if (!w) return;
          gesture = {
            x: ev.clientX, y: ev.clientY, width: at.width, height: at.height, moved: false,
            from: w, flipped: drawnFlipY(), mercator: isMercator(), target: null, preview: null,
          };
        }

        // The window a drag to the client point (x, y) asks for. A drag down
        // moves the map south on a north-up image and north on a flipped one:
        // the place grabbed follows the pointer (#1147).
        function dragTarget(g, x, y) {
          const dfy = (y - g.y) / g.height;
          return panMapWindow(g.from, g.mercator, (x - g.x) / g.width, g.flipped ? -dfy : dfy);
        }

        // The same window with its longitudes kept in the frame of the one
        // the drag started from, for the preview: panMapWindow writes the
        // west edge in [-180, 180).
        function dragPreview(g, target, x) {
          const span = g.from.lonMax - g.from.lonMin;
          const lonMin = g.from.lonMin - ((x - g.x) / g.width) * span;
          return { latMin: target.latMin, latMax: target.latMax, lonMin: lonMin, lonMax: lonMin + span };
        }

        function onMouseMove(ev) {
          if (!gesture) return;
          const dx = ev.clientX - gesture.x;
          const dy = ev.clientY - gesture.y;
          // A few pixels of wobble is still a click, which probes.
          if (!gesture.moved && Math.hypot(dx, dy) < 4) return;
          if (!gesture.moved && pendingZoom) {
            // A wheel turned between the press and the drag: the drag
            // carries on from that zoom.
            flushPendingZoom();
            const w = currentMapWindow();
            if (!w) { gesture = null; return; }
            gesture.from = w;
          }
          gesture.moved = true;
          const wrap = document.querySelector('.canvas-wrap');
          if (wrap) wrap.classList.add('dragging');
          gesture.target = dragTarget(gesture, ev.clientX, ev.clientY);
          gesture.preview = dragPreview(gesture, gesture.target, ev.clientX);
          previewWindow(gesture.preview, gesture.from);
        }

        function onMouseUp(ev) {
          if (!gesture) return;
          const g = gesture;
          gesture = null;
          const wrap = document.querySelector('.canvas-wrap');
          if (wrap) wrap.classList.remove('dragging');
          if (!g.moved) return;
          suppressNextClick = true;
          if (!currentMapWindow()) { dropGesture(); return; }
          const target = dragTarget(g, ev.clientX, ev.clientY);
          applyMapWindow(target, { t: dragPreview(g, target, ev.clientX), ref: g.from });
        }

        function zoomButton(factor) {
          flushPendingZoom();
          const w = currentMapWindow();
          if (w) applyMapWindow(zoomMapWindow(w, isMercator(), 0.5, 0.5, factor));
        }

        function requestProbe(clientX, clientY) {
          if (!lastPayload || !lastPayload.width) return;
          const canvas = document.getElementById('canvas');
          const rect = canvas && canvas.getBoundingClientRect();
          if (!rect || rect.width <= 0 || rect.height <= 0) return;
          const px = Math.floor((clientX - rect.left) / rect.width * lastPayload.width);
          const py = Math.floor((clientY - rect.top) / rect.height * lastPayload.height);
          if (px < 0 || py < 0 || px >= lastPayload.width || py >= lastPayload.height) return;
          // Carry the compare rider so a click on a difference map reads the
          // combined field, not field A (#329). The slice and the rider are
          // the image's on screen, not the pickers' (#1145).
          vscode.postMessage(Object.assign(
            { type: 'probeRequest', seq: ++probeSeq, px: px, py: py, options: currentOptions() },
            drawnSliceFields(), drawnCompareRequest()));
        }

        function handleProbeResult(msg) {
          if (msg.seq !== probeSeq) return; // stale: an older click or render
          const el = document.getElementById('probe');
          if (!el) return;
          const r = msg.result;
          if (!r) { el.textContent = ''; probedCell = null; return; }
          const coord = formatLatLon(r.lat, r.lon);
          const value = composeProbeValue(r, UNITS);
          const grid = (r.gridI != null && r.gridJ != null)
            ? ' · grid ' + r.gridI + ',' + r.gridJ : '';
          el.textContent = (coord ? coord + ' · ' : '') + value + grid;
          offerLine(r);
        }

        // --- Line through the probed cell (#172) -----------------------------
        // A slice panel only: a GRIB message has no axis to read along. The cell
        // a probe resolved is remembered, so picking another axis re-reads the
        // same cell rather than asking for a new click.
        //
        // The plot is drawn only while the Line plot box is ticked (#1037),
        // which starts unticked: a click is usually meant to read a value, and
        // with the box off it does only that, as in a GRIB panel. probedCell
        // is the cell the readout on screen describes, kept while the box is
        // off, so ticking it plots that cell without another click.
        let lineCell = null;
        let probedCell = null;
        // The line on screen, so a resize can draw it again at the new width.
        let lastLine = null;
        // What the plot asked for last is of: on a Compare map, the operation,
        // as the status line names it (#1208), and nothing for one field.
        let lineOf = '';

        // The operation a request's compare rider names, as its menu entry
        // reads, or '' for one field.
        function compareOpLabel(rider) {
          const op = rider && rider.compare ? rider.compare.op : '';
          const opEl = document.getElementById('compare-op');
          if (!op || !opEl) return '';
          const entry = Array.from(opEl.options).find((o) => o.value === op);
          return entry ? entry.text : '';
        }

        // Put the plot away. The readout's cell is kept, so the plot can come
        // back for it; hideLine forgets that too, for a readout cleared.
        function putLineAway() {
          lineCell = null;
          lastLine = null;
          const panel = document.getElementById('line-panel');
          if (panel) panel.hidden = true;
        }

        function hideLine() {
          putLineAway();
          probedCell = null;
        }

        // Whether a click draws the line through its cell: the box is ticked
        // and the variable has an axis to plot along.
        function lineOn() {
          const toggle = document.getElementById('line-toggle');
          const control = document.getElementById('line-control');
          return !!(toggle && toggle.checked && control && !control.hidden);
        }

        // Every axis with more than one step. A non-horizontal one first — a
        // time series or a vertical profile, which is what the plot is for — but
        // the horizontal axes too, where a line is a transect through the cell.
        // They are the drawn slice's, since the cell plotted is a cell of the
        // image on screen (#1145).
        function lineAxes() {
          const spec = drawnSpec();
          if (!spec) return [];
          const v = sliceVariable(spec.variableIndex);
          if (!v) return [];
          const axes = v.dims
            .map((d, i) => ({ i: i, name: d.name, length: d.length }))
            .filter((d) => d.length > 1);
          const held = axes.filter((d) => d.i !== spec.yDim && d.i !== spec.xDim);
          const horizontal = axes.filter((d) => d.i === spec.yDim || d.i === spec.xDim);
          return held.concat(horizontal);
        }

        // Offer the slice's axes in the line plot's picker, keeping the one
        // chosen. Filled when the slice is set up as well as on a click, so a
        // panel written again shows the saved axis before anything is probed.
        // Returns whether there is an axis to plot along.
        //
        // The Line plot box is hidden for a variable with no such axis, a
        // single cell, and only there (#1037): plotting is the user's choice,
        // not something a grid rules out, so it is not in the "Not available"
        // line, and the box keeps its state for the next variable.
        function fillLineAxes() {
          const axes = lineAxes();
          const control = document.getElementById('line-control');
          if (control) control.hidden = axes.length === 0;
          const sel = document.getElementById('line-axis');
          if (!sel) return false;
          // The picker's label names what is plotted: the variable, through
          // the clicked point, across the axis chosen (#1037).
          const name = document.getElementById('line-variable');
          const spec = drawnSpec();
          const v = spec ? sliceVariable(spec.variableIndex) : null;
          if (name) name.textContent = v && v.name ? v.name : 'Values';
          const previous = sel.value !== '' ? sel.value : preferredLineAxis;
          sel.innerHTML = axes
            .map((d) => '<option value="' + d.i + '">' + escapeAttr(d.name) + '</option>')
            .join('');
          if (axes.some((d) => String(d.i) === previous)) sel.value = previous;
          return axes.length > 0;
        }

        function offerLine(r) {
          probedCell = SLICE && r.gridI != null && r.gridJ != null ? { gridI: r.gridI, gridJ: r.gridJ } : null;
          // With the box off a click only reads the value (#1037), and a
          // zonal average on screen stays.
          if (!lineOn()) return;
          if (!probedCell) { hideLine(); return; }
          if (!fillLineAxes()) { hideLine(); return; }
          lineCell = probedCell;
          // A click asks for this cell's line, which replaces a zonal mean. The
          // click is outside the toolbar, so the untick saves itself (#1027).
          // A Zonal average box hidden for this grid shows nothing, and keeps
          // its value for the next field that takes it (#1036).
          const toggle = document.getElementById('zonal-toggle');
          if (toggle && zonalOn()) {
            toggle.checked = false;
            snapshotState();
          }
          setAxisPickerHidden(false);
          requestLine();
        }

        function requestLine() {
          const sel = document.getElementById('line-axis');
          if (!lineCell || !sel || sel.value === '') return;
          // On a Compare map, the line through the map drawn (#1208).
          const rider = drawnCompareRequest();
          lineOf = compareOpLabel(rider);
          vscode.postMessage(Object.assign({
            type: 'lineRequest',
            seq: ++lineSeq,
            alongDim: Number(sel.value),
            gridI: lineCell.gridI,
            gridJ: lineCell.gridJ,
            // The slice the probed cell is a cell of (#1145).
            slice: drawnSpec(),
          }, rider));
        }

        {
          const toggle = document.getElementById('line-toggle');
          if (toggle) toggle.addEventListener('change', () => {
            // Outside the toolbar, so it snapshots for itself (#1027).
            snapshotState();
            if (toggle.checked) {
              // The cell the readout describes, if any, is plotted at once.
              if (probedCell) offerLine(probedCell);
            } else if (lineCell) {
              // The cell's line goes, but not a zonal average.
              putLineAway();
            }
          });
        }

        {
          const sel = document.getElementById('line-axis');
          if (sel) sel.addEventListener('change', () => {
            preferredLineAxis = sel.value;
            // The line plot sits outside the toolbar, whose listener snapshots
            // every other control.
            snapshotState();
            requestLine();
          });
        }

        const SVG_NS = 'http://www.w3.org/2000/svg';
        function svgEl(tag, attrs) {
          const el = document.createElementNS(SVG_NS, tag);
          for (const k in attrs) el.setAttribute(k, String(attrs[k]));
          return el;
        }

        function setAxisPickerHidden(hidden) {
          const label = document.getElementById('line-axis-label');
          if (label) label.hidden = hidden;
        }

        function handleLineResult(msg) {
          if (msg.seq !== lineSeq) return;
          if (!lineCell) return;
          const alongDim = Number(document.getElementById('line-axis').value);
          const spec = drawnSpec();
          // Where the cell sits along the axis plotted: the index held on a
          // held axis, and the cell's own row or column along one of the
          // image's axes, a transect through it (#1386).
          let here = null;
          if (spec) {
            if (alongDim === spec.yDim) here = lineCell.gridJ;
            else if (alongDim === spec.xDim) here = lineCell.gridI;
            else here = spec.sliceIndices[alongDim];
          }
          drawLine(msg.result, here, 'No line through this cell.', lineOf);
        }

        // --- Zonal average (#240) --------------------------------------------
        // Each row's mean over longitude, drawn with the same plot as a cell's
        // line. It describes the whole field, so it has no cell and no axis to
        // pick, and a re-render asks for it again rather than clearing it.
        // Only for a field whose rows are circles of latitude (#945): on any
        // other the toggle is hidden, with the reason in the "Not available"
        // line (#1036), and keeps its state for the next field that takes it.
        function zonalOn() {
          const toggle = document.getElementById('zonal-toggle');
          return fieldSupports.zonalMean === true && !!(toggle && toggle.checked);
        }

        function requestZonal() {
          if (awaitingPick) return;
          // Of the field on screen (#1145), a Compare map's combined field
          // included (#1208).
          const rider = drawnCompareRequest();
          lineOf = compareOpLabel(rider);
          vscode.postMessage(Object.assign({ type: 'zonalRequest', seq: ++lineSeq }, drawnSliceFields(), rider));
        }

        {
          const toggle = document.getElementById('zonal-toggle');
          if (toggle) toggle.addEventListener('change', () => {
            // Outside the toolbar, so it snapshots for itself (#1027).
            snapshotState();
            if (zonalOn()) {
              zonalReplacesLine();
              requestZonal();
            } else {
              putLineAway();
            }
          });
        }

        // The zonal average takes the plot from a cell's line, so the Line plot
        // box unticks, as a click's line unticks Zonal average (offerLine): each
        // replaces the other (#1234). The untick is outside the toolbar, so it
        // saves itself (#1027). A Line plot box hidden for this variable keeps
        // its tick, as a hidden Zonal average box does (#1036).
        function zonalReplacesLine() {
          const toggle = document.getElementById('line-toggle');
          if (lineCell && lineOn()) {
            toggle.checked = false;
            snapshotState();
          }
          lineCell = null;
          setAxisPickerHidden(true);
        }

        function handleZonalResult(msg) {
          if (msg.seq !== lineSeq) return;
          if (!zonalOn()) return;
          setAxisPickerHidden(true);
          // A refusal says why — a rotated or projected grid has no latitude
          // circles to average along — rather than leaving an empty plot.
          const reason = msg.error ? 'No zonal average: ' + msg.error : 'No zonal average for this field.';
          drawLine(msg.result, null, reason, lineOf);
        }

        // Draw one line into the panel: a cell's profile or a field's zonal mean.
        // markIndex is the point to rule a dashed line through — where the
        // slice on screen sits — or null for none. The last argument names
        // the operation of a Compare map's line, which the caption leads with.
        function drawLine(line, markIndex, emptyText, of) {
          const panel = document.getElementById('line-panel');
          const svg = document.getElementById('line-plot');
          const caption = document.getElementById('line-caption');
          if (!panel || !svg || !caption) return;
          panel.hidden = false;
          while (svg.firstChild) svg.removeChild(svg.firstChild);
          // Kept before anything is drawn, the empty answer included, so a resize
          // redraws what is on screen rather than the line before it.
          lastLine = { line: line, markIndex: markIndex, emptyText: emptyText, of: of };
          const lead = of ? of + ' · ' : '';
          // The size this plot is drawn for, as the rails record theirs, once
          // its class (a profile is taller) has settled that size.
          const fit = () => { svg.dataset.fitted = sizeKey(svg.getBoundingClientRect()); };
          if (!line) { svg.classList.remove('profile'); fit(); caption.textContent = emptyText; return; }

          const n = line.values.length;
          // Position each point by its coordinate when the axis has one, and by
          // its index otherwise — an axis with a hole in its coordinates arrives
          // without them, rather than with a gap to invent across.
          const indices = line.values.map((_, i) => i);
          const hasCoordinates = !!(line.coordinates && line.coordinates.length === n);
          const xs = hasCoordinates ? line.coordinates : indices;
          const present = [];
          for (let i = 0; i < n; i++) {
            if (line.mask[i] === 1 && Number.isFinite(line.values[i])) present.push(i);
          }
          const lineUnits = displayUnits(line.units);
          const units = lineUnits ? ' ' + lineUnits : '';
          const alongUnits = displayUnits(line.coordinateUnits);
          const along = line.dimension + (alongUnits ? ' (' + alongUnits + ')' : '');
          // A vertical axis (CF §4.3) is drawn as a profile: the coordinate up
          // the page and the value across it, the way up its positive attribute says
          // (#1043). A pressure is spaced by its logarithm, so the upper
          // levels are not squeezed into the top of the plot.
          const vertical = hasCoordinates ? line.coordinateVertical || null : null;
          svg.classList.toggle('profile', !!vertical);
          fit();
          if (!present.length) {
            caption.textContent = lead + (line.variable || 'Values') + ' across ' + along + ': no data at this cell.';
            return;
          }

          const width = svg.clientWidth || 400;
          const height = svg.clientHeight || 160;
          svg.setAttribute('viewBox', '0 0 ' + width + ' ' + height);
          const logScale = !!(vertical && vertical.logPressure && xs.every((x) => x > 0));
          const scaled = (x) => (logScale ? Math.log(x) : x);
          let cmin = Math.min.apply(null, xs.map(scaled)), cmax = Math.max.apply(null, xs.map(scaled));
          let vmin = line.min, vmax = line.max;
          if (!(Number.isFinite(vmin) && Number.isFinite(vmax))) {
            vmin = Math.min.apply(null, present.map((i) => line.values[i]));
            vmax = Math.max.apply(null, present.map((i) => line.values[i]));
          }
          // The range the caption states, before any padding below.
          const range = composeLineRange(Number(vmin), Number(vmax));
          // A flat line or a single point still needs a span to draw into.
          if (cmax === cmin) { cmin -= 0.5; cmax += 0.5; }
          if (vmax === vmin) { vmin -= 0.5; vmax += 0.5; }

          // The ticks: round numbers on the value axis, and on the other,
          // round coordinates where it is a run of numbers (#1229) and round
          // dates on a time (#1392), read the way the cross-section's ticks
          // are; a pressure's levels and an axis's indices tick at points of
          // the line instead.
          // Measured before the plot area is fixed, so the left margin fits
          // the widest label down the side.
          const coordinateLabel = (x) => (hasCoordinates ? formatAxisValue(x, line.coordinateUnits) : String(x));
          const measured = new Map();
          const measure = (text) => {
            if (measured.has(text)) return measured.get(text);
            const t = svgEl('text', { class: 'axis-label', x: 0, y: 0 });
            t.textContent = text;
            svg.appendChild(t);
            const box = t.getBBox();
            svg.removeChild(t);
            const size = { w: box.width, h: box.height || 10 };
            measured.set(text, size);
            return size;
          };
          const valueTicks = niceTicks(Number(vmin), Number(vmax), 5)
            .map((v) => ({ at: v, text: formatAxisValue(v, null) }));
          // As many coordinate ticks as fit with room between them; the
          // layout below drops any that still overlap.
          const extent = vertical ? height : width - 40;
          const roomFor = (sizes) => (vertical
            ? Math.max(...sizes.map((s) => s.h)) + 6
            : Math.max(...sizes.map((s) => s.w)) + 24);
          const coordinateAxis = hasCoordinates
            ? { coordinates: xs, units: line.coordinateUnits, vertical: vertical }
            : null;
          // Round coordinates: the most whose labels fit between their
          // neighbours, judged by the closest two, since months are not all
          // one length. Failing that, the fewest there are.
          let candidates = null;
          let fewest = null;
          for (let want = 12; want >= 2 && candidates === null; want--) {
            const nice = coordinateTicks(coordinateAxis, want);
            if (!nice) break;
            const step = Math.min(...nice.slice(1).map((t, k) => Math.abs(t.value - nice[k].value)));
            if ((extent * step) / (cmax - cmin) >= roomFor(nice.map((t) => measure(t.text)))) candidates = nice;
            fewest = nice;
          }
          candidates = candidates || fewest;
          if (candidates) {
            candidates = candidates.map((t) => ({ at: t.value, text: t.text }));
          } else {
            // Points of the line, judged by the end labels.
            const room = roomFor([measure(coordinateLabel(xs[0])), measure(coordinateLabel(xs[n - 1]))]);
            candidates = axisTickIndices(n, Math.max(2, Math.floor(extent / room)))
              .map((i) => ({ at: scaled(xs[i]), text: coordinateLabel(xs[i]) }));
          }
          const yTicks = (vertical ? candidates : valueTicks).map((t) => Object.assign(t, measure(t.text)));
          const xTicks = (vertical ? valueTicks : candidates).map((t) => Object.assign(t, measure(t.text)));
          const labelH = Math.max(10, ...yTicks.concat(xTicks).map((t) => t.h));
          const pad = {
            l: Math.ceil(Math.max(0, ...yTicks.map((t) => t.w))) + 9,
            r: 10,
            t: Math.ceil(labelH / 2) + 4,
            b: Math.ceil(labelH) + 8,
          };
          const plotW = Math.max(1, width - pad.l - pad.r);
          const plotH = Math.max(1, height - pad.t - pad.b);
          const bottom = pad.t + plotH;
          // Each axis maps its value to a fraction of the plot, 0 at the left
          // or the bottom. A down axis runs the other way up the page.
          const coordinateFraction = (x) => (scaled(x) - cmin) / (cmax - cmin);
          const valueFraction = (v) => (v - vmin) / (vmax - vmin);
          // A Z axis nothing orients is drawn the way a plot's own y axis runs.
          const up = !vertical || vertical.positive !== 'down';
          const toY = (f) => bottom - f * plotH;
          const toX = (f) => pad.l + f * plotW;
          const point = (i) => vertical
            ? { x: toX(valueFraction(line.values[i])), y: toY(up ? coordinateFraction(xs[i]) : 1 - coordinateFraction(xs[i])) }
            : { x: toX(coordinateFraction(xs[i])), y: toY(valueFraction(line.values[i])) };
          // A tick's position along its own axis, from the value it labels.
          const xTickAt = (t) => toX(vertical ? (t.at - vmin) / (vmax - vmin) : (t.at - cmin) / (cmax - cmin));
          const yTickAt = (t) => {
            const f = vertical ? (t.at - cmin) / (cmax - cmin) : (t.at - vmin) / (vmax - vmin);
            return toY(vertical && !up ? 1 - f : f);
          };

          // The two axis lines, then each axis's ticks and labels, fitted so
          // that none overlaps another or leaves its side of the plot.
          svg.appendChild(svgEl('path', {
            class: 'axis', d: 'M ' + pad.l + ' ' + pad.t + ' V ' + bottom + ' H ' + (pad.l + plotW),
          }));
          const yStarts = layoutAxisLabels(yTicks.map((t) => ({ centre: yTickAt(t), size: t.h })), 0, bottom + labelH / 2, 2);
          yTicks.forEach((t, k) => {
            if (yStarts[k] === null) return;
            const y = yTickAt(t);
            svg.appendChild(svgEl('line', { class: 'axis', x1: pad.l - 4, x2: pad.l, y1: y, y2: y }));
            const label = svgEl('text', {
              class: 'axis-label tick-label', 'data-axis': 'y', 'text-anchor': 'end',
              'dominant-baseline': 'central', x: pad.l - 6, y: (yStarts[k] + t.h / 2).toFixed(1),
            });
            label.textContent = t.text;
            svg.appendChild(label);
          });
          const xStarts = layoutAxisLabels(xTicks.map((t) => ({ centre: xTickAt(t), size: t.w })), pad.l, width - 2, 8);
          xTicks.forEach((t, k) => {
            if (xStarts[k] === null) return;
            const x = xTickAt(t);
            svg.appendChild(svgEl('line', { class: 'axis', x1: x, x2: x, y1: bottom, y2: bottom + 4 }));
            const label = svgEl('text', {
              class: 'axis-label tick-label', 'data-axis': 'x', 'text-anchor': 'start',
              'dominant-baseline': 'hanging', x: xStarts[k].toFixed(1), y: bottom + 6,
            });
            label.textContent = t.text;
            svg.appendChild(label);
          });

          // The slice on screen, marked where it sits along this axis: across
          // a profile, upright on any other line.
          const here = markIndex;
          if (here != null && here < n) {
            if (vertical) {
              const y = point(here).y;
              svg.appendChild(svgEl('line', { class: 'rule', x1: pad.l, x2: pad.l + plotW, y1: y, y2: y }));
            } else {
              const x = toX(coordinateFraction(xs[here]));
              svg.appendChild(svgEl('line', { class: 'rule', x1: x, x2: x, y1: pad.t, y2: bottom }));
            }
          }

          // Broken at a masked point: a gap in the data is a gap in the line,
          // not a straight stroke drawn across it.
          let d = '';
          let open = false;
          for (let i = 0; i < n; i++) {
            if (line.mask[i] !== 1 || !Number.isFinite(line.values[i])) { open = false; continue; }
            const p = point(i);
            d += (open ? ' L ' : ' M ') + p.x.toFixed(1) + ' ' + p.y.toFixed(1);
            open = true;
          }
          svg.appendChild(svgEl('path', {
            class: 'trace', d: d.trim(), fill: 'none', stroke: 'currentColor', 'stroke-width': 1.5,
            'vector-effect': 'non-scaling-stroke',
          }));
          // Points too, when there are few enough to see: a two-step axis is
          // otherwise a bare segment with no sign of where the samples are.
          if (present.length <= 60) {
            present.forEach((i) => {
              const p = point(i);
              svg.appendChild(svgEl('circle', {
                cx: p.x.toFixed(1), cy: p.y.toFixed(1), r: 2.5, fill: 'currentColor', 'data-index': i,
              }));
            });
          }

          // A line with no name (#775) is captioned as values, not as nothing.
          // The range is written with "to" and a true minus (#1037), and
          // "across" as the axis picker's label has it.
          caption.textContent = lead + (line.variable || 'Values') + ' across ' + along + ' · ' +
            range + units +
            ' · ' + present.length + ' of ' + n + ' points';
        }

        // Stroke the projected runs onto the overlay canvas. The overlay's
        // backing store is sized to the image's *displayed* pixels (× DPR) so
        // lines stay crisp instead of inheriting the image's pixelated upscale;
        // raster-space coordinates from Rust are scaled to that size here.
        // The first argument is ignored (a resize event lands there).
        // \`into\` ({ ctx, width, height }) draws the layers, and no key, onto
        // that canvas context at the PNG's own size instead (#1366): an export
        // scales the panel-sized overlay down onto a small raster, where a
        // hairline becomes a faint blend. Line weights there are the panel's
        // CSS weights carried over to the PNG, floored at EXPORT_MIN_STROKE so
        // a stroke covers a whole pixel somewhere along its length.
        function drawOverlay(_event, into) {
          const o = document.getElementById('overlay');
          const ctx = into ? into.ctx : o && o.getContext('2d');
          if (!o || !ctx || !lastPayload) {
            // Nothing is drawn, so no key, and nothing to say about one.
            const keyNote = document.getElementById('vector-key-note');
            if (keyNote) keyNote.textContent = '';
            return;
          }
          const screenDpr = window.devicePixelRatio || 1;
          // Size the backing store to the overlay's *content* box (the canvas
          // bitmap fills the content box, inside its border) so a raster pixel
          // maps exactly onto the same spot as the image's content box — no
          // ~1px border offset. clientWidth/Height exclude the border.
          if (!into) {
            o.width = Math.max(1, Math.round(o.clientWidth * screenDpr));
            o.height = Math.max(1, Math.round(o.clientHeight * screenDpr));
          }
          const cw0 = into ? into.width : o.width;
          const ch0 = into ? into.height : o.height;
          ctx.clearRect(0, 0, cw0, ch0);
          if (!lastPayload.width) return;
          const sx = cw0 / lastPayload.width;
          const sy = ch0 / lastPayload.height;
          // A CSS line weight in this canvas's pixels.
          const px = (w) => into
            ? Math.max(EXPORT_MIN_STROKE, o.clientWidth > 0 ? w * into.width / o.clientWidth : w)
            : w * screenDpr;
          // Stroke one run set (flat xy + per-run vertex counts) as a path.
          const strokeRuns = (xy, segs) => {
            ctx.beginPath();
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
          };
          // Arrows sit with the contours, under the geographic overlays.
          if (lastVectors && lastVectors.xy && vectorsEnabled()) {
            const colour = (document.getElementById('color-vectors') || {}).value
              || themedForeground();
            const vw = Number((document.getElementById('width-vectors') || {}).value);
            ctx.save();
            ctx.strokeStyle = colour;
            ctx.lineWidth = px(Number.isFinite(vw) && vw > 0 ? vw : 1);
            ctx.lineJoin = 'round';
            strokeRuns(lastVectors.xy || [], lastVectors.segLengths || []);
            ctx.restore();
          }
          // Contours sit under the geographic overlays so a coastline reads on
          // top of them.
          if (lastContour && lastContour.xy && contoursEnabled()) {
            const colour = (document.getElementById('color-contours') || {}).value
              || themedForeground();
            const cw = Number((document.getElementById('width-contours') || {}).value);
            ctx.save();
            ctx.globalAlpha = 0.9;
            ctx.strokeStyle = colour;
            ctx.lineWidth = px(Number.isFinite(cw) && cw > 0 ? cw : 0.8);
            strokeRuns(lastContour.xy || [], lastContour.segLengths || []);
            ctx.restore();
          }
          if (lastOverlay && lastOverlay.layers && overlaysOffered()) {
            const styles = overlayStrokeStyles();
            for (const layer of lastOverlay.layers) {
              // The provider labels each run with the layer id the styles key
              // on; an unknown label falls back to the coastline's style rather
              // than going unstroked.
              const style = styles[layer.name] || styles.coastline;
              ctx.save();
              ctx.globalAlpha = style.alpha || 1;
              ctx.strokeStyle = style.color;
              ctx.lineWidth = px(style.width);
              // Tolerate either typed arrays or plain arrays from the serializer.
              strokeRuns(layer.xy || [], layer.segLengths || []);
              ctx.restore();
            }
          }
          // The arrow key goes on top of every layer, so nothing hides it. An
          // export leaves it off and draws its own at the PNG's resolution.
          if (into) return;
          drawVectorKey(ctx, sx, sy, screenDpr);
        }

        // The arrow key (#1046): one full-length arrow, in the arrows' colour
        // and weight, labelled with the speed it stands for, in the map's
        // lower-left corner. Its length is not worked out here: Rust draws
        // every arrow at one on-screen length per speed and reports a
        // full-speed arrow's (\`referenceLength\`, in raster pixels), and it is
        // scaled onto the canvas by the same \`sx\` the arrows are, so a change
        // of spacing, zoom or projection moves both together.
        function drawVectorKey(ctx, sx, sy, dpr) {
          const o = ctx.canvas;
          delete o.dataset.vectorKey;
          const note = document.getElementById('vector-key-note');
          // A map drawn stretched has no one scale for an arrow's length (#1251):
          // the arrows are laid out in square raster pixels, so the key, scaled
          // by sx, is wrong for every arrow not along x. Not drawn, and said so.
          // An image setDisplayAspect held is stretched by definition, however
          // small the measured skew (#1321). One that was not held is judged
          // by the measured ratio, and a canvas not laid out yet has none.
          // Rounding the overlay's two sides to whole pixels can skew the ratio
          // by about one pixel over the shorter side, so that much is allowed.
          const rounding = 2 / Math.max(1, Math.min(o.width, o.height));
          const stretched = displayHeld
            || (o.clientWidth > 0 && Math.abs(sx / sy - 1) > 0.05 + rounding);
          if (lastVectors && vectorsEnabled() && stretched) {
            if (note) note.textContent = VECTOR_KEY_STRETCHED;
            return;
          }
          const vw = Number((document.getElementById('width-vectors') || {}).value);
          const drawn = paintVectorKey(ctx, [0, 0, o.width, o.height], sx,
            (Number.isFinite(vw) && vw > 0 ? vw : 1) * dpr, dpr);
          // A map too small for the key says so, rather than showing none
          // without a word (#1168). A canvas not laid out yet has no size to
          // be too small for, and is drawn again once it has one (the resize
          // observer below redraws the overlay when its size moves, #1178).
          if (note) {
            note.textContent = drawn && drawn.leftOff && o.clientWidth > 0 ? VECTOR_KEY_LEFT_OFF : '';
          }
          // What was drawn, in canvas pixels, for a test to measure against.
          if (drawn && !drawn.leftOff) o.dataset.vectorKey = JSON.stringify(drawn);
        }

        // Paint the arrow key into the map at \`region\` ([x, y, width, height]
        // of \`ctx\`), where one raster pixel is \`scale\` pixels, with the shaft
        // \`lineWidth\` wide and the label and spacing sized in units of \`unit\`
        // pixels: a CSS pixel on screen, a PNG pixel in an export. Returns what
        // it drew, \`{ leftOff: true }\` when the key does not fit the region,
        // or null when there is no key to draw.
        function paintVectorKey(ctx, region, scale, lineWidth, unit) {
          if (!lastVectors || !vectorsEnabled()) return null;
          // No arrow drawn, so no scale: Rust reports neither a speed nor a
          // length, and the map draws no key.
          if (lastVectors.referenceSpeed == null || lastVectors.referenceLength == null) return null;
          const full = Number(lastVectors.referenceLength);
          const speed = Number(lastVectors.referenceSpeed);
          if (!Number.isFinite(speed) || !Number.isFinite(full) || full <= 0) return null;
          const [rx, ry, rw, rh] = region;
          const colour = (document.getElementById('color-vectors') || {}).value
            || themedForeground();
          const pad = 6 * unit;
          const inset = 8 * unit;
          const fontPx = 11 * unit;
          ctx.save();
          ctx.font = fontPx + 'px sans-serif';
          // The head is the one Rust gives each arrow: barbs a third of the
          // shaft long (HEAD_FRACTION), turned 155° from it (HEAD_ANGLE_DEG).
          const turn = 155 * Math.PI / 180;
          // Always a full-length arrow, so the label is the reference speed the
          // Vectors row gives. Rust holds a full-length arrow to a quarter of
          // the map's shorter side (#1168), so it fits beside its label on any
          // map but a very small one, which has no key and says so rather
          // than run the key off its edge.
          const label = composeVectorScale(speed, UNITS);
          const length = full * scale;
          const barb = 0.32 * length;
          const textW = ctx.measureText(label).width;
          const headH = barb * Math.sin(turn);
          const boxW = pad + length + pad + textW + pad;
          const boxH = Math.max(fontPx, 2 * headH + lineWidth) + 2 * pad;
          if (boxW > rw - 2 * inset || boxH > rh - 2 * inset) {
            ctx.restore();
            return { leftOff: true };
          }
          const boxX = rx + inset;
          const boxY = ry + rh - inset - boxH;
          // A backing in the editor's own background, so the key reads over
          // any field, and the default arrow colour (the foreground) on it.
          ctx.globalAlpha = 0.8;
          ctx.fillStyle = themedBackground();
          ctx.fillRect(boxX, boxY, boxW, boxH);
          ctx.globalAlpha = 1;
          const tail = boxX + pad;
          const tip = tail + length;
          const y = boxY + boxH / 2;
          ctx.strokeStyle = colour;
          ctx.lineWidth = lineWidth;
          ctx.lineJoin = 'round';
          // Tail, tip, one barb, back to the tip, the other: the arrows' stroke.
          ctx.beginPath();
          ctx.moveTo(tail, y);
          ctx.lineTo(tip, y);
          ctx.lineTo(tip + barb * Math.cos(turn), y - barb * Math.sin(turn));
          ctx.lineTo(tip, y);
          ctx.lineTo(tip + barb * Math.cos(turn), y + barb * Math.sin(turn));
          ctx.stroke();
          const labelColour = themedForeground();
          ctx.fillStyle = labelColour;
          ctx.textBaseline = 'middle';
          ctx.textAlign = 'left';
          ctx.fillText(label, tip + pad, y);
          ctx.restore();
          return {
            tail: tail, tip: tip, y: y, length: length, lineWidth: lineWidth,
            colour: colour, label: label, labelColour: labelColour, fontPx: fontPx,
            box: [boxX, boxY, boxW, boxH], region: [rx, ry, rw, rh],
          };
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

        // Export the current render as a PNG (#243): composite the raster and
        // overlay canvases at the raster's native resolution, then draw the
        // colorbar and title/subtitle beside/above it, and hand the data URL to
        // the host to save. The map is pixel-exact (canvas drawImage); the
        // colorbar and title are redrawn (their on-screen form is DOM/CSS).
        // The arrow key the last export drew, in PNG pixels, for a test.
        let exportedKey = null;
        // Whether the last export's map was too small for the key (#1168),
        // which the export's result line then says.
        let exportKeyLeftOff = false;
        // Clicking Export PNG takes focus from a range field, whose blur or
        // change asks for a render just before the click runs. The picture on
        // screen is then the one from before the edit, so the export waits for
        // the answer to the last request and applies its checks to that
        // (#1374). The wait is bounded: a render that is never answered ends it
        // with a line saying so. The timer lives in the page, which goes with
        // the panel.
        const EXPORT_WAIT_MS = 15000;
        let exportWait = null;
        // A render that moved the scene (projection, slice, bounds, grid) asks
        // for its coastlines, contours and arrows only after its picture
        // arrives, so the picture is not what the PNG shows until they have
        // answered too (#1379). A layer that is off asks nothing, and one that
        // fails answers with an error, so neither holds the export.
        function linesPending() {
          return (overlayAsked !== 0 && overlayAsked === overlaySeq) ||
            (contourAsked !== 0 && contourAsked === contourSeq) ||
            (vectorAsked !== 0 && vectorAsked === vectorSeq);
        }
        function exportAnswered() {
          if (exportWait === null || answeredSeq < renderSeq || linesPending()) return;
          clearTimeout(exportWait);
          exportWait = null;
          exportPng();
        }
        function exportPng() {
          exportKeyLeftOff = false;
          if (awaitingPick) {
            setExportStatus(pickBeforeExport('a PNG'));
            return;
          }
          if (answeredSeq < renderSeq || linesPending()) {
            if (exportWait === null) {
              setExportStatus('Waiting for the picture to finish drawing before exporting a PNG…');
              exportWait = setTimeout(() => {
                exportWait = null;
                setExportStatus('Export failed: the picture is still being drawn; try again.');
              }, EXPORT_WAIT_MS);
            }
            return;
          }
          // The picture on screen was drawn on a range the fields no longer
          // hold, so a PNG of it would not match them: say so instead (#1174).
          if (rangeRefused) {
            setExportStatus('Export failed: the colour range in the fields was refused; see the range fields.');
            return;
          }
          // The last render failed for another reason, so lastPayload is an
          // older picture than the panel's selection (#1187).
          if (lastRenderFailed) {
            setExportStatus('Export failed: the last render failed; see the status line.');
            return;
          }
          if (!lastPayload || !lastPayload.width) {
            setExportStatus('Render the field before exporting a PNG.');
            return;
          }
          const raster = document.getElementById('canvas');
          const overlay = document.getElementById('overlay');
          if (!raster) { setExportStatus('Export failed: no image.'); return; }
          const W = lastPayload.width, H = lastPayload.height;
          const margin = 14, titleH = SUB_LINE ? 46 : 30, gap = 16, cbW = 18, labelW = 64;
          // No colour bar for a field with no range (#871), so no room for one.
          const hasRange = lastPayload.usedMin != null && lastPayload.usedMax != null;
          const mapBlockW = margin + W + (hasRange ? gap + cbW + labelW : 0) + margin;
          // Measure the header with the exact fonts fillText will use below;
          // the title is bold 18px and the subtitle 12px.
          let headerW = 0;
          const measure = document.createElement('canvas').getContext('2d');
          if (measure) {
            measure.font = '600 18px sans-serif';
            headerW = measure.measureText(TITLE_LINE).width;
            if (SUB_LINE) {
              measure.font = '12px sans-serif';
              headerW = Math.max(headerW, measure.measureText(SUB_LINE).width);
            }
          }
          const outW = Math.ceil(exportCanvasWidth(mapBlockW, headerW, margin));
          const outH = margin + titleH + H + margin;
          const out = document.createElement('canvas');
          out.width = outW; out.height = outH;
          const g = out.getContext('2d');
          if (!g) { setExportStatus('Export failed: no canvas context.'); return; }
          g.fillStyle = '#ffffff';
          g.fillRect(0, 0, outW, outH);
          // Title + subtitle.
          g.fillStyle = '#111111';
          g.textBaseline = 'alphabetic';
          g.textAlign = 'left';
          g.font = '600 18px sans-serif';
          g.fillText(TITLE_LINE, margin, margin + 18);
          if (SUB_LINE) {
            g.font = '12px sans-serif';
            g.fillStyle = '#555555';
            g.fillText(SUB_LINE, margin, margin + 36);
          }
          // Map: raster at native resolution, the layers drawn on top at the same size.
          const mapY = margin + titleH;
          // The map area gets the editor's background first, as on screen: cells
          // the raster leaves transparent (masked, off the projection's disc,
          // non-positive under log10) show it, and under Contours + only (#1270)
          // the raster is left out and it is all that shows. So the
          // theme-coloured isolines, coastlines and arrow key read as they do
          // on screen (#1300). The colour bar stays either way.
          g.fillStyle = themedBackground();
          g.fillRect(margin, mapY, W, H);
          if (!contoursOnlyOn()) g.drawImage(raster, margin, mapY, W, H);
          if (overlay) {
            // The layers are drawn straight at the PNG's size (#1366) onto a
            // canvas of their own, not scaled down from the panel-sized
            // overlay, so a stroke keeps its colour on a small raster. The
            // arrow key goes on after, also at the PNG's own resolution: the
            // arrows' scale (one raster pixel is one PNG pixel), their weight
            // as exported, and a label the size of the colour bar's (#1046).
            const strokes = document.createElement('canvas');
            strokes.width = W; strokes.height = H;
            const sctx = strokes.getContext('2d');
            if (sctx) {
              drawOverlay(null, { ctx: sctx, width: W, height: H });
              g.drawImage(strokes, margin, mapY);
              strokes.width = 0;
            }
            const vw = Number((document.getElementById('width-vectors') || {}).value);
            const vwCss = Number.isFinite(vw) && vw > 0 ? vw : 1;
            const weight = Math.max(EXPORT_MIN_STROKE, overlay.clientWidth > 0 ? vwCss * W / overlay.clientWidth : vwCss);
            const key = paintVectorKey(g, [margin, mapY, W, H], 1, weight, 1);
            exportedKey = key && !key.leftOff ? key : null;
            exportKeyLeftOff = !!(key && key.leftOff);
            drawOverlay();
          }
          // Colorbar: gradient (bottom = min, top = max) + min/mid/max labels.
          // Left off when the field had no values to take a range from (#871).
          try {
            const name = (document.getElementById('picker-colormap') || {}).value;
            const entry = COLORMAPS.find((c) => c.name === name) || COLORMAPS[0];
            if (entry && hasRange) {
              const rev = !!(document.getElementById('reverse-colormap')
                && document.getElementById('reverse-colormap').checked);
              const stops = rev ? entry.stops.slice().reverse() : entry.stops;
              const cbX = margin + W + gap;
              const grad = g.createLinearGradient(0, mapY + H, 0, mapY);
              stops.forEach((c, i) => grad.addColorStop(stops.length > 1 ? i / (stops.length - 1) : 0, c));
              g.fillStyle = grad;
              g.fillRect(cbX, mapY, cbW, H);
              g.strokeStyle = '#888888';
              g.strokeRect(cbX, mapY, cbW, H);
              g.fillStyle = '#111111';
              g.font = '11px sans-serif';
              const lx = cbX + cbW + 5;
              // The colorbar's mid-height maps to the value the colormap paints
              // there: the geometric mean under log10 (mid-ramp = 10^((log min +
              // log max)/2) = √(min·max)), the arithmetic mean when linear. Both
              // usedMin/usedMax are > 0 under log10 (the toggle needs a positive
              // floor), so √(min·max) is well-defined. Matches scale_position in
              // fieldglass-core::colormap (#331).
              const mid = isLog10Active()
                ? Math.sqrt(lastPayload.usedMin * lastPayload.usedMax)
                : (lastPayload.usedMin + lastPayload.usedMax) / 2;
              g.textBaseline = 'top';
              g.fillText(Number(lastPayload.usedMax).toPrecision(4), lx, mapY);
              g.textBaseline = 'middle';
              g.fillText(Number(mid).toPrecision(4), lx, mapY + H / 2);
              g.textBaseline = 'bottom';
              g.fillText(Number(lastPayload.usedMin).toPrecision(4), lx, mapY + H);
            }
          } catch (e) {
            // The colorbar is best-effort; the map still exports without it.
          }
          const dataUrl = out.toDataURL('image/png');
          document.body.dataset.exportedKey = exportedKey ? JSON.stringify(exportedKey) : '';
          vscode.postMessage({ type: 'exportPng', dataUrl: dataUrl, defaultName: DEFAULT_PNG_NAME });
          setExportStatus('Exporting PNG…');
        }

        // The host owns the save dialog and the result notification, so the
        // panel only learns how the export ended from this reply. Resolve the
        // "Exporting PNG…" status either way — a cancel is not an error, but
        // leaving it pending reads as one.
        function handleExportPngDone(msg) {
          const outcome = msg.outcome || {};
          if (outcome.status === 'saved') {
            setExportStatus('Exported PNG to ' + outcome.path
              + (exportKeyLeftOff ? '. ' + VECTOR_KEY_LEFT_OFF : ''));
          } else if (outcome.status === 'cancelled') {
            setExportStatus('PNG export cancelled.');
          } else {
            setExportStatus('Export failed: ' + (outcome.reason || 'unknown error'));
          }
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
          // Resampling acts only on a warp: Source paints the array as stored (#1091).
          const sampling = document.getElementById('resampling-control');
          if (sampling) sampling.toggleAttribute('hidden', controlHidden('resampling'));
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
          const userSet = (id) => {
            const el = document.getElementById(id);
            return !!(el && el.dataset.userSet);
          };
          const radio = (name) => {
            const el = document.querySelector('input[name="' + name + '"]:checked');
            return el ? el.value : undefined;
          };
          // While a pick is awaited the slice held is the new file's first
          // variable, which nobody chose: the one saved before stays, so a page
          // written for a file that has it again restores it (#1237).
          const held = SLICE && awaitingPick ? (vscode.getState() || {}) : null;
          vscode.setState({
            projection: heldProjection !== null ? heldProjection : val('picker-projection'),
            centerLon: val('picker-center-lon'),
            centerLat: val('picker-center-lat'),
            polarPreset: val('picker-preset-polar'),
            centralMeridian: val('picker-central-meridian'),
            worldMeridian: val('picker-world-meridian'),
            resampling: val('picker-resampling'),
            colormap: val('picker-colormap'),
            reverseColormap: chk('reverse-colormap'),
            scaleLog: chk('scale-log'),
            compareOp: val('compare-op'),
            compareFieldB: val('compare-field-b'),
            // Field B's identity as well as its index: a GRIB panel written
            // again for a file that changed finds Field B by it (#959).
            compareKeyB: (() => {
              const b = document.getElementById('compare-field-b');
              const chosen = b && b.selectedOptions[0];
              return chosen ? chosen.dataset.key : undefined;
            })(),
            compareIndices: compareIndices.slice(),
            flipY: chk('flip-y'),
            rangeMode: radio('range-mode'),
            rangeMin: val('range-min'),
            rangeMax: val('range-max'),
            boundsMode: radio('bounds-mode'),
            boundsLatMin: val('bounds-lat-min'),
            boundsLatMax: val('bounds-lat-max'),
            boundsLonMin: val('bounds-lon-min'),
            boundsLonMax: val('bounds-lon-max'),
            layers: OVERLAY_LAYERS.reduce((acc, l) => {
              acc[l.id] = {
                on: chk('overlay-' + l.flag),
                color: val('color-' + l.id),
                // Only a picked colour is kept as the user's; a seeded one
                // follows the theme (#1340).
                colorUserSet: userSet('color-' + l.id),
                width: val('width-' + l.id),
              };
              return acc;
            }, {}),
            graticuleSpacing: val('graticule-spacing'),
            contours: chk('overlay-contours'),
            contourColor: val('color-contours'),
            contourColorUserSet: userSet('color-contours'),
            contourWidth: val('width-contours'),
            contourInterval: val('contour-interval'),
            contoursOnly: chk('contours-only'),
            // This cycle's controls (#1027): arrows, the zonal average, the
            // animation and the line plot's axis.
            vectors: chk('overlay-vectors'),
            vectorFieldV: val('vector-field-v'),
            vectorKeyV: (() => {
              const v = document.getElementById('vector-field-v');
              const chosen = v && v.selectedOptions[0];
              return chosen ? chosen.dataset.key : undefined;
            })(),
            vectorSpacing: val('vector-spacing'),
            vectorColor: val('color-vectors'),
            vectorColorUserSet: userSet('color-vectors'),
            vectorWidth: val('width-vectors'),
            zonal: chk('zonal-toggle'),
            linePlot: chk('line-toggle'),
            // Only an axis the user picked: buildAnimationControls applies it
            // wherever it is offered, so the default on screen is not saved,
            // or it would stick to its position for the next variable.
            animateDim: preferredAnimateDim ?? undefined,
            animSpeed: val('anim-speed'),
            animLoop: chk('anim-loop'),
            // The line plot's axis as shown, or the saved choice while its
            // picker is empty.
            lineAxis: val('line-axis') || preferredLineAxis || undefined,
            // The range the frame on screen was drawn on when playback
            // stopped there (#1227).
            pausedRange: pausedRange ? pausedRange.slice() : undefined,
            // The page number it was drawn under: the provider numbers a page
            // afresh only when it writes it for a changed file (#839, #1217),
            // whose data the range is not of.
            pausedPage: pausedRange ? PAGE : undefined,
            slice: held ? held.slice : sliceState,
            // The variable by name as well as by index: a panel rewritten for
            // a file that changed under it restores by name (#839).
            sliceName: held
              ? held.sliceName
              : sliceState && sliceVariable(sliceState.variableIndex)
                ? sliceVariable(sliceState.variableIndex).name
                : undefined,
          });
        }

        // Field B of a GRIB panel. The page can have been written again for a
        // file that changed under it, where the saved index may name another
        // field (#959). So the saved message is kept where the option at its
        // index is still that message, found by its identity where it moved,
        // and otherwise left unchosen, which compares nothing. A snapshot with
        // no identity (a NetCDF panel's, or one saved before #959) restores by
        // index, as before. True when the saved Field B is not in this file.
        function restoreFieldB(index, key) {
          return restoreMessagePick('compare-field-b', index, key);
        }

        // A message picker's saved choice, kept where its index still names
        // that message and found by identity where it moved. Field B (#959) and
        // the V component (#1027) both use it. True when the saved message is
        // not in this file, so the picker is left with nothing chosen.
        function restoreMessagePick(id, index, key) {
          const b = document.getElementById(id);
          if (!b || index === undefined || index === null) return false;
          if (typeof key !== 'string') {
            if (Array.from(b.options).some((o) => o.value === String(index))) b.value = String(index);
            return false;
          }
          const options = Array.from(b.options);
          const same = options.filter((o) => o.dataset.key === key);
          const kept = same.find((o) => o.value === String(index));
          const chosen = kept || (same.length === 1 ? same[0] : null);
          if (chosen) b.value = chosen.value;
          else b.selectedIndex = -1;
          return !chosen;
        }

        // Write a saved snapshot back into the DOM (and sliceState). Runs
        // before attachControls so setupSlice builds the restored variable's
        // axis controls and syncProjectionControls shows the restored
        // projection's preset/bounds groups.
        function restoreState() {
          const s = vscode.getState();
          if (!s) return false;
          // Whether a saved Field B or V component was not in this file, so
          // Compare or the arrows were turned off.
          let dropped = false;
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
          // A slice panel's options are the ones for the slice it was written
          // for, which a hide and show brings back even after the picker moved
          // on (#822). A saved map target is offered again here, and the
          // provider's first render — which draws it, or the source view if the
          // restored slice cannot take it — says which, through applySliceAnswer.
          // So does its error, if that render fails (#839).
          if (SLICE && !mapsOffered && MAP_PROJECTIONS.some((t) => t.value === s.projection)) {
            const projection = document.getElementById('picker-projection');
            if (projection) projection.innerHTML = projectionOptionsHtml(MAP_PROJECTIONS, true);
            mapsOffered = true;
          }
          setVal('picker-projection', s.projection);
          setVal('picker-center-lon', s.centerLon);
          setVal('picker-center-lat', s.centerLat);
          setVal('picker-preset-polar', s.polarPreset);
          setVal('picker-central-meridian', s.centralMeridian);
          setVal('picker-world-meridian', s.worldMeridian);
          setVal('picker-resampling', s.resampling);
          setVal('picker-colormap', s.colormap);
          setChk('reverse-colormap', s.reverseColormap);
          setChk('scale-log', s.scaleLog);
          setVal('compare-op', s.compareOp);
          // A Field B the changed file no longer holds once leaves Compare off,
          // with Field B to pick: an operation left on over no Field B named a
          // single field A − B (#1230).
          if (restoreFieldB(s.compareFieldB, s.compareKeyB)) {
            setVal('compare-op', '');
            dropped = true;
          }
          // Restore Field B's indices before setupSlice builds its steppers.
          if (Array.isArray(s.compareIndices)) compareIndices = s.compareIndices.slice();
          setChk('flip-y', s.flipY);
          setRadio('range-mode', s.rangeMode);
          setVal('range-min', s.rangeMin);
          setVal('range-max', s.rangeMax);
          setRadio('bounds-mode', s.boundsMode);
          setVal('bounds-lat-min', s.boundsLatMin);
          setVal('bounds-lat-max', s.boundsLatMax);
          setVal('bounds-lon-min', s.boundsLonMin);
          setVal('bounds-lon-max', s.boundsLonMax);
          // Per-layer toggle + style. A restored colour the user picked is marked
          // user-set so initLayerColours won't overwrite it with the theme
          // default; a seeded one is not saved as theirs and is seeded from the
          // current theme below (#1340). A snapshot from before the flag existed
          // has no say, so its colours stay as picked.
          const savedUserSet = (flag) => flag !== false;
          for (const layer of OVERLAY_LAYERS) {
            const saved = (s.layers || {})[layer.id];
            if (!saved) continue;
            setChk('overlay-' + layer.flag, saved.on);
            setVal('width-' + layer.id, saved.width);
            const colour = document.getElementById('color-' + layer.id);
            if (colour && saved.color && savedUserSet(saved.colorUserSet)) {
              colour.value = saved.color;
              colour.dataset.userSet = '1';
            }
          }
          setVal('graticule-spacing', s.graticuleSpacing);
          // Contours (#238): restore the toggle, style, interval, and mode.
          setChk('overlay-contours', s.contours);
          setVal('width-contours', s.contourWidth);
          setVal('contour-interval', s.contourInterval);
          setChk('contours-only', s.contoursOnly);
          const cc = document.getElementById('color-contours');
          if (cc && s.contourColor && savedUserSet(s.contourColorUserSet)) {
            cc.value = s.contourColor;
            cc.dataset.userSet = '1';
          }
          // Arrows, the zonal average, the animation and the line plot's axis
          // (#1027). Arrows restored on a panel that does not offer them stay
          // unchecked in effect: vectorsEnabled() also asks the field.
          setChk('overlay-vectors', s.vectors);
          // The arrows likewise go off with their V component (#1230).
          if (restoreMessagePick('vector-field-v', s.vectorFieldV, s.vectorKeyV)) {
            setChk('overlay-vectors', false);
            dropped = true;
          }
          setVal('vector-spacing', s.vectorSpacing);
          setVal('width-vectors', s.vectorWidth);
          const vc = document.getElementById('color-vectors');
          if (vc && s.vectorColor && savedUserSet(s.vectorColorUserSet)) {
            vc.value = s.vectorColor;
            vc.dataset.userSet = '1';
          }
          // Seed the colours the user did not set from the theme now showing.
          initLayerColours();
          setChk('zonal-toggle', s.zonal);
          setChk('line-toggle', s.linePlot);
          setVal('anim-speed', s.animSpeed);
          // Loop starts on; a snapshot from before #1027 has no say.
          if (typeof s.animLoop === 'boolean') setChk('anim-loop', s.animLoop);
          // The animated axis comes back, but not playback: a panel shown again
          // is paused on the step it was showing.
          if (typeof s.animateDim === 'string' && s.animateDim !== '') preferredAnimateDim = s.animateDim;
          if (typeof s.lineAxis === 'string' && s.lineAxis !== '') preferredLineAxis = s.lineAxis;
          // Dependent visibility the change handlers would normally toggle.
          // (The projection-dependent groups are syncProjectionControls' job.)
          const rm = document.getElementById('range-manual-fields');
          if (rm) rm.toggleAttribute('hidden', s.rangeMode !== 'manual');
          const bm = document.getElementById('bounds-manual-fields');
          if (bm) bm.classList.toggle('spacing-hidden', s.boundsMode !== 'manual');
          const gl = document.getElementById('graticule-spacing-label');
          if (gl) gl.classList.toggle('spacing-hidden', !s.graticule);
          const cil = document.getElementById('contour-interval-label');
          if (cil) cil.classList.toggle('spacing-hidden', !s.contours);
          const col = document.getElementById('contours-only-label');
          if (col) col.classList.toggle('spacing-hidden', !s.contours);
          applyContoursOnly();
          // NetCDF slice: adopt the saved spec only if it still describes a
          // variable in this panel with sane axes; indices clamp per dimension.
          // The variable is found by name when the snapshot has one: a panel
          // rewritten for a file that changed under it numbers its variables
          // afresh, and an index the new file reuses names another variable
          // (#839). One it no longer has leaves the panel's own default.
          const v = !SLICE || !s.slice
            ? undefined
            : typeof s.sliceName === 'string'
              ? SLICE.variables.find((x) => x.name === s.sliceName)
              : sliceVariable(s.slice.variableIndex);
          const clampTo = (dims, saved) => dims.map((d, i) => {
            const raw = Math.floor(Number(saved[i]) || 0);
            return Math.min(Math.max(0, raw), Math.max(0, d.length - 1));
          });
          let adopted = false;
          if (v) {
            const nd = v.dims.length;
            const okAxis = (a) => Number.isInteger(a) && a >= 0 && a < nd;
            if (okAxis(s.slice.yDim) && okAxis(s.slice.xDim) && s.slice.yDim !== s.slice.xDim &&
                Array.isArray(s.slice.sliceIndices)) {
              sliceState = {
                variableIndex: v.index,
                yDim: s.slice.yDim,
                xDim: s.slice.xDim,
                sliceIndices: clampTo(v.dims, s.slice.sliceIndices),
              };
              // Field B's indices are positions on the same dimensions.
              compareIndices = clampTo(v.dims, compareIndices);
              adopted = true;
            }
          }
          // Field B's saved indices are for a variable this panel did not adopt.
          if (SLICE && !adopted && sliceState) compareIndices = sliceState.sliceIndices.slice();
          // The range playback stopped on, for the frame it stopped on: only
          // where that frame is the one restored, from the same file (the
          // same page number), on an automatic range (#1227). Anything else
          // is drawn on its own range.
          const r = s.pausedRange;
          if (adopted && s.pausedPage === PAGE && s.rangeMode !== 'manual' &&
              Array.isArray(r) && r.length === 2 &&
              Number.isFinite(r[0]) && Number.isFinite(r[1]) && r[1] > r[0] &&
              s.slice.sliceIndices.length === sliceState.sliceIndices.length &&
              s.slice.sliceIndices.every((x, i) => x === sliceState.sliceIndices[i])) {
            pausedRange = [r[0], r[1]];
          }
          return dropped;
        }

        function attachControls() {
          setupSlice();
          const projPick = document.getElementById('picker-projection');
          // A choice the user makes replaces one held for them (#1099).
          if (projPick) projPick.addEventListener('change', () => { heldProjection = null; syncProjectionControls(); requestRender(); });
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
          // The log toggle only changes the value→colour mapping, not the ramp,
          // so it re-renders without touching the legend gradient.
          const logToggle = document.getElementById('scale-log');
          if (logToggle) logToggle.addEventListener('change', () => { rangeEdited(); requestRender(); });
          // Compare picker (#239): changing the operation or Field B re-renders
          // through the combined path (or back to single render when off).
          ['compare-op', 'compare-field-b'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', requestRender);
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
              // Switching auto↔manual changes whether log has a positive floor.
              updateLogAvailability();
              rangeEdited();
              requestRender();
            });
          });
          ['range-min', 'range-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (!el) return;
            const ask = () => { updateLogAvailability(); rangeEdited(); requestRender(); };
            // Text the field cannot parse (1e999, a lone minus sign) leaves
            // its value '' unchanged, so the browser fires no change event on
            // leaving it, nor when it is deleted again (#1341). Those are read
            // when the user leaves the field, by Tab, a click away or Enter:
            // not while typing, where -5 passes through a lone minus sign.
            // An entry that is bad asks for a render, and so does a field the
            // refusal marked that is now empty, so the automatic end comes
            // back and the note clears. The state is read at that moment, not
            // kept from the last one, which the data's own prefill rewrites
            // without an event. A duplicate ask costs one more refused render.
            const leave = () => {
              const bad = !!(el.validity && el.validity.badInput);
              const emptied = el.value === '' && el.getAttribute('aria-invalid') === 'true';
              if (bad || emptied) ask();
            };
            el.addEventListener('blur', leave);
            el.addEventListener('keydown', (e) => { if (e.key === 'Enter') leave(); });
            // A manual minimum above 0 is what enables log on a field whose auto
            // range dips to/below zero, so re-check availability as it's typed.
            el.addEventListener('change', ask);
          });
          document.querySelectorAll('input[name="bounds-mode"]').forEach((el) => {
            el.addEventListener('change', () => {
              const manual = document.getElementById('bounds-manual-fields');
              const isManual = el.value === 'manual' && el.checked;
              if (manual) manual.classList.toggle('spacing-hidden', !isManual);
              // The window asked for now is the user's, not a gesture's.
              boundsAsked();
              requestRender();
            });
          });
          ['bounds-lat-min', 'bounds-lat-max', 'bounds-lon-min', 'bounds-lon-max'].forEach((id) => {
            const el = document.getElementById(id);
            if (el) el.addEventListener('change', () => { boundsAsked(); requestRender(); });
          });
          // Overlay toggles never re-render the image — they only reproject the
          // vector layer, so the image paint is untouched when toggling.
          initLayerColours();
          for (const layer of OVERLAY_LAYERS) {
            const toggle = document.getElementById('overlay-' + layer.flag);
            if (toggle) toggle.addEventListener('change', requestOverlay);
            // Restyling is a *repaint*, not a reprojection: the pixel geometry
            // is unchanged, so redraw the cached runs instead of asking Rust to
            // project them again.
            const colour = document.getElementById('color-' + layer.id);
            if (colour) {
              colour.addEventListener('input', () => {
                colour.dataset.userSet = '1';
                drawOverlay();
              });
            }
            const width = document.getElementById('width-' + layer.id);
            if (width) width.addEventListener('input', drawOverlay);
          }
          const grat = document.getElementById('overlay-graticule');
          if (grat) {
            grat.addEventListener('change', () => {
              const label = document.getElementById('graticule-spacing-label');
              // visibility toggle (not display) so the field's box stays
              // reserved — showing it never grows the toolbar row.
              if (label) label.classList.toggle('spacing-hidden', !grat.checked);
            });
          }
          const spacing = document.getElementById('graticule-spacing');
          if (spacing) spacing.addEventListener('change', requestOverlay);
          // Contours (#238): the toggle reveals the interval field and fetches
          // (or drops) the isolines; the interval refetches; colour/width just
          // repaint the cached runs; "only" hides the raster.
          const contourToggle = document.getElementById('overlay-contours');
          if (contourToggle) {
            contourToggle.addEventListener('change', () => {
              const label = document.getElementById('contour-interval-label');
              if (label) label.classList.toggle('spacing-hidden', !contourToggle.checked);
              const onlyLabel = document.getElementById('contours-only-label');
              if (onlyLabel) onlyLabel.classList.toggle('spacing-hidden', !contourToggle.checked);
              applyContoursOnly();
              requestContours();
            });
          }
          const vectorToggle = document.getElementById('overlay-vectors');
          if (vectorToggle) vectorToggle.addEventListener('change', requestVectors);
          const vectorField = document.getElementById('vector-field-v');
          if (vectorField) vectorField.addEventListener('change', requestVectors);
          const vectorSpacing = document.getElementById('vector-spacing');
          if (vectorSpacing) vectorSpacing.addEventListener('change', requestVectors);
          const vectorColour = document.getElementById('color-vectors');
          if (vectorColour) {
            vectorColour.addEventListener('input', () => {
              vectorColour.dataset.userSet = '1';
              drawOverlay();
            });
          }
          const vectorWidth = document.getElementById('width-vectors');
          if (vectorWidth) vectorWidth.addEventListener('input', drawOverlay);
          const contourInterval = document.getElementById('contour-interval');
          if (contourInterval) contourInterval.addEventListener('change', requestContours);
          const contourColour = document.getElementById('color-contours');
          if (contourColour) {
            contourColour.addEventListener('input', () => {
              contourColour.dataset.userSet = '1';
              drawOverlay();
            });
          }
          const contourWidth = document.getElementById('width-contours');
          if (contourWidth) contourWidth.addEventListener('input', drawOverlay);
          const contoursOnly = document.getElementById('contours-only');
          if (contoursOnly) contoursOnly.addEventListener('change', applyContoursOnly);
          // Click the image to read the field there (#172). The overlay canvas
          // sits on top, so listen on the wrapper to catch the click regardless.
          const canvasWrap = document.querySelector('.canvas-wrap');
          if (canvasWrap) {
            canvasWrap.addEventListener('click', (ev) => {
              // The click that ends a drag is not a probe.
              if (suppressNextClick) { suppressNextClick = false; return; }
              requestProbe(ev.clientX, ev.clientY);
            });
            canvasWrap.addEventListener('wheel', onWheel, { passive: false });
            canvasWrap.addEventListener('mousedown', onMouseDown);
          }
          window.addEventListener('mousemove', onMouseMove);
          window.addEventListener('mouseup', onMouseUp);
          const zoomIn = document.getElementById('zoom-in');
          if (zoomIn) zoomIn.addEventListener('click', () => zoomButton(0.5));
          const zoomOut = document.getElementById('zoom-out');
          if (zoomOut) zoomOut.addEventListener('click', () => zoomButton(2));
          const zoomReset = document.getElementById('zoom-reset');
          if (zoomReset) zoomReset.addEventListener('click', resetMapWindow);
          // Keep the overlay aligned + crisp as the panel (and the displayed
          // image size) resizes.
          window.addEventListener('resize', drawOverlay);
          // The axis labels are fitted to the room they have (#1043), so a
          // narrower panel drops the ones that no longer fit and a wider one
          // brings them back. Each fit records the size it was made for
          // (data-fitted), and anything that may have moved it asks for a
          // refit only where that size is stale, so a refit, which keeps the
          // size, never triggers itself. Watched on the rails, the image and
          // the plot (a rail is laid out or resized when the image arrives or
          // the toolbar wraps, with no window resize), and on the window as
          // well, in case a resize is not observed.
          function refitIfStale() {
            fitColorbar();
            // The overlay is drawn at the size it has when a reply lands, and
            // its backing store keeps that size. A map laid out later (a
            // panel shown, widened or resized with no window resize) is drawn
            // again, the arrow key with it (#1178). Only where the size is
            // stale, so the redraw, which keeps it, never triggers itself.
            const over = document.getElementById('overlay');
            if (over && lastPayload) {
              const ratio = window.devicePixelRatio || 1;
              if (over.width !== Math.max(1, Math.round(over.clientWidth * ratio))
                || over.height !== Math.max(1, Math.round(over.clientHeight * ratio))) {
                drawOverlay();
              }
            }
            const wrap = document.getElementById('plot-wrap');
            const image = document.getElementById('canvas');
            const rail = document.getElementById('axis-y');
            if (wrap && image && rail && !rail.hidden && wrap.dataset.fitted !== sizeKey(image.getBoundingClientRect())) {
              drawAxisRails();
            }
            const svg = document.getElementById('line-plot');
            const panel = document.getElementById('line-panel');
            if (svg && lastLine && panel && !panel.hidden && svg.dataset.fitted !== sizeKey(svg.getBoundingClientRect())) {
              drawLine(lastLine.line, lastLine.markIndex, lastLine.emptyText, lastLine.of);
            }
          }
          window.addEventListener('resize', refitIfStale);
          if (typeof ResizeObserver === 'function') {
            const refit = new ResizeObserver(refitIfStale);
            for (const id of ['axis-y', 'axis-x', 'canvas', 'overlay', 'line-plot']) {
              const el = document.getElementById(id);
              if (el) refit.observe(el);
            }
          }
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
          // Export CSV sits beside Export PNG in every panel (#1039). A slice
          // panel exports the slice on screen, as PNG export does (#1145):
          // while a render is on its way, or after one failed or was refused,
          // the pickers name a field not drawn. A GRIB panel exports the
          // message it draws, which the provider knows. With a Compare map on
          // screen, either exports Field A, not the difference, and the layout
          // prompt says so (fieldA).
          const exportCsvBtn = document.getElementById('export-csv');
          if (exportCsvBtn) {
            exportCsvBtn.addEventListener('click', () => {
              // Nothing is drawn while a pick is awaited, and the slice the
              // pickers hold is a variable nobody chose (#1230).
              if (awaitingPick) {
                setExportStatus(pickBeforeExport('CSV'));
                return;
              }
              // Nor while no picture is on screen: a failed render cleared
              // one of another field, or the file cannot be read, and the
              // pickers then name a field that was not drawn (#1145, #1388).
              // Said as Export PNG says it.
              if (!lastPayload) {
                setExportStatus(lastRenderFailed
                  ? 'Export failed: the last render failed; see the status line.'
                  : 'Render the field before exporting a CSV.');
                return;
              }
              const fieldA = !!drawnCompareRequest().compare;
              vscode.postMessage(SLICE
                ? Object.assign({ type: 'exportSliceCsv', fieldA: fieldA }, drawnSliceFields())
                : { type: 'exportCsv', fieldA: fieldA });
            });
          }
          const exportPngBtn = document.getElementById('export-png');
          if (exportPngBtn) {
            exportPngBtn.addEventListener('click', exportPng);
          }
        }

        window.addEventListener('message', (event) => {
          const msg = event.data;
          if (!msg || typeof msg.type !== 'string') return;
          if (msg.type === 'gridReady') handleGridReady(msg);
          else if (msg.type === 'gridError') handleGridError(msg);
          else if (msg.type === 'overlayReady') handleOverlayReady(msg);
          else if (msg.type === 'overlayError') handleOverlayError(msg);
          else if (msg.type === 'contourReady') handleContourReady(msg);
          else if (msg.type === 'probeResult') handleProbeResult(msg);
          else if (msg.type === 'lineResult') handleLineResult(msg);
          else if (msg.type === 'axisResult') handleAxisResult(msg);
          else if (msg.type === 'vectorResult') handleVectorResult(msg);
          else if (msg.type === 'zonalResult') handleZonalResult(msg);
          else if (msg.type === 'contourError') handleContourError(msg);
          else if (msg.type === 'exportPngDone') handleExportPngDone(msg);
          // An export clicked while a render was on its way goes ahead once
          // the last request is answered (#1374), and its lines with it (#1379).
          exportAnswered();
        });

        // A GRIB panel written while it waits for a message to be picked
        // (#959) opens on the picker, not on the message that is gone.
        const PICK = ${pickMessages ? JSON.stringify(pickMessages).replace(/</g, "\\u003c") : "null"};
        if (PICK) offerMessages(PICK);
        // A Field B or V component turned off for want of it is saved off at
        // once: otherwise a later file that holds it again would restore the
        // old choice and turn Compare or the arrows back on, although the user
        // last saw them off (#1230).
        const droppedPicks = restoreState();
        attachControls();
        if (droppedPicks) snapshotState();
        // Attach the (possibly restored) selections so the provider's first
        // paint honours them; a fresh panel just sends its defaults.
        //
        // Sent at once, never held: a page written again over a changed or
        // reopened file (#839, #959) has every request but its ready dropped
        // until the ready arrives, so a held ready would leave it blank. A page
        // restored onto a cross-section whose Y axis is not yet known is
        // painted in storage order, then asks for the axis, and draws again
        // only if the axis turns its rows over (#1043).
        const readyOptions = currentOptions(true);
        vscode.postMessage(Object.assign({ type: 'ready', seq: newAsk(readyOptions) }, readyOptions, sliceFields(), compareRequest()));
        //
        // Restoring the page asked for its axes already, ahead of the ready,
        // and a page written again has those requests dropped. So they are
        // forgotten and asked for again after it: a second answer to one that
        // did arrive only draws the same rails again.
        axisPending = {};
        if (isCrossSection() && !yAxisKnown()) {
          awaitingYAxis = () => {
            if (awaitingPick || rowsFlipped() === readyOptions.flipY) return;
            // Not requestRender: the first paint may not have answered yet,
            // and this one has to follow it either way.
            postRerender();
          };
        }
        if (isCrossSection()) {
          requestAxis(sliceState.yDim);
          requestAxis(sliceState.xDim);
        }
        // The steppers' axes too (#1041), for their coordinates, the Animate
        // label and the subtitle. Field B's steppers hold the same axes.
        requestStepperAxes();
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
    /* An author display rule beats the user-agent [hidden] rule, so any element
       given a display and toggled with hidden showed anyway (#1035). One rule
       here means no element needs its own override. */
    [hidden] { display: none !important; }
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
    /* The controls this grid can't use (#1036), right under the caption it
       qualifies: collapsed to one line, opened for the reasons. The caption's
       bottom margin moves below the line while it shows. */
    .projection:has(+ .unavailable:not([hidden])) { margin-bottom: 0.2rem; }
    .unavailable { color: var(--vscode-descriptionForeground); font-size: 0.8rem; margin-bottom: 0.75rem; }
    .unavailable summary { cursor: pointer; }
    .unavailable summary:focus-visible { outline: 1px solid var(--vscode-focusBorder); outline-offset: 2px; }
    .unavailable ul { margin: 0.25rem 0 0; padding-left: 1.4rem; }
    .picker-note { display: block; color: var(--vscode-descriptionForeground); font-size: 0.8rem; margin-top: 0.25rem; }
    /* A colour range Rust refused (#1155): the field it names, and why. */
    input[aria-invalid="true"] { outline: 1px solid var(--vscode-inputValidation-errorBorder, #be1100); outline-offset: -1px; }
    #range-note { color: var(--vscode-errorForeground); }
    /* The status line and the probe readout hold one line's height (1lh)
       empty or full, so neither moves the map when a render clears or
       writes it (#1228): 1.1em was shorter than the line. */
    #status { font-size: 0.85rem; margin-bottom: 0.75rem; min-height: 1lh; }
    .contour-status {
      font-size: 0.85rem;
      color: var(--vscode-editorWarning-foreground, #cca700);
      margin-top: -0.5rem;
      margin-bottom: 0.5rem;
    }
    .contour-status:empty { display: none; }
    #export-status { font-size: 0.85rem; margin-bottom: 0.5rem; }
    #export-status:empty { display: none; }
    /* A line cleared by a render keeps its room (#1255). */
    .contour-status.kept-room:empty, #export-status.kept-room:empty { display: block; }
    #export-status.failed { color: var(--vscode-editorWarning-foreground, #cca700); }
    .probe-readout {
      font-size: 0.85rem;
      font-variant-numeric: tabular-nums;
      min-height: 1lh;
      margin-top: -0.5rem;
      margin-bottom: 0.5rem;
      color: var(--vscode-descriptionForeground, inherit);
    }
    .plot-toggle { display: inline-block; font-size: 0.85rem; margin: 0 0.75rem 0.35rem 0; color: var(--vscode-descriptionForeground, inherit); }
    .line-panel { margin: 0 0 0.75rem; }
    .line-axis { font-size: 0.85rem; color: var(--vscode-descriptionForeground, inherit); }
    .line-plot {
      display: block;
      width: 100%;
      height: 160px;
      margin-top: 0.35rem;
      border: 1px solid var(--vscode-panel-border, rgba(128, 128, 128, 0.35));
      color: var(--vscode-charts-blue, var(--vscode-foreground));
    }
    .line-plot .rule { stroke: var(--vscode-descriptionForeground, currentColor); stroke-dasharray: 3 3; opacity: 0.7; }
    .line-plot .axis-label { fill: var(--vscode-descriptionForeground, currentColor); font-size: 10px; font-variant-numeric: tabular-nums; }
    .line-plot .axis { stroke: var(--vscode-descriptionForeground, currentColor); fill: none; opacity: 0.6; }
    /* A vertical profile (#1043) stands upright, so it is given the height. */
    .line-plot.profile { height: 280px; max-width: 420px; }
    .line-caption {
      font-size: 0.8rem;
      font-variant-numeric: tabular-nums;
      color: var(--vscode-descriptionForeground, inherit);
      margin-top: 0.2rem;
    }
    .canvas-wrap { cursor: crosshair; }
    .render-area {
      display: flex;
      align-items: flex-start;
      gap: 0.75rem;
    }
    /* The image canvas and the vector overlay share one positioned box so the
       overlay sits exactly on top. The image is pixel-upscaled; the overlay is
       a crisp vector layer drawn at display resolution (see drawOverlay). */
    .canvas-wrap { position: relative; flex: 1 1 auto; display: flex; overflow: hidden; }
    .canvas-wrap.dragging { cursor: grabbing; }
    /* An image that would be taller than most of the page at the panel's
       width (#1226, #1252) is drawn as tall as that, its width following its
       aspect, rather than across the panel's width, which would make it
       several screens tall. blit() sets --drawn-aspect, the image's width
       over its height as drawn. The wrapper is capped, not the canvas, so it
       shrinks with the image and the overlay still covers only the image.
       The 2px is the canvas's border, so every capped image is the same
       height whatever its aspect. Unset, before the first image, the cap is
       dropped. */
    .canvas-wrap { max-width: calc(80vh * var(--drawn-aspect) + 2px); }
    .zoom-buttons { margin-left: 0.5rem; white-space: nowrap; }
    /* Bounds' Manual fields hide the way the graticule spacing does, keeping
       their room (#1228). A wheel or a drag ticks Manual and shows them in
       the middle of the gesture, and the map must not drop under the pointer
       when they do. They follow the zoom buttons, so on Auto the room they
       keep is at the end of the row. They hide through .spacing-hidden. */
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
      /* 320px at most, and never taller than the image: fitColorbar sets
         --image-h to the image's height (#1393). */
      height: min(320px, var(--image-h, 320px));
      flex: 0 0 auto;
    }
    /* The gradient is painted by syncColorbar() from the Rust colormap
       registry (top = max, bottom = min) — never hardcoded here, so the strip
       cannot drift from the colours the grid is actually painted with. */
    .cb {
      width: 24px;
      height: 100%;
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
    .toolbar legend {
      padding: 0;
      font-size: 0.8rem;
      color: var(--vscode-descriptionForeground);
    }
    /* A label's text and its control stay together: nowrap keeps a name such
       as "Y axis (rows)" from breaking word by word beside its select in a
       narrow panel (#1393). The row itself wraps between labels. */
    .toolbar label { display: inline-flex; align-items: center; gap: 0.25rem; white-space: nowrap; }
    /* The azimuthal centre groups (orthographic centre lon/lat, polar
       hemisphere + central meridian) bundle their fields in a span so
       syncProjectionControls can toggle the whole group; inline-flex spaces the
       fields like the other groups. */
    .toolbar-row > span { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.5rem 1rem; }
    /* The graticule-spacing field hides without reflowing: visibility:hidden
       keeps its box in the toolbar's flow, so toggling Graticule on/off can't
       grow the toolbar and shove the canvas down (it also drops the field out
       of the tab order while hidden). */
    /* Every field that shows only while its toggle is on: the graticule
       spacing, the contour interval and the Bounds fields (#1260). One rule,
       so a field added to the pattern can't be left showing. */
    .spacing-hidden { visibility: hidden; }
    /* One overlay layer per group: its toggle, then its colour and line weight.
       Keeping the three together stops the row from reading as a loose pile of
       inputs when five layers sit side by side. */
    .overlay-layer {
      display: inline-flex;
      align-items: center;
      gap: 0.25rem;
      margin-right: 0.6rem;
    }
    .toolbar .layer-width { width: 3.5rem; }
    .toolbar .layer-color {
      width: 1.6rem;
      height: 1.3rem;
      padding: 0;
      border: 1px solid var(--vscode-panel-border);
      background: none;
    }
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
    .slice-steppers { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.5rem 0.6rem; }
    .slice-steppers:empty { display: none; }
    .slice-index { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.2rem 0.3rem; max-width: 100%; }
    /* A stepper's name, its coordinate and its range are each kept whole, so
       in a narrow panel the label breaks between them, never inside one
       (#1393). */
    .slice-index .slice-name, .slice-index .slice-coord, .slice-index .slice-len { white-space: nowrap; }
    /* The dot between two steppers ends the first one's range text, so it
       stays with that text and never takes a line of its own when steppers
       wrap (#1393). */
    .slice-index:not(:last-child) .slice-len::after { content: ' ·'; color: var(--vscode-descriptionForeground); }
    .slice-coord { font-variant-numeric: tabular-nums; }
    .slice-coord:empty { display: none; }
    /* Field B's steppers (#1041): set apart from Field A's by a rule and a
       tint, so with the main steppers scrolled away they are not taken for
       the panel's own slice. */
    .compare-b {
      padding: 0.15rem 0.5rem;
      border-left: 3px solid var(--vscode-charts-orange, #d18616);
      background: var(--vscode-editorWidget-background, rgba(128, 128, 128, 0.12));
    }
    .slice-index input[type="range"] { vertical-align: middle; }
    .slice-index input[type="number"] { width: 4.5rem; }
    .slice-len { color: var(--vscode-descriptionForeground); font-size: 0.8rem; }
    /* Cross-section axes (#171): two rails of absolutely-placed labels, one
       down the left of the image and one under it, so a label sits at the
       fraction of the image its index does. */
    .plot-wrap { display: flex; flex-direction: column; flex: 1 1 auto; min-width: 0; }
    .plot-row { display: flex; align-items: stretch; min-width: 0; }
    .axis-rail {
      position: relative;
      color: var(--vscode-descriptionForeground);
      font-size: 0.75rem;
      font-variant-numeric: tabular-nums;
    }
    .axis-y { width: 6.5rem; flex: 0 0 auto; }
    .axis-y span { position: absolute; right: 0.35rem; transform: translateY(-50%); white-space: nowrap; }
    .axis-x { height: 1.1rem; margin-left: 6.5rem; }
    .axis-x span { position: absolute; transform: translateX(-50%); white-space: nowrap; }
    .axis-caption {
      margin-left: 6.5rem;
      color: var(--vscode-descriptionForeground);
      font-size: 0.78rem;
    }
    /* Every button on the panel takes the theme's secondary button colours
       (#1118): Export, the transport row and the zoom buttons. Without this
       a dark theme draws the browser's light-grey default. In a
       high-contrast theme the border is the theme's contrast border. */
    button {
      background: var(--vscode-button-secondaryBackground, var(--vscode-button-background));
      color: var(--vscode-button-secondaryForeground, var(--vscode-button-foreground));
      border: 1px solid var(--vscode-button-border, var(--vscode-contrastBorder, transparent));
      padding: 0.15rem 0.6rem;
      cursor: pointer;
      font-family: inherit;
      font-size: inherit;
      border-radius: 2px;
    }
    button:hover:not(:disabled) {
      background: var(--vscode-button-secondaryHoverBackground, var(--vscode-button-hoverBackground));
    }
    button:focus-visible {
      outline: 1px solid var(--vscode-focusBorder);
      outline-offset: 1px;
    }
    button:disabled { opacity: 0.4; cursor: default; }
    /* The transport buttons hold drawn icons (#1042), not font glyphs, so
       every icon is the same size and sits in the middle of its button. Play
       holds both play and pause and shows one, so it keeps its size as they
       swap. */
    .animate-transport button {
      min-width: 2rem;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      vertical-align: middle;
    }
    .transport-icon { display: block; flex: 0 0 auto; width: 1.2em; height: 1.2em; }
    #anim-play .icon-pause, #anim-play.playing .icon-play { display: none; }
    #anim-play.playing .icon-pause { display: block; }
  </style>
</head>
<body>
  <h1 id="title-line">${escapeHtml(titleLine)}</h1>
  <div class="subtitle">${escapeHtml(subLine)}</div>
  <div class="projection" id="projection-summary">${pickMessages ? "" : escapeHtml(projectionSummary)}</div>
  <details class="unavailable" id="unavailable"${unavailable.length === 0 ? " hidden" : ""}>${unavailableHtml(unavailable)}</details>
  <div class="toolbar" role="toolbar" aria-label="Render settings">
    <div class="toolbar-row">
      <button type="button" id="export-png">Export PNG…</button>
      <button type="button" id="export-csv" title="${escapeHtml(EXPORT_CSV_TITLE)}">Export CSV…</button>
    </div>
${slice
    ? `    <div class="toolbar-row" id="slice-row">
      <label>Variable <select id="slice-variable"></select></label>
      <label>Y axis (rows) <select id="slice-y"></select></label>
      <label>X axis (cols) <select id="slice-x"></select></label>
      <span id="slice-dims" class="slice-steppers"></span>
    </div>
    <div class="toolbar-row" id="animate-row" hidden>
      <label>Animate <select id="animate-dim"></select></label>
      <span class="animate-transport">
        <button type="button" id="anim-first" aria-label="First step" title="First step">${transportIconSvg("first")}</button>
        <button type="button" id="anim-back" aria-label="Step back" title="Step back">${transportIconSvg("back")}</button>
        <button type="button" id="anim-play" aria-label="Play" title="Play">${transportIconSvg("play")}${transportIconSvg("pause")}</button>
        <button type="button" id="anim-forward" aria-label="Step forward" title="Step forward">${transportIconSvg("forward")}</button>
        <button type="button" id="anim-last" aria-label="Last step" title="Last step">${transportIconSvg("last")}</button>
      </span>
      <label>Speed <select id="anim-speed">
        <option value="0.5">0.5 / s</option>
        <option value="1">1 / s</option>
        <option value="2" selected>2 / s</option>
        <option value="5">5 / s</option>
        <option value="10">10 / s</option>
      </select></label>
      <label><input type="checkbox" id="anim-loop" checked> Loop</label>
      <span id="anim-frame" class="slice-len" aria-live="polite"></span>
    </div>`
    // A GRIB panel's message can be gone from a file that changed under it, or
    // be there more than once; the panel then offers the new file's messages
    // here, with none chosen (#959).
    : `    <div class="toolbar-row" id="message-pick-row" hidden>
      <label>Message <select id="message-pick"></select></label>
    </div>`}
    <div class="toolbar-row">
      <span id="cross-section-note" class="picker-note" hidden></span>
      <label id="projection-control"${hides("projection") ? " hidden" : ""}>Projection
        <select id="picker-projection">${projectionOptionsHtml(MAP_PROJECTIONS, meta.reprojectable)}</select>
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
      <label id="resampling-control" hidden>Resampling
        <select id="picker-resampling">
          <option value="nearest" selected>Nearest</option>
          <option value="bilinear">Bilinear</option>
        </select>
      </label>
      <label><input type="checkbox" id="flip-y"> Flip Y axis</label>
    </div>
${colormapFieldsetHtml(colormaps)}
${slice ? netcdfCompareFieldsetHtml(combineOps, picking) : gribCompareFieldsetHtml(compareFields ?? [], combineOps, picking)}
${slice ? "" : vectorFieldsetHtml(compareFields ?? [], meta.uvRelativeToGrid === true, hides("arrows") || picking)}
    <fieldset>
      <legend>Color Range:</legend>
      <label><input type="radio" name="range-mode" value="auto" checked> Auto</label>
      <label><input type="radio" name="range-mode" value="manual"> Manual</label>
      <span id="range-manual-fields" hidden>
        <label>min <input type="number" id="range-min" step="any" aria-describedby="range-note"></label>
        <label>max <input type="number" id="range-max" step="any" aria-describedby="range-note"></label>
      </span>
      <span class="picker-note" id="range-note" role="alert" hidden></span>
    </fieldset>
    <fieldset id="bounds-fieldset" hidden>
      <legend>Bounds:</legend>
      <label><input type="radio" name="bounds-mode" value="auto" checked> Auto</label>
      <label><input type="radio" name="bounds-mode" value="manual"> Manual</label>
      <span class="zoom-buttons" title="Scroll over the map to zoom, drag it to pan">
        <button type="button" id="zoom-in" aria-label="Zoom in">+</button>
        <button type="button" id="zoom-out" aria-label="Zoom out">−</button>
        <button type="button" id="zoom-reset">Reset view</button>
      </span>
      <span id="bounds-manual-fields" class="spacing-hidden">
        <label>lat min <input type="number" id="bounds-lat-min" step="any"></label>
        <label>lat max <input type="number" id="bounds-lat-max" step="any"></label>
        <label>lon min <input type="number" id="bounds-lon-min" step="any"></label>
        <label>lon max <input type="number" id="bounds-lon-max" step="any"></label>
      </span>
    </fieldset>
    <fieldset id="overlay-fieldset"${hides("overlay") ? " hidden" : ""}>
      <legend>Overlay:</legend>
      <span class="overlay-layer">
        <label><input type="checkbox" id="overlay-coastlines"> Coastlines</label>
        <input type="color" id="color-coastline" class="layer-color" aria-label="Coastline colour">
        <input type="number" id="width-coastline" class="layer-width" value="1.1" min="0.2" max="5" step="0.1" aria-label="Coastline line weight">
      </span>
      <span class="overlay-layer">
        <label><input type="checkbox" id="overlay-borders"> Borders</label>
        <input type="color" id="color-borders" class="layer-color" aria-label="Border colour">
        <input type="number" id="width-borders" class="layer-width" value="0.8" min="0.2" max="5" step="0.1" aria-label="Border line weight">
      </span>
      <span class="overlay-layer">
        <label><input type="checkbox" id="overlay-lakes"> Lakes</label>
        <input type="color" id="color-lakes" class="layer-color" value="#4f9fd8" aria-label="Lake colour">
        <input type="number" id="width-lakes" class="layer-width" value="0.9" min="0.2" max="5" step="0.1" aria-label="Lake line weight">
      </span>
      <span class="overlay-layer">
        <label><input type="checkbox" id="overlay-rivers"> Rivers</label>
        <input type="color" id="color-rivers" class="layer-color" value="#4f9fd8" aria-label="River colour">
        <input type="number" id="width-rivers" class="layer-width" value="0.7" min="0.2" max="5" step="0.1" aria-label="River line weight">
      </span>
      <span class="overlay-layer">
        <label><input type="checkbox" id="overlay-graticule"> Graticule</label>
        <input type="color" id="color-graticule" class="layer-color" aria-label="Graticule colour">
        <input type="number" id="width-graticule" class="layer-width" value="0.6" min="0.2" max="5" step="0.1" aria-label="Graticule line weight">
        <label id="graticule-spacing-label" class="spacing-hidden">spacing
          <input type="number" id="graticule-spacing" value="30" min="5" max="90" step="5"></label>
      </span>
      <span class="overlay-layer" id="contour-layer"${hides("contours") ? " hidden" : ""}>
        <label><input type="checkbox" id="overlay-contours"> Contours</label>
        <input type="color" id="color-contours" class="layer-color" aria-label="Contour colour">
        <input type="number" id="width-contours" class="layer-width" value="0.8" min="0.2" max="5" step="0.1" aria-label="Contour line weight">
        <label id="contour-interval-label" class="spacing-hidden">interval
          <input type="number" id="contour-interval" min="0" step="any" placeholder="auto" title="Level spacing; blank picks ~8 nice levels"></label>
        <label id="contours-only-label" class="spacing-hidden"><input type="checkbox" id="contours-only"> only</label>
      </span>
    </fieldset>
  </div>
  <div id="status">Rendering…</div>
  <div id="contour-status" class="contour-status" aria-live="polite"></div>
  <div id="vector-status" class="contour-status" aria-live="polite"></div>
  <div id="overlay-status" class="contour-status" aria-live="polite"></div>
  <div id="export-status" aria-live="polite"></div>
  <div id="probe" class="probe-readout" aria-live="polite"></div>
  <label class="plot-toggle" id="zonal-control"${hides("zonal") ? " hidden" : ""}><input type="checkbox" id="zonal-toggle"> Zonal average</label>
${slice ? `  <label class="plot-toggle" id="line-control"><input type="checkbox" id="line-toggle"> Line plot</label>\n` : ""}  <div id="line-panel" class="line-panel" hidden>
    <label class="line-axis" id="line-axis-label"><span id="line-variable">Values</span> at this point, across <select id="line-axis"></select></label>
    <svg id="line-plot" class="line-plot" role="img" aria-label="The variable through the probed cell"></svg>
    <div id="line-caption" class="line-caption"></div>
  </div>
  <div class="render-area">
    <div class="plot-wrap" id="plot-wrap">
      <div class="plot-row">
        <div class="axis-rail axis-y" id="axis-y" hidden></div>
        <div class="canvas-wrap">
          <canvas id="canvas" width="320" height="320"></canvas>
          <canvas id="overlay"></canvas>
        </div>
      </div>
      <div class="axis-rail axis-x" id="axis-x" hidden></div>
      <div class="axis-caption" id="axis-caption" hidden></div>
    </div>
    <!-- Hidden until an image with a range is drawn: a page whose first
         render fails has no range to show (#1053), as a field with no values
         has none (#871). -->
    <div class="colorbar-wrap" id="colorbar" hidden>
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
