/**
 * The made-up desktop Ghost floats over (world.css has why): its markup and its city, drawn
 * once on a fixed seed so every picture shows the same skyline. `mountWorld` fills an element
 * with class `world` (1280x720 CSS pixels); the page lays Ghost over it and scales it.
 */
export const WORLD_W = 1280;
export const WORLD_H = 720;

export const WORLD_HTML = `
<div class="wall"><canvas class="city"></canvas></div>

<div class="bar">
  <span class="pill">Activities</span>
  <span>Thu 14:32</span>
  <span class="tray"><span>⌁</span><span>◖ 62%</span><span>⏻</span></span>
</div>

<!-- The browser, behind on the right: a made-up launch board on a made-up local site. -->
<div class="win browser">
  <div class="head"><span class="dots"><i></i><i></i><i></i></span><span class="t">Launch review · Board</span></div>
  <div class="url"><span style="color: var(--mute)">◀ ▶ ⟳</span><b>board.local/launch/review</b></div>
  <div class="page">
    <h1>Launch review</h1>
    <div class="sub">Thursday 16:00 · 9 open · 2 waiting on review</div>
    <div class="cols">
      <div class="col">
        <h2>Blocked</h2>
        <div class="card"><span class="tag r">P1</span>Invoice totals off by a cent<small>billing · #212</small></div>
        <div class="card"><span class="tag a">P2</span>Migration step times out<small>deploy · #219</small></div>
      </div>
      <div class="col">
        <h2>In review</h2>
        <div class="card"><span class="tag v">PR</span>Retry the export with backoff<small>infra · #148</small></div>
        <div class="card"><span class="tag v">PR</span>Pricing page copy<small>site · #151</small></div>
      </div>
      <div class="col">
        <h2>Done</h2>
        <div class="card"><span class="tag g">✓</span>Status page<small>ops · #133</small></div>
        <div class="card"><span class="tag g">✓</span>Receipts in PDF<small>billing · #140</small></div>
      </div>
    </div>
    <div class="bars"></div>
  </div>
</div>

<!-- The editor: this project's own code, as someone would have it open. -->
<div class="win editor">
  <div class="head"><span class="dots"><i></i><i></i><i></i></span><span class="t">turn.rs — ghost</span></div>
  <div class="body">
    <div class="tree">
      <div class="dir">▾ crates</div>
      <div class="dir" style="--d: 1">▸ ghost-core</div>
      <div class="dir" style="--d: 1">▾ ghost-voice</div>
      <div class="dir" style="--d: 2">▾ src</div>
      <div style="--d: 3">acks.rs</div>
      <div style="--d: 3">activity.rs</div>
      <div style="--d: 3">headless.rs</div>
      <div style="--d: 3">prompt.rs</div>
      <div style="--d: 3">tap.rs</div>
      <div class="on" style="--d: 3">turn.rs</div>
      <div style="--d: 3">vad.rs</div>
      <div class="dir" style="--d: 1">▸ ghostd</div>
      <div class="dir">▸ apps</div>
      <div class="dir">▸ docs</div>
      <div>CLAUDE.md</div>
      <div>Makefile</div>
    </div>
    <div class="code">
      <div class="tabs"><span class="on">turn.rs</span><span>tap.rs</span><span>acks.rs</span></div>
<pre><span class="l"><span class="c">/// When the operator's turn is over: a pause long enough, and words that end a thought.</span></span><span class="l"><span class="k">pub fn</span> <span class="f">turn_closed</span>(pause: <span class="ty">Duration</span>, words: &amp;<span class="ty">str</span>, held: <span class="ty">bool</span>) -&gt; <span class="ty">Close</span> {</span><span class="l">    <span class="k">if</span> held {</span><span class="l">        <span class="k">return</span> <span class="ty">Close</span>::<span class="m">Wait</span>;</span><span class="l">    }</span><span class="l">    <span class="k">let</span> trailing = words.<span class="f">trim_end</span>().<span class="f">ends_with</span>([<span class="s">','</span>, <span class="s">'-'</span>]);</span><span class="l">    <span class="k">let</span> needed = <span class="k">if</span> trailing { <span class="n">LONG_PAUSE</span> } <span class="k">else</span> { <span class="n">SHORT_PAUSE</span> };</span><span class="l">    <span class="k">match</span> pause {</span><span class="l">        p <span class="k">if</span> p &gt;= needed =&gt; <span class="ty">Close</span>::<span class="m">Now</span>,</span><span class="l">        p <span class="k">if</span> p &gt;= <span class="n">SHORT_PAUSE</span> =&gt; <span class="ty">Close</span>::<span class="m">Soon</span>(needed - p),</span><span class="l">        _ =&gt; <span class="ty">Close</span>::<span class="m">Wait</span>,</span><span class="l">    }</span><span class="l">}</span><span class="l"> </span><span class="l"><span class="c">#[cfg(test)]</span></span><span class="l"><span class="k">mod</span> tests {</span><span class="l">    <span class="k">use super</span>::*;</span><span class="l"> </span><span class="l">    <span class="c">#[test]</span></span><span class="l cur">    <span class="k">fn</span> <span class="f">a_trailing_comma_waits_for_more</span>() {<span class="caret"></span></span><span class="l">        <span class="k">let</span> c = <span class="f">turn_closed</span>(<span class="ty">Duration</span>::<span class="f">from_millis</span>(<span class="n">600</span>), <span class="s">"and then,"</span>, <span class="k">false</span>);</span></pre>
    </div>
  </div>
</div>

<!-- The terminal: the tests running. -->
<div class="win term">
  <div class="head"><span class="dots"><i></i><i></i><i></i></span><span class="t">~/ghost — cargo</span></div>
<pre><span class="p">❯</span> cargo nextest run -p ghost-voice turn
<span class="d">    Finished</span> test [unoptimized] target(s) in 4.12s
<span class="d">    Starting</span> 14 tests across 1 binary
<span class="ok">        PASS</span> <span class="d">[ 0.004s]</span> ghost-voice turn::tests::a_full_stop_closes_now
<span class="ok">        PASS</span> <span class="d">[ 0.003s]</span> ghost-voice turn::tests::a_held_turn_never_closes
<span class="ok">        PASS</span> <span class="d">[ 0.005s]</span> ghost-voice turn::tests::filler_keeps_the_turn_open
<span class="ok">        PASS</span> <span class="d">[ 0.004s]</span> ghost-voice turn::tests::a_long_pause_closes_anything
<span class="y">        RUN </span> <span class="d">[      ]</span> ghost-voice turn::tests::a_trailing_comma_waits_for_more
</pre>
</div>
`;

/** Puts the desktop into `root` (which should have class `world`) and draws its city. */
export function mountWorld(root: HTMLElement): void {
  root.innerHTML = WORLD_HTML;
  // A fixed seed: the same city in every picture.
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;

  const bars = root.querySelector<HTMLElement>(".bars");
  for (let i = 0; i < 24; i++) bars?.insertAdjacentHTML("beforeend", `<i style="height:${18 + Math.round(rnd() * 82)}%"></i>`);

  // The city: towers against a pink haze, lit windows, a few neon signs, rain.
  const cv = root.querySelector<HTMLCanvasElement>("canvas.city");
  const g = cv?.getContext("2d");
  if (!cv || !g) return;
  const W = WORLD_W;
  const H = WORLD_H;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  cv.width = W * dpr;
  cv.height = H * dpr;
  g.scale(dpr, dpr);
  const towers = (count: number, minH: number, maxH: number, shade: string, windows: number) => {
    let x = -20;
    while (x < W + 20) {
      const w = 26 + rnd() * 70;
      const h = minH + rnd() * (maxH - minH);
      const top = H - h;
      g.fillStyle = shade;
      g.fillRect(x, top, w, h);
      if (rnd() < 0.3) g.fillRect(x + w / 2 - 1, top - 30 - rnd() * 40, 2, 70);
      for (let wy = top + 8; wy < H - 6; wy += 9)
        for (let wx = x + 5; wx < x + w - 5; wx += 7)
          if (rnd() < windows) {
            const hue = rnd();
            g.fillStyle = hue < 0.55 ? "rgba(255,190,120,0.55)" : hue < 0.85 ? "rgba(95,227,255,0.5)" : "rgba(255,79,163,0.6)";
            g.fillRect(wx, wy, 3, 4);
          }
      if (rnd() < 0.22) {
        const c = rnd() < 0.5 ? "#ff4fa3" : "#5fe3ff";
        g.save();
        g.shadowColor = c;
        g.shadowBlur = 18;
        g.fillStyle = c;
        g.fillRect(x + w * 0.2, top + 20 + rnd() * 60, 3, 40 + rnd() * 60);
        g.restore();
      }
      x += w + 2 + rnd() * count;
    }
  };
  towers(40, 260, 560, "#0b0814", 0.05);
  const haze = g.createLinearGradient(0, H * 0.45, 0, H);
  haze.addColorStop(0, "rgba(255,60,140,0)");
  haze.addColorStop(1, "rgba(255,60,140,0.18)");
  g.fillStyle = haze;
  g.fillRect(0, 0, W, H);
  towers(10, 140, 380, "#050409", 0.09);
  g.strokeStyle = "rgba(160,200,255,0.06)";
  g.lineWidth = 1;
  for (let i = 0; i < 260; i++) {
    const x = rnd() * W;
    const y = rnd() * H;
    const l = 10 + rnd() * 24;
    g.beginPath();
    g.moveTo(x, y);
    g.lineTo(x - l * 0.18, y + l);
    g.stroke();
  }
}
