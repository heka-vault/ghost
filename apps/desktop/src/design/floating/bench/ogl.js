import { Renderer, Program, Mesh, Triangle } from "ogl"; import { VERT, FRAG } from "../field.ts"; import { inputs, stats, RES } from "./scene.js";
const renderer = new Renderer({ dpr: RES, alpha: true, premultipliedAlpha: true, webgl: 2, width: innerWidth, height: innerHeight });
document.body.appendChild(renderer.gl.canvas); renderer.gl.canvas.style.cssText = "width:100vw;height:100vh;display:block";
// OGL's Triangle carries attributes; the field's vertex shader ignores them and uses gl_VertexID.
const uniforms = { uRes: { value: [renderer.gl.canvas.width, renderer.gl.canvas.height] }, uScale: { value: RES } };
for (const [k, v] of Object.entries(inputs(0, innerWidth, innerHeight))) uniforms[k] = { value: v };
const mesh = new Mesh(renderer.gl, { geometry: new Triangle(renderer.gl), program: new Program(renderer.gl, { vertex: VERT, fragment: FRAG, uniforms, transparent: true, depthTest: false }) });
const tick = stats("ogl");
const loop = (now) => { for (const [k, v] of Object.entries(inputs(now / 1000, innerWidth, innerHeight))) uniforms[k].value = v; renderer.render({ scene: mesh }); tick(now); requestAnimationFrame(loop); };
requestAnimationFrame(loop);
