// A render panel driven in a real webview, for the tests that need the panel's
// own script to run (#840, #839, #841).
//
// The panel is opened the way a user opens it, through the editor's message
// handler, and a small driver script added to its page flips the controls a
// user would and reports what the page shows. A test then reads what crossed
// the boundary in both directions: the requests the panel sent, and the
// provider's answers.

import * as assert from "assert";
import * as zlib from "zlib";
import * as vscode from "vscode";

import type { FieldglassApi } from "../../extension";
import type { FieldglassDocument, FieldglassEditorProvider } from "../../provider";

const EXT_ID = "fieldglass.fieldglass";

export function extensionPath(): string {
  const ext = vscode.extensions.getExtension(EXT_ID);
  assert.ok(ext, "extension is installed");
  return ext.extensionPath;
}

export async function provider(): Promise<FieldglassEditorProvider> {
  const ext = vscode.extensions.getExtension<FieldglassApi>(EXT_ID);
  assert.ok(ext, "extension is installed");
  return (await ext.activate()).provider;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Msg = any;

/** The controls a grid can't use, which the panel hides (#1036), by the key
 *  `UnavailableGroup` names them with. The long CSV layout is not on the page. */
export const PAGE_CONTROLS = ["projection", "resampling", "overlay", "contours", "arrows", "zonal"] as const;
export type PageControl = (typeof PAGE_CONTROLS)[number];

/** What acts on a drawn field, which a panel with nothing drawn puts away
 *  (#1426): the image's frame, the Line plot and Zonal average boxes, the
 *  cross-section note, Flip Y, the projection picker, and the Colors, Color
 *  Range and Overlay rows. */
export const DRAWN_CONTROLS = [
  "frame",
  "linePlot",
  "zonal",
  "crossSectionNote",
  "flipY",
  "projection",
  "colormap",
  "range",
  "overlay",
] as const;
export type DrawnControl = (typeof DRAWN_CONTROLS)[number];

/** A panel with nothing drawn shows nothing that acts on a drawn field: no
 *  frame, no Line plot or Zonal average box, no cross-section note, no Flip Y,
 *  projection, Colors, Color Range or Overlay row (#1426). */
export function assertNothingActs(state: PageState, what: string): void {
  const shown = DRAWN_CONTROLS.filter((key) => state.drawnControlsShown[key] === true);
  assert.deepStrictEqual(shown, [], `${what}: still on screen`);
}

/** The panel's "Not available for this grid" disclosure (#1036). */
export interface UnavailableLine {
  /** Its computed `display`. */
  display: string;
  open: boolean;
  summary: string;
  /** One per reason, in order: `data-kind` and the text. */
  items: Array<{ kind: string | null; text: string }>;
  /** Whether it, or anything in it, announces changes (`aria-live`). */
  live: boolean;
  /** The element right before it, as `tag#id`. */
  after: string | null;
  /** Whether its summary is in the tab order. */
  summaryFocusable: boolean;
}

/** The reason the "Not available for this grid" line gives for the control
 *  labelled `label` (#1036), `null` when the line is not shown or does not
 *  name it. */
export function reasonFor(state: PageState, label: string): string | null {
  const line = state.unavailable;
  if (!line || line.display === "none") return null;
  for (const item of line.items) {
    const at = item.text.indexOf(": ");
    if (at >= 0 && item.text.slice(0, at).split(", ").includes(label)) return item.text.slice(at + 2);
  }
  return null;
}

/** What the driver reports of the page. */
export interface PageState {
  /** The Resampling picker's value, hidden or not; `null` if the panel has none. */
  resampling: string | null;
  /** Whether the Overlay row carries `hidden`. */
  overlayHidden: boolean;
  /** The Vectors row's note, `null` for a panel with no Vectors row. */
  vectorNote: string | null;
  /** The Vectors row's note on whether the pair is a u/v pair (#1049), `null`
   *  for a panel with no Vectors row. */
  vectorPairNote: string | null;
  coastlines: boolean;
  graticule: boolean;
  contours: boolean;
  /** Each control's own computed `display`, `null` where the panel has no
   *  such control (a GRIB file of one message has no Vectors row). Found by
   *  the ids every panel has had, not by any one build's wrapper ids. */
  controlDisplay: Record<PageControl, string | null>;
  /** Whether each control takes up room on screen: false inside a hidden
   *  ancestor as well (contours in a hidden Overlay row). */
  controlShown: Record<PageControl, boolean | null>;
  /** The "Not available for this grid" line, `null` when the page has none. */
  unavailable: UnavailableLine | null;
  /** The focused element as `tag#id.class`, suffixed ` in #unavailable` when
   *  it is inside the line. */
  focused: string | null;
  /** `document.hasFocus()`: false when the window behind the page is not focused. */
  docHasFocus: boolean;
  projection: string;
  /** Whether the projection `<select>` itself is disabled. */
  projectionDisabled: boolean;
  /** Every target the projection picker offers, in order. */
  projectionOffers: string[];
  /** The point-probe readout. */
  probe: string;
  /** A slice panel's variable picker: the name selected and every name
   *  offered, `null` for a GRIB panel. */
  sliceVariable: string | null;
  sliceVariables: string[] | null;
  /** The panel heading. */
  title: string;
  /** The line under the heading. */
  subtitle: string;
  /** The projection caption under the image's heading. */
  caption: string;
  /** Whether a slice panel's Y and X axis pickers and its index steppers each
   *  take up room on screen (#1388), `null` for a GRIB panel. */
  sliceAxesShown: { y: boolean; x: boolean; steppers: boolean } | null;
  /** Whether each control in `DRAWN_CONTROLS` takes up room on screen, `null`
   *  where the page has none (a GRIB panel has no Line plot box). */
  drawnControlsShown: Record<DrawnControl, boolean | null>;
  /** Everything the page shows a user: its rendered text, and every tooltip
   *  and accessible label on it (#1038). */
  text: string;
  /** The status line under the image. */
  status: string;
  /** The contour, arrow, overlay and export lines (#1051), each its own. */
  contourStatus: string;
  /** The computed `visibility` of each field the panel shows or hides as its
   *  toggle changes (`spacing-hidden`, #1260), by element id. */
  fieldVisibility: Record<"graticule-spacing-label" | "contour-interval-label" | "contours-only-label" | "bounds-manual-fields", string | null>;
  vectorStatus: string;
  overlayStatus: string;
  exportStatus: string;
  /** The refused colour range shown under the min/max fields (#1155), `null`
   *  while none is on screen. */
  rangeNote: string | null;
  /** The min/max fields marked invalid, by id. */
  rangeInvalid: string[];
  /** The manual min and max fields' text, `null` for a field the page lacks. */
  rangeFields: Array<string | null>;
  /** Field B's stepper box and the toolbar around it: computed background and
   *  border colors, `null` for a panel with no Field B (#1390). */
  compareBox: { background: string; toolbarBackground: string; border: string; borderWidth: string } | null;
  /** Whether the Animate row's Play button is disabled. */
  playDisabled: boolean;
  /** A GRIB panel's message picker, offered when its message is gone from a
   *  file that changed (#959): every message offered, `null` while hidden,
   *  and the one chosen (`""` for none). */
  messageOffers: string[] | null;
  messagePicked: string;
  /** The computed `display` of the Message and Animate rows, `null` when the
   *  panel has no such row (#1035). The rows' `hidden` attribute is not
   *  enough: an author `display` rule beats it. */
  messageRowDisplay: string | null;
  animateRowDisplay: string | null;
  /** Every element carrying `hidden` whose computed `display` is not `none`,
   *  as `tag#id.class`: what a user sees although the page put it away. */
  shownWhileHidden: string[];
  /** The Compare row's Field B, `null` when the panel has no such row. */
  compareFieldB: string | null;
  /** Whether the Compare row is on screen, `null` when the panel has none. */
  compareShown: boolean | null;
  /** Whether any pixel of the image is painted, and whether its colour bar
   *  is on screen: false for both once a picture is cleared (#1237). */
  canvasPainted: boolean;
  colorbarShown: boolean;
  /** The colour bar's two labels, low end first. */
  colorbarLabels: [string, string];
  canvasVisibility: string;
  /** The image's border box on the page, and the page's own height
   *  (`innerHeight`), which a tall image is held inside (#1226). */
  image: Box;
  viewportHeight: number;
  /** The row the image sits in, beside its Y rail: the width an image that
   *  is not fitted to the page's height fills (#1252). */
  plotRow: Box;
  /** The toolbar's layout (#1429). */
  toolbar: ToolbarLayout;
  /** The ` · `-joined lines as drawn, one string per line (#1429). */
  drawnLines: { status: string[]; probe: string[]; subtitle: string[]; lineCaption: string[] };
  /** The line plot (#172, #1037), `null` for a panel with none. A GRIB panel
   *  has one, for its zonal average, but no Line plot box (`boxShown` false). */
  linePlot: LinePlotState | null;
  /** A slice panel's axis rails (#171, #1043), `null` for a GRIB panel. */
  rails: RailsState | null;
  /** A slice panel's index steppers (#1041), `null` for a GRIB panel. */
  steppers: SteppersState | null;
  /** The Animate row's label, `null` for a panel with no Animate row. */
  animateLabel: string | null;
  /** The Animate row's transport buttons (#1042), `null` for a panel with no
   *  Animate row. */
  transport: TransportButton[] | null;
  /** Every `<button>` on the page as drawn (#1118), with the theme's own
   *  colours resolved beside what the button computed. */
  panelButtons: PanelButton[];
  /** Every toolbar button whose label starts `Export` (#1039): its id, its
   *  label, the toolbar row it sits in counted from the top (-1 for none),
   *  whether it is shown, and its tooltip. */
  exports: Array<{ id: string; text: string; row: number; shown: boolean; title: string | null }>;
  /** The theme the page is drawn in, as VS Code tells it: `vscode-light`,
   *  `vscode-dark`, `vscode-high-contrast`… */
  themeKind: string;
  /** The Contours colour input's value, and the theme's `--vscode-foreground`
   *  as the page reads it: the input is seeded from the second at load. */
  contoursColour: string | null;
  foreground: string;
  /** The Coastlines and Vectors colour inputs' values (#1340), `null` when
   *  absent. */
  coastlineColour: string | null;
  vectorsColour: string | null;
  /** The arrow key on the map (#1046), `null` when none is drawn. */
  vectorKey: VectorKeyState | null;
  /** The arrow key the last Export PNG drew, in PNG pixels (#1046), `null`
   *  when it drew none or nothing was exported. */
  exportedKey: ExportedKey | null;
}

/** The arrow key as an export drew it, in the PNG's own pixels. */
export interface ExportedKey {
  tail: number;
  tip: number;
  y: number;
  length: number;
  lineWidth: number;
  label: string;
  labelColour: string;
  fontPx: number;
  box: [number, number, number, number];
  /** `[x, y, width, height]` of the map in the PNG. */
  region: [number, number, number, number];
}

/** The arrow key as the page drew it (#1046), in overlay canvas pixels. */
export interface VectorKeyState {
  /** What the page says it drew. */
  drawn: {
    tail: number;
    tip: number;
    y: number;
    length: number;
    lineWidth: number;
    colour: string;
    label: string;
    /** `[x, y, width, height]` of the key's backing. */
    box: [number, number, number, number];
    /** `[x, y, width, height]` of the map the key was fitted into. */
    region: [number, number, number, number];
  };
  /** The shaft read off the canvas's own pixels, along the row through its
   *  middle: the first pixel at least half the arrow's colour, and one past
   *  the last. `null` when no pixel on that row is. */
  measured: { from: number; to: number } | null;
  /** The overlay canvas's backing size. */
  canvas: { width: number; height: number };
  /** The page's device pixel ratio, which the canvas is drawn at. */
  dpr: number;
}

/** One panel button as drawn (#1118). `want*` are the theme variables the
 *  button should take, resolved by a probe element on the same page. */
export interface PanelButton {
  id: string;
  text: string;
  shown: boolean;
  background: string;
  color: string;
  borderColor: string;
  borderWidth: string;
  wantBackground: string;
  wantColor: string;
  wantBorder: string;
  /** Computed opacity of a disabled copy of it. */
  disabledOpacity: string;
  /** Computed opacity as found. */
  opacity: string;
  /** Whether it is the page's focused element, and its computed outline. */
  focused: boolean;
  focusOutlineColor: string;
  focusOutlineStyle: string;
  wantFocus: string;
}

/** One transport button as drawn (#1042). */
export interface TransportButton {
  id: string;
  ariaLabel: string | null;
  title: string | null;
  /** Its text, trimmed: `""` when it draws an icon rather than a glyph. */
  text: string;
  /** Its computed text colour. */
  color: string;
  box: Box;
  /** The icons it shows (an icon put away is not listed). */
  icons: Array<{
    /** The icon's `class`. */
    name: string | null;
    box: Box;
    viewBox: string | null;
    /** Each shape's computed fill. */
    fills: string[];
    /** Each shape's box, in the icon's own units (`getBBox`). */
    shapes: Array<{ x: number; y: number; width: number; height: number }>;
  }>;
}

/** One index stepper as a user sees it (#1041). */
export interface StepperState {
  /** The dimension it holds. */
  dim: number;
  /** Its text, whitespace collapsed: name, coordinate and range. */
  text: string;
  /** The coordinate shown beside it, `""` for none. */
  coordinate: string | null;
  /** The index in its number box. */
  index: string;
  box: Box;
  /** The name span and the range span: each is one line when its box is no
   *  taller than a line (#1393). */
  nameBox: Box;
  lenBox: Box;
  /** The `::after` content of its range: the dot to the next stepper, or none. */
  after: string;
}

/** Field A's steppers and the Compare row's Field B steppers (#1041). */
export interface SteppersState {
  main: StepperState[];
  fieldB: StepperState[];
  /** Each container's whole text, separators included. */
  mainText: string;
  fieldBText: string;
  /** Each container's left rule and background, as computed. */
  mainStyle: string;
  fieldBStyle: string;
}

/** The toolbar as laid out (#1429). */
export interface ToolbarLayout {
  /** Its content box: inside its border and padding. */
  box: Box;
  /** Every row, group or control drawn past either side of `box`, a
   *  control kept hidden with its room included. */
  outside: Array<{ what: string; box: Box }>;
  /** The Bounds row's Manual fields, shown or keeping their room; `null`
   *  while the Bounds row is not shown. */
  boundsFields: Box | null;
  /** The tallest of the Bounds row's own controls (its radios and zoom
   *  buttons): one control row's height. */
  controlHeight: number;
  /** The tallest single Manual field or its label. */
  manualHeight: number;
  /** Whether the page is wider than its window, and the window's width. */
  pageScrollsSideways: boolean;
  viewportWidth: number;
  /** How far the Bounds fields' line is scrolled sideways, and how far it
   *  can be; `null` for a panel with no Bounds row. */
  boundsScroll: { left: number; room: number } | null;
  /** Which edges of that line fade, for more beyond them. */
  boundsFade: { left: boolean; right: boolean } | null;
  /** Its lat and lon pairs: each axis label and its two fields. */
  boundsPairs: Array<{ label: string; labelBox: Box; inputs: Box[]; clipped: number[]; need: number; paged: boolean }>;
  /** Whether the page cancelled the last `wheelY` a drive turned, `null`
   *  before any. */
  wheelPrevented: boolean | null;
  /** The focused control, when it is in the toolbar, and its id. */
  focused: Box | null;
  focusedId: string | null;
}

/** The line plot as a user sees it (#1037). */
export interface LinePlotState {
  /** Whether the Line plot box takes up room on screen. */
  boxShown: boolean;
  checked: boolean;
  /** Whether the plot itself is on screen. */
  shown: boolean;
  /** The axis picker's label as read, with the axis chosen in place of the
   *  picker; `null` while the picker is put away (a zonal average). */
  label: string | null;
  caption: string;
  /** What the plot draws, in page pixels (#1043). */
  plot: LinePlotGeometry;
}

/** A box on the page, from `getBoundingClientRect`. */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** One axis label as drawn: its text and where it sits. */
export interface AxisLabel {
  text: string;
  box: Box;
}

/** The line plot's marks (#1043). */
export interface LinePlotGeometry {
  /** Whether it is drawn as a vertical profile. */
  profile: boolean;
  /** Each sample's dot, by its index along the axis, at the dot's centre. */
  points: Array<{ index: number; x: number; y: number }>;
  /** The line's `d`: one `M` per run, so two unjoined points are two `M`s
   *  and no `L`. */
  path: string;
  /** The dashed rule through the slice on screen, `null` for none. */
  rule: { x1: number; y1: number; x2: number; y2: number } | null;
  /** The tick labels on each axis. */
  x: AxisLabel[];
  y: AxisLabel[];
  /** The plot itself. */
  box: Box;
}

/** A cross-section's axis rails and what sits around them (#1043). */
export interface RailsState {
  /** Whether the rails are shown. */
  shown: boolean;
  /** The tick labels down the left of the image, top first. */
  y: AxisLabel[];
  /** The tick labels under it, left first. */
  x: AxisLabel[];
  /** The caption naming the two axes (`x: time · y: lat`), and whether it
   *  is shown (#1408). */
  caption: string;
  /** The caption as the lines it is drawn on (#1429). */
  captionLines: string[];
  /** Whether any of its text is drawn outside its box or the page (#1429). */
  captionClipped: boolean;
  captionShown: boolean;
  /** The image, the colour bar, and the cross-section note above. */
  image: Box;
  colorbar: Box | null;
  /** The colour bar's end labels; `null` when one is not shown (#1414). */
  colorbarMax: Box | null;
  colorbarMin: Box | null;
  note: Box | null;
}

/** The page-side reader of every `<button>` (#1118), as script text: the
 *  driver's and the dump-panel test's pages both run it. */
export const PANEL_BUTTONS_SCRIPT = `
// Every button on the page (#1118): what it computed, beside the theme
// variables it should take, resolved by a probe element so the test needs
// no copy of the theme's colours. A high-contrast theme sets no secondary
// colours, so the primary button's are the ones it should take.
function panelButtons() {
  const probe = document.createElement('span');
  probe.style.display = 'none';
  document.body.appendChild(probe);
  const resolve = (prop, value) => {
    probe.style[prop] = '';
    probe.style[prop] = value;
    return getComputedStyle(probe)[prop];
  };
  const out = Array.from(document.querySelectorAll('button')).map((b) => {
    const s = getComputedStyle(b);
    const opacity = s.opacity;
    // A disabled copy beside it: toggling the real button would blur it.
    const copy = b.cloneNode(true);
    copy.disabled = true;
    b.after(copy);
    const disabledOpacity = getComputedStyle(copy).opacity;
    copy.remove();
    return {
      id: b.id || b.className,
      text: b.textContent.trim(),
      shown: b.getClientRects().length > 0,
      background: s.backgroundColor,
      color: s.color,
      borderColor: s.borderTopColor,
      borderWidth: s.borderTopWidth,
      wantBackground: resolve('backgroundColor', 'var(--vscode-button-secondaryBackground, var(--vscode-button-background))'),
      wantColor: resolve('color', 'var(--vscode-button-secondaryForeground, var(--vscode-button-foreground))'),
      wantBorder: resolve('borderTopColor', 'var(--vscode-button-border, var(--vscode-contrastBorder, transparent))'),
      disabledOpacity,
      opacity,
      focused: document.activeElement === b,
      focusOutlineColor: s.outlineColor,
      focusOutlineStyle: s.outlineStyle,
      wantFocus: resolve('color', 'var(--vscode-focusBorder)'),
    };
  });
  probe.remove();
  return out;
}
`;

/** Added to the panel's page ahead of its own script. It keeps the webview API
 *  the panel acquires, so it can answer on the same channel, and on each
 *  `test:drive` sets the named controls, fires their `change` like a user's
 *  edit, and reports the page. `test:probe` clicks the middle of the image, as
 *  a user reads a value; `test:wheel` and `test:drag` zoom and pan it. A
 *  message is handled after every one posted before it, so the report
 *  follows whatever the page did with those. */
const DRIVER = `
  (function () {
    const acquire = window.acquireVsCodeApi;
    let api = null;
    window.acquireVsCodeApi = function () {
      if (!api) api = acquire();
      return api;
    };
    const byId = (id) => document.getElementById(id);
    // The page keeps a dot with the text before it by a no-break space
    // (#1429); a test reads the text a user reads, with a plain space.
    const plain = (t) => (t == null ? t : String(t).replace(/\u00a0/g, ' '));
    let wheelPrevented = null;
    const labelOf = (id) => (byId(id) ? byId(id).closest('label') : null);
    // Each control a grid can rule out, by the element a user would see go.
    const CONTROLS = {
      projection: () => labelOf('picker-projection'),
      resampling: () => labelOf('picker-resampling'),
      overlay: () => byId('overlay-fieldset'),
      contours: () => byId('contour-layer'),
      arrows: () => byId('vector-fieldset'),
      zonal: () => labelOf('zonal-toggle'),
    };
    function controlsBy(read) {
      const out = {};
      for (const key of Object.keys(CONTROLS)) {
        const el = CONTROLS[key]();
        out[key] = el ? read(el) : null;
      }
      return out;
    }
    function unavailableLine() {
      const line = byId('unavailable');
      if (!line) return null;
      const summary = line.querySelector('summary');
      const before = line.previousElementSibling;
      return {
        display: getComputedStyle(line).display,
        open: line.open === true,
        summary: summary ? summary.textContent : '',
        items: Array.from(line.querySelectorAll('li')).map((li) => ({
          kind: li.getAttribute('data-kind'),
          text: li.textContent,
        })),
        live: line.hasAttribute('aria-live') || !!line.querySelector('[aria-live]'),
        after: before ? before.tagName.toLowerCase() + (before.id ? '#' + before.id : '') : null,
        summaryFocusable: !!summary && summary.tabIndex >= 0,
      };
    }
    function linePlot() {
      // A GRIB panel has the plot, for its zonal average, but no Line plot
      // box or axis picker.
      if (!byId('line-panel')) return null;
      const toggle = byId('line-toggle');
      const label = byId('line-axis-label');
      const read = (node) => node.tagName === 'SELECT'
        ? (node.selectedOptions[0] ? node.selectedOptions[0].textContent : '')
        : node.textContent;
      return {
        boxShown: !!toggle && labelOf('line-toggle').getClientRects().length > 0,
        checked: !!toggle && toggle.checked,
        shown: byId('line-panel').getClientRects().length > 0,
        label: label && label.getClientRects().length > 0 ? Array.from(label.childNodes).map(read).join('') : null,
        caption: plain(byId('line-caption').textContent),
        plot: lineGeometry(),
      };
    }
    function boxOf(el) {
      const r = el.getBoundingClientRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    }
    const labelsIn = (root, selector) => Array.from(root.querySelectorAll(selector))
      .filter((el) => el.getClientRects().length > 0)
      .map((el) => ({ text: el.textContent, box: boxOf(el) }));
    function lineGeometry() {
      const svg = byId('line-plot');
      const trace = svg.querySelector('path.trace');
      const rule = svg.querySelector('line.rule');
      // From the SVG's own units to the page's, so a dot and a label compare.
      const m = svg.getScreenCTM();
      const toPage = (x, y) => {
        const p = svg.createSVGPoint();
        p.x = x; p.y = y;
        const q = m ? p.matrixTransform(m) : p;
        return { x: q.x, y: q.y };
      };
      const n = (el, a) => Number(el.getAttribute(a));
      return {
        profile: svg.classList.contains('profile'),
        points: Array.from(svg.querySelectorAll('circle')).map((c) => Object.assign(
          { index: Number(c.getAttribute('data-index')) }, toPage(n(c, 'cx'), n(c, 'cy')))),
        path: trace ? trace.getAttribute('d') : '',
        rule: rule ? (() => {
          const a = toPage(n(rule, 'x1'), n(rule, 'y1'));
          const b = toPage(n(rule, 'x2'), n(rule, 'y2'));
          return { x1: a.x, y1: a.y, x2: b.x, y2: b.y };
        })() : null,
        x: labelsIn(svg, 'text.tick-label[data-axis="x"]'),
        y: labelsIn(svg, 'text.tick-label[data-axis="y"]'),
        box: boxOf(svg),
      };
    }
    function rails() {
      const railY = byId('axis-y');
      const railX = byId('axis-x');
      if (!railY || !railX) return null;
      const colorbar = byId('colorbar');
      const note = byId('cross-section-note');
      return {
        shown: railY.getClientRects().length > 0,
        y: labelsIn(railY, 'span'),
        x: labelsIn(railX, 'span'),
        caption: byId('axis-caption') ? plain(byId('axis-caption').textContent) : '',
        captionLines: byId('axis-caption') ? textLines(byId('axis-caption')).map(plain) : [],
        captionClipped: !!byId('axis-caption') && clipped(byId('axis-caption')),
        captionShown: !!byId('axis-caption') && byId('axis-caption').getClientRects().length > 0,
        image: boxOf(byId('canvas')),
        colorbar: colorbar && colorbar.getClientRects().length > 0 ? boxOf(colorbar) : null,
        colorbarMax: byId('cb-max') && byId('cb-max').getClientRects().length > 0 && getComputedStyle(byId('cb-max')).visibility !== 'hidden' ? boxOf(byId('cb-max')) : null,
        colorbarMin: byId('cb-min') && byId('cb-min').getClientRects().length > 0 && getComputedStyle(byId('cb-min')).visibility !== 'hidden' ? boxOf(byId('cb-min')) : null,
        note: note && note.getClientRects().length > 0 ? boxOf(note) : null,
      };
    }
    // An element's text as the lines it is drawn on (#1429): every character
    // is placed by a Range, and the characters sharing a line's top are
    // that line, in order. Whatever spans or pseudo-content the page builds
    // it from, this is what a user reads.
    function textLines(el) {
      const lines = [];
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        for (let i = 0; i < node.data.length; i++) {
          range.setStart(node, i);
          range.setEnd(node, i + 1);
          const r = Array.from(range.getClientRects()).find((q) => q.width > 0 || q.height > 0);
          if (!r) continue;
          let line = lines.find((l) => Math.abs(l.top - r.top) < r.height / 2);
          if (!line) { line = { top: r.top, text: '' }; lines.push(line); }
          line.text += node.data[i];
        }
      }
      return lines.sort((a, b) => a.top - b.top).map((l) => l.text.trim()).filter((t) => t !== '');
    }
    // Whether any character of el's text is drawn outside el's own box, or
    // outside the page (#1429): a unit cut off at the edge.
    function clipped(el) {
      const box = el.getBoundingClientRect();
      const right = Math.min(box.right, document.documentElement.clientWidth);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        for (let i = 0; i < node.data.length; i++) {
          if (/\s/.test(node.data[i])) continue;
          range.setStart(node, i);
          range.setEnd(node, i + 1);
          for (const r of Array.from(range.getClientRects())) {
            if (r.right > right + 0.5 || r.left < box.left - 0.5) return true;
          }
        }
      }
      return el.scrollWidth > el.clientWidth + 1;
    }
    // The toolbar as laid out (#1429): its content box, every control or
    // group in it that is drawn past that box, and the Bounds row's Manual
    // fields beside the height of one of its controls. A control kept hidden
    // with its room (.spacing-hidden) counts: its room is past the edge too.
    function toolbarLayout() {
      const bar = document.querySelector('.toolbar');
      const r = bar.getBoundingClientRect();
      const cs = getComputedStyle(bar);
      const px = (v) => parseFloat(v) || 0;
      const box = {
        left: r.left + px(cs.borderLeftWidth) + px(cs.paddingLeft),
        top: r.top + px(cs.borderTopWidth) + px(cs.paddingTop),
        right: r.right - px(cs.borderRightWidth) - px(cs.paddingRight),
        bottom: r.bottom - px(cs.borderBottomWidth) - px(cs.paddingBottom),
      };
      const describe = (el) => {
        const id = el.id || (el.querySelector('[id]') ? 'with #' + el.querySelector('[id]').id : '');
        return el.tagName.toLowerCase() + (id && el.id ? '#' + id : id ? ' (' + id + ')' : '');
      };
      // A line that scrolls sideways (the Bounds fields, #1429) is held to
      // the toolbar itself; what it holds is reached by scrolling it, and is
      // judged where focus scrolls it to, not here.
      const scrolls = (el) => /^(auto|scroll)$/.test(getComputedStyle(el).overflowX);
      const scrolled = (el) => {
        for (let up = el.parentElement; up && up !== bar; up = up.parentElement) if (scrolls(up)) return true;
        return false;
      };
      const outside = Array.from(bar.querySelectorAll('fieldset, .toolbar-row, label, input, select, button, span'))
        .filter((el) => el.getClientRects().length > 0 && !scrolled(el))
        .map((el) => ({ what: describe(el), box: boxOf(el) }))
        .filter((c) => c.box.right > c.box.left && (c.box.right > box.right + 0.5 || c.box.left < box.left - 0.5));
      const fields = byId('bounds-manual-fields');
      const boundsShown = !!fields && fields.getClientRects().length > 0;
      const controls = byId('bounds-fieldset')
        ? Array.from(byId('bounds-fieldset').querySelectorAll('label, button, input'))
          .filter((el) => el.getClientRects().length > 0 && !fields.contains(el))
        : [];
      return {
        box,
        outside,
        boundsFields: boundsShown ? boxOf(fields) : null,
        controlHeight: controls.reduce((h, el) => Math.max(h, el.getBoundingClientRect().height), 0),
        manualHeight: boundsShown
          ? Array.from(fields.querySelectorAll('label, input'))
            .reduce((h, el) => Math.max(h, el.getBoundingClientRect().height), 0)
          : 0,
        pageScrollsSideways: document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
        viewportWidth: document.documentElement.clientWidth,
        boundsScroll: fields ? { left: fields.scrollLeft, room: fields.scrollWidth - fields.clientWidth } : null,
        boundsFade: fields ? { left: fields.classList.contains('fade-left'), right: fields.classList.contains('fade-right') } : null,
        boundsPairs: fields
          ? Array.from(fields.querySelectorAll('.bounds-pair')).map((pair) => ({
            label: pair.querySelector('.bounds-axis').textContent,
            labelBox: boxOf(pair.querySelector('.bounds-axis')),
            inputs: Array.from(pair.querySelectorAll('input')).map(boxOf),
            // How far each field's text is wider than the room inside it,
            // measured in the field's own font (a focused field's scroll
            // width also counts its caret).
            clipped: Array.from(pair.querySelectorAll('input')).map((el) => {
              const cs = getComputedStyle(el);
              const ctx = document.createElement('canvas').getContext('2d');
              ctx.font = cs.font;
              const room = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
              return Math.max(0, Math.ceil(ctx.measureText(el.value).width - room));
            }),
            paged: fields.classList.contains('paged'),
            need: Math.round(pair.scrollWidth),
          }))
          : [],
        wheelPrevented,
        focusedId: document.activeElement ? document.activeElement.id || null : null,
        focused: document.activeElement && document.activeElement !== document.body && bar.contains(document.activeElement)
          ? boxOf(document.activeElement) : null,
      };
    }
    function report() {
      const overlays = byId('overlay-fieldset');
      const vectors = byId('vector-fieldset');
      const picker = byId('picker-projection');
      const variable = byId('slice-variable');
      const chosen = variable && variable.selectedOptions[0];
      const messages = byId('message-pick');
      const messageRow = byId('message-pick-row');
      const picked = messages && messages.selectedOptions[0];
      const fieldB = byId('compare-field-b');
      const displayOf = (id) => (byId(id) ? getComputedStyle(byId(id)).display : null);
      const describe = (el) => el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') +
        (typeof el.className === 'string' && el.className.trim() ? '.' + el.className.trim().split(/\\s+/).join('.') : '');
      return {
        resampling: byId('picker-resampling') ? byId('picker-resampling').value : null,
        messageRowDisplay: displayOf('message-pick-row'),
        animateRowDisplay: displayOf('animate-row'),
        shownWhileHidden: Array.from(document.querySelectorAll('[hidden]'))
          .filter((el) => getComputedStyle(el).display !== 'none')
          .map(describe),
        messageOffers: messages && messageRow && !messageRow.hidden
          ? Array.from(messages.options).map((o) => o.textContent)
          : null,
        messagePicked: picked ? picked.textContent : '',
        compareFieldB: fieldB ? fieldB.value : null,
        compareShown: byId('compare-fieldset') ? byId('compare-fieldset').getClientRects().length > 0 : null,
        canvasPainted: (() => {
          const canvas = byId('canvas');
          const ctx = canvas && canvas.width > 0 && canvas.height > 0 ? canvas.getContext('2d') : null;
          if (!ctx) return false;
          const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
          for (let i = 3; i < data.length; i += 4) if (data[i] !== 0) return true;
          return false;
        })(),
        colorbarShown: !!byId('colorbar') && byId('colorbar').getClientRects().length > 0,
        colorbarLabels: [byId('cb-min').textContent, byId('cb-max').textContent],
        overlayHidden: !!(overlays && overlays.hasAttribute('hidden')),
        vectorNote: vectors ? byId('vector-note').textContent : null,
        vectorPairNote: vectors ? byId('vector-pair-note').textContent : null,
        coastlines: byId('overlay-coastlines').checked,
        graticule: byId('overlay-graticule').checked,
        contours: byId('overlay-contours').checked,
        controlDisplay: controlsBy((el) => getComputedStyle(el).display),
        controlShown: controlsBy((el) => el.getClientRects().length > 0),
        unavailable: unavailableLine(),
        docHasFocus: document.hasFocus(),
        focused: document.activeElement ? describe(document.activeElement) + (document.activeElement.closest('#unavailable') ? ' in #unavailable' : '') : null,
        projection: picker.value,
        projectionDisabled: picker.disabled,
        projectionOffers: Array.from(picker.options).map((o) => o.value),
        probe: plain(byId('probe').textContent),
        sliceVariable: variable ? (chosen ? chosen.textContent : '') : null,
        sliceVariables: variable ? Array.from(variable.options).map((o) => o.textContent) : null,
        title: byId('title-line').textContent,
        subtitle: plain(document.querySelector('.subtitle').textContent),
        caption: byId('projection-summary').textContent,
        drawnControlsShown: Object.fromEntries(Object.entries({
          frame: document.querySelector('.render-area'),
          linePlot: labelOf('line-toggle'),
          zonal: labelOf('zonal-toggle'),
          crossSectionNote: byId('cross-section-note'),
          flipY: labelOf('flip-y'),
          projection: labelOf('picker-projection'),
          colormap: labelOf('picker-colormap'),
          range: document.querySelector('input[name="range-mode"]'),
          overlay: byId('overlay-fieldset'),
        }).map(([k, el]) => [k, el ? el.getClientRects().length > 0 : null])),
        sliceAxesShown: byId('slice-y')
          ? {
            y: byId('slice-y').getClientRects().length > 0,
            x: byId('slice-x').getClientRects().length > 0,
            steppers: Array.from(byId('slice-dims').querySelectorAll('.slice-index'))
              .some((el) => el.getClientRects().length > 0),
          }
          : null,
        text: [plain(document.body.innerText)].concat(
          Array.from(document.querySelectorAll('[title], [aria-label], [placeholder]'))
            .filter((el) => el.getClientRects().length > 0)
            .flatMap((el) => ['title', 'aria-label', 'placeholder'].map((a) => el.getAttribute(a) || '')),
        ).join('\\n'),
        status: plain(byId('status').textContent),
        contourStatus: byId('contour-status').textContent,
        fieldVisibility: Object.fromEntries(
          ['graticule-spacing-label', 'contour-interval-label', 'contours-only-label', 'bounds-manual-fields']
            .map((id) => [id, byId(id) ? getComputedStyle(byId(id)).visibility : null])),
        vectorStatus: byId('vector-status').textContent,
        overlayStatus: byId('overlay-status').textContent,
        exportStatus: byId('export-status').textContent,
        rangeNote: byId('range-note') && byId('range-note').getClientRects().length > 0
          ? byId('range-note').textContent : null,
        rangeInvalid: ['range-min', 'range-max']
          .filter((id) => byId(id) && byId(id).getAttribute('aria-invalid') === 'true'),
        compareBox: byId('compare-dims') ? (() => {
          const box = getComputedStyle(byId('compare-dims'));
          return {
            background: box.backgroundColor,
            toolbarBackground: getComputedStyle(byId('compare-dims').closest('.toolbar')).backgroundColor,
            border: box.borderTopColor,
            borderWidth: box.borderTopWidth,
          };
        })() : null,
        rangeFields: ['range-min', 'range-max'].map((id) => (byId(id) ? byId(id).value : null)),
        playDisabled: !!(byId('anim-play') && byId('anim-play').disabled),
        canvasVisibility: byId('canvas').style.visibility,
        image: boxOf(byId('canvas')),
        viewportHeight: window.innerHeight,
        plotRow: boxOf(document.querySelector('.plot-row')),
        linePlot: linePlot(),
        rails: rails(),
        toolbar: toolbarLayout(),
        drawnLines: {
          status: textLines(byId('status')).map(plain),
          probe: textLines(byId('probe')).map(plain),
          subtitle: textLines(document.querySelector('.subtitle')).map(plain),
          lineCaption: textLines(byId('line-caption')).map(plain),
        },
        steppers: steppers(),
        animateLabel: byId('anim-frame') ? byId('anim-frame').textContent : null,
        transport: transport(),
        panelButtons: panelButtons(),
        exports: Array.from(document.querySelectorAll('.toolbar button'))
          .filter((b) => /^Export /.test(b.textContent.trim()))
          .map((b) => ({
            id: b.id,
            text: b.textContent.trim(),
            row: Array.from(document.querySelectorAll('.toolbar-row')).indexOf(b.closest('.toolbar-row')),
            shown: b.getClientRects().length > 0,
            title: b.getAttribute('title'),
          })),
        themeKind: document.body.getAttribute('data-vscode-theme-kind') || document.body.className,
        contoursColour: (document.getElementById('color-contours') || {}).value || null,
        coastlineColour: (document.getElementById('color-coastline') || {}).value || null,
        vectorsColour: (document.getElementById('color-vectors') || {}).value || null,
        foreground: getComputedStyle(document.documentElement).getPropertyValue('--vscode-foreground').trim(),
        vectorKey: vectorKey(),
        exportedKey: document.body.dataset.exportedKey ? JSON.parse(document.body.dataset.exportedKey) : null,
      };
    }
    // The arrow key (#1046): what the page says it drew, and the shaft as the
    // canvas holds it. A pixel counts as the shaft's when it is at least half
    // way from the key's backing to the arrow's colour, and on the line
    // between the two: the label beside it is another colour.
    function vectorKey() {
      const o = byId('overlay');
      if (!o || !o.dataset.vectorKey) return null;
      const drawn = JSON.parse(o.dataset.vectorKey);
      const ctx = o.getContext('2d');
      const [bx, by, bw] = drawn.box;
      const x0 = Math.max(0, Math.floor(bx));
      const x1 = Math.min(o.width, Math.ceil(bx + bw));
      const row = ctx.getImageData(x0, Math.floor(drawn.y), x1 - x0, 1).data;
      const back = ctx.getImageData(Math.floor(bx) + 1, Math.floor(by) + 1, 1, 1).data;
      const hex = drawn.colour.replace('#', '');
      const ink = [0, 2, 4].map((k) => parseInt(hex.slice(k, k + 2), 16));
      const span = ink.reduce((sum, c, k) => sum + (c - back[k]) * (c - back[k]), 0);
      let from = -1;
      let to = -1;
      for (let x = 0; x < x1 - x0; x++) {
        let dot = 0;
        for (let k = 0; k < 3; k++) dot += (row[x * 4 + k] - back[k]) * (ink[k] - back[k]);
        const t = span > 0 ? dot / span : 0;
        let off = 0;
        for (let k = 0; k < 3; k++) {
          const d = row[x * 4 + k] - back[k] - t * (ink[k] - back[k]);
          off += d * d;
        }
        if (t >= 0.5 && Math.sqrt(off) < 60) {
          if (from < 0) from = x;
          to = x + 1;
        }
      }
      return {
        drawn: drawn,
        measured: from < 0 ? null : { from: x0 + from, to: x0 + to },
        canvas: { width: o.width, height: o.height },
        dpr: window.devicePixelRatio || 1,
      };
    }
    // The Animate row's transport buttons as drawn (#1042): each button's box
    // and labels, and the icons it shows, with every shape's box in the
    // icon's own 16 × 16 grid.
    function transport() {
      const row = document.querySelector('.animate-transport');
      if (!row) return null;
      return Array.from(row.querySelectorAll('button')).map((b) => ({
        id: b.id,
        ariaLabel: b.getAttribute('aria-label'),
        title: b.getAttribute('title'),
        text: b.textContent.trim(),
        color: getComputedStyle(b).color,
        box: boxOf(b),
        icons: Array.from(b.querySelectorAll('svg'))
          .filter((svg) => svg.getClientRects().length > 0)
          .map((svg) => ({
            name: svg.getAttribute('class'),
            box: boxOf(svg),
            viewBox: svg.getAttribute('viewBox'),
            fills: Array.from(svg.querySelectorAll('path')).map((p) => getComputedStyle(p).fill),
            shapes: Array.from(svg.querySelectorAll('path')).map((p) => {
              const r = p.getBBox();
              return { x: r.x, y: r.y, width: r.width, height: r.height };
            }),
          })),
      }));
    }
    ${PANEL_BUTTONS_SCRIPT}
    // A slice panel's index steppers, Field A's and Field B's (#1041).
    function steppers() {
      const main = byId('slice-dims');
      if (!main) return null;
      const read = (root) => Array.from(root.querySelectorAll('.slice-index')).map((el) => {
        const coord = el.querySelector('.slice-coord');
        const number = el.querySelector('.slice-number');
        return {
          dim: Number(number.dataset.dim),
          text: el.textContent.replace(/\\s+/g, ' ').trim(),
          coordinate: coord ? coord.textContent : null,
          index: number.value,
          box: boxOf(el),
          nameBox: boxOf(el.querySelector('.slice-name')),
          lenBox: boxOf(el.querySelector('.slice-len')),
          after: getComputedStyle(el.querySelector('.slice-len'), '::after').content,
        };
      });
      const fieldB = byId('compare-dims');
      const style = (el) => {
        const s = getComputedStyle(el);
        return s.borderLeftStyle + ' ' + s.borderLeftWidth + ' ' + s.backgroundColor;
      };
      return {
        main: read(main),
        fieldB: fieldB ? read(fieldB) : [],
        mainText: main.textContent,
        fieldBText: fieldB ? fieldB.textContent : '',
        mainStyle: style(main),
        fieldBStyle: fieldB ? style(fieldB) : '',
      };
    }
    // Every control on the page with an id, by id, and each radio group's
    // choice, by name: what a snapshot has to carry across a rebuild (#1027).
    const settable = (el) => el.type !== 'button' && el.type !== 'file' && el.type !== 'radio';
    function controls() {
      const out = {};
      document.querySelectorAll('input[id], select[id]').forEach((el) => {
        if (settable(el)) out[el.id] = el.type === 'checkbox' ? el.checked : el.value;
      });
      document.querySelectorAll('input[type="radio"][name]:checked').forEach((el) => {
        out['radio:' + el.name] = el.value;
      });
      return out;
    }
    // Move every enabled control off its value, as a user would, firing the
    // events a user's edit fires. \`values\` gives a control a value of the
    // test's choosing; \`skip\` leaves one alone. Reports what moved, what
    // could not move (a select with one option), and what the driver did not
    // know how to move.
    function perturb(skip, values) {
      const moved = [];
      const stuck = [];
      const unknown = [];
      const fire = (el) => {
        el.dispatchEvent(new Event('input', { bubbles: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
      };
      document.querySelectorAll('input[id], select[id]').forEach((el) => {
        if (!settable(el) || skip.includes(el.id) || el.matches(':disabled')) return;
        const before = el.type === 'checkbox' ? el.checked : el.value;
        if (el.id in values) el.value = String(values[el.id]);
        else if (el.type === 'checkbox') el.checked = !el.checked;
        else if (el.tagName === 'SELECT') {
          const other = Array.from(el.options).filter((o) => o.value !== el.value).pop();
          if (!other) { stuck.push(el.id); return; }
          el.value = other.value;
        } else if (el.type === 'number') {
          const step = Number(el.step) || 1;
          const now = el.value === '' ? NaN : Number(el.value);
          let next = Number.isFinite(now) ? now + step : (el.min !== '' ? Number(el.min) : 0) + step;
          if (el.max !== '' && next > Number(el.max)) next = (Number.isFinite(now) ? now : Number(el.max)) - step;
          el.value = String(next);
        } else if (el.type === 'color') {
          el.value = el.value.toLowerCase() === '#123456' ? '#654321' : '#123456';
        } else {
          unknown.push(el.id);
          return;
        }
        if ((el.type === 'checkbox' ? el.checked : el.value) !== before) moved.push(el.id);
        fire(el);
      });
      const groups = new Set();
      document.querySelectorAll('input[type="radio"][name]').forEach((el) => groups.add(el.name));
      groups.forEach((name) => {
        if (skip.includes('radio:' + name)) return;
        const other = Array.from(document.querySelectorAll('input[type="radio"][name="' + name + '"]'))
          .find((el) => !el.checked && !el.matches(':disabled'));
        if (!other) return;
        other.checked = true;
        moved.push('radio:' + name);
        fire(other);
      });
      return { moved: moved, stuck: stuck, unknown: unknown };
    }
    // Whether the rails and the line plot are fitted to their current size,
    // by the size each records it was last fitted for (data-fitted).
    function fitted() {
      const key = (el) => {
        const r = el.getBoundingClientRect();
        return Math.round(r.width) + 'x' + Math.round(r.height);
      };
      const rail = byId('axis-y');
      const railsOk = !rail || rail.getClientRects().length === 0 ||
        byId('plot-wrap').dataset.fitted === key(byId('canvas'));
      const svg = byId('line-plot');
      const plotOk = !svg || svg.getClientRects().length === 0 || svg.dataset.fitted === key(svg);
      return railsOk && plotOk;
    }
    // Run done once fitted() holds, checked every 16 ms (not by frames, which
    // a throttled page may not paint), or after five
    // seconds regardless, so a test reads what is there and says why.
    function whenFitted(done) {
      const start = Date.now();
      let nudged = false;
      const check = () => {
        if (fitted() || Date.now() - start > 5000) { done(); return; }
        // A resize the page has not seen after a while is told it the way a
        // window resize would; the page refits only what is stale.
        if (!nudged && Date.now() - start > 250) {
          nudged = true;
          window.dispatchEvent(new Event('resize'));
        }
        setTimeout(check, 16);
      };
      setTimeout(check, 0);
    }
    window.addEventListener('message', (ev) => {
      const m = ev.data;
      if (m && m.type === 'test:controls') {
        api.postMessage({ type: 'test:controls', tag: m.tag, controls: controls() });
        return;
      }
      // A timer of the page's own, answered when it fires. Timers fire in
      // the order they fall due, throttled or not, so its answer means a
      // shorter one the page set before it has fired too (#1259).
      if (m && m.type === 'test:timer') {
        setTimeout(() => api.postMessage({ type: 'test:timer', tag: m.tag }), m.ms);
        return;
      }
      // A message posted to the provider as it is, as some page of this
      // panel's would post it: one already replaced, say (#1202).
      if (m && m.type === 'test:send') {
        api.postMessage(m.message);
        return;
      }
      if (m && m.type === 'test:perturb') {
        const done = perturb(m.skip, m.values);
        api.postMessage({ type: 'test:perturbed', tag: m.tag, moved: done.moved, stuck: done.stuck, unknown: done.unknown });
        return;
      }
      // A wheel turn or a drag over the image, at fractions of its width and
      // height from the top left, as a user zooms or pans (#1147). A drag
      // ends with the click a browser fires after the button comes up, or,
      // released outside the image (m.outside), with none: the browser fires
      // a click on what the press and the release share, not on the image.
      // A wheel can instead name a point of the page (m.client), as a pointer
      // held still while the map under it is redrawn (#1228); the reply says
      // which point it turned at.
      if (m && (m.type === 'test:wheel' || m.type === 'test:drag')) {
        const r = byId('canvas').getBoundingClientRect();
        const at = (f) => ({ clientX: r.left + r.width * f[0], clientY: r.top + r.height * f[1] });
        const wrap = document.querySelector('.canvas-wrap');
        let point = null;
        // Whether the page cancelled the wheel, so the browser would not
        // scroll the page with it (#1269).
        let prevented = null;
        // The preview the image is drawn under, either side of the gesture:
        // a wheel the page takes moves it at once (#1257).
        const transform = [byId('canvas').style.transform];
        if (m.type === 'test:wheel') {
          point = m.client ? { clientX: m.client[0], clientY: m.client[1] } : at(m.at);
          prevented = !wrap.dispatchEvent(new WheelEvent('wheel', Object.assign(
            { bubbles: true, cancelable: true, deltaY: m.deltaY }, point)));
        } else {
          // Either end can be a client point ({ client }) instead.
          const end = (f) => (f.client ? { clientX: f.client[0], clientY: f.client[1] } : at(f));
          const from = end(m.from);
          const to = end(m.to);
          const mid = { clientX: (from.clientX + to.clientX) / 2, clientY: (from.clientY + to.clientY) / 2 };
          wrap.dispatchEvent(new MouseEvent('mousedown', Object.assign({ bubbles: true, button: 0 }, from)));
          window.dispatchEvent(new MouseEvent('mousemove', Object.assign({ bubbles: true }, mid)));
          window.dispatchEvent(new MouseEvent('mousemove', Object.assign({ bubbles: true }, to)));
          const up = m.outside ? document.body : wrap;
          up.dispatchEvent(new MouseEvent('mouseup', Object.assign({ bubbles: true, button: 0 }, to)));
          if (!m.outside) wrap.dispatchEvent(new MouseEvent('click', Object.assign({ bubbles: true, button: 0 }, to)));
        }
        transform.push(byId('canvas').style.transform);
        api.postMessage({
          type: 'test:gestured', tag: m.tag, width: r.width, height: r.height, prevented: prevented, transform,
          point: point && [point.clientX, point.clientY], rect: [r.left, r.top, r.width, r.height],
        });
        return;
      }
      if (!m || (m.type !== 'test:drive' && m.type !== 'test:probe')) return;
      if (m.type === 'test:probe') {
        const r = byId('canvas').getBoundingClientRect();
        const point = {
          bubbles: true,
          button: 0,
          // Halfway across and down unless the test names fractions of the
          // width and height.
          clientX: m.client ? m.client[0] : r.left + r.width * (typeof m.fx === 'number' ? m.fx : 0.5),
          clientY: m.client ? m.client[1] : r.top + r.height * (typeof m.fy === 'number' ? m.fy : 0.5),
        };
        // A press, a release and the click, as a user's click arrives.
        const wrap = document.querySelector('.canvas-wrap');
        wrap.dispatchEvent(new MouseEvent('mousedown', point));
        wrap.dispatchEvent(new MouseEvent('mouseup', point));
        wrap.dispatchEvent(new MouseEvent('click', point));
        api.postMessage({
          type: 'test:probed', tag: m.tag, width: r.width, height: r.height,
          point: [point.clientX, point.clientY], rect: [r.left, r.top, r.width, r.height],
        });
        return;
      }
      // A width change is fitted by the page's ResizeObserver, whenever the
      // browser delivers it: the report waits until the page says it has
      // fitted the rails and the plot to the sizes they now have (#1043).
      let resized = false;
      // An action that cannot be taken (no such element) is reported back
      // as the report's failure, not left for the test to time out on.
      const failed = (e) => api.postMessage({ type: 'test:state', tag: m.tag, error: String((e && e.stack) || e) });
      try {
        for (const a of m.actions) {
          // A control with no id (a slice stepper) is named by a selector.
          const el = a.selector ? document.querySelector(a.selector) : a.id === 'body' ? document.body : byId(a.id);
          if (!el) throw new Error('no ' + (a.selector || '#' + a.id));
          // A disclosure opens the way a click on its summary opens it.
          if ('open' in a) { if (el.open !== a.open) el.querySelector('summary').click(); continue; }
          if ('focus' in a) {
            // As the Tab key moves focus: the page has focus, and the focus is
            // keyboard-visible whatever the pointer did last, so a ring shows.
            window.focus();
            el.focus({ focusVisible: true });
            continue;
          }
          // A button pressed as a user presses it.
          if ('click' in a) { el.click(); continue; }
          // The page scrolled until the element is at the top of the window.
          if ('scroll' in a) { el.scrollIntoView({ block: 'start' }); continue; }
          // A wheel turned over the element, at its middle. Whether the page
          // cancelled it, so the browser would not also scroll the page, is
          // reported as wheelPrevented.
          if ('wheelY' in a) {
            const r = el.getBoundingClientRect();
            wheelPrevented = !el.dispatchEvent(new WheelEvent('wheel', {
              bubbles: true, cancelable: true, deltaY: a.wheelY, ctrlKey: !!a.ctrlKey,
              clientX: r.left + r.width / 2, clientY: r.top + r.height / 2,
            }));
            continue;
          }
          // A narrower or wider panel, as a user drags its edge.
          if ('width' in a) { el.style.width = a.width; resized = true; continue; }
          // An element taken out of the layout and put back. Not
          // waited on and no resize is nudged, so the page is left to notice.
          if ('display' in a) { el.style.display = a.display; continue; }
          // Keys typed into a field, one at a time, as a user types them. Unlike
          // setting the value, this keeps text a number field cannot parse
          // (1e999, a lone minus sign) in the field: its value reads '' and
          // validity.badInput is set. An empty string is the Delete key. The
          // page fires what the keys fire and nothing more: no change, no blur.
          if ('keys' in a) {
            if (document.activeElement !== el) { el.focus(); el.select(); }
            if (a.keys === '') document.execCommand('delete');
            else for (const ch of a.keys) document.execCommand('insertText', false, ch);
            continue;
          }
          // Text pasted into a field: a paste event carrying it goes to the
          // field, and if the page leaves it alone the text is inserted, as the
          // browser does by default. Then the change a user's Tab would fire.
          if ('paste' in a) {
            el.focus();
            el.select();
            const data = new DataTransfer();
            data.setData('text/plain', a.paste);
            const ev = new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true });
            if (el.dispatchEvent(ev)) document.execCommand('insertText', false, a.paste);
            el.dispatchEvent(new Event('change', { bubbles: true }));
            continue;
          }
          // Leaving the field, as Tab does: the browser fires a change only if
          // the value differs from when it was entered.
          if ('blur' in a) { el.blur(); continue; }
          if ('key' in a) {
            el.dispatchEvent(new KeyboardEvent('keydown', { key: a.key, bubbles: true }));
            continue;
          }
          if ('type' in a) {
            el.focus();
            el.select();
            document.execCommand('insertText', false, a.type);
          } else if ('checked' in a) el.checked = a.checked; else el.value = String(a.value);
          // A colour picker fires \`input\` as the colour is chosen, and the
          // panel marks the colour as the user's on it (#1340).
          if (el.type === 'color') el.dispatchEvent(new Event('input', { bubbles: true }));
          el.dispatchEvent(new Event('change', { bubbles: true }));
        }
      } catch (e) {
        failed(e);
        return;
      }
      const send = () => {
        let state;
        try { state = report(); } catch (e) {
          failed(e);
          return;
        }
        api.postMessage({ type: 'test:state', tag: m.tag, state });
      };
      if (resized) whenFitted(send); else send();
    });
  })();
`;

/** The page with the driver ahead of the panel's script, under its nonce. */
function withDriver(html: string): string {
  const nonce = /<script nonce="([^"]+)">/.exec(html);
  assert.ok(nonce, "the panel script carries a nonce");
  return html.replace("</head>", `<script nonce="${nonce[1]}">${DRIVER}</script>\n</head>`);
}

/** Where a click or a wheel landed, as client coordinates of the page, and
 *  the image's box (left, top, width, height) as the page had it then. */
export interface Pointed {
  point: [number, number];
  rect: [number, number, number, number];
}

export interface RealPanel {
  /** What the page sent the provider, oldest first. */
  received: Msg[];
  /** What the provider posted to the page, oldest first. */
  sent: Msg[];
  /** Set controls as a user would, then read the page back. */
  drive(
    actions: Array<{
      /** The control's id; or `selector`, for one with no id. */
      id?: string;
      selector?: string;
      value?: string | number;
      /** Text typed into the field with the keyboard, in place of its value. */
      type?: string;
      /** Keys typed one at a time, with no change or blur fired for them; an empty string is Delete. */
      keys?: string;
      /** Text pasted into the field, as a paste event carries it. */
      paste?: string;
      /** Leave the field. */
      blur?: boolean;
      /** Press this key in the field. */
      key?: string;
      checked?: boolean;
      open?: boolean;
      focus?: boolean;
      /** Click it, as a user presses a button. */
      click?: boolean;
      /** Scroll the page until it is at the top of the window. */
      scroll?: boolean;
      /** Turn the mouse wheel this far over it, with Ctrl held if `ctrlKey`. */
      wheelY?: number;
      ctrlKey?: boolean;
      width?: string;
      /** `none` takes it out of the layout, an empty string puts it back. */
      display?: string;
    }>,
  ): Promise<PageState>;
  /** Click the middle of the image, as a user reads the value there, or the
   *  point `fy` of the way down it and `fx` of the way across. Resolves once
   *  the page has sent the click; the reply comes after. */
  probe(fy?: number, fx?: number): Promise<Pointed>;
  /** Click the page at the client point `client`, wherever the image is
   *  now (#1228). */
  probeAt(client: [number, number]): Promise<Pointed>;
  /** Turn the mouse wheel by `deltaY` over the point `at` of the image, as
   *  fractions of its width and height from the top left, or over the client
   *  point `{ client }` of the page, wherever the image is now (#1228).
   *  Resolves to where it turned, where the image was, whether the page
   *  cancelled the wheel (one it lets through scrolls the page, #1269), and
   *  the image's preview transform before and after the turn: a wheel the
   *  page takes as a zoom changes it (#1257). */
  wheel(
    at: [number, number] | { client: [number, number] },
    deltaY: number,
  ): Promise<Pointed & { prevented: boolean; transform: [string, string] }>;
  /** Drag the image with the left button from `from` to `to`, fractions as
   *  for `wheel`, ending with the click a browser fires on release. With
   *  `outside`, the button comes up off the image, and no click follows.
   *  Either end can be a client point of the page instead (#1228). */
  drag(
    from: [number, number] | { client: [number, number] },
    to: [number, number] | { client: [number, number] },
    outside?: boolean,
  ): Promise<void>;
  /** Post `m` to the page as the provider would. */
  post(m: Msg): Promise<void>;
  /** Have the page post `m` to the provider exactly as given, unnumbered
   *  unless `m.page` says otherwise, as a page this panel showed before
   *  would have posted it (#1202). Resolves once the provider has heard it. */
  sendAsPage(m: Msg): Promise<void>;
  /** Every control on the page, by id (radio groups as `radio:<name>`). */
  controls(): Promise<Record<string, string | boolean>>;
  /** Resolves once a timer of `ms` the page sets now has fired: after every
   *  shorter wait the page started before it, such as a wheel settling,
   *  however much a hidden page's timers are held back. */
  pageTimer(ms: number): Promise<void>;
  /** Move every enabled control off its value, as a user would. `values`
   *  chooses a control's value; `skip` leaves controls alone. */
  perturb(
    skip: string[],
    values: Record<string, string | number>,
  ): Promise<{ moved: string[]; stuck: string[]; unknown: string[] }>;
  /** Wait for the provider to post a message matching `pred`, after `from`. */
  waitSent(pred: (m: Msg) => boolean, from: number, what: string): Promise<Msg>;
  /** Write the panel again the way an imported colormap does, which reloads
   *  the page from its HTML and restores its saved state. */
  rebuild(): void;
  /** Hide the panel's tab behind another editor and show it again, as a user
   *  switching tabs does. The panel keeps no context while hidden, so the page
   *  is loaded again from the HTML the provider last wrote, and restores its
   *  saved state. Resolves once the panel is visible again. */
  hideShow(): Promise<void>;
  /** Give this panel focus in its own column, as a click on its tab does.
   *  `workbench.action.focusActiveEditorGroup` focuses whichever group is
   *  active, which can hold another editor an earlier test left open
   *  (#1257). */
  takeFocus(): void;
  /** The page as the provider last wrote it, driver included. */
  html(): string;
  /** The panel's tab title, as the provider last set it. */
  title(): string;
  /** Run `cb` once, as the provider next writes the page, before the new page
   *  can send anything. */
  onNextWrite(cb: () => void): void;
  /** Keep back what the provider posts that `pred` accepts, as if it were
   *  still on its way. `deliver` posts the held messages `which` accepts, in
   *  order, and keeps holding; `release` posts the rest and stops. Held
   *  messages are in `sent` only once delivered. */
  hold(pred: (m: Msg) => boolean): {
    held: Msg[];
    deliver: (which: (m: Msg) => boolean) => Promise<void>;
    release: () => Promise<void>;
  };
  dispose(): void;
}

export async function waitUntil<T>(get: () => T | undefined, what: string, ms = 15_000): Promise<T> {
  const until = Date.now() + ms;
  for (;;) {
    const got = get();
    if (got !== undefined) return got;
    if (Date.now() > until) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 20));
  }
}

/** Run `open`, which makes `p` create one render panel, and hand back that real
 *  panel with the driver on its page and both directions recorded. */
export async function openReal(p: FieldglassEditorProvider, open: () => void): Promise<RealPanel> {
  const received: Msg[] = [];
  const sent: Msg[] = [];
  let made: vscode.WebviewPanel | undefined;
  let nextWrite: (() => void) | undefined;
  let holding: ((m: Msg) => boolean) | undefined;
  const held: Msg[] = [];
  let deliver: ((m: Msg) => Thenable<boolean>) | undefined;
  const origCreate = vscode.window.createWebviewPanel;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (vscode.window as any).createWebviewPanel = (...args: Parameters<typeof origCreate>) => {
    const panel = origCreate.apply(vscode.window, args);
    const webview = panel.webview;
    // The provider writes the page through `html`; the driver goes in on the way.
    let proto = Object.getPrototypeOf(webview);
    let html: PropertyDescriptor | undefined;
    while (proto && !(html = Object.getOwnPropertyDescriptor(proto, "html"))) {
      proto = Object.getPrototypeOf(proto);
    }
    assert.ok(html?.get && html.set, "the webview's html is an accessor");
    const { get, set } = html;
    Object.defineProperty(webview, "html", {
      configurable: true,
      get: () => get.call(webview),
      set: (v: string) => {
        set.call(webview, withDriver(v));
        const cb = nextWrite;
        nextWrite = undefined;
        cb?.();
      },
    });
    const post = webview.postMessage.bind(webview);
    deliver = (m: Msg) => {
      sent.push(m);
      return post(m);
    };
    webview.postMessage = (m: Msg) => {
      if (holding?.(m)) {
        held.push(m);
        return Promise.resolve(true);
      }
      sent.push(m);
      return post(m);
    };
    webview.onDidReceiveMessage((m) => received.push(m));
    made = panel;
    return panel;
  };
  try {
    open();
  } finally {
    vscode.window.createWebviewPanel = origCreate;
  }
  const panel = made;
  assert.ok(panel, "a render panel was created");
  // The builder `trackRenderPanel` keeps for this panel: what an imported
  // colormap runs to write every open panel again.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const builders = (p as any)._renderPanelBuilders as Map<unknown, () => void>;
  const build = builders.get(panel);
  assert.ok(build, "the panel is tracked for rebuilds");
  let tag = 0;
  const gesture = async (m: Msg): Promise<Msg> => {
    const mine = ++tag;
    await panel.webview.postMessage({ ...m, tag: mine });
    const reply = await waitUntil(
      () => received.find((r) => r.type === "test:gestured" && r.tag === mine),
      `the page's gesture ${mine}`,
    );
    assert.ok(reply.width > 0 && reply.height > 0, "the image is laid out, so the gesture lands on it");
    return reply;
  };
  const click = async (where: { fy?: number; fx?: number; client?: [number, number] }): Promise<Pointed> => {
    const mine = ++tag;
    await panel.webview.postMessage({ type: "test:probe", tag: mine, ...where });
    const reply = await waitUntil(
      () => received.find((m) => m.type === "test:probed" && m.tag === mine),
      `the page's click ${mine}`,
    );
    assert.ok(reply.width > 0 && reply.height > 0, "the image is laid out, so a click lands on it");
    return { point: reply.point, rect: reply.rect };
  };
  return {
    received,
    sent,
    drive: async (actions) => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:drive", tag: mine, actions });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:state" && m.tag === mine),
        `the page's report ${mine}`,
      );
      // The driver's own failure, rather than a timeout that hides it.
      if (reply.error) throw new Error(`the page's report ${mine} failed: ${reply.error}`);
      return reply.state as PageState;
    },
    probe: async (fy?: number, fx?: number) => click({ fy, fx }),
    probeAt: async (client) => click({ client }),
    wheel: async (at, deltaY) => {
      const reply = await gesture(
        Array.isArray(at) ? { type: "test:wheel", at, deltaY } : { type: "test:wheel", client: at.client, deltaY },
      );
      return { point: reply.point, rect: reply.rect, prevented: reply.prevented, transform: reply.transform };
    },
    drag: async (from, to, outside) => {
      await gesture({ type: "test:drag", from, to, outside: outside === true });
    },
    post: async (m) => {
      // Recorded in `sent` by the wrapper above, like the provider's own.
      await panel.webview.postMessage(m);
    },
    sendAsPage: async (m) => {
      const from = received.length;
      const wire = JSON.stringify(m);
      await panel.webview.postMessage({ type: "test:send", message: m });
      // The provider's handler and this recorder hear the same event.
      await waitUntil(
        () => (received.slice(from).some((r) => JSON.stringify(r) === wire) ? true : undefined),
        `the page's ${m.type}`,
      );
    },
    controls: async () => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:controls", tag: mine });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:controls" && m.tag === mine),
        `the page's controls ${mine}`,
      );
      return reply.controls;
    },
    pageTimer: async (ms) => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:timer", tag: mine, ms });
      await waitUntil(
        () => received.find((m) => m.type === "test:timer" && m.tag === mine),
        `the page's timer ${mine}`,
        60_000,
      );
    },
    perturb: async (skip, values) => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:perturb", tag: mine, skip, values });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:perturbed" && m.tag === mine),
        `the page's perturbation ${mine}`,
      );
      return { moved: reply.moved, stuck: reply.stuck, unknown: reply.unknown };
    },
    waitSent: (pred, from, what) =>
      waitUntil(() => sent.slice(from).find(pred), what).catch((err: Error) => {
        // Say what did arrive, which is usually the reason.
        const got = sent.slice(from).map((m) => (m.error ? `${m.type}: ${m.error}` : m.type));
        throw new Error(`${err.message}; the provider sent [${got.join(", ")}]`);
      }),
    rebuild: build,
    hideShow: async () => {
      const column = panel.viewColumn;
      const doc = await vscode.workspace.openTextDocument({ content: "" });
      await vscode.window.showTextDocument(doc, { viewColumn: column, preview: false });
      await waitUntil(() => (panel.visible ? undefined : true), "the panel hidden");
      panel.reveal(column, false);
      await waitUntil(() => (panel.visible ? true : undefined), "the panel shown");
      // The editor that hid it goes, unsaved.
      for (const group of vscode.window.tabGroups.all) {
        for (const tab of group.tabs) {
          if (tab.input instanceof vscode.TabInputText && tab.input.uri.toString() === doc.uri.toString()) {
            await vscode.window.tabGroups.close(tab, true);
          }
        }
      }
    },
    takeFocus: () => panel.reveal(panel.viewColumn, false),
    html: () => panel.webview.html,
    title: () => panel.title,
    onNextWrite: (cb) => {
      nextWrite = cb;
    },
    hold: (pred) => {
      assert.ok(!holding, "one hold at a time");
      holding = pred;
      const post = async (which: (m: Msg) => boolean) => {
        const send = deliver;
        assert.ok(send, "the panel was made");
        const go = held.filter(which);
        held.splice(0, held.length, ...held.filter((m) => !go.includes(m)));
        for (const m of go) await send(m);
      };
      return {
        held,
        deliver: post,
        release: async () => {
          holding = undefined;
          await post(() => true);
        },
      };
    },
    dispose: () => panel.dispose(),
  };
}

/** An editor opened on a file, through a stand-in editor webview whose message
 *  handler the test calls the way the table's page does. */
export interface Editor {
  /** Send the provider a message as the table's page would: numbered as the
   *  page the provider last wrote (#1217), unless `m` carries its own `page`. */
  send(m: object): void;
  /** The editor page's HTML as the provider last wrote it. */
  html(): string;
  /** Every message the provider has posted to the editor page. */
  sent: Msg[];
  /** The document the editor is open on. */
  document: FieldglassDocument;
  /** Close the editor tab, which drops the document's reader handle once no
   *  editor on it is left. */
  close(): void;
}

/** Open `file` in the editor. */
export async function openEditor(p: FieldglassEditorProvider, file: string): Promise<Editor> {
  const doc = (await p.openCustomDocument(
    vscode.Uri.file(file),
    {} as vscode.CustomDocumentOpenContext,
    new vscode.CancellationTokenSource().token,
  )) as FieldglassDocument;
  let handler: ((m: object) => void) | undefined;
  const sent: Msg[] = [];
  const onDispose: Array<() => void> = [];
  const editor = {
    webview: {
      html: "",
      options: {},
      cspSource: "vscode-webview:",
      onDidReceiveMessage: (cb: (m: object) => void) => {
        handler = cb;
        return { dispose: () => undefined };
      },
      postMessage: (m: Msg) => {
        sent.push(m);
        return Promise.resolve(true);
      },
    },
    onDidDispose: (cb: () => void) => {
      onDispose.push(cb);
      return { dispose: () => undefined };
    },
    onDidChangeViewState: () => ({ dispose: () => undefined }),
    dispose: () => undefined,
  };
  await p.resolveCustomEditor(doc, editor as unknown as vscode.WebviewPanel);
  assert.ok(handler, "the editor registers a message handler");
  const send = handler;
  return {
    send: (m) => send({ page: pageNumberOf(editor.webview.html), ...m }),
    html: () => editor.webview.html,
    sent,
    document: doc,
    close: () => {
      for (const cb of onDispose.splice(0)) cb();
    },
  };
}

/** The number the provider wrote the page `html` with, which the page sends
 *  back on every message (#1202). A stand-in webview that posts what the page
 *  would has to carry it too, or the provider takes the message for a page it
 *  has replaced. */
export function pageNumberOf(html: string): number {
  const page = /const PAGE = (\d+);/.exec(html);
  assert.ok(page, "the panel script is numbered");
  return Number(page[1]);
}

export const isType = (type: string) => (m: Msg) => m && m.type === type;
export const overlayTraffic = (m: Msg) =>
  m && ["overlayRequest", "contourRequest", "vectorRequest"].includes(m.type);
export const refusals = (m: Msg) =>
  m && ["overlayError", "contourError", "gridError"].includes(m.type);

/** A classic NetCDF (CDF-1) file, written by hand: `dims` by name and length,
 *  and float64 variables over them with text attributes, in that order. Enough
 *  of the format for a test file the editor opens like any other. */
export function classicNetcdf(
  dims: Array<[string, number]>,
  vars: Array<{ name: string; dims: string[]; attrs: Record<string, string>; values: number[] }>,
): Buffer {
  const parts: Buffer[] = [];
  const int = (v: number) => {
    const b = Buffer.alloc(4);
    b.writeInt32BE(v);
    return b;
  };
  const name = (s: string) => {
    const text = Buffer.from(s, "utf8");
    return Buffer.concat([int(text.length), text, Buffer.alloc((4 - (text.length % 4)) % 4)]);
  };
  const header = (begins: number[]) => {
    const out: Buffer[] = [Buffer.from("CDF\x01", "latin1"), int(0)];
    out.push(int(0x0a), int(dims.length));
    for (const [n, len] of dims) out.push(name(n), int(len));
    out.push(int(0), int(0)); // no global attributes
    out.push(int(0x0b), int(vars.length));
    vars.forEach((v, k) => {
      out.push(name(v.name), int(v.dims.length));
      for (const d of v.dims) out.push(int(dims.findIndex(([n]) => n === d)));
      const attrs = Object.entries(v.attrs);
      out.push(int(0x0c), int(attrs.length));
      for (const [an, av] of attrs) out.push(name(an), int(2), name(av)); // NC_CHAR
      out.push(int(6), int(v.values.length * 8), int(begins[k])); // NC_DOUBLE
    });
    return Buffer.concat(out);
  };
  // The header's length does not depend on the offsets in it.
  let at = header(vars.map(() => 0)).length;
  const begins = vars.map((v) => {
    const b = at;
    at += v.values.length * 8;
    return b;
  });
  parts.push(header(begins));
  for (const v of vars) {
    const data = Buffer.alloc(v.values.length * 8);
    v.values.forEach((x, i) => data.writeDoubleBE(x, i * 8));
    parts.push(data);
  }
  return Buffer.concat(parts);
}

/** An 8-bit RGB or RGBA PNG, non-interlaced, as canvas `toDataURL` writes. */
export function decodePng(png: Buffer): { width: number; height: number; rgba: Uint8Array } {
  assert.strictEqual(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a", "a PNG signature");
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat: Buffer[] = [];
  while (at < png.length) {
    const length = png.readUInt32BE(at);
    const type = png.toString("ascii", at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      assert.strictEqual(data[8], 8, "8-bit samples");
      assert.ok(data[9] === 6 || data[9] === 2, `RGB or RGBA, got colour type ${data[9]}`);
      channels = data[9] === 6 ? 4 : 3;
      assert.strictEqual(data[12], 0, "not interlaced");
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    at += 12 + length;
  }
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const rows = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? rows[y * stride + x - channels] : 0;
      const b = y > 0 ? rows[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? rows[(y - 1) * stride + x - channels] : 0;
      let v = line[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += Math.floor((a + b) / 2);
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      rows[y * stride + x] = v & 0xff;
    }
  }
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    for (let k = 0; k < 3; k++) rgba[i * 4 + k] = rows[i * channels + k];
    rgba[i * 4 + 3] = channels === 4 ? rows[i * channels + 3] : 255;
  }
  return { width, height, rgba };
}
