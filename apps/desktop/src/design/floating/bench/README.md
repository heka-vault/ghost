# Motion stack bench (SPIKE)

The same scene, the field's own shader (`../field.ts`) at half resolution with the thinking
state's inputs, drawn through each GPU candidate, so the difference is the library's own
overhead; `three-bloom` adds three.js's UnrealBloomPass over it, the cost of a real bloom.
`sizes.mjs` measures each library's minified and gzipped weight for the imports a stack like
this would use. `build.sh` installs the candidates outside the repo (`/tmp/flt/bench`), bundles each scene,
prints the sizes and serves them on :1432, so nothing joins the lockfile.
Results are in docs/design/floating-2026-11.md.
