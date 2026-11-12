// Shared uniforms: the thinking state, animated by time only.
export function inputs(t, w, h) {
  const core = [w / 2, h * 0.37], m = Math.min(w, h);
  const nodes = [0, 1, 2, 3].map((i) => {
    const a = Math.PI * (1.1 + (i / 3) * 0.8);
    return [core[0] + Math.cos(a) * m * 0.46, core[1] + 0.06 * m + Math.sin(a) * m * 0.3, 0.5 + 0.5 * Math.sin(t * 2 + i)];
  });
  const flat = new Float32Array(18); nodes.forEach((n, i) => flat.set(n, i * 3));
  return { uTime: t, uDim: 0.58, uScan: -1, uGlow: 1, uVoice: 0.5 + 0.5 * Math.sin(t * 5), uRing: 1, uLevel: 0.4, uOkay: 0, uShock: -1, uCore: core, uTint: [0.66, 0.5, 1], uNodes: flat };
}
export function stats(name) {
  let gaps = [], last = 0, from = performance.now();
  return (now) => {
    if (last) gaps.push(now - last); last = now;
    if (now - from > 2000) {
      gaps.sort((a, b) => a - b);
      const mean = gaps.reduce((a, b) => a + b, 0) / gaps.length;
      const line = `bench ${name} fps=${(1000 / mean).toFixed(1)} p95_ms=${gaps[Math.floor(gaps.length * 0.95)].toFixed(1)}`;
      window.__TAURI_INTERNALS__?.invoke("floating_report", { line }) ?? console.log(line);
      gaps = []; from = now;
    }
  };
}
export const RES = 0.5;
