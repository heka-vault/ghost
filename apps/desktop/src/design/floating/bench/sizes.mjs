// node sizes.mjs <node_modules dir>: minified and gzipped bytes for what a stack like ours imports.
import { build } from "esbuild"; import { gzipSync } from "node:zlib"; import { statSync } from "node:fs";
const nm = process.argv[2];
const cases = {
  "ogl (Renderer, Program, Mesh, Triangle)": `export { Renderer, Program, Mesh, Triangle } from "ogl";`,
  "three (WebGLRenderer, RawShaderMaterial, Mesh)": `export { WebGLRenderer, RawShaderMaterial, Mesh, BufferGeometry, Scene, Camera } from "three";`,
  "three + EffectComposer + UnrealBloomPass": `export { WebGLRenderer, RawShaderMaterial, Mesh } from "three"; export { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js"; export { UnrealBloomPass } from "three/examples/jsm/postprocessing/UnrealBloomPass.js";`,
  "pixi.js (Application, Filter, Sprite)": `export { Application, Filter, GlProgram, Sprite, Texture } from "pixi.js";`,
  "@rive-app/canvas (JS, wasm separate)": `export { Rive } from "@rive-app/canvas";`,
  "lottie-web (full player)": `export { default } from "lottie-web";`,
  "lottie-web light (svg player)": `export { default } from "lottie-web/build/player/lottie_light.js";`,
  "motion (animate, vanilla)": `export { animate } from "motion";`,
  "motion/react (motion, AnimatePresence, layout)": `export { motion, AnimatePresence } from "motion/react";`,
  "gsap (core)": `export { gsap } from "gsap";`,
};
for (const [name, src] of Object.entries(cases)) {
  const r = await build({ stdin: { contents: src, resolveDir: nm + "/..", loader: "js" }, bundle: true, minify: true, write: false, format: "esm", external: ["react", "react-dom"], logLevel: "silent" });
  const b = r.outputFiles[0].contents;
  console.log(`${name.padEnd(48)} ${(b.length / 1024).toFixed(0).padStart(5)} KB min  ${(gzipSync(b).length / 1024).toFixed(0).padStart(4)} KB gz`);
}



import { readFileSync } from "node:fs";
for (const f of ["@rive-app/canvas/rive.wasm", "@rive-app/webgl2/rive.wasm"]) {
  const b = readFileSync(`${nm}/${f}`);
  console.log(`${f.padEnd(48)} ${(b.length / 1024).toFixed(0).padStart(5)} KB      ${(gzipSync(b).length / 1024).toFixed(0).padStart(4)} KB gz`);
}
