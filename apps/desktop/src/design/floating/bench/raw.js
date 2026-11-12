import { VERT, FRAG } from "../field.ts"; import { inputs, stats, RES } from "./scene.js";
const c = document.querySelector("canvas"), gl = c.getContext("webgl2", { premultipliedAlpha: true });
c.width = innerWidth * RES; c.height = innerHeight * RES;
const sh = (t, s) => { const o = gl.createShader(t); gl.shaderSource(o, s); gl.compileShader(o); return o; };
const p = gl.createProgram(); gl.attachShader(p, sh(gl.VERTEX_SHADER, VERT)); gl.attachShader(p, sh(gl.FRAGMENT_SHADER, FRAG)); gl.linkProgram(p); gl.useProgram(p);
gl.bindVertexArray(gl.createVertexArray()); gl.viewport(0, 0, c.width, c.height);
const tick = stats("raw");
const loop = (now) => {
  const u = inputs(now / 1000, innerWidth, innerHeight);
  gl.uniform2f(gl.getUniformLocation(p, "uRes"), c.width, c.height); gl.uniform1f(gl.getUniformLocation(p, "uScale"), RES);
  for (const [k, v] of Object.entries(u)) { const l = gl.getUniformLocation(p, k);
    if (k === "uNodes") gl.uniform3fv(l, v); else if (v.length === 2) gl.uniform2fv(l, v); else if (v.length === 3) gl.uniform3fv(l, v); else gl.uniform1f(l, v); }
  gl.drawArrays(gl.TRIANGLES, 0, 3); tick(now); requestAnimationFrame(loop);
};
requestAnimationFrame(loop);
