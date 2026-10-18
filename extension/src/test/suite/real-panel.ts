// A render panel driven in a real webview, for the tests that need the panel's
// own script to run (#840, #839, #841).
//
// The panel is opened the way a user opens it, through the editor's message
// handler, and a small driver script added to its page flips the controls a
// user would and reports what the page shows. A test then reads what crossed
// the boundary in both directions: the requests the panel sent, and the
// provider's answers.

import * as assert from "assert";
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
  /** Everything the page shows a user: its rendered text, and every tooltip
   *  and accessible label on it (#1038). */
  text: string;
  /** The status line under the image. */
  status: string;
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
  canvasVisibility: string;
  /** The line plot (#172, #1037), `null` for a panel with no Line plot box. */
  linePlot: LinePlotState | null;
  /** A slice panel's axis rails (#171, #1043), `null` for a GRIB panel. */
  rails: RailsState | null;
  /** A slice panel's index steppers (#1041), `null` for a GRIB panel. */
  steppers: SteppersState | null;
  /** The Animate row's label, `null` for a panel with no Animate row. */
  animateLabel: string | null;
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
  /** The image, the colour bar, and the cross-section note above. */
  image: Box;
  colorbar: Box | null;
  note: Box | null;
}

/** Added to the panel's page ahead of its own script. It keeps the webview API
 *  the panel acquires, so it can answer on the same channel, and on each
 *  `test:drive` sets the named controls, fires their `change` like a user's
 *  edit, and reports the page. `test:probe` clicks the middle of the image, as
 *  a user reads a value. A message is handled after every one posted before
 *  it, so the report follows whatever the page did with those. */
const DRIVER = `
  (function () {
    const acquire = window.acquireVsCodeApi;
    let api = null;
    window.acquireVsCodeApi = function () {
      if (!api) api = acquire();
      return api;
    };
    const byId = (id) => document.getElementById(id);
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
      const toggle = byId('line-toggle');
      if (!toggle) return null;
      const label = byId('line-axis-label');
      const read = (node) => node.tagName === 'SELECT'
        ? (node.selectedOptions[0] ? node.selectedOptions[0].textContent : '')
        : node.textContent;
      return {
        boxShown: labelOf('line-toggle').getClientRects().length > 0,
        checked: toggle.checked,
        shown: byId('line-panel').getClientRects().length > 0,
        label: label.getClientRects().length > 0 ? Array.from(label.childNodes).map(read).join('') : null,
        caption: byId('line-caption').textContent,
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
        image: boxOf(byId('canvas')),
        colorbar: colorbar && colorbar.getClientRects().length > 0 ? boxOf(colorbar) : null,
        note: note && note.getClientRects().length > 0 ? boxOf(note) : null,
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
        overlayHidden: !!(overlays && overlays.hasAttribute('hidden')),
        vectorNote: vectors ? byId('vector-note').textContent : null,
        coastlines: byId('overlay-coastlines').checked,
        graticule: byId('overlay-graticule').checked,
        contours: byId('overlay-contours').checked,
        controlDisplay: controlsBy((el) => getComputedStyle(el).display),
        controlShown: controlsBy((el) => el.getClientRects().length > 0),
        unavailable: unavailableLine(),
        focused: document.activeElement ? describe(document.activeElement) + (document.activeElement.closest('#unavailable') ? ' in #unavailable' : '') : null,
        projection: picker.value,
        projectionDisabled: picker.disabled,
        projectionOffers: Array.from(picker.options).map((o) => o.value),
        probe: byId('probe').textContent,
        sliceVariable: variable ? (chosen ? chosen.textContent : '') : null,
        sliceVariables: variable ? Array.from(variable.options).map((o) => o.textContent) : null,
        title: byId('title-line').textContent,
        subtitle: document.querySelector('.subtitle').textContent,
        text: [document.body.innerText].concat(
          Array.from(document.querySelectorAll('[title], [aria-label], [placeholder]'))
            .filter((el) => el.getClientRects().length > 0)
            .flatMap((el) => ['title', 'aria-label', 'placeholder'].map((a) => el.getAttribute(a) || '')),
        ).join('\\n'),
        status: byId('status').textContent,
        playDisabled: !!(byId('anim-play') && byId('anim-play').disabled),
        canvasVisibility: byId('canvas').style.visibility,
        linePlot: linePlot(),
        rails: rails(),
        steppers: steppers(),
        animateLabel: byId('anim-frame') ? byId('anim-frame').textContent : null,
      };
    }
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
      if (m && m.type === 'test:perturb') {
        const done = perturb(m.skip, m.values);
        api.postMessage({ type: 'test:perturbed', tag: m.tag, moved: done.moved, stuck: done.stuck, unknown: done.unknown });
        return;
      }
      if (!m || (m.type !== 'test:drive' && m.type !== 'test:probe')) return;
      if (m.type === 'test:probe') {
        const r = byId('canvas').getBoundingClientRect();
        document.querySelector('.canvas-wrap').dispatchEvent(new MouseEvent('click', {
          bubbles: true,
          clientX: r.left + r.width / 2,
          // Halfway down unless the test names a fraction of the height.
          clientY: r.top + r.height * (typeof m.fy === 'number' ? m.fy : 0.5),
        }));
        api.postMessage({ type: 'test:probed', tag: m.tag, width: r.width, height: r.height });
        return;
      }
      // A width change is fitted by the page's ResizeObserver, whenever the
      // browser delivers it: the report waits until the page says it has
      // fitted the rails and the plot to the sizes they now have (#1043).
      let resized = false;
      for (const a of m.actions) {
        // A control with no id (a slice stepper) is named by a selector.
        const el = a.selector ? document.querySelector(a.selector) : a.id === 'body' ? document.body : byId(a.id);
        if (!el) throw new Error('no ' + (a.selector || '#' + a.id));
        // A disclosure opens the way a click on its summary opens it.
        if ('open' in a) { if (el.open !== a.open) el.querySelector('summary').click(); continue; }
        if ('focus' in a) { el.focus(); continue; }
        // A narrower or wider panel, as a user drags its edge.
        if ('width' in a) { el.style.width = a.width; resized = true; continue; }
        if ('checked' in a) el.checked = a.checked; else el.value = String(a.value);
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      const send = () => api.postMessage({ type: 'test:state', tag: m.tag, state: report() });
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
      checked?: boolean;
      open?: boolean;
      focus?: boolean;
      width?: string;
    }>,
  ): Promise<PageState>;
  /** Click the middle of the image, as a user reads the value there, or the
   *  point `fy` of the way down it. Resolves once the page has sent the click;
   *  the reply comes after. */
  probe(fy?: number): Promise<void>;
  /** Post `m` to the page as the provider would. */
  post(m: Msg): Promise<void>;
  /** Every control on the page, by id (radio groups as `radio:<name>`). */
  controls(): Promise<Record<string, string | boolean>>;
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
  /** The page as the provider last wrote it, driver included. */
  html(): string;
  /** The panel's tab title, as the provider last set it. */
  title(): string;
  /** Run `cb` once, as the provider next writes the page, before the new page
   *  can send anything. */
  onNextWrite(cb: () => void): void;
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
    webview.postMessage = (m: Msg) => {
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
      return reply.state as PageState;
    },
    probe: async (fy?: number) => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:probe", tag: mine, fy });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:probed" && m.tag === mine),
        `the page's click ${mine}`,
      );
      assert.ok(reply.width > 0 && reply.height > 0, "the image is laid out, so a click lands on it");
    },
    post: async (m) => {
      // Recorded in `sent` by the wrapper above, like the provider's own.
      await panel.webview.postMessage(m);
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
    html: () => panel.webview.html,
    title: () => panel.title,
    onNextWrite: (cb) => {
      nextWrite = cb;
    },
    dispose: () => panel.dispose(),
  };
}

/** An editor opened on a file, through a stand-in editor webview whose message
 *  handler the test calls the way the table's page does. */
export interface Editor {
  /** Send the provider a message as the table's page would. */
  send(m: object): void;
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
      postMessage: () => Promise.resolve(true),
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
    send,
    close: () => {
      for (const cb of onDispose.splice(0)) cb();
    },
  };
}

export const isType = (type: string) => (m: Msg) => m && m.type === type;
export const overlayTraffic = (m: Msg) =>
  m && ["overlayRequest", "contourRequest", "vectorRequest"].includes(m.type);
export const refusals = (m: Msg) =>
  m && ["overlayError", "contourError", "gridError"].includes(m.type);
