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

/** What the driver reports of the page. */
export interface PageState {
  overlayDisabled: boolean;
  overlayHidden: boolean;
  /** The Overlay row's note when shown, `null` when hidden. */
  overlayNote: string | null;
  vectorDisabled: boolean | null;
  vectorNote: string | null;
  coastlines: boolean;
  graticule: boolean;
  contours: boolean;
  /** Whether the Contours checkbox is disabled, and the note beside it when
   *  shown (#945). */
  contoursDisabled: boolean;
  contourNote: string | null;
  /** Whether the Zonal average toggle is disabled, and its note when shown. */
  zonalDisabled: boolean;
  zonalNote: string | null;
  projection: string;
  /** Every target the projection picker offers, in order. */
  projectionOffers: string[];
  /** The note beside the picker when shown, `null` when hidden. */
  reprojectNote: string | null;
  /** The point-probe readout. */
  probe: string;
  /** A slice panel's variable picker: the name selected and every name
   *  offered, `null` for a GRIB panel. */
  sliceVariable: string | null;
  sliceVariables: string[] | null;
  /** The panel heading. */
  title: string;
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
    const shownText = (el) => (el && !el.hasAttribute('hidden') ? el.textContent : null);
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
        overlayDisabled: !!(overlays && overlays.disabled),
        overlayHidden: !!(overlays && overlays.hasAttribute('hidden')),
        overlayNote: shownText(byId('overlay-note')),
        vectorDisabled: vectors ? vectors.disabled : null,
        vectorNote: vectors ? byId('vector-note').textContent : null,
        coastlines: byId('overlay-coastlines').checked,
        graticule: byId('overlay-graticule').checked,
        contours: byId('overlay-contours').checked,
        contoursDisabled: byId('overlay-contours').disabled,
        contourNote: shownText(byId('contour-note')),
        zonalDisabled: byId('zonal-toggle').disabled,
        zonalNote: shownText(byId('zonal-note')),
        projection: picker.value,
        projectionOffers: Array.from(picker.options).map((o) => o.value),
        reprojectNote: shownText(byId('reproject-note')),
        probe: byId('probe').textContent,
        sliceVariable: variable ? (chosen ? chosen.textContent : '') : null,
        sliceVariables: variable ? Array.from(variable.options).map((o) => o.textContent) : null,
        title: byId('title-line').textContent,
        status: byId('status').textContent,
        playDisabled: !!(byId('anim-play') && byId('anim-play').disabled),
        canvasVisibility: byId('canvas').style.visibility,
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
          clientY: r.top + r.height / 2,
        }));
        api.postMessage({ type: 'test:probed', tag: m.tag, width: r.width, height: r.height });
        return;
      }
      for (const a of m.actions) {
        const el = byId(a.id);
        if (!el) throw new Error('no #' + a.id);
        if ('checked' in a) el.checked = a.checked; else el.value = String(a.value);
        el.dispatchEvent(new Event('change', { bubbles: true }));
      }
      api.postMessage({ type: 'test:state', tag: m.tag, state: report() });
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
  drive(actions: Array<{ id: string; value?: string | number; checked?: boolean }>): Promise<PageState>;
  /** Click the middle of the image, as a user reads the value there. Resolves
   *  once the page has sent the click; the reply comes after. */
  probe(): Promise<void>;
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
    probe: async () => {
      const mine = ++tag;
      await panel.webview.postMessage({ type: "test:probe", tag: mine });
      const reply = await waitUntil(
        () => received.find((m) => m.type === "test:probed" && m.tag === mine),
        `the page's click ${mine}`,
      );
      assert.ok(reply.width > 0 && reply.height > 0, "the image is laid out, so a click lands on it");
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
