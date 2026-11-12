import { Application, Filter, GlProgram, Sprite, Texture } from "pixi.js"; import { FRAG } from "../field.ts"; import { inputs, stats, RES } from "./scene.js";
const app = new Application();
await app.init({ preference: "webgl", backgroundAlpha: 0, resolution: RES, width: innerWidth, height: innerHeight, antialias: false });
document.body.appendChild(app.canvas); app.canvas.style.cssText = "width:100vw;height:100vh;display:block";
// A Pixi filter over a full-screen sprite; Pixi's filter vertex stage replaces the field's.
const vertex = `in vec2 aPosition; uniform vec4 uInputSize; uniform vec4 uOutputFrame; uniform vec4 uOutputTexture;
void main(){ vec2 p = aPosition * uOutputFrame.zw + uOutputFrame.xy; p.x = p.x * (2.0 / uOutputTexture.x) - 1.0; p.y = p.y * (2.0*uOutputTexture.z / uOutputTexture.y) - uOutputTexture.z; gl_Position = vec4(p, 0.0, 1.0); }`;
const frag = FRAG.replace("#version 300 es", "").replace("precision highp float;", "precision highp float;").replace("out vec4 outColor;", "out vec4 finalColor;").replace(/outColor =/g, "finalColor =");
const u0 = inputs(0, innerWidth, innerHeight);
const fu = { uRes: { value: [app.canvas.width, app.canvas.height], type: "vec2<f32>" }, uScale: { value: RES, type: "f32" } };
for (const [k, v] of Object.entries(u0)) fu[k] = k === "uNodes" ? { value: v, type: "vec3<f32>", size: 6 } : { value: v, type: v.length === 2 ? "vec2<f32>" : v.length === 3 ? "vec3<f32>" : "f32" };
const filter = new Filter({ glProgram: GlProgram.from({ vertex, fragment: frag }), resources: { field: fu } });
const s = new Sprite(Texture.WHITE); s.width = innerWidth; s.height = innerHeight; s.alpha = 1; s.filters = [filter]; app.stage.addChild(s);
const tick = stats("pixi");
app.ticker.add(() => { const now = performance.now(); const u = inputs(now / 1000, innerWidth, innerHeight); for (const [k, v] of Object.entries(u)) filter.resources.field.uniforms[k] = v; tick(now); });
