/**
 * The field: everything soft behind Ghost, in one WebGL2 draw per frame. SPIKE.
 *
 * One full-screen triangle and one fragment shader, drawn at a fraction of the screen's
 * resolution and stretched by the compositor: the dim and the vignette over the desktop, the
 * haze (two octaves of value noise drifting), the scanned light of the wake, faint scan
 * lines, the glow behind Wisp, the operator's voice as a ring of energy, the beams to the
 * apps and agents Ghost is reaching, and dust. The glow is analytic (a falloff from a
 * distance), so there is no bloom pass, no framebuffer and no texture: the look of bloom at
 * the cost of one pass. Everything is driven by uniforms the page sets each frame.
 *
 * Nothing here loops by itself: the page calls `draw()` from its own frame loop, which runs
 * only while Ghost is on screen.
 */

export interface FieldInputs {
  time: number;
  /** 0..1, how dark the desktop goes behind Ghost. */
  dim: number;
  /** 0..1 position of the scanned light down the screen; < 0 none. */
  scan: number;
  /** Glow centre in CSS pixels, and its strength 0..1. */
  core: [number, number];
  glow: number;
  /** RGB 0..1 of the current phase's light. */
  tint: [number, number, number];
  /** The operator's voice as energy 0..1 (listening), and how much the ring shows. */
  voice: number;
  ring: number;
  /** Ghost's own voice level 0..1 (speaking). */
  level: number;
  /** Up to 6 reach targets in CSS pixels, each with its activity 0..1. */
  nodes: { x: number; y: number; on: number }[];
  /** 0..1 amber "waiting for your yes". */
  okay: number;
  /** 0..1 progress of the arrival shockwave; < 0 none. */
  shock: number;
}

export const VERT = `#version 300 es
void main() {
  vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

export const FRAG = `#version 300 es
precision highp float;
uniform vec2 uRes;        // drawing buffer size in pixels
uniform float uScale;     // drawing pixels per CSS pixel
uniform float uTime, uDim, uScan, uGlow, uVoice, uRing, uLevel, uOkay, uShock;
uniform vec2 uCore;
uniform vec3 uTint;
uniform vec3 uNodes[6];   // x, y (CSS px), activity
out vec4 outColor;

float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), u.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), u.x), u.y);
}
float segDist(vec2 p, vec2 a, vec2 b) {
  vec2 pa = p - a, ba = b - a;
  float h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
  return length(pa - ba * h);
}

void main() {
  vec2 frag = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y) / uScale; // CSS px, y down
  vec2 css = uRes / uScale;
  vec2 uv = frag / css;
  float minDim = min(css.x, css.y);

  // The dim: darker at the edges (vignette), lifted around Ghost.
  vec2 dc = (frag - uCore) / minDim;
  float vign = smoothstep(0.15, 1.1, length((uv - 0.5) * vec2(css.x / css.y, 1.0)));
  float shade = uDim * (0.72 + 0.28 * vign);

  // Haze: slow drifting noise, brighter near the core.
  float t = uTime;
  vec2 hp = frag / 420.0;
  float haze = noise(hp + vec2(t * 0.020, -t * 0.012)) * 0.6 + noise(hp * 2.3 - vec2(t * 0.031, t * 0.017)) * 0.4;
  float nearCore = exp(-dot(dc, dc) * 2.2);
  vec3 col = uTint * haze * haze * (0.05 + 0.20 * nearCore) * uDim * 1.6;

  // Glow behind Wisp: a tight core and a wide falloff (bloom without a pass).
  float d = length(dc);
  float ang = atan(dc.y, dc.x);
  float g = uGlow * (0.018 / (d * d + 0.0045) * 0.06 + exp(-d * 4.5) * 0.35) * (0.85 + 0.6 * uLevel);
  col += uTint * g;

  // An anamorphic streak through the core, the light of a lens that isn't there.
  float streak = exp(-abs(dc.y) * 160.0) * exp(-abs(dc.x) * 2.4);
  col += mix(uTint, vec3(1.0), 0.35) * streak * uGlow * (0.30 + 0.5 * uLevel);

  // The instrument: a hairline ring of ticks around Ghost, turning slowly. Each tick grows
  // with the voice in its direction: the operator's while listening, Ghost's own while it
  // speaks. Energy, never words.
  float R = 0.2;
  float turn = ang + t * 0.05;
  float ticks = 120.0;
  float k = floor((turn / 6.2831853 + 0.5) * ticks);
  float tickA = fract((turn / 6.2831853 + 0.5) * ticks);
  float drive = max(uVoice * uRing, uLevel * 0.9);
  float amp = noise(vec2(k * 0.37, t * 3.0)) * drive;
  float len = 0.008 + 0.05 * amp + (mod(k, 10.0) < 0.5 ? 0.012 : 0.0);
  float inTick = step(abs(tickA - 0.5), 0.12) * step(R, d) * step(d, R + len);
  col += uTint * inTick * uGlow * (0.22 + 1.4 * amp);
  col += uTint * exp(-abs(d - R) * 900.0) * 0.18 * uGlow;
  col += uTint * exp(-abs(d - (R + 0.085)) * 1400.0) * 0.08 * uGlow * step(0.5, fract(turn * 12.0 / 6.2831853));

  // The arrival: one shockwave out of the core.
  if (uShock >= 0.0) {
    float sr = uShock * 0.9;
    col += mix(uTint, vec3(1.0), 0.5) * exp(-abs(d - sr) * 90.0) * (1.0 - uShock) * 1.2;
  }

  // The operator's voice: a ring of energy that thickens and ripples with the level.
  float ripple = noise(vec2(ang * 3.0 + t * 1.7, t * 0.9)) * 2.0 - 1.0;
  float r0 = 0.15 + 0.03 * uVoice * ripple + 0.015 * sin(ang * 6.0 + t * 2.0) * uVoice;
  float ringD = abs(d - r0);
  float ringW = 0.0025 + 0.012 * uVoice;
  col += uTint * uRing * (ringW * ringW / (ringD * ringD + ringW * ringW)) * (0.6 + 1.6 * uVoice);
  // A second, fainter ring trailing outwards on loud moments.
  float r1 = r0 + 0.06 + 0.05 * fract(t * 0.6);
  col += uTint * uRing * uVoice * 0.25 * exp(-abs(d - r1) * 220.0);

  // Reach: beams from the core to each app or agent, a node glow, a packet travelling out.
  for (int i = 0; i < 6; i++) {
    vec3 n = uNodes[i];
    if (n.z <= 0.001) continue;
    float bd = segDist(frag, uCore, n.xy) / minDim;
    float flow = fract(t * 0.9 + float(i) * 0.37);
    vec2 pkt = mix(uCore, n.xy, flow);
    float pd = length(frag - pkt) / minDim;
    col += uTint * n.z * (0.000006 / (bd * bd + 0.000006) * 0.55 + 0.00006 / (pd * pd + 0.00006) * 0.7);
    float nd = length(frag - n.xy) / minDim;
    col += uTint * n.z * (0.0004 / (nd * nd + 0.0004)) * 0.5;
  }

  // Waiting for a yes: an amber horizon under Ghost.
  vec3 amber = vec3(1.0, 0.66, 0.18);
  float horizon = exp(-abs(uv.y - 0.86) * 38.0) * smoothstep(0.5, 0.0, abs(uv.x - 0.5));
  col += amber * uOkay * horizon * 0.55;

  // The scanned light: a band sweeping down, a hot line at its front.
  if (uScan >= 0.0) {
    float s = uv.y - uScan;
    col += uTint * (exp(-abs(s) * 520.0) * 0.9 + exp(-max(-s, 0.0) * 18.0) * step(s, 0.0) * 0.07);
  }

  // Dust: a sparse grid of motes drifting up through the haze.
  vec2 gp = frag / 46.0 + vec2(0.0, t * 0.12);
  vec2 cell = floor(gp);
  float h = hash(cell);
  vec2 mote = cell + vec2(hash(cell + 7.1), hash(cell + 3.3));
  float md = length(gp - mote);
  float twinkle = 0.5 + 0.5 * sin(t * (1.0 + 2.0 * h) + h * 40.0);
  col += uTint * step(0.86, h) * exp(-md * 18.0) * twinkle * uDim * 0.5 * (0.4 + nearCore);

  // Scan lines over the lit area only.
  float lines = 0.92 + 0.08 * sin(frag.y * 3.14159 * 0.5);
  col *= lines;

  // Premultiplied: black at 'shade' alpha, light added on top.
  float light = max(col.r, max(col.g, col.b));
  float alpha = clamp(shade + light, 0.0, 1.0);
  outColor = vec4(col, alpha);
}`;

export interface Field {
  draw(i: FieldInputs): void;
  resize(): void;
  clear(): void;
  /** False when WebGL2 is missing: the page falls back to CSS only. */
  ok: boolean;
  renderer: string;
}

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const s = gl.createShader(type)!;
  gl.shaderSource(s, src);
  gl.compileShader(s);
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
  return s;
}

/** `resolution` is drawing pixels per CSS pixel (0.5 = a quarter of the pixels). */
export function createField(canvas: HTMLCanvasElement, resolution = 0.5): Field {
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: false, depth: false, stencil: false, powerPreference: "low-power" });
  if (!gl) return { ok: false, renderer: "none", draw() {}, resize() {}, clear() {} };
  const prog = gl.createProgram()!;
  gl.attachShader(prog, compile(gl, gl.VERTEX_SHADER, VERT));
  gl.attachShader(prog, compile(gl, gl.FRAGMENT_SHADER, FRAG));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? "link");
  gl.useProgram(prog);
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);
  const u = (n: string) => gl.getUniformLocation(prog, n);
  const loc = {
    res: u("uRes"), scale: u("uScale"), time: u("uTime"), dim: u("uDim"), scan: u("uScan"), glow: u("uGlow"),
    voice: u("uVoice"), shock: u("uShock"), ring: u("uRing"), level: u("uLevel"), okay: u("uOkay"), core: u("uCore"), tint: u("uTint"), nodes: u("uNodes"),
  };
  const dbg = gl.getExtension("WEBGL_debug_renderer_info");
  const renderer = dbg ? String(gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL)) : String(gl.getParameter(gl.RENDERER));
  let scale = 1;
  const nodes = new Float32Array(18);

  const resize = () => {
    scale = (window.devicePixelRatio || 1) * resolution;
    const w = Math.max(1, Math.round(canvas.clientWidth * scale));
    const h = Math.max(1, Math.round(canvas.clientHeight * scale));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
  };
  resize();

  return {
    ok: true,
    renderer,
    resize,
    clear() {
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
    },
    draw(i) {
      gl.uniform2f(loc.res, canvas.width, canvas.height);
      gl.uniform1f(loc.scale, scale);
      gl.uniform1f(loc.time, i.time);
      gl.uniform1f(loc.dim, i.dim);
      gl.uniform1f(loc.scan, i.scan);
      gl.uniform1f(loc.glow, i.glow);
      gl.uniform1f(loc.voice, i.voice);
      gl.uniform1f(loc.ring, i.ring);
      gl.uniform1f(loc.level, i.level);
      gl.uniform1f(loc.okay, i.okay);
      gl.uniform1f(loc.shock, i.shock);
      gl.uniform2f(loc.core, i.core[0], i.core[1]);
      gl.uniform3f(loc.tint, i.tint[0], i.tint[1], i.tint[2]);
      nodes.fill(0);
      i.nodes.slice(0, 6).forEach((n, k) => nodes.set([n.x, n.y, n.on], k * 3));
      gl.uniform3fv(loc.nodes, nodes);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    },
  };
}
