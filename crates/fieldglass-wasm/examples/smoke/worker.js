// The Worker the smoke page runs everything in.
//
// Decode is synchronous and can take hundreds of milliseconds on a real field,
// so it belongs off the main thread — and `panic = "abort"` means a decoder
// panic kills this Worker rather than the page. The host's job is to treat it
// as disposable and start another.
//
// What it proves: a WebGL `readPixels` of the exported shader over the field
// and its `Palette` equals `render()`'s RGBA from the same options.

import init, { open, glslSnippet } from '../../pkg/web/fieldglass_wasm.js';

const VERTEX = `#version 300 es
in vec2 a_pos;
out vec2 v_uv;
void main() {
    v_uv = a_pos * 0.5 + 0.5;
    gl_Position = vec4(a_pos, 0.0, 1.0);
}
`;

/** Wrap the exported snippet in the smallest fragment shader that uses it. */
function fragmentShader() {
  return `#version 300 es
precision highp float;
in vec2 v_uv;
out vec4 fragColor;

${glslSnippet()}

void main() {
    fragColor = fieldglassColor(v_uv);
}
`;
}

function compile(gl, kind, source) {
  const shader = gl.createShader(kind);
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`shader did not compile: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

function program(gl) {
  const p = gl.createProgram();
  gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, VERTEX));
  gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, fragmentShader()));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    throw new Error(`program did not link: ${gl.getProgramInfoLog(p)}`);
  }
  return p;
}

function texture(gl, unit, internalFormat, format, type, width, height, data, filter) {
  const tex = gl.createTexture();
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  gl.texImage2D(gl.TEXTURE_2D, 0, internalFormat, width, height, 0, format, type, data);
  return tex;
}

async function run({ bytes, canvas, messageIndex, paletteOptions }) {
  await init({ module_or_path: new URL('../../pkg/web/fieldglass_wasm_bg.wasm', import.meta.url) });

  const t0 = performance.now();
  const handle = open(new Uint8Array(bytes));
  const field = handle.decode(messageIndex, {});
  const decodeMs = performance.now() - t0;

  const ni = field.ni();
  const nj = field.nj();
  const grid = field.grid();
  const palette = handle.palette(field, paletteOptions);

  // The CPU oracle: exactly what a canvas would show without a GPU.
  const cpu = handle.render(field, paletteOptions, false);

  // The GPU path: the same palette, the same field, through the exported
  // shader and nothing else.
  const shaderValues = handle.shaderValues(field, paletteOptions);
  const shaderMask = handle.shaderMask(field, paletteOptions);

  canvas.width = ni;
  canvas.height = nj;
  const gl = canvas.getContext('webgl2', { preserveDrawingBuffer: true, antialias: false });
  if (!gl) throw new Error('this browser has no WebGL2 context');

  const p = program(gl);
  gl.useProgram(p);

  const quad = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, quad);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(p, 'a_pos');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);

  // NEAREST throughout: the lookup table is a step function and the field is
  // compared texel-for-texel against the CPU painter, so any filtering would
  // be comparing a blend against a value.
  texture(gl, 0, gl.R32F, gl.RED, gl.FLOAT, ni, nj, shaderValues, gl.NEAREST);
  texture(gl, 1, gl.R8, gl.RED, gl.UNSIGNED_BYTE, ni, nj, shaderMask, gl.NEAREST);
  texture(gl, 2, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, 256, 1, palette.lut, gl.NEAREST);

  gl.uniform1i(gl.getUniformLocation(p, 'u_field'), 0);
  gl.uniform1i(gl.getUniformLocation(p, 'u_mask'), 1);
  gl.uniform1i(gl.getUniformLocation(p, 'u_lut'), 2);
  gl.uniform1f(gl.getUniformLocation(p, 'u_span'), palette.span);
  const m = palette.maskedRgba;
  gl.uniform4f(
    gl.getUniformLocation(p, 'u_maskedRgba'),
    m[0] / 255,
    m[1] / 255,
    m[2] / 255,
    m[3] / 255,
  );

  gl.viewport(0, 0, ni, nj);
  gl.clearColor(0, 0, 0, 0);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.TRIANGLES, 0, 3);

  const gpu = new Uint8Array(ni * nj * 4);
  gl.readPixels(0, 0, ni, nj, gl.RGBA, gl.UNSIGNED_BYTE, gpu);

  // No flip. Two conventions cancel: `texImage2D` puts data row 0 at `v = 0`,
  // which GL draws at the *bottom* of the framebuffer, and `readPixels` starts
  // at the bottom too — so its row `j` is data row `j`, the same row
  // `render(.., flipY = false)` emits `j`th. Flipping "to correct for
  // readPixels" mirrors the image and makes 97% of the pixels disagree, which
  // is how this comment came to be written.
  //
  // The comparison is on the *lookup index*, not on the RGB triple: the
  // acceptance rule allows one index at a bin edge, and two adjacent entries of
  // a smooth ramp can differ by several units per channel. The index is
  // recovered by finding the palette entry the pixel came from.
  const lut = palette.lut;
  const indexOf = new Map();
  for (let i = 0; i < 256; i++) {
    indexOf.set(`${lut[i * 4]},${lut[i * 4 + 1]},${lut[i * 4 + 2]}`, i);
  }
  const maskedKey = `${m[0]},${m[1]},${m[2]}`;

  let compared = 0;
  let offByOne = 0;
  let mismatched = 0;
  let firstMismatch = null;
  for (let j = 0; j < nj; j++) {
    const gpuRow = j * ni;
    for (let i = 0; i < ni; i++) {
      const g = (gpuRow + i) * 4;
      const c = (j * ni + i) * 4;
      const gKey = `${gpu[g]},${gpu[g + 1]},${gpu[g + 2]}`;
      const cKey = `${cpu[c]},${cpu[c + 1]},${cpu[c + 2]}`;
      if (gKey === cKey && gpu[g + 3] === cpu[c + 3]) {
        compared++;
        continue;
      }
      // One of them painted the masked colour and the other did not: that is a
      // hole in the picture, never an acceptable rounding difference.
      if (gKey === maskedKey || cKey === maskedKey) {
        mismatched++;
        if (!firstMismatch) firstMismatch = { i, j, reason: 'mask' };
        continue;
      }
      const gi = indexOf.get(gKey);
      const ci = indexOf.get(cKey);
      compared++;
      if (gi !== undefined && ci !== undefined && Math.abs(gi - ci) <= 1) {
        offByOne++;
      } else {
        mismatched++;
        if (!firstMismatch) firstMismatch = { i, j, gpu: gKey, cpu: cKey, gi, ci };
      }
    }
  }

  const bitmap = canvas.transferToImageBitmap ? canvas.transferToImageBitmap() : null;
  const report = {
    ok: mismatched === 0,
    ni,
    nj,
    kind: grid.kind,
    proj4: grid.proj4,
    dtype: field.dtype(),
    parameter: field.parameter(),
    units: field.units(),
    stats: field.stats(),
    decodeMs: Math.round(decodeMs),
    compared,
    offByOne,
    mismatched,
    firstMismatch,
  };

  field.free();
  handle.free();
  return { report, bitmap };
}

self.onmessage = async (event) => {
  try {
    const { report, bitmap } = await run(event.data);
    self.postMessage({ report, bitmap }, bitmap ? [bitmap] : []);
  } catch (e) {
    self.postMessage({ error: `${e && e.code ? `[${e.code}] ` : ''}${e.message || e}` });
  }
};
